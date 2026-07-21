//! `SensorListenerManager`: binds/tears-down a dedicated network listener per
//! enabled `sensor` datasource. A sensor datasource is a network endpoint
//! (not a capture device), so it is driven here rather than by the
//! `CaptureSupervisor`. Each running listener is its own `axum::serve` on the
//! datasource's `bind_ip:bind_port`, tracked by data_source id.
//!
//! Phase 2B serves only `GET /sensor/health`. Enrollment/ingest routes are
//! added in later phases; they will extend [`listener_router`].

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use axum::routing::get;
use axum::Router;
use serde_json::Value;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use uuid::Uuid;

use fluxfang_db::DataSourceRepo;

/// A running listener: a shutdown trigger + the serving task's handle.
struct ListenerHandle {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

/// Manages the lifecycle of sensor-datasource network listeners.
pub struct SensorListenerManager {
    pool: PgPool,
    running: Mutex<HashMap<Uuid, ListenerHandle>>,
}

/// The router each sensor listener serves. Phase 2B: liveness only.
fn listener_router() -> Router {
    Router::new().route(
        "/sensor/health",
        get(|| async { axum::http::StatusCode::OK }),
    )
}

/// Parse `bind_ip`/`bind_port` out of a sensor datasource's `config` jsonb.
fn parse_bind(config: &Value) -> Option<SocketAddr> {
    let ip: IpAddr = config.get("bind_ip")?.as_str()?.parse().ok()?;
    let port: u16 = u16::try_from(config.get("bind_port")?.as_u64()?).ok()?;
    Some(SocketAddr::new(ip, port))
}

impl SensorListenerManager {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            running: Mutex::new(HashMap::new()),
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

        let (shutdown, shutdown_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let service = listener_router().into_make_service_with_connect_info::<SocketAddr>();
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
}
