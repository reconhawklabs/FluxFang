//! Embedded MCP server (see
//! docs/superpowers/specs/2026-07-17-fluxfang-mcp-server-design.md).
//! Mounted as its own router group behind `guard::mcp_guard` (loopback-only) —
//! NOT the session-auth group.

pub mod guard;

use axum::routing::post;
use axum::Router;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mcp", post(placeholder))
        .route_layer(axum::middleware::from_fn(guard::mcp_guard))
}

// Replaced in Task 5 by the real JSON-RPC handler.
async fn placeholder() -> &'static str {
    "mcp"
}
