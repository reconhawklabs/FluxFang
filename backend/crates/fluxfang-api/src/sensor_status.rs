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
    let node = AppConfigRepo::node_config(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let stats = CachedEmissionRepo::stats(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let sensor = node.as_ref().and_then(|n| n.sensor.as_ref())
        .map(|s| json!({ "host": s.host, "port": s.port }));
    Ok(Json(json!({
        "role": node.as_ref().map(|n| n.role),
        "node_sensor_id": node.as_ref().map(|n| n.node_sensor_id.clone()),
        "cache": { "total": stats.total, "undelivered": stats.undelivered },
        "sensor": sensor,
    })))
}

#[derive(Deserialize)]
struct Limit { limit: Option<i64> }

async fn cached(State(state): State<AppState>, Query(q): Query<Limit>) -> Result<Json<Value>, StatusCode> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let rows = CachedEmissionRepo::list_recent(&state.pool, limit).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::to_value(rows).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?))
}
