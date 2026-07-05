use axum::{routing::get, Json, Router};
use serde_json::json;

pub fn app() -> Router {
    Router::new().route(
        "/api/health",
        get(|| async { Json(json!({"status":"ok"})) }),
    )
}
