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
    key: String,
}

/// Slug rule shared with Phase 1 setup: non-empty, ≤64, `[A-Za-z0-9_-]`.
fn is_valid_sensor_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    // 1. Identify the claimed sensor (non-authoritative header) + its key.
    let Some(sensor_id) = headers.get("X-Sensor-Id").and_then(|v| v.to_str().ok()) else {
        return (StatusCode::BAD_REQUEST, "missing X-Sensor-Id").into_response();
    };
    let sensor = match SensorRepo::get_by_sensor_id(&st.pool, st.data_source_id, sensor_id).await
    {
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

    // 4. Ingest each emission; dedup handled by insert_remote.
    let mut accepted = Vec::new();
    for em in batch.emissions {
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
    let _ = SensorRepo::touch_last_seen(&st.pool, sensor.id).await; // heartbeat

    (
        StatusCode::OK,
        axum::Json(serde_json::json!({ "accepted": accepted })),
    )
        .into_response()
}

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

    // 1. Window gate.
    {
        let map = st.windows.lock().await;
        if !window_is_open(&map, st.data_source_id) {
            return (StatusCode::FORBIDDEN, "enrollment window is closed").into_response();
        }
    }
    // 2. Slug + key validity (fail closed, no panic).
    if !is_valid_sensor_id(&req.sensor_id) {
        return (StatusCode::BAD_REQUEST, "invalid sensor_id").into_response();
    }
    let key = match fluxfang_sensor_proto::decode_key(&req.key) {
        Ok(k) => k,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid key").into_response(),
    };
    let fingerprint = fluxfang_sensor_proto::fingerprint(&req.sensor_id, &key);
    let source_ip = peer.ip().to_string();

    // 3. Upsert policy by current status.
    let existing = SensorRepo::get_by_sensor_id(&st.pool, st.data_source_id, &req.sensor_id).await;
    let existing = match existing {
        Ok(e) => e,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let result = match existing {
        None => SensorRepo::insert_pending(
            &st.pool,
            st.data_source_id,
            &req.sensor_id,
            &req.key,
            &fingerprint,
            Some(&source_ip),
        )
        .await
        .map(|s| (StatusCode::OK, s.status)),
        Some(s) if s.status == "pending" => {
            match SensorRepo::update_pending_key(
                &st.pool,
                s.id,
                &req.key,
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
        Some(s) if s.status == "approved" => {
            // Constant-time compare of the DECODED 32-byte keys (defense in
            // depth on an unauthenticated endpoint) rather than a
            // short-circuiting String/base64 compare. If the stored key
            // somehow fails to decode (should never happen — we only ever
            // store valid base64 here) treat it as a mismatch, not a panic.
            // subtle 2.x implements `ConstantTimeEq` for slices, not fixed-
            // size arrays, so compare via `.as_slice()`.
            use subtle::ConstantTimeEq;
            let stored_key = fluxfang_sensor_proto::decode_key(&s.key);
            let same =
                matches!(&stored_key, Ok(sk) if bool::from(sk.as_slice().ct_eq(key.as_slice())));
            if same {
                let _ = SensorRepo::touch_last_seen(&st.pool, s.id).await;
                Ok((StatusCode::OK, "approved".to_string()))
            } else {
                // An approved id re-enrolling with a DIFFERENT key — refuse.
                return (
                    StatusCode::CONFLICT,
                    "sensor_id already approved with a different key",
                )
                    .into_response();
            }
        }
        // revoked / rejected -> refuse; do not resurrect.
        Some(_) => {
            return (StatusCode::FORBIDDEN, "sensor is not permitted to enroll").into_response()
        }
    };

    match result {
        Ok((code, status)) => (
            code,
            Json(serde_json::json!({ "status": status, "fingerprint": fingerprint })),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
    /// `enrollment_window_secs` (default 900). Returns remaining seconds, or
    /// `None` if the datasource doesn't exist.
    pub async fn open_enrollment_window(&self, data_source_id: Uuid) -> Option<u64> {
        let source = DataSourceRepo::get(&self.pool, data_source_id)
            .await
            .ok()??;
        let secs = source
            .config
            .get("enrollment_window_secs")
            .and_then(Value::as_u64)
            .unwrap_or(900);
        let expiry = Instant::now() + std::time::Duration::from_secs(secs);
        self.windows.lock().await.insert(data_source_id, expiry);
        Some(secs)
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
