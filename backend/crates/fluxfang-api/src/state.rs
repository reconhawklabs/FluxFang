//! Shared application state threaded through every handler via axum's
//! `State` extractor.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sqlx::PgPool;

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
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            login_limiter: Arc::new(LoginLimiter::default()),
        }
    }
}
