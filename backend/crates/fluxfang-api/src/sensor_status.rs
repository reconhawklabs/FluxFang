//! Sensor-node UI endpoints: forwarding status + the local emission cache.
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use fluxfang_db::{AppConfigRepo, CachedEmissionRepo};

use crate::state::AppState;

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/api/sensor/status", get(status))
        .route("/api/cached-emissions", get(cached))
}

async fn status(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let node = AppConfigRepo::node_config(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let stats = CachedEmissionRepo::stats(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let since = chrono::Utc::now() - chrono::Duration::hours(1);
    let delivered_last_hour = CachedEmissionRepo::delivered_count_since(&state.pool, since)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let sensor_cfg = node.as_ref().and_then(|n| n.sensor.as_ref());
    let sensor = sensor_cfg.map(|s| json!({ "host": s.host, "port": s.port }));
    // Live connectivity check to the Standalone's listener (the Dashboard
    // polls this ~every minute). None when this isn't a configured sensor.
    let connected = match sensor_cfg {
        Some(s) => Some(standalone_reachable(&s.host, s.port).await),
        None => None,
    };
    // `connected` only ever meant "the Standalone's listener answered a
    // health GET". That is a statement about the network, not about
    // forwarding, and it is why a sensor could sit there reporting
    // "connected" while the Standalone listed it as down: the listener was up
    // the whole time, but every batch was failing. Keep it (a reachability
    // check is still the right first thing to look at) and publish what the
    // forwarding loop is actually achieving alongside it.
    let forwarding = state.forwarder_health.snapshot();
    Ok(Json(json!({
        "role": node.as_ref().map(|n| n.role),
        "node_sensor_id": node.as_ref().map(|n| n.node_sensor_id.clone()),
        "cache": { "total": stats.total, "undelivered": stats.undelivered },
        "delivered_last_hour": delivered_last_hour,
        "connected": connected,
        "forwarding": forwarding,
        "sensor": sensor,
    })))
}

/// Best-effort reachability probe of the Standalone's sensor listener. A quick,
/// short-timeout GET of its public `/sensor/health` — any successful response
/// means the listener is up and routable from here.
async fn standalone_reachable(host: &str, port: u16) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    else {
        return false;
    };
    client
        .get(format!("http://{host}:{port}/sensor/health"))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

#[derive(Deserialize)]
struct Limit {
    limit: Option<i64>,
}

async fn cached(
    State(state): State<AppState>,
    Query(q): Query<Limit>,
) -> Result<Json<Value>, StatusCode> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let rows = CachedEmissionRepo::list_recent(&state.pool, limit)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        serde_json::to_value(rows).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    ))
}
