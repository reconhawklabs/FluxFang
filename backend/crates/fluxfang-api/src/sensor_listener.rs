//! `SensorListenerManager`: binds/tears-down a dedicated network listener per
//! enabled `sensor` datasource. A sensor datasource is a network endpoint
//! (not a capture device), so it is driven here rather than by the
//! `CaptureSupervisor`. Each running listener is its own `axum::serve` on the
//! datasource's `bind_ip:bind_port`, tracked by data_source id.
//!
//! `GET /sensor/health` provides liveness; `POST /sensor/enroll` (Phase 3A)
//! lets a sensor self-register during an open enrollment window. Ingest
//! routes are added in later phases; they will extend [`listener_router`].

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, State};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use uuid::Uuid;

use fluxfang_db::{DataSourceRepo, SensorRepo};

use crate::ingest::IngestCtx;

/// Fixed enrollment-window length. No longer operator-configurable per
/// datasource — a single short, predictable default (15 minutes).
pub const DEFAULT_ENROLLMENT_WINDOW_SECS: u64 = 900;

/// Per-datasource enrollment-window expiry (monotonic). Absent/past = closed.
pub(crate) type WindowMap = Arc<tokio::sync::Mutex<HashMap<Uuid, Instant>>>;

/// True if `id` has a window whose expiry is still in the future.
pub(crate) fn window_is_open(map: &HashMap<Uuid, Instant>, id: Uuid) -> bool {
    map.get(&id).is_some_and(|&exp| exp > Instant::now())
}

/// A running listener: a shutdown trigger + the serving task's handle.
struct ListenerHandle {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

/// Manages the lifecycle of sensor-datasource network listeners.
pub struct SensorListenerManager {
    pool: PgPool,
    running: Mutex<HashMap<Uuid, ListenerHandle>>,
    windows: WindowMap,
    /// Carried through to each listener's [`EnrollState`] so the
    /// `/sensor/ingest` handler (Task 6) can call `ingest::ingest_remote`.
    /// Unused until then.
    ingest: IngestCtx,
}

/// State shared into a listener's router: the DB pool, THIS listener's
/// datasource id, the enrollment-window map, and the `IngestCtx` used by the
/// (Task 6) `/sensor/ingest` handler.
#[derive(Clone)]
pub(crate) struct EnrollState {
    pub pool: PgPool,
    pub data_source_id: Uuid,
    pub windows: WindowMap,
    pub ingest: IngestCtx,
}

#[derive(Deserialize)]
struct EnrollRequest {
    sensor_id: String,
    /// One-way fingerprint of the sensor's key — the key itself is NEVER sent
    /// over the wire. The operator supplies the actual key in the approval
    /// dialog, and the Standalone verifies it reproduces this fingerprint.
    fingerprint: String,
}

/// Slug rule shared with Phase 1 setup: non-empty, ≤64, `[A-Za-z0-9_-]`.
fn is_valid_sensor_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Fingerprint wire format, matching `fluxfang_sensor_proto::fingerprint`:
/// eight uppercase hex bytes joined by dashes, e.g. `A1-B2-C3-D4-E5-F6-07-18`.
/// Validating the shape keeps malformed/oversized input out of the keyring on
/// this unauthenticated endpoint.
fn is_valid_fingerprint(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 8
        && parts.iter().all(|p| {
            p.len() == 2
                && p.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_lowercase())
        })
}

/// The router each sensor listener serves.
fn listener_router(state: EnrollState) -> Router {
    Router::new()
        .route(
            "/sensor/health",
            get(|| async { axum::http::StatusCode::OK }),
        )
        .route("/sensor/enroll", post(enroll))
        .route("/sensor/ingest", post(ingest_handler))
        .with_state(state)
}

/// Replay-window skew tolerance for `POST /sensor/ingest` batches.
const MAX_SKEW_MS: i64 = 300_000;

/// How long `/sensor/ingest` may spend storing one batch before it stops and
/// ACKs what it managed.
///
/// This is the structural guard against the stall that motivated all of this,
/// and it holds even if some future change makes ingest slow again. Without
/// it, a batch that takes longer than the Sensor's HTTP timeout gets its
/// connection dropped mid-loop; axum then cancels the handler, so nothing is
/// ACKed even though rows were committed, and the Sensor re-sends the exact
/// same rows on its next cycle. That is a livelock, not slowness: the queue
/// never advances no matter how long you wait.
///
/// Returning early instead guarantees every batch makes ACKed forward
/// progress, so the backlog is monotonically decreasing. Whatever is left
/// over simply leads the next batch. Kept well under
/// `forwarder::HTTP_TIMEOUT` so the response always wins the race.
const INGEST_BUDGET: Duration = Duration::from_secs(20);

/// A permanent ingest failure (a DB constraint/check violation) can never
/// succeed on retry — ACK it so an at-least-once forwarder drops the poison
/// pill instead of retrying forever. `anyhow::Error` from `ingest_remote`
/// wraps the underlying `sqlx::Error`; a `Database` variant is a
/// constraint/data problem (permanent), anything else (IO/pool/timeout) is
/// transient.
fn is_permanent_ingest_error(err: &anyhow::Error) -> bool {
    matches!(
        err.downcast_ref::<sqlx::Error>(),
        Some(sqlx::Error::Database(_))
    )
}

/// `POST /sensor/ingest` — an approved sensor forwards an AEAD-sealed batch
/// of emissions. A successful AEAD open under the claimed sensor's stored
/// key IS the authentication (the `X-Sensor-Id` header is non-authoritative,
/// only used to look up which key to try). Never panics on attacker input:
/// the body is raw bytes, every fallible step returns early with an error
/// response instead of unwrapping.
async fn ingest_handler(
    State(st): State<EnrollState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    // 1. Identify the claimed sensor (non-authoritative header) + its key.
    let Some(sensor_id) = headers.get("X-Sensor-Id").and_then(|v| v.to_str().ok()) else {
        return (StatusCode::BAD_REQUEST, "missing X-Sensor-Id").into_response();
    };
    let sensor = match SensorRepo::get_by_sensor_id(&st.pool, st.data_source_id, sensor_id).await {
        Ok(Some(s)) if s.status == "approved" => s,
        Ok(_) => return (StatusCode::FORBIDDEN, "unknown or unapproved sensor").into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let Ok(key) = fluxfang_sensor_proto::decode_key(&sensor.key) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    // 2. AEAD-open the batch (a successful open IS authentication).
    let batch = match fluxfang_sensor_proto::open_batch(&key, &body) {
        Ok(b) => b,
        Err(_) => return (StatusCode::UNAUTHORIZED, "decrypt failed").into_response(),
    };
    // Body sensor_id must match the header/looked-up sensor.
    if batch.sensor_id != sensor.sensor_id {
        return (StatusCode::BAD_REQUEST, "sensor_id mismatch").into_response();
    }
    // 3. Replay window.
    let now_ms = chrono::Utc::now().timestamp_millis();
    if !fluxfang_sensor_proto::within_replay_window(batch.sent_at_ms, now_ms, MAX_SKEW_MS) {
        return (StatusCode::BAD_REQUEST, "stale batch").into_response();
    }

    // 4. Record liveness BEFORE doing any of the work.
    //
    // The batch is authenticated at this point -- it opened under this
    // sensor's key -- which is already proof the sensor is alive and talking
    // to us, independent of whether storing it succeeds. Stamping it after
    // the loop (as this used to) meant a batch that outran the sensor's HTTP
    // timeout had the handler cancelled before it got here, so a sensor that
    // was busily forwarding the whole time aged past the 60s online threshold
    // and showed as offline on the Sensors page.
    //
    // `source_ip` is refreshed here too: it was only ever written at
    // enrollment, so an approved sensor that changed address (DHCP lease,
    // reboot, new link) showed its address from enrollment day forever.
    let peer_ip = peer.ip().to_string();
    let _ = SensorRepo::touch_seen_from(&st.pool, sensor.id, &peer_ip).await;

    // 5. Ingest each emission; dedup handled by insert_remote.
    let deadline = std::time::Instant::now() + INGEST_BUDGET;
    let total = batch.emissions.len();
    let mut accepted = Vec::new();
    for em in batch.emissions {
        // Stop only once something has been ACKed: a first emission slower
        // than the whole budget would otherwise return an empty ACK forever,
        // which is the livelock again. Making progress one row per batch is
        // pathological but still progress, and the error is visible.
        if !accepted.is_empty() && std::time::Instant::now() >= deadline {
            eprintln!(
                "sensor ingest: {} of {} emissions from {} stored within {:?}; ACKing those and \
                 leaving the rest for the next batch",
                accepted.len(),
                total,
                sensor.sensor_id,
                INGEST_BUDGET,
            );
            break;
        }
        let id = em.id;
        match crate::ingest::ingest_remote(
            &st.ingest,
            st.data_source_id,
            &sensor.sensor_id,
            sensor.auto_group_emitters,
            em,
        )
        .await
        {
            Ok(_) => accepted.push(id), // accept whether newly-inserted or a dup
            Err(e) if is_permanent_ingest_error(&e) => accepted.push(id), // drop poison pill, don't retry forever
            Err(_) => { /* transient — omit from accepted so the sensor retries */ }
        }
    }
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({ "accepted": accepted })),
    )
        .into_response()
}

/// Every outcome is logged with the peer address and the reason.
///
/// A rejected enrollment used to be completely silent on this side, which is
/// the worst possible place for silence: the operator watching an empty
/// pending list had no way to distinguish "no sensor is reaching me" from
/// "a sensor is knocking and I am turning it away", and the sensor's own
/// retry loop is the only other party that knows. `docker logs` on the
/// Standalone now answers that directly.
///
/// `POST /sensor/enroll` — a sensor self-registers `{sensor_id, key}` during
/// an open enrollment window. Returns `{status, fingerprint}`; the sensor
/// displays the fingerprint for out-of-band verification before an operator
/// approves it. Never panics on attacker input.
async fn enroll(
    State(st): State<EnrollState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(req): Json<EnrollRequest>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    // 1. Slug + fingerprint validity (fail closed, no panic). The key is NOT
    // present in the request — only its one-way fingerprint.
    if !is_valid_sensor_id(&req.sensor_id) {
        eprintln!("sensor enroll from {peer}: REJECTED — invalid sensor_id");
        return (StatusCode::BAD_REQUEST, "invalid sensor_id").into_response();
    }
    if !is_valid_fingerprint(&req.fingerprint) {
        eprintln!(
            "sensor enroll from {peer} (id {:?}): REJECTED — malformed fingerprint",
            req.sensor_id
        );
        return (StatusCode::BAD_REQUEST, "invalid fingerprint").into_response();
    }
    let fingerprint = req.fingerprint.clone();
    let source_ip = peer.ip().to_string();

    // 2. Look up any existing row for this (datasource, sensor_id).
    let existing = SensorRepo::get_by_sensor_id(&st.pool, st.data_source_id, &req.sensor_id).await;
    let existing = match existing {
        Ok(e) => e,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // 3. Already-approved sensors may re-enroll ANY time — this is their
    // liveness heartbeat, so it must NOT be gated on the enrollment window
    // (which auto-closes on approval). It only bumps `last_seen_at`; no new
    // registration happens. A DIFFERENT fingerprint means a node is squatting
    // an approved id with another key — refuse. Fingerprints are public
    // (one-way hashes), so a plain compare is fine here.
    if let Some(s) = existing.as_ref() {
        if s.status == "approved" {
            if req.fingerprint == s.fingerprint {
                // Same refresh as the ingest path: this is the heartbeat an
                // idle (or backlogged, or erroring) sensor uses, so it is
                // often the only place an address change is observed.
                let _ = SensorRepo::touch_seen_from(&st.pool, s.id, &source_ip).await;
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({ "status": "approved", "fingerprint": fingerprint })),
                )
                    .into_response();
            }
            eprintln!(
                "sensor enroll from {peer}: REJECTED — id {:?} is already approved under a \
                 different key (its fingerprint is {}, this node presented {}). Rotate the key \
                 or revoke the existing sensor.",
                req.sensor_id, s.fingerprint, req.fingerprint
            );
            return (
                StatusCode::CONFLICT,
                "sensor_id already approved with a different key",
            )
                .into_response();
        }
    }

    // 4. Everything else (a NEW or still-pending registration) requires an
    // open enrollment window.
    {
        let map = st.windows.lock().await;
        if !window_is_open(&map, st.data_source_id) {
            // The single most likely reason an operator sees an empty pending
            // list while a sensor insists it is enrolling. Note the window is
            // in-memory, so restarting this backend closes it even though the
            // UI countdown (a client-side timer) keeps running.
            eprintln!(
                "sensor enroll from {peer} (id {:?}): REJECTED — the enrollment window is \
                 closed. Click \"Allow new Sensors\" on the Sensors page; note a backend \
                 restart closes it.",
                req.sensor_id
            );
            return (StatusCode::FORBIDDEN, "enrollment window is closed").into_response();
        }
    }

    // 5. Upsert policy by current status.
    let result = match existing {
        None => SensorRepo::insert_pending(
            &st.pool,
            st.data_source_id,
            &req.sensor_id,
            &fingerprint,
            Some(&source_ip),
        )
        .await
        .map(|s| (StatusCode::OK, s.status)),
        Some(s) if s.status == "pending" => {
            match SensorRepo::update_pending_fingerprint(
                &st.pool,
                s.id,
                &fingerprint,
                Some(&source_ip),
            )
            .await
            {
                Ok(Some(updated)) => Ok((StatusCode::OK, updated.status)),
                // Raced out of `pending` (approved/revoked/rejected) between
                // our read and this write — do NOT overwrite. The sensor's
                // 30s retry will re-read the now-current status.
                Ok(None) => {
                    return (StatusCode::CONFLICT, "enrollment state changed, retry")
                        .into_response()
                }
                Err(e) => Err(e),
            }
        }
        // revoked / rejected -> refuse; do not resurrect. (approved is handled
        // above, before the window gate.)
        Some(s) => {
            eprintln!(
                "sensor enroll from {peer}: REJECTED — id {:?} is {} and will not be \
                 resurrected. Delete that row to let it enroll fresh.",
                req.sensor_id, s.status
            );
            return (StatusCode::FORBIDDEN, "sensor is not permitted to enroll").into_response();
        }
    };

    match result {
        Ok((code, status)) => {
            eprintln!(
                "sensor enroll from {peer}: ACCEPTED — id {:?} is now {status} \
                 (fingerprint {fingerprint}); approve it on the Sensors page.",
                req.sensor_id
            );
            (
                code,
                Json(serde_json::json!({ "status": status, "fingerprint": fingerprint })),
            )
                .into_response()
        }
        Err(e) => {
            eprintln!(
                "sensor enroll from {peer} (id {:?}): FAILED to persist: {e}",
                req.sensor_id
            );
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Parse `bind_ip`/`bind_port` out of a sensor datasource's `config` jsonb.
fn parse_bind(config: &Value) -> Option<SocketAddr> {
    let ip: IpAddr = config.get("bind_ip")?.as_str()?.parse().ok()?;
    let port: u16 = u16::try_from(config.get("bind_port")?.as_u64()?).ok()?;
    Some(SocketAddr::new(ip, port))
}

impl SensorListenerManager {
    pub fn new(pool: PgPool, ingest: IngestCtx) -> Self {
        Self {
            pool,
            running: Mutex::new(HashMap::new()),
            windows: Arc::new(Mutex::new(HashMap::new())),
            ingest,
        }
    }

    /// Bind and serve the listener for datasource `id`. No-op if already
    /// running. On bind failure the datasource is marked `error`; on success,
    /// `running`.
    pub async fn start(&self, id: Uuid) {
        let mut running = self.running.lock().await;
        if running.contains_key(&id) {
            return;
        }

        let Ok(Some(source)) = DataSourceRepo::get(&self.pool, id).await else {
            return;
        };
        let Some(addr) = parse_bind(&source.config) else {
            let _ = DataSourceRepo::set_status(
                &self.pool,
                id,
                "error",
                Some("sensor listener config missing/invalid bind_ip:bind_port"),
            )
            .await;
            return;
        };

        let listener = match TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                let _ = DataSourceRepo::set_status(
                    &self.pool,
                    id,
                    "error",
                    Some(&format!("failed to bind sensor listener on {addr}: {e}")),
                )
                .await;
                return;
            }
        };

        let enroll_state = EnrollState {
            pool: self.pool.clone(),
            data_source_id: id,
            windows: self.windows.clone(),
            ingest: self.ingest.clone(),
        };
        let (shutdown, shutdown_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let service =
                listener_router(enroll_state).into_make_service_with_connect_info::<SocketAddr>();
            let _ = axum::serve(listener, service)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        running.insert(id, ListenerHandle { shutdown, task });
        let _ = DataSourceRepo::set_status(&self.pool, id, "running", None).await;
    }

    /// Gracefully stop the listener for datasource `id` (no-op if not
    /// running) and mark it `stopped`.
    pub async fn stop(&self, id: Uuid) {
        let handle = self.running.lock().await.remove(&id);
        if let Some(handle) = handle {
            let _ = handle.shutdown.send(());
            let _ = handle.task.await;
        }
        let _ = DataSourceRepo::set_status(&self.pool, id, "stopped", None).await;
    }

    /// Startup: bind every `sensor` datasource the user left `running`.
    pub async fn resume_running(&self) {
        let Ok(sources) = DataSourceRepo::list(&self.pool).await else {
            return;
        };
        for source in sources {
            if source.kind == "sensor" && source.desired_state == "running" {
                self.start(source.id).await;
            }
        }
    }

    /// Open the enrollment window for `data_source_id` for its configured
    /// Open a fixed-length enrollment window for `data_source_id`. Returns the
    /// window length in seconds, or `None` if the datasource doesn't exist.
    /// The duration is a fixed default — it is no longer operator-configurable
    /// per datasource (kept short and predictable by design).
    pub async fn open_enrollment_window(&self, data_source_id: Uuid) -> Option<u64> {
        // Confirm the datasource exists before opening a window for it.
        DataSourceRepo::get(&self.pool, data_source_id)
            .await
            .ok()??;
        // ...and that its listener is actually bound. A window is only
        // meaningful if something is accepting connections: opening one for a
        // stopped listener gives the operator a ticking countdown while every
        // sensor gets connection-refused, which looks identical to "my sensor
        // is broken" from the UI.
        if !self.running.lock().await.contains_key(&data_source_id) {
            eprintln!(
                "refusing to open an enrollment window for data source {data_source_id}: its \
                 listener is not running — start the Sensor data source first"
            );
            return None;
        }
        let secs = DEFAULT_ENROLLMENT_WINDOW_SECS;
        let expiry = Instant::now() + std::time::Duration::from_secs(secs);
        self.windows.lock().await.insert(data_source_id, expiry);
        Some(secs)
    }

    /// Close `data_source_id`'s enrollment window immediately (e.g. once the
    /// operator has approved a sensor). A no-op if none is open.
    pub async fn close_enrollment_window(&self, data_source_id: Uuid) {
        self.windows.lock().await.remove(&data_source_id);
    }

    /// Remaining seconds on the window, or 0 if closed/expired.
    pub async fn enrollment_window_remaining(&self, data_source_id: Uuid) -> u64 {
        let map = self.windows.lock().await;
        map.get(&data_source_id)
            .map(|&exp| exp.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio::time::{Duration, Instant};

    #[test]
    fn window_open_and_expiry_logic() {
        let windows: WindowMap = Arc::new(Mutex::new(HashMap::new()));
        let id = Uuid::new_v4();
        // Not opened yet -> closed.
        assert!(!window_is_open(&windows.try_lock().unwrap(), id));
        // Open for 1s.
        windows
            .try_lock()
            .unwrap()
            .insert(id, Instant::now() + Duration::from_secs(1));
        assert!(window_is_open(&windows.try_lock().unwrap(), id));
        // Expired.
        windows
            .try_lock()
            .unwrap()
            .insert(id, Instant::now() - Duration::from_secs(1));
        assert!(!window_is_open(&windows.try_lock().unwrap(), id));
    }
}
