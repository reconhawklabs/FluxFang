//! `CaptureSupervisor` (Task 6.2): the orchestration layer that turns a
//! configured `data_source` row into an actually-running capture (a wifi
//! monitor-mode sniffer or a GPS fix stream), and back.
//!
//! ## Pieces
//!
//! - [`CapturerFactory`] is the seam between this module and real hardware:
//!   given a `data_source` row it builds either a `fluxfang_capture::Capturer`
//!   (wifi) or a `fluxfang_capture::GpsSource` (gps). [`RealCapturerFactory`]
//!   builds the genuine hardware-touching types
//!   (`WifiMonitorCapturer`/`GpsdSource`/`SerialGpsSource`); [`MockCapturerFactory`]
//!   builds `fluxfang_capture::mock::{MockCapturer, MockGps}` instead, so
//!   this module's own tests (and `tests/data_sources.rs`) never touch
//!   hardware. Which one `AppState` holds is a constructor argument — see
//!   `state.rs`.
//! - [`CaptureSupervisor`] holds the running set (`data_source_id ->
//!   RunningHandle`) and the one shared "survey session" (see below), and
//!   implements [`CaptureSupervisor::start`]/[`CaptureSupervisor::stop`].
//!
//! ## Session lifecycle: first-start opens, last-stop closes
//!
//! Every `ingest()` call requires an active `survey_session` (see
//! `ingest`'s module docs). So starting *any* data source — wifi or gps —
//! must ensure a session is open, and stopping the *last* running source
//! must close it. This module tracks that with one
//! `Mutex<Option<SharedSession>>`:
//!
//! - If a **gps** source starts and no session is open, it opens a fresh
//!   `SessionManager` fed by that gps source's real `GpsSource` — exactly
//!   Task 5.1's intended wiring, with the [`host_zone_hook`] closing the
//!   loop back to Task 5.4's `zones::update_host_zones`.
//! - If a **wifi** source starts and no session is open, it opens a
//!   `SessionManager` fed by [`InertGps`] — a `GpsSource` that never yields
//!   a fix and never exhausts — with [`WIFI_ONLY_SESSION_GAP`] (a
//!   multi-decade "gap") in place of the real 5-minute default, so the
//!   *absence* of any GPS hardware doesn't auto-close the session; only an
//!   explicit last-stop does. Host-zone tracking is simply unavailable in
//!   this mode (there's no real position to evaluate), which is the
//!   expected, documented tradeoff of a wifi-only survey.
//! - A second source of either kind, once a session is already open, just
//!   reuses it — **except** starting a second concurrent gps source (or a
//!   gps source while a wifi-only/`InertGps`-backed session is already
//!   open), which is rejected outright: see [`CaptureSupervisor::ensure_gps_session`]'s
//!   doc comment for why, and for the known limitation this implies.
//! - [`CaptureSupervisor::stop`] closes the session once the running set
//!   becomes empty.
//!
//! ## Known limitation: mixed-kind stop ordering
//!
//! If a gps source and one or more wifi sources are running together
//! (sharing one session, gps-backed), stopping the *gps* source alone does
//! not stop that session's underlying fix loop — the loop lives inside the
//! shared `SessionManager`, which only this supervisor's own last-stop
//! closes. The gps data source's own `status` is still correctly flipped to
//! `stopped`, but `location_fix` rows keep being written (using the same
//! physical gps hardware) until every other running source also stops.
//! Fixing this properly would need decoupling "the session's bookkeeping"
//! from "which `GpsSource` feeds it" (e.g. a hot-swappable or multi-source
//! `SessionManager`) — out of scope (YAGNI) for this slice, which only
//! requires the two TDD scenarios (wifi-only, gps-only) to work correctly.
//!
//! ## Known limitation: `zone_membership` TOCTOU
//!
//! With more than one source running, `ingest`/`update_host_zones` calls
//! happen concurrently against the same pool. `zones::update_subject_zones`
//! does a plain get-then-upsert against `zone_membership` with no row lock,
//! so two *exactly* simultaneous transitions for the same subject could
//! both read the prior "outside" state and both fire an
//! enter/leave notification (a rare duplicate, not a lost or corrupted
//! transition). Acceptable for this slice per the task brief; a proper fix
//! is a row-level lock (`SELECT ... FOR UPDATE`) or an `ON CONFLICT`-based
//! atomic upsert-and-return-previous, deferred rather than added here to
//! avoid introducing new locking behavior this task wasn't asked to test.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use fluxfang_capture::gps::ALLOWED_BAUD_RATES;
use fluxfang_capture::mock::{MockCapturer, MockGps};
use fluxfang_capture::wifi::monitor::WifiMonitorCapturer;
use fluxfang_capture::wifi::scan::WifiScanCapturer;
use fluxfang_capture::{Capturer, GpsFix, GpsSource, RawObservation};
use fluxfang_db::models::DataSource;
use fluxfang_db::DataSourceRepo;
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::ingest::session::{HostZoneHook, SessionManager, SessionManagerConfig};
use crate::ingest::zones::update_host_zones;
use crate::ingest::{ingest, Event, IngestCtx};

/// `SessionManagerConfig::inactivity_gap` used for a session opened without
/// a real GPS source (see module docs). ~50 years: long enough to never
/// fire in practice, short enough that `Instant + gap` can't overflow.
const WIFI_ONLY_SESSION_GAP: Duration = Duration::from_secs(60 * 60 * 24 * 365 * 50);

/// What a [`CapturerFactory`] builds for one `data_source` row, matching its
/// `kind`.
pub enum BuiltCapture {
    Wifi(Box<dyn Capturer>),
    Gps(Box<dyn GpsSource + Send>),
}

/// Builds the capture backend for a `data_source` row. The seam that lets
/// [`CaptureSupervisor`] (and its tests) stay decoupled from real hardware —
/// see [`RealCapturerFactory`] and [`MockCapturerFactory`].
#[async_trait]
pub trait CapturerFactory: Send + Sync {
    async fn build(&self, source: &DataSource) -> anyhow::Result<BuiltCapture>;
}

/// Production factory: builds the genuine hardware-touching capturers.
/// Deliberately thin (per the task brief) and **not unit-tested** — it just
/// forwards a validated `data_source` row's fields to the matching
/// `fluxfang_capture` constructor; the interesting logic (parsing,
/// validation, retry, ...) lives in those constructors themselves, which
/// already have their own doc comments explaining why *they* aren't
/// exercised by the automated suite (no monitor-mode adapter / gpsd daemon /
/// serial device in CI).
pub struct RealCapturerFactory;

#[async_trait]
impl CapturerFactory for RealCapturerFactory {
    async fn build(&self, source: &DataSource) -> anyhow::Result<BuiltCapture> {
        match source.kind.as_str() {
            "wifi" => {
                let interface = source
                    .interface
                    .clone()
                    .ok_or_else(|| anyhow!("wifi data source is missing its interface"))?;
                match source.mode.as_str() {
                    "monitor" => Ok(BuiltCapture::Wifi(Box::new(WifiMonitorCapturer::new(
                        interface,
                    )))),
                    "scan" => Ok(BuiltCapture::Wifi(Box::new(WifiScanCapturer::new(
                        interface,
                    )))),
                    other => Err(anyhow!("unsupported wifi mode '{other}'")),
                }
            }
            "gps" => match source.mode.as_str() {
                "gpsd" => {
                    let host = source
                        .config
                        .get("host")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("gps gpsd config missing 'host'"))?;
                    let port = source
                        .config
                        .get("port")
                        .and_then(|v| v.as_u64())
                        .ok_or_else(|| anyhow!("gps gpsd config missing 'port'"))?
                        as u16;
                    let gps = fluxfang_capture::gps::GpsdSource::connect(host, port).await?;
                    Ok(BuiltCapture::Gps(Box::new(gps)))
                }
                "serial" => {
                    let device = source
                        .config
                        .get("device")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("gps serial config missing 'device'"))?;
                    let baud = source
                        .config
                        .get("baud")
                        .and_then(|v| v.as_u64())
                        .ok_or_else(|| anyhow!("gps serial config missing 'baud'"))?
                        as u32;
                    let gps = fluxfang_capture::gps::SerialGpsSource::open(device, baud)?;
                    Ok(BuiltCapture::Gps(Box::new(gps)))
                }
                other => Err(anyhow!("unsupported gps mode '{other}'")),
            },
            other => Err(anyhow!("unsupported data source kind '{other}'")),
        }
    }
}

/// Test/dev factory: builds `fluxfang_capture::mock::{MockCapturer, MockGps}`
/// instead of touching hardware. Configure with [`MockCapturerFactory::with_wifi_observations`]
/// or [`MockCapturerFactory::with_gps_fixes`] before starting the matching
/// kind of data source; every `build()` call for that kind clones the same
/// configured replay data (fine for this slice's single-source-at-a-time
/// tests). [`Self::set_wifi_observations`] additionally lets a caller
/// already holding a live `Arc<MockCapturerFactory>` stage a *different*
/// batch for a later data-source start, and [`Self::looping_gps`] keeps a
/// gps-backed session alive indefinitely instead of self-closing as soon as
/// a finite fix list drains — both added for `tests/e2e.rs`'s multi-stage
/// scenario (see their own doc comments).
///
/// Not `#[cfg(test)]`: `tests/data_sources.rs` (a separate integration-test
/// binary) needs to construct it too, and `#[cfg(test)]` items aren't
/// visible outside the crate they're compiled in. Always compiling it in is
/// the same tradeoff `fluxfang_capture::mock` itself already makes.
pub struct MockCapturerFactory {
    wifi_observations: std::sync::Mutex<Vec<RawObservation>>,
    gps_fixes: std::sync::Mutex<Vec<GpsFix>>,
    /// When set, every wifi `build()` call's `MockCapturer` is configured
    /// via [`MockCapturer::failing`] so its `start()` always errors —
    /// see [`Self::failing_wifi_start`].
    fail_wifi_start: std::sync::atomic::AtomicBool,
    /// When set, every gps `build()` call's `MockGps` is configured via
    /// [`MockGps::looping`] instead of stopping once its fix list drains —
    /// see [`Self::looping_gps`].
    loop_gps: std::sync::atomic::AtomicBool,
}

impl MockCapturerFactory {
    /// A factory with nothing configured yet — `build()` for either kind
    /// returns an empty (immediately-exhausted) mock.
    pub fn new() -> Self {
        Self {
            wifi_observations: std::sync::Mutex::new(Vec::new()),
            gps_fixes: std::sync::Mutex::new(Vec::new()),
            fail_wifi_start: std::sync::atomic::AtomicBool::new(false),
            loop_gps: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Every wifi `build()` call replays `observations` once, a few
    /// milliseconds apart.
    pub fn with_wifi_observations(observations: Vec<RawObservation>) -> Self {
        let factory = Self::new();
        *factory.wifi_observations.lock().expect("mutex poisoned") = observations;
        factory
    }

    /// Every gps `build()` call replays `fixes` once, then exhausts.
    pub fn with_gps_fixes(fixes: Vec<GpsFix>) -> Self {
        let factory = Self::new();
        *factory.gps_fixes.lock().expect("mutex poisoned") = fixes;
        factory
    }

    /// Replace the wifi observations every subsequent wifi `build()` call
    /// will replay, without needing a fresh factory instance. Runtime
    /// (`&self`, not a consuming builder like [`Self::with_wifi_observations`])
    /// so a caller that's already handed `Arc<dyn CapturerFactory>` to a live
    /// `CaptureSupervisor`/`AppState` (and so only has this concrete type
    /// behind its own separate `Arc<MockCapturerFactory>`) can still stage a
    /// second, different batch of observations for a later data-source start
    /// — e.g. `tests/e2e.rs`'s round-1 (unassigned, pre-emitter) vs round-2
    /// (post-alert-rule) emissions, which a single fixed `Vec` set once at
    /// construction couldn't express since each round needs its own distinct
    /// content.
    pub fn set_wifi_observations(&self, observations: Vec<RawObservation>) {
        *self.wifi_observations.lock().expect("mutex poisoned") = observations;
    }

    /// Chainable: make every wifi `build()` call's `MockCapturer` fail its
    /// `start()` — used to test `CaptureSupervisor::start`'s failure path
    /// (status flips to `error`, and — the regression this guards — no
    /// dangling `survey_session` is left behind) without real broken
    /// hardware.
    pub fn failing_wifi_start(self) -> Self {
        self.fail_wifi_start
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self
    }

    /// Chainable: make every gps `build()` call's `MockGps` loop its
    /// configured fixes forever instead of stopping once the list drains.
    ///
    /// Needed because [`MockGps`] (unlike [`MockCapturer`], which paces
    /// itself via a real `interval` between sends) has no artificial delay
    /// between fixes at all — a finite, non-looping track drains as fast as
    /// each fix's `location_fix` DB write completes, which
    /// [`session::run_ingest_loop`](crate::ingest::session) then treats as
    /// **source exhaustion**: the shared `survey_session` self-closes almost
    /// immediately, well before a realistic caller (e.g. an end-to-end test
    /// driving several more HTTP requests, or a wifi-only data source
    /// wanting to keep reusing this same gps-backed session) is done with
    /// it. Looping keeps the session's `current_session_id()` valid and its
    /// `latest_fix()` fresh for as long as the caller needs, only ending
    /// when the gps data source is explicitly stopped.
    pub fn looping_gps(self) -> Self {
        self.loop_gps
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self
    }
}

impl Default for MockCapturerFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CapturerFactory for MockCapturerFactory {
    async fn build(&self, source: &DataSource) -> anyhow::Result<BuiltCapture> {
        match source.kind.as_str() {
            "wifi" => {
                let observations = self
                    .wifi_observations
                    .lock()
                    .expect("mutex poisoned")
                    .clone();
                let mut capturer = MockCapturer::new(observations, Duration::from_millis(5));
                if self
                    .fail_wifi_start
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    capturer = capturer.failing();
                }
                Ok(BuiltCapture::Wifi(Box::new(capturer)))
            }
            "gps" => {
                let fixes = self.gps_fixes.lock().expect("mutex poisoned").clone();
                let mut gps = MockGps::new(fixes);
                if self.loop_gps.load(std::sync::atomic::Ordering::SeqCst) {
                    gps = gps.looping(true);
                }
                Ok(BuiltCapture::Gps(Box::new(gps)))
            }
            other => Err(anyhow!("MockCapturerFactory: unsupported kind '{other}'")),
        }
    }
}

/// A `GpsSource` that never yields a fix and never exhausts — used to back
/// a wifi-only session's `SessionManager` (see module docs). Paired with
/// [`WIFI_ONLY_SESSION_GAP`] so the loop's `select!` never actually races a
/// completed `next_fix()` against the gap timer in practice.
struct InertGps;

#[async_trait]
impl GpsSource for InertGps {
    async fn next_fix(&mut self) -> Option<GpsFix> {
        std::future::pending().await
    }
}

/// Adapts an owned `Box<dyn GpsSource + Send>` back into a plain
/// `GpsSource` impl, since `SessionManager::open<G: GpsSource + Send +
/// 'static>` needs a concrete-enough `G` to be generic over, not a bare
/// trait object (there's no blanket `impl GpsSource for Box<dyn GpsSource +
/// Send>` in `fluxfang_capture`, which otherwise has no reason to know
/// about boxed trait objects at all).
struct BoxedGps(Box<dyn GpsSource + Send>);

#[async_trait]
impl GpsSource for BoxedGps {
    async fn next_fix(&mut self) -> Option<GpsFix> {
        self.0.next_fix().await
    }
}

/// Validate a proposed data source's `(kind, mode, interface, config)`
/// combination beyond what the DB's `CHECK` constraints already enforce
/// (see `migrations/0001_init.sql`'s `data_source` table): wifi needs a
/// non-empty `interface`; gps `gpsd` config needs `{host, port}`; gps
/// `serial` config needs `{device, baud}` with `baud` one of
/// `fluxfang_capture::gps::ALLOWED_BAUD_RATES`. Returns a human-readable
/// message on failure, which callers surface as a `400`
/// (`data_sources::create_data_source`/`update_data_source`) or as a
/// `data_source.last_error` (`CaptureSupervisor::start`'s defensive
/// re-check of a row that might have been hand-edited since it was last
/// validated).
pub(crate) fn validate_data_source(
    kind: &str,
    mode: &str,
    interface: Option<&str>,
    config: &Value,
) -> Result<(), String> {
    match kind {
        "wifi" => {
            if mode != "monitor" && mode != "scan" {
                return Err(format!(
                    "wifi data sources must use mode 'monitor' or 'scan', got '{mode}'"
                ));
            }
            match interface {
                Some(i) if !i.trim().is_empty() => Ok(()),
                _ => Err("wifi data sources require a non-empty interface".to_string()),
            }
        }
        "gps" => match mode {
            "gpsd" => {
                let host_ok = config
                    .get("host")
                    .and_then(|v| v.as_str())
                    .is_some_and(|h| !h.trim().is_empty());
                if !host_ok {
                    return Err("gps gpsd config requires a non-empty 'host' string".to_string());
                }
                let port_ok = config
                    .get("port")
                    .and_then(|v| v.as_u64())
                    .is_some_and(|p| p > 0 && p <= u16::MAX as u64);
                if !port_ok {
                    return Err(
                        "gps gpsd config requires a numeric 'port' between 1 and 65535".to_string(),
                    );
                }
                Ok(())
            }
            "serial" => {
                let device_ok = config
                    .get("device")
                    .and_then(|v| v.as_str())
                    .is_some_and(|d| !d.trim().is_empty());
                if !device_ok {
                    return Err(
                        "gps serial config requires a non-empty 'device' string".to_string()
                    );
                }
                let baud_ok = config
                    .get("baud")
                    .and_then(|v| v.as_u64())
                    .is_some_and(|b| ALLOWED_BAUD_RATES.contains(&(b as u32)));
                if !baud_ok {
                    return Err(format!(
                        "gps serial config 'baud' must be one of {ALLOWED_BAUD_RATES:?}"
                    ));
                }
                Ok(())
            }
            other => Err(format!(
                "unknown gps mode '{other}'; expected 'gpsd' or 'serial'"
            )),
        },
        "bluetooth" => {
            if mode != "scan" {
                return Err(format!(
                    "bluetooth data sources must use mode 'scan', got '{mode}'"
                ));
            }
            match interface {
                Some(i) if !i.trim().is_empty() => Ok(()),
                _ => Err("bluetooth data sources require a non-empty interface".to_string()),
            }
        }
        other => Err(format!(
            "unknown data source kind '{other}'; expected 'wifi', 'gps', or 'bluetooth'"
        )),
    }
}

/// One running data source's teardown handle.
enum RunningHandle {
    Wifi {
        capturer: Box<dyn Capturer>,
        reader: JoinHandle<()>,
    },
    /// A running gps source has nothing of its own to stop: its `GpsSource`
    /// was consumed by the shared `SessionManager` when the session opened
    /// (or, per the module docs' known limitation, wasn't wired in at all
    /// if a session was already open). Its presence in the running map is
    /// still what makes last-stop accounting correct.
    Gps,
}

/// Whether the currently-open session (if any) is backed by a real
/// `GpsSource` (opened for a gps-kind start) or [`InertGps`] (opened for a
/// wifi-kind start with no gps running yet). See module docs.
struct SharedSession {
    manager: Arc<SessionManager>,
    has_real_gps: bool,
}

/// The Task 6.2 orchestrator: turns `data_source` rows on and off. See
/// module docs for the full design (session lifecycle, factory injection,
/// known limitations).
pub struct CaptureSupervisor {
    pool: PgPool,
    events: broadcast::Sender<Event>,
    secret_key: [u8; 32],
    factory: Arc<dyn CapturerFactory>,
    session: Mutex<Option<SharedSession>>,
    /// Guards both the running-set map itself *and* serializes
    /// `start`/`stop` end-to-end (the lock is held for a whole call, not
    /// just the map mutation) — deliberately coarse-grained: this is a
    /// single-admin, homelab-scale app with no expectation of concurrent
    /// start/stop traffic, and holding one lock for the duration is what
    /// makes "check not-already-running, then start" and "check
    /// last-running, then close the session" both race-free without a
    /// second, more fiddly locking scheme.
    running: Mutex<HashMap<Uuid, RunningHandle>>,
}

impl CaptureSupervisor {
    pub fn new(
        pool: PgPool,
        events: broadcast::Sender<Event>,
        secret_key: [u8; 32],
        factory: Arc<dyn CapturerFactory>,
    ) -> Self {
        Self {
            pool,
            events,
            secret_key,
            factory,
            session: Mutex::new(None),
            running: Mutex::new(HashMap::new()),
        }
    }

    /// An `IngestCtx` with `sessions: None` — used only to build the
    /// host-zone hook, which is handed to `SessionManager::open` *before*
    /// the `SessionManager` (and thus the "full" ctx's `Some(Arc<...>)`)
    /// exists. See `IngestCtx::sessions`'s doc comment.
    fn hookless_ctx(&self) -> IngestCtx {
        IngestCtx {
            pool: self.pool.clone(),
            sessions: None,
            events: self.events.clone(),
            secret_key: self.secret_key,
        }
    }

    /// A fresh subscriber to the ingest `Event` broadcast channel (Task
    /// 7.1's `/ws` handler). `broadcast::Sender::subscribe` only sees events
    /// sent *after* this call, so a caller that must not miss the very next
    /// emission (e.g. a test) needs to subscribe before triggering it.
    pub fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// The most recent GPS fix seen by the currently-open session, if any
    /// (Phase 5's `GET /api/gps/status`). `None` whenever no session is
    /// open (nothing running at all, or only a wifi-only/`InertGps`-backed
    /// session — see module docs) or a real gps session is open but hasn't
    /// received its first fix yet; both cases are indistinguishable from
    /// here on purpose, since the handler derives its own `"acquiring"` vs
    /// `"disabled"` distinction separately from whether a `kind='gps'` data
    /// source is `running` (see `gps_status.rs`), not from this alone.
    pub async fn latest_gps_fix(&self) -> Option<GpsFix> {
        let guard = self.session.lock().await;
        guard.as_ref()?.manager.latest_fix()
    }

    fn ctx_for(&self, manager: Arc<SessionManager>) -> IngestCtx {
        IngestCtx {
            pool: self.pool.clone(),
            sessions: Some(manager),
            events: self.events.clone(),
            secret_key: self.secret_key,
        }
    }

    fn host_zone_hook(&self) -> HostZoneHook {
        let ctx = self.hookless_ctx();
        Arc::new(move |fix: GpsFix| {
            let ctx = ctx.clone();
            Box::pin(async move {
                update_host_zones(&ctx, &fix).await;
            })
        })
    }

    /// Ensure a session is open for a **wifi** start, opening one backed by
    /// [`InertGps`] if none exists yet. Reuses whatever session is already
    /// open (gps-backed or not) otherwise.
    ///
    /// Returns `(ctx, opened_fresh)`: `opened_fresh` is `true` only when
    /// *this* call was the one that opened a brand-new session (as opposed
    /// to reusing one already open from another running source). Callers
    /// that can still fail *after* this returns (see `start_wifi`) need that
    /// bit to know whether they're the one responsible for rolling the
    /// session back on such a failure — rolling back a session some other
    /// still-running source legitimately opened would incorrectly yank it
    /// out from under that source.
    async fn ensure_wifi_session(&self) -> anyhow::Result<(IngestCtx, bool)> {
        let mut guard = self.session.lock().await;
        if let Some(shared) = guard.as_ref() {
            return Ok((self.ctx_for(shared.manager.clone()), false));
        }
        let hook = self.host_zone_hook();
        let manager = SessionManager::open(
            self.pool.clone(),
            InertGps,
            SessionManagerConfig {
                inactivity_gap: WIFI_ONLY_SESSION_GAP,
                write_interval: Duration::ZERO,
            },
            hook,
        )
        .await?;
        let manager = Arc::new(manager);
        *guard = Some(SharedSession {
            manager: manager.clone(),
            has_real_gps: false,
        });
        Ok((self.ctx_for(manager), true))
    }

    /// Close and clear a session *this call itself* just opened via
    /// [`Self::ensure_wifi_session`], because the capturer it was opened for
    /// then failed to actually start. Without this, a failed wifi start
    /// would leave `self.session` permanently `Some(...)` with nothing in
    /// the running map: the DB's `survey_session` row would stay open
    /// forever (no last-stop to close it), and every subsequent gps start
    /// would be wrongly rejected by [`Self::ensure_gps_session`] ("session
    /// already open").
    ///
    /// Only ever called when the caller knows *it* opened the session this
    /// call (`ensure_wifi_session`'s `opened_fresh` was `true`) — `start`
    /// holds `self.running`'s lock for its entire call (see that field's
    /// doc comment), so nothing else can have raced in and touched
    /// `self.session` between opening it and this rollback.
    async fn rollback_freshly_opened_session(&self) {
        let mut guard = self.session.lock().await;
        if let Some(shared) = guard.take() {
            if let Err(err) = shared.manager.close().await {
                eprintln!(
                    "CaptureSupervisor: failed to close session while rolling back a failed \
                     capturer start: {err:#}"
                );
            }
        }
    }

    /// Ensure a session is open for a **gps** start, opening one backed by
    /// `gps` if none exists yet.
    ///
    /// Rejects starting a gps source if a session is *already* open,
    /// whether backed by another real gps source (concurrent gps sources
    /// aren't supported — a `SessionManager` can only be fed by one
    /// `GpsSource`) or by [`InertGps`] (an already-running wifi-only
    /// session can't be rewired onto a real gps source after the fact
    /// without reopening — and losing the continuity of — the session).
    /// Both are documented, deliberate limitations for this slice (see
    /// module docs); the caller (`start`) surfaces the rejection as this
    /// data source's `status = 'error'` + `last_error`, not a crash.
    async fn ensure_gps_session(&self, gps: Box<dyn GpsSource + Send>) -> anyhow::Result<()> {
        let mut guard = self.session.lock().await;
        if let Some(shared) = guard.as_ref() {
            // Rejected here, `gps` is simply never wired into anything and
            // is dropped when this function returns; that runs its own
            // `Drop` impl (e.g. `SerialGpsSource` explicitly stops its
            // reader thread, `GpsdSource`'s socket halves close via their
            // own default `Drop`) with no extra teardown needed here.
            if shared.has_real_gps {
                anyhow::bail!(
                    "a gps source is already driving the active session; only one \
                     concurrent gps source is supported"
                );
            } else {
                anyhow::bail!(
                    "cannot start a gps source while a session is already open without gps \
                     support; stop the other running source(s) first"
                );
            }
        }
        let hook = self.host_zone_hook();
        let manager = SessionManager::open(
            self.pool.clone(),
            BoxedGps(gps),
            SessionManagerConfig::default(),
            hook,
        )
        .await?;
        *guard = Some(SharedSession {
            manager: Arc::new(manager),
            has_real_gps: true,
        });
        Ok(())
    }

    async fn start_wifi(
        &self,
        data_source_id: Uuid,
        mut capturer: Box<dyn Capturer>,
    ) -> anyhow::Result<RunningHandle> {
        let (ctx, opened_session) = self.ensure_wifi_session().await?;
        let (tx, mut rx) = mpsc::channel::<RawObservation>(256);
        if let Err(err) = capturer.start(tx) {
            // The capturer failed to actually start (bad interface, no
            // monitor mode, permissions -- a realistic hardware failure).
            // If this call was the one that just opened the shared session,
            // it must not linger: `start`'s caller will mark this source
            // `error` and never reach the `running.insert` below, so
            // nothing would ever close it otherwise. A session that was
            // already open before this call (from another running source)
            // is left untouched.
            if opened_session {
                self.rollback_freshly_opened_session().await;
            }
            return Err(err);
        }

        let reader = tokio::spawn(async move {
            while let Some(obs) = rx.recv().await {
                if let Err(err) = ingest(&ctx, data_source_id, obs).await {
                    // No tracing/log crate is wired into this workspace yet
                    // (same situation `session.rs`'s ingest loop documents)
                    // -- a single failed ingest (most commonly: the shared
                    // session closed out from under this still-running
                    // wifi source, see the module docs' known limitation)
                    // must not kill the whole reader task; log and keep
                    // draining.
                    eprintln!(
                        "CaptureSupervisor: ingest failed for data source {data_source_id}: {err:#}"
                    );
                }
            }
        });

        Ok(RunningHandle::Wifi { capturer, reader })
    }

    async fn start_gps(&self, gps: Box<dyn GpsSource + Send>) -> anyhow::Result<RunningHandle> {
        // No rollback needed here, unlike `start_wifi`: `ensure_gps_session`
        // only ever sets `self.session` *after* `SessionManager::open`
        // already succeeded, and there is no further fallible step in this
        // function afterward -- so a failure from the line below always
        // means no session was opened at all, nothing to roll back.
        self.ensure_gps_session(gps).await?;
        Ok(RunningHandle::Gps)
    }

    /// Start capturing from `data_source_id`.
    ///
    /// No-op (`Ok(())`, nothing touched) if it's already running -- a
    /// client retrying or double-clicking "start" shouldn't produce an
    /// error. On any failure (unknown id, invalid config, factory build
    /// failure, or a rejected session-sharing attempt -- see
    /// [`Self::ensure_gps_session`]) the row's `status` is set to `'error'`
    /// with `last_error` describing why, and this returns `Err` with the
    /// same message; it never panics.
    pub async fn start(&self, data_source_id: Uuid) -> anyhow::Result<()> {
        let mut running = self.running.lock().await;
        if running.contains_key(&data_source_id) {
            return Ok(());
        }

        let source = DataSourceRepo::get(&self.pool, data_source_id)
            .await?
            .ok_or_else(|| anyhow!("data source {data_source_id} not found"))?;

        if let Err(msg) = validate_data_source(
            &source.kind,
            &source.mode,
            source.interface.as_deref(),
            &source.config,
        ) {
            DataSourceRepo::set_status(&self.pool, data_source_id, "error", Some(&msg)).await?;
            return Err(anyhow!(msg));
        }

        let built = match self.factory.build(&source).await {
            Ok(built) => built,
            Err(err) => {
                DataSourceRepo::set_status(
                    &self.pool,
                    data_source_id,
                    "error",
                    Some(&err.to_string()),
                )
                .await?;
                return Err(err);
            }
        };

        let handle = match built {
            BuiltCapture::Wifi(capturer) => self.start_wifi(data_source_id, capturer).await,
            BuiltCapture::Gps(gps) => self.start_gps(gps).await,
        };

        let handle = match handle {
            Ok(handle) => handle,
            Err(err) => {
                DataSourceRepo::set_status(
                    &self.pool,
                    data_source_id,
                    "error",
                    Some(&err.to_string()),
                )
                .await?;
                return Err(err);
            }
        };

        running.insert(data_source_id, handle);
        DataSourceRepo::set_status(&self.pool, data_source_id, "running", None).await?;
        Ok(())
    }

    /// Reconcile persisted `status = 'running'` rows with this freshly-built
    /// supervisor's (empty) in-memory running set by actually (re)starting
    /// capture for each. Call once at process startup: a data source's
    /// `status` survives a restart in Postgres, but the running capturers and
    /// the shared survey session do not — so without this, a source that was
    /// running when the process last stopped comes back as a phantom
    /// "running" that captures nothing (GPS never acquires, wifi ingests
    /// nothing), and whose Stop button, with no in-memory handle to remove,
    /// would silently no-op (now also handled defensively in [`Self::stop`]).
    ///
    /// GPS sources are resumed before wifi sources so the shared session is
    /// opened gps-backed: a wifi-first resume would open an [`InertGps`]
    /// session that a subsequent gps resume can't join (see
    /// [`Self::ensure_gps_session`]), needlessly downgrading a gps+wifi survey
    /// to wifi-only and flipping the gps source to `error`.
    ///
    /// Best-effort per source and never fatal: a source whose hardware isn't
    /// available at boot is flipped to `error`/`last_error` by [`Self::start`]
    /// itself, leaving the others (and startup) unaffected.
    pub async fn resume_running(&self) {
        let sources = match DataSourceRepo::list(&self.pool).await {
            Ok(sources) => sources,
            Err(err) => {
                eprintln!(
                    "CaptureSupervisor: could not list data sources to resume after restart: \
                     {err:#}"
                );
                return;
            }
        };

        let mut to_resume: Vec<DataSource> = sources
            .into_iter()
            .filter(|source| source.status == "running")
            .collect();
        // gps (0) before wifi (1) — see the doc comment above.
        to_resume.sort_by_key(|source| if source.kind == "gps" { 0 } else { 1 });

        for source in to_resume {
            if let Err(err) = self.start(source.id).await {
                eprintln!(
                    "CaptureSupervisor: failed to resume data source {} ({}) after restart: {err:#}",
                    source.id, source.kind
                );
            }
        }
    }

    /// Stop capturing from `data_source_id`.
    ///
    /// No-op (`Ok(())`) if it isn't currently running. Otherwise stops its
    /// capturer/loop, marks it `'stopped'`, and — if this was the last
    /// running source — closes the shared session (see module docs for the
    /// known limitation when a gps source stops while others remain
    /// running).
    pub async fn stop(&self, data_source_id: Uuid) -> anyhow::Result<()> {
        let mut running = self.running.lock().await;
        let Some(handle) = running.remove(&data_source_id) else {
            // No in-memory handle. Usually the source is already stopped and
            // this is a harmless no-op — but after a process restart the DB
            // can still carry `status = 'running'` for a source this fresh
            // supervisor never started (its in-memory set starts empty;
            // `resume_running` normally repopulates it, but a source that
            // failed to resume — or a Stop that races resume — would
            // otherwise be stuck "running" forever with no handle to remove).
            // Reconcile only that phantom-running case, leaving an already
            // `stopped`/`error` row (and its `last_error`) untouched.
            if let Some(source) = DataSourceRepo::get(&self.pool, data_source_id).await? {
                if source.status == "running" {
                    DataSourceRepo::set_status(&self.pool, data_source_id, "stopped", None).await?;
                }
            }
            return Ok(());
        };

        match handle {
            RunningHandle::Wifi {
                mut capturer,
                reader,
            } => {
                // `Capturer::stop` is a synchronous method that, for real
                // hardware (`WifiMonitorCapturer`), blocks on joining its
                // capture thread -- run it on the blocking-pool so it never
                // stalls this async task/the runtime's worker threads.
                tokio::task::spawn_blocking(move || capturer.stop())
                    .await
                    .map_err(|err| anyhow!("capturer stop task panicked: {err}"))?;
                // The reader task ends on its own once `stop()` drops the
                // capturer's sender half (closing the channel); await it so
                // `stop()` doesn't return before the last in-flight
                // observation has actually been ingested.
                let _ = reader.await;
            }
            RunningHandle::Gps => {}
        }

        DataSourceRepo::set_status(&self.pool, data_source_id, "stopped", None).await?;

        if running.is_empty() {
            let mut session_guard = self.session.lock().await;
            if let Some(shared) = session_guard.take() {
                shared.manager.close().await?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod validate_data_source_tests {
    use super::validate_data_source;
    use serde_json::json;

    #[test]
    fn bluetooth_scan_with_interface_is_ok() {
        assert!(validate_data_source("bluetooth", "scan", Some("hci0"), &json!({})).is_ok());
        // active_scan / auto_create_emitters are optional booleans; their
        // absence or presence never fails validation.
        assert!(validate_data_source(
            "bluetooth",
            "scan",
            Some("hci0"),
            &json!({"auto_create_emitters": true, "active_scan": true})
        )
        .is_ok());
    }

    #[test]
    fn bluetooth_rejects_non_scan_mode() {
        let err = validate_data_source("bluetooth", "sniff", Some("hci0"), &json!({})).unwrap_err();
        assert!(err.contains("scan"), "message should mention scan: {err}");
    }

    #[test]
    fn bluetooth_rejects_empty_interface() {
        assert!(validate_data_source("bluetooth", "scan", None, &json!({})).is_err());
        assert!(validate_data_source("bluetooth", "scan", Some("   "), &json!({})).is_err());
    }

    #[test]
    fn unknown_kind_message_lists_bluetooth() {
        let err = validate_data_source("zigbee", "scan", Some("x"), &json!({})).unwrap_err();
        assert!(
            err.contains("bluetooth"),
            "message should list bluetooth: {err}"
        );
    }
}
