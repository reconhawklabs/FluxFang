//! `CaptureSupervisor`: the orchestration layer that turns a configured
//! `data_source` row into an actually-running capture (a wifi monitor-mode
//! sniffer or a GPS fix stream), and back.
//!
//! ## Pieces
//!
//! - [`CapturerFactory`] is the seam between this module and real hardware:
//!   given a `data_source` row it builds either a `fluxfang_capture::Capturer`
//!   (wifi) or a `fluxfang_capture::LocationSource` (gps). [`RealCapturerFactory`]
//!   builds the genuine hardware-touching types
//!   (`WifiMonitorCapturer`/`GpsdSource`/`SerialGpsSource`); [`MockCapturerFactory`]
//!   builds `fluxfang_capture::mock::{MockCapturer, MockGps}` instead, so
//!   this module's own tests (and `tests/data_sources.rs`) never touch
//!   hardware. Which one `AppState` holds is a constructor argument — see
//!   `state.rs`.
//! - [`CaptureSupervisor`] holds the running set (`data_source_id ->
//!   RunningHandle`), one shared [`LocationProvider`] (the current fix), and
//!   the one shared "survey session" (see below), and implements
//!   [`CaptureSupervisor::start`]/[`CaptureSupervisor::stop`].
//!
//! ## Session lifecycle: first-start opens, last-stop closes (GPS-agnostic)
//!
//! Every `ingest()` call requires an active `survey_session` (see
//! `ingest`'s module docs). So starting *any* data source — wifi or gps —
//! must ensure a session is open, and stopping the *last* running source
//! must close it. This module tracks that with one
//! `Mutex<Option<Arc<SessionManager>>>`:
//!
//! - The **first** source of any kind opens a fresh `SessionManager`
//!   ([`Self::ensure_session`]); the session is no longer tied to GPS.
//! - A **location** source additionally spins up a [`LocationPump`] that
//!   feeds the shared [`LocationProvider`] and logs the host track into
//!   `location_fix`, with the [`host_zone_hook`] closing the loop back to
//!   `zones::update_host_zones`. Only **one** location source may run at a
//!   time ([`Self::a_location_source_is_running`]); a second is rejected as
//!   `error`.
//! - Any subsequent source, once a session is already open, just reuses it.
//! - [`CaptureSupervisor::stop`] closes the session once the running set
//!   becomes empty.
//!
//! ## Device failure
//!
//! A running source whose device dies (a wifi capturer's channel closing
//! unexpectedly, or a location source's `next_fix()` returning `None`)
//! reports its id on the failure channel; [`Self::spawn_background`]'s drain
//! task flips it to `status='error'`, drops its handle, and closes the
//! session if it was the last one. [`Self::spawn_background`]'s 10s recovery
//! reconciler ([`Self::reconcile_once`]) then brings such a source back while
//! `desired_state='running'` — the auto-recover-on-replug behavior.
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
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use fluxfang_capture::gps::ALLOWED_BAUD_RATES;
use fluxfang_capture::mock::{MockCapturer, MockGps};
use fluxfang_capture::wifi::monitor::WifiMonitorCapturer;
use fluxfang_capture::wifi::scan::WifiScanCapturer;
use fluxfang_capture::{Capturer, GpsFix, LocationSource, RawObservation};
use fluxfang_db::models::DataSource;
use fluxfang_db::DataSourceRepo;
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::ingest::location::LocationProvider;
use crate::ingest::pump::{LocationPump, OnExhausted};
use crate::ingest::session::{HostZoneHook, SessionManager};
use crate::ingest::zones::update_host_zones;
use crate::ingest::{ingest, Event, IngestCtx};

/// How often the recovery reconciler retries sources the user wants running
/// but that aren't (failed device, not-yet-available hardware at boot).
const RECONCILE_INTERVAL: Duration = Duration::from_secs(10);

/// What a [`CapturerFactory`] builds for one `data_source` row, matching its
/// `kind`.
pub enum BuiltCapture {
    Wifi(Box<dyn Capturer>),
    Location(Box<dyn LocationSource>),
    Bluetooth(Box<dyn Capturer>),
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
                    Ok(BuiltCapture::Location(Box::new(gps)))
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
                    Ok(BuiltCapture::Location(Box::new(gps)))
                }
                other => Err(anyhow!("unsupported gps mode '{other}'")),
            },
            "bluetooth" => {
                let interface = source
                    .interface
                    .clone()
                    .ok_or_else(|| anyhow!("bluetooth data source is missing its interface"))?;
                let active_scan = source
                    .config
                    .get("active_scan")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Ok(BuiltCapture::Bluetooth(Box::new(
                    fluxfang_capture::bluetooth::BluetoothScanCapturer::new(interface, active_scan),
                )))
            }
            "rtl_sdr" => {
                let frequency = source
                    .config
                    .get("frequency")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("rtl_sdr data source missing 'frequency'"))?
                    .to_string();
                let device_serial = source
                    .config
                    .get("device_serial")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                // Reuse BuiltCapture::Wifi: it's an emitting source that
                // routes through start_wifi (the GPS-less capture path), same
                // as Bluetooth.
                Ok(BuiltCapture::Wifi(Box::new(
                    fluxfang_capture::rtl::TpmsCapturer::new(frequency, device_serial),
                )))
            }
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
    /// between fixes at all — a finite, non-looping track drains almost
    /// instantly, and the [`LocationPump`] then treats that exhaustion as a
    /// device disconnect (reporting the source as failed). Looping keeps the
    /// pump feeding the shared [`LocationProvider`] for as long as the caller
    /// needs (e.g. an end-to-end test driving several more HTTP requests, or a
    /// gps-status poll expecting a fresh fix), only ending when the gps data
    /// source is explicitly stopped.
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
                Ok(BuiltCapture::Location(Box::new(gps)))
            }
            "bluetooth" => {
                let observations = self
                    .wifi_observations
                    .lock()
                    .expect("mutex poisoned")
                    .clone();
                let capturer = MockCapturer::new(observations, Duration::from_millis(5));
                Ok(BuiltCapture::Bluetooth(Box::new(capturer)))
            }
            "rtl_sdr" => {
                let observations = self
                    .wifi_observations
                    .lock()
                    .expect("mutex poisoned")
                    .clone();
                let capturer = MockCapturer::new(observations, Duration::from_millis(5));
                Ok(BuiltCapture::Wifi(Box::new(capturer)))
            }
            other => Err(anyhow!("MockCapturerFactory: unsupported kind '{other}'")),
        }
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
        "rtl_sdr" => {
            if mode != "tpms" {
                return Err(format!(
                    "rtl_sdr data sources must use mode 'tpms', got '{mode}'"
                ));
            }
            let freq_ok = config
                .get("frequency")
                .and_then(Value::as_str)
                .is_some_and(|f| f == "315M" || f == "433.92M");
            if !freq_ok {
                return Err(
                    "rtl_sdr tpms config requires a 'frequency' of '315M' or '433.92M'".to_string(),
                );
            }
            if let Some(serial) = config.get("device_serial") {
                let serial_ok = serial.as_str().is_some_and(|s| !s.trim().is_empty());
                if !serial_ok {
                    return Err(
                        "rtl_sdr 'device_serial' must be a non-empty string when provided"
                            .to_string(),
                    );
                }
            }
            Ok(())
        }
        other => Err(format!(
            "unknown data source kind '{other}'; expected 'wifi', 'gps', 'bluetooth', or 'rtl_sdr'"
        )),
    }
}

/// One running data source's teardown handle.
enum RunningHandle {
    Wifi {
        capturer: Box<dyn Capturer>,
        reader: JoinHandle<()>,
        /// Set true when *we* asked the capturer to stop, so the reader task
        /// can distinguish a deliberate stop from an unexpected device death
        /// when its channel closes (fully exercised in a later task; declared
        /// here so the type is stable).
        stopping: Arc<std::sync::atomic::AtomicBool>,
    },
    Location {
        pump: LocationPump,
    },
}

/// The orchestrator: turns `data_source` rows on and off. See module docs
/// for the full design (session lifecycle, factory injection, known
/// limitations).
pub struct CaptureSupervisor {
    pool: PgPool,
    events: broadcast::Sender<Event>,
    secret_key: [u8; 32],
    factory: Arc<dyn CapturerFactory>,
    /// The shared "where am I?" value, fed by the running location source's
    /// [`LocationPump`] and read by every ingest task + the gps status
    /// endpoint.
    provider: Arc<LocationProvider>,
    session: Mutex<Option<Arc<SessionManager>>>,
    /// Guards both the running-set map itself *and* serializes
    /// `start`/`stop` end-to-end (the lock is held for a whole call, not
    /// just the map mutation) — deliberately coarse-grained: this is a
    /// single-admin, homelab-scale app with no expectation of concurrent
    /// start/stop traffic, and holding one lock for the duration is what
    /// makes "check not-already-running, then start" and "check
    /// last-running, then close the session" both race-free without a
    /// second, more fiddly locking scheme.
    running: Mutex<HashMap<Uuid, RunningHandle>>,
    /// Sender half of the failure channel; each running source's task sends
    /// its own id here on unexpected death. Drained by `spawn_background`.
    failure_tx: mpsc::UnboundedSender<Uuid>,
    /// The receiver half, taken by the first `spawn_background` call.
    failure_rx: StdMutex<Option<mpsc::UnboundedReceiver<Uuid>>>,
}

impl CaptureSupervisor {
    pub fn new(
        pool: PgPool,
        events: broadcast::Sender<Event>,
        secret_key: [u8; 32],
        factory: Arc<dyn CapturerFactory>,
    ) -> Self {
        let (failure_tx, failure_rx) = mpsc::unbounded_channel();
        Self {
            pool,
            events,
            secret_key,
            factory,
            provider: Arc::new(LocationProvider::new()),
            session: Mutex::new(None),
            running: Mutex::new(HashMap::new()),
            failure_tx,
            failure_rx: StdMutex::new(Some(failure_rx)),
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
            location: self.provider.clone(),
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

    /// The most recent raw GPS fix in the shared [`LocationProvider`], if any
    /// (Phase 5's `GET /api/gps/status`). `None` whenever no location source
    /// has fed a fix yet (or one was cleared on stop); the handler derives its
    /// own `"acquiring"`/`"disabled"`/`"degraded"` distinctions from this plus
    /// whether a `kind='gps'` data source is `running` (see `gps_status.rs`).
    pub async fn latest_gps_fix(&self) -> Option<GpsFix> {
        self.provider.latest_raw()
    }

    fn ctx_for(&self, manager: Arc<SessionManager>) -> IngestCtx {
        IngestCtx {
            pool: self.pool.clone(),
            sessions: Some(manager),
            location: self.provider.clone(),
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

    /// Ensure a `survey_session` is open, opening one if none exists. Any
    /// data source of any kind can be the opener — the session is no longer
    /// tied to GPS. Returns the ingest `ctx` and the shared session manager.
    async fn ensure_session(&self) -> anyhow::Result<(IngestCtx, Arc<SessionManager>)> {
        let mut guard = self.session.lock().await;
        if let Some(manager) = guard.as_ref() {
            return Ok((self.ctx_for(manager.clone()), manager.clone()));
        }
        let manager = Arc::new(SessionManager::open(self.pool.clone()).await?);
        *guard = Some(manager.clone());
        Ok((self.ctx_for(manager.clone()), manager))
    }

    /// Whether any currently-running source is a location source (only one is
    /// allowed at a time). Associated fn — inspects the passed running map
    /// without locking (the caller already holds the `running` guard).
    fn a_location_source_is_running(running: &HashMap<Uuid, RunningHandle>) -> bool {
        running
            .values()
            .any(|h| matches!(h, RunningHandle::Location { .. }))
    }

    /// Close the shared session iff `running` is empty. Caller passes its
    /// already-held `running` guard (no re-lock — avoids deadlock).
    async fn close_session_if_idle_locked(&self, running: &mut HashMap<Uuid, RunningHandle>) {
        if running.is_empty() {
            let mut guard = self.session.lock().await;
            if let Some(manager) = guard.take() {
                if let Err(err) = manager.close().await {
                    eprintln!("CaptureSupervisor: failed to close idle session: {err:#}");
                }
            }
        }
    }

    async fn start_wifi(
        &self,
        data_source_id: Uuid,
        mut capturer: Box<dyn Capturer>,
    ) -> anyhow::Result<RunningHandle> {
        let (ctx, _session) = self.ensure_session().await?;
        let (tx, mut rx) = mpsc::channel::<RawObservation>(256);
        // `Capturer::start` can now block for several seconds -- the bluetooth
        // capturer performs a startup handshake that waits on a channel -- so
        // run it on the blocking-pool instead of directly on this async task,
        // otherwise it stalls a runtime worker thread (and, because
        // `CaptureSupervisor::start` awaits this while holding `self.running`,
        // serializes all data-source start/stop). `start` takes `&mut capturer`
        // and we still need `capturer` afterward for the `RunningHandle`, so
        // move it into the closure and hand it back out alongside the result.
        let (capturer, start_result) = tokio::task::spawn_blocking(move || {
            let result = capturer.start(tx);
            (capturer, result)
        })
        .await
        .map_err(|err| anyhow!("capturer start task panicked: {err}"))?;
        // The capturer failed to actually start (bad interface, no monitor
        // mode, permissions -- a realistic hardware failure). `start` (our
        // caller) holds the `running` lock and will close the session via
        // `close_session_if_idle_locked` on this Err. Don't re-lock `running`
        // here.
        start_result?;

        let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping_reader = stopping.clone();
        let failure_tx = self.failure_tx.clone();
        let reader = tokio::spawn(async move {
            while let Some(obs) = rx.recv().await {
                if let Err(err) = ingest(&ctx, data_source_id, obs).await {
                    // No tracing/log crate is wired into this workspace yet
                    // (same situation `ingest`'s own docs note) -- a single
                    // failed ingest must not kill the whole reader task; log
                    // and keep draining.
                    eprintln!(
                        "CaptureSupervisor: ingest failed for data source {data_source_id}: {err:#}"
                    );
                }
            }
            // Channel closed. If we weren't asked to stop, the capturer died
            // unexpectedly (device unplugged / driver error) -> report failure.
            if !stopping_reader.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = failure_tx.send(data_source_id);
            }
        });

        Ok(RunningHandle::Wifi {
            capturer,
            reader,
            stopping,
        })
    }

    /// Start a location source: spin up a [`LocationPump`] feeding the shared
    /// [`LocationProvider`]. The one-location-source check is done by `start`
    /// before dispatch, so this does not re-lock `running`.
    async fn start_location(
        &self,
        data_source_id: Uuid,
        source: Box<dyn LocationSource>,
    ) -> anyhow::Result<RunningHandle> {
        let (_ctx, session) = self.ensure_session().await?;
        let provider = self.provider.clone();
        let hook = self.host_zone_hook();
        let failure_tx = self.failure_tx.clone();
        let on_exhausted: OnExhausted = Arc::new(move || {
            let failure_tx = failure_tx.clone();
            Box::pin(async move {
                let _ = failure_tx.send(data_source_id);
            })
        });
        let pump = LocationPump::start(
            self.pool.clone(),
            source,
            provider,
            session,
            Duration::ZERO,
            hook,
            on_exhausted,
        );
        Ok(RunningHandle::Location { pump })
    }

    /// Start capturing from `data_source_id`.
    ///
    /// No-op (`Ok(())`, nothing touched) if it's already running -- a
    /// client retrying or double-clicking "start" shouldn't produce an
    /// error. On any failure (unknown id, invalid config, factory build
    /// failure, or a second concurrent location source) the row's `status`
    /// is set to `'error'` with `last_error` describing why, and this returns
    /// `Err` with the same message; it never panics.
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

        // One location source at a time.
        let is_location = source.kind == "gps"; // future: || source.kind == "static_location"
        if is_location && Self::a_location_source_is_running(&running) {
            let msg = "another location source is already running; stop it first".to_string();
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
            BuiltCapture::Bluetooth(capturer) => self.start_wifi(data_source_id, capturer).await,
            BuiltCapture::Location(source) => self.start_location(data_source_id, source).await,
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
                // start_wifi/start_location may have opened the session; close
                // if nothing ended up running.
                self.close_session_if_idle_locked(&mut running).await;
                return Err(err);
            }
        };

        running.insert(data_source_id, handle);
        DataSourceRepo::set_status(&self.pool, data_source_id, "running", None).await?;
        Ok(())
    }

    /// Reconcile persisted `desired_state = 'running'` rows with this
    /// freshly-built supervisor's (empty) in-memory running set by actually
    /// (re)starting capture for each. Call once at process startup: a data
    /// source's `desired_state` — the user's recorded intent, set by
    /// `data_sources::start_data_source`/`stop_data_source` via
    /// `DataSourceRepo::set_desired_state` — survives a restart in Postgres,
    /// but the running capturers and the shared survey session do not — so
    /// without this, a source the user wanted running comes back as a
    /// phantom "running" `status` that captures nothing (GPS never acquires,
    /// wifi ingests nothing), and whose Stop button, with no in-memory
    /// handle to remove, would silently no-op (now also handled defensively
    /// in [`Self::stop`]).
    ///
    /// Keying off `desired_state` rather than `status` also means a source
    /// left in `'error'` after a crash (its device died mid-run, per
    /// [`Self::handle_source_failed`]) still comes back on the next restart,
    /// since the user's intent to have it running never changed.
    ///
    /// No kind ordering: the shared session is GPS-agnostic (see
    /// [`Self::ensure_session`]) and opened by whichever source starts
    /// first, so sources are resumed in whatever order
    /// `DataSourceRepo::list` returns them.
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

        let to_resume: Vec<DataSource> = sources
            .into_iter()
            .filter(|source| source.desired_state == "running")
            .collect();
        // No kind ordering: the session is GPS-agnostic now.

        for source in to_resume {
            if let Err(err) = self.start(source.id).await {
                eprintln!(
                    "CaptureSupervisor: failed to resume data source {} ({}) after restart: {err:#}",
                    source.id, source.kind
                );
            }
        }
    }

    /// One reconciliation sweep: (re)start every source the user wants
    /// running that isn't currently in the running set. `start` itself is a
    /// no-op for already-running sources and records `error`/`last_error` on
    /// failure, so this is safe to call repeatedly.
    async fn reconcile_once(&self) {
        let sources = match DataSourceRepo::list(&self.pool).await {
            Ok(s) => s,
            Err(err) => {
                eprintln!("CaptureSupervisor: reconcile could not list sources: {err:#}");
                return;
            }
        };
        let want: Vec<Uuid> = {
            let running = self.running.lock().await;
            sources
                .into_iter()
                .filter(|s| s.desired_state == "running" && !running.contains_key(&s.id))
                .map(|s| s.id)
                .collect()
        };
        for id in want {
            if let Err(err) = self.start(id).await {
                // Expected while hardware is absent; start() already recorded
                // error/last_error. Log once per attempt for visibility.
                eprintln!("CaptureSupervisor: reconcile retry for {id} failed: {err:#}");
            }
        }
    }

    /// Stop capturing from `data_source_id`.
    ///
    /// No-op (`Ok(())`) if it isn't currently running. Otherwise stops its
    /// capturer/pump, marks it `'stopped'`, clears the provider (for a
    /// location source), and — if this was the last running source — closes
    /// the shared session.
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
                stopping,
            } => {
                // Mark the stop deliberate so the reader task doesn't report
                // the channel closing as an unexpected device death.
                stopping.store(true, std::sync::atomic::Ordering::SeqCst);
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
            RunningHandle::Location { pump } => {
                pump.abort();
                // User-initiated stop clears the provider so emissions read
                // `none` immediately (a failure would leave it `stale`).
                self.provider.clear();
            }
        }

        DataSourceRepo::set_status(&self.pool, data_source_id, "stopped", None).await?;
        self.close_session_if_idle_locked(&mut running).await;
        Ok(())
    }

    /// Start the supervisor's background tasks: the failure drain (flips a
    /// disconnected source to `error` and drops its handle) and the 10s
    /// recovery reconciler (re-starts any `desired_state='running'` source
    /// that isn't running). Together these deliver unplug → `error` →
    /// auto-recover-on-replug. Call once at startup, after construction, with
    /// the `Arc<CaptureSupervisor>` from `AppState`.
    pub fn spawn_background(self: &Arc<Self>) {
        let rx = self
            .failure_rx
            .lock()
            .expect("failure_rx mutex poisoned")
            .take();
        let Some(mut rx) = rx else {
            return; // already started
        };
        let this = self.clone();
        tokio::spawn(async move {
            while let Some(id) = rx.recv().await {
                this.handle_source_failed(id).await;
            }
        });

        let reconciler = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);
            loop {
                ticker.tick().await;
                reconciler.reconcile_once().await;
            }
        });
    }

    /// A running source's device died. Mark it `error` and drop its handle so
    /// [`Self::reconcile_once`] can retry it while `desired_state='running'`.
    /// Does NOT clear the provider (a lost fix stays `stale` and ages out).
    async fn handle_source_failed(&self, data_source_id: Uuid) {
        let mut running = self.running.lock().await;
        // Only act if we still think it's running (ignore races with stop()).
        if running.remove(&data_source_id).is_none() {
            return;
        }
        let msg = "capture device stopped unexpectedly (disconnected or failed)";
        if let Err(err) =
            DataSourceRepo::set_status(&self.pool, data_source_id, "error", Some(msg)).await
        {
            eprintln!("CaptureSupervisor: failed to mark source {data_source_id} error: {err:#}");
        }
        self.close_session_if_idle_locked(&mut running).await;
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

    #[test]
    fn rtl_sdr_tpms_with_frequency_is_ok() {
        assert!(validate_data_source(
            "rtl_sdr",
            "tpms",
            None,
            &json!({"frequency": "315M", "device_serial": "67475624"})
        )
        .is_ok());
        // device_serial optional (single-device fallback); auto flags ignored.
        assert!(validate_data_source(
            "rtl_sdr",
            "tpms",
            None,
            &json!({"frequency": "433.92M", "auto_create_emitters": true, "auto_correlate_tpms": true})
        )
        .is_ok());
    }

    #[test]
    fn rtl_sdr_rejects_bad_frequency_and_mode() {
        let bad_freq = validate_data_source("rtl_sdr", "tpms", None, &json!({"frequency": "868M"}))
            .unwrap_err();
        assert!(bad_freq.contains("frequency"), "got: {bad_freq}");
        let bad_mode = validate_data_source("rtl_sdr", "adsb", None, &json!({"frequency": "315M"}))
            .unwrap_err();
        assert!(bad_mode.contains("tpms"), "got: {bad_mode}");
    }

    #[test]
    fn rtl_sdr_rejects_blank_device_serial_when_present() {
        let err = validate_data_source(
            "rtl_sdr",
            "tpms",
            None,
            &json!({"frequency": "315M", "device_serial": "   "}),
        )
        .unwrap_err();
        assert!(err.contains("device_serial"), "got: {err}");
    }
}

#[cfg(test)]
mod reconcile_tests {
    use std::sync::Arc;

    use fluxfang_db::models::NewDataSource;
    use fluxfang_db::DataSourceRepo;
    use tokio::sync::broadcast;

    use super::{CaptureSupervisor, MockCapturerFactory};
    use crate::test_support::fresh_pool;

    /// The recovery reconciler brings back a source the user wants running
    /// (`desired_state = 'running'`) that isn't in the running set and is
    /// sitting in `status = 'error'` — exactly the state
    /// `handle_source_failed` leaves a device that died mid-capture (status
    /// flipped to `error`, handle dropped from the running set). A fresh
    /// supervisor's running set is empty, so the row's absence from it models
    /// that post-failure state directly. One `reconcile_once()` pass, with a
    /// factory that now succeeds (the device is "replugged"), must
    /// (re)`start` it and flip `status` back to `running` — the
    /// unplug→error→auto-recover-on-replug behavior.
    #[tokio::test]
    async fn reconcile_restarts_a_failed_desired_running_source() {
        let pool = fresh_pool().await;

        // Arrange: a wifi source the user wants running, currently `error`
        // and absent from the (empty) running set — the state a failed device
        // is left in by `handle_source_failed`.
        let created = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0"))
            .await
            .expect("insert data source");
        DataSourceRepo::set_desired_state(&pool, created.id, "running")
            .await
            .expect("mark desired_state running");
        DataSourceRepo::set_status(&pool, created.id, "error", Some("device died"))
            .await
            .expect("mark status error");

        // Guard non-vacuity: it really is in the failed state pre-reconcile.
        let before = DataSourceRepo::get(&pool, created.id)
            .await
            .expect("get")
            .expect("row exists");
        assert_eq!(before.status, "error", "precondition: source is failed");

        // A factory that now succeeds (the device is back), and a fresh
        // supervisor whose running set is empty.
        let factory = Arc::new(MockCapturerFactory::new());
        let (events_tx, _events_rx) = broadcast::channel(8);
        let supervisor = CaptureSupervisor::new(pool.clone(), events_tx, [0u8; 32], factory);

        // Act: one reconciliation sweep. (Called directly, not via
        // `spawn_background`'s 10s loop, so the suite stays deterministic.)
        supervisor.reconcile_once().await;

        // Assert: the row genuinely transitioned back to running.
        let after = DataSourceRepo::get(&pool, created.id)
            .await
            .expect("get")
            .expect("row exists");
        assert_eq!(
            after.status, "running",
            "reconcile_once should (re)start a desired-running source that isn't running"
        );
    }
}
