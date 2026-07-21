//! Shared application state threaded through every handler via axum's
//! `State` extractor.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::capture::{CaptureSupervisor, CapturerFactory, RealCapturerFactory};
use crate::ingest::Event;
use crate::sensor_listener::SensorListenerManager;

/// How many failed `/api/login` attempts are tolerated inside
/// [`LOGIN_RATE_LIMIT_WINDOW`] before further attempts get `429 Too Many
/// Requests`, regardless of whether the password given this time happens to
/// be correct.
///
/// This is intentionally a *global* counter, not per-IP or per-account:
/// FluxFang has exactly one admin credential and no concept of client
/// identity beyond "does the request carry an authenticated session cookie"
/// (which, by definition, an attacker guessing at `/api/login` doesn't
/// have). A per-client counter would need infrastructure this single-admin
/// slice doesn't have yet — extracting and trusting a client IP behind
/// whatever reverse proxy fronts this (see `frontend/nginx.conf`), keyed
/// storage, eviction. The tradeoff: someone who wants to temporarily block
/// the real admin from *attempting* logins can do so by tripping the
/// limiter themselves; existing authenticated sessions are unaffected, and
/// the window rolls over in a minute. Acceptable for a single-admin
/// homelab-scale app; revisit if this ever grows multiple accounts behind a
/// shared, identity-bearing proxy.
pub const LOGIN_RATE_LIMIT_MAX_ATTEMPTS: usize = 10;

/// Sliding window over which [`LOGIN_RATE_LIMIT_MAX_ATTEMPTS`] is counted.
pub const LOGIN_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// In-memory sliding-window counter of recent failed `/api/login` attempts.
/// Resets on backend restart, same caveat as the session store below — see
/// `AppState` docs.
#[derive(Default)]
pub struct LoginLimiter {
    failures: Mutex<VecDeque<Instant>>,
}

impl LoginLimiter {
    /// Whether the caller is currently rate-limited (at/over the max
    /// failures within the window). Also prunes stale entries so the
    /// counter doesn't grow unboundedly.
    pub fn is_limited(&self) -> bool {
        let mut failures = self.failures.lock().expect("login limiter mutex poisoned");
        prune(&mut failures);
        failures.len() >= LOGIN_RATE_LIMIT_MAX_ATTEMPTS
    }

    /// Record a failed login attempt.
    pub fn record_failure(&self) {
        let mut failures = self.failures.lock().expect("login limiter mutex poisoned");
        prune(&mut failures);
        failures.push_back(Instant::now());
    }

    /// Forget prior failures (called after a successful login).
    pub fn record_success(&self) {
        let mut failures = self.failures.lock().expect("login limiter mutex poisoned");
        failures.clear();
    }
}

/// Drop entries older than the window. Uses `checked_sub` (rather than
/// plain subtraction) because `Instant - Duration` can panic if the
/// resulting instant would predate the clock's own epoch — a real
/// possibility here since the window is only 60s and a freshly-started
/// process/container could in principle be younger than that on some
/// platforms.
fn prune(failures: &mut VecDeque<Instant>) {
    let Some(cutoff) = Instant::now().checked_sub(LOGIN_RATE_LIMIT_WINDOW) else {
        return;
    };
    while matches!(failures.front(), Some(t) if *t < cutoff) {
        failures.pop_front();
    }
}

/// Placeholder AES-256-GCM key used by [`AppState::new`] (test/dev
/// convenience only — see its doc comment). Never used in production:
/// `main.rs` always goes through [`AppState::with_capture`] with a real key
/// loaded from `FLUXFANG_SECRET_KEY` via `fluxfang_core::secrets::key_from_base64`.
const PLACEHOLDER_SECRET_KEY: [u8; 32] = [0u8; 32];

/// Capacity of [`AppState`]'s `ingest::Event` broadcast channel (Task 7.1's
/// WebSocket handler is the eventual real subscriber). Sized generously
/// relative to this single-admin app's expected emission rate; a slow or
/// absent subscriber just misses old events (`broadcast::Sender::send`
/// never blocks), it doesn't back-pressure ingest.
const EVENTS_CHANNEL_CAPACITY: usize = 256;

/// Application-wide state threaded through handlers via `State<AppState>`.
///
/// Kept intentionally minimal for Task 2.2 — just what setup/login/logout
/// need. Later tasks add fields as new subsystems (ingest workers, websocket
/// broadcast channels, ...) come online; this is *not* meant to be the
/// final shape.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub login_limiter: Arc<LoginLimiter>,
    /// Task 6.2's data-source start/stop orchestrator. See `crate::capture`
    /// module docs for the full design.
    pub capture: Arc<CaptureSupervisor>,
    /// Phase 2B's `sensor`-kind datasource orchestrator: binds/tears-down a
    /// dedicated network listener per enabled sensor datasource, mirroring
    /// `capture`'s role for `wifi`/`gps`/`bluetooth`/`rtl_sdr` kinds. See
    /// `crate::sensor_listener` module docs.
    pub sensor_listeners: Arc<SensorListenerManager>,
    /// AES-256-GCM key used to encrypt/decrypt `alert_method.config_encrypted`
    /// (Task 6.6). The same 32 bytes handed to `CaptureSupervisor`/`IngestCtx`
    /// for alert dispatch during ingest — duplicated here (rather than routed
    /// through `capture`) so `alert_methods`/`alert_rules` handlers can
    /// encrypt a freshly-submitted config or decrypt one for a test-send
    /// without reaching into `CaptureSupervisor`'s private internals.
    pub secret_key: [u8; 32],
}

impl AppState {
    /// Convenience constructor for callers that don't care about capture
    /// (most of this crate's existing tests: health/auth/catalog). Builds a
    /// [`CaptureSupervisor`] wired to the real, hardware-touching
    /// [`RealCapturerFactory`] and a placeholder (all-zero) secret key —
    /// fine as long as nothing actually calls `start`/`stop` or dispatches
    /// an alert notification, which none of those tests do. Production
    /// (`main.rs`) and this crate's own data-source tests use
    /// [`AppState::with_capture`] instead, supplying a real key and/or a
    /// `MockCapturerFactory`.
    pub fn new(pool: PgPool) -> Self {
        Self::with_capture(pool, PLACEHOLDER_SECRET_KEY, Arc::new(RealCapturerFactory))
    }

    /// Full constructor: `secret_key` is the parsed 32-byte
    /// `FLUXFANG_SECRET_KEY` (production) or a fixed test key (tests), and
    /// `factory` is the `CapturerFactory` the `CaptureSupervisor` uses to
    /// build capturers on `start` — `RealCapturerFactory` in production,
    /// `MockCapturerFactory` in `tests/data_sources.rs`.
    pub fn with_capture(
        pool: PgPool,
        secret_key: [u8; 32],
        factory: Arc<dyn CapturerFactory>,
    ) -> Self {
        let (events_tx, _events_rx) = broadcast::channel::<Event>(EVENTS_CHANNEL_CAPACITY);
        let capture = Arc::new(CaptureSupervisor::new(
            pool.clone(),
            events_tx,
            secret_key,
            factory,
        ));
        let sensor_listeners = Arc::new(SensorListenerManager::new(pool.clone()));
        Self {
            pool,
            login_limiter: Arc::new(LoginLimiter::default()),
            capture,
            sensor_listeners,
            secret_key,
        }
    }
}
