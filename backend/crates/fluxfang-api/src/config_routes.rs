//! `GET /api/config` — the current node's role + id, so the frontend can
//! gate nav and routes. Deliberately read-only and secret-free: it never
//! returns the sensor `key` or any other credential. (Role *changes* are a
//! later Settings-phase concern.)

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use fluxfang_db::AppConfigRepo;

use crate::state::AppState;

pub fn protected_routes() -> Router<AppState> {
    Router::new().route("/api/config", get(get_config))
}

async fn get_config(State(state): State<AppState>) -> Result<Json<serde_json::Value>, StatusCode> {
    match AppConfigRepo::node_config(&state.pool).await {
        Ok(Some(node)) => Ok(Json(json!({
            "role": node.role,
            "node_sensor_id": node.node_sensor_id,
        }))),
        // Behind require_auth this "setup not completed" case shouldn't be
        // reachable, but answer defensively rather than 500.
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            eprintln!("fluxfang-api: db error in GET /api/config: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
