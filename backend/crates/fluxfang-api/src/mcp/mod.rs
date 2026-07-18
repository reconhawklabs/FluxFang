//! Embedded MCP server (see
//! docs/superpowers/specs/2026-07-17-fluxfang-mcp-server-design.md).
//! Mounted as its own router group behind `guard::mcp_guard` (loopback-only) —
//! NOT the session-auth group.

pub mod guard;
pub mod protocol;
pub mod tools;

use axum::routing::post;
use axum::Router;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/mcp",
            post(protocol::handle).get(|| async { axum::http::StatusCode::METHOD_NOT_ALLOWED }),
        )
        .route_layer(axum::middleware::from_fn(guard::mcp_guard))
}
