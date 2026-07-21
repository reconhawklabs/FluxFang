//! fluxfang-api: Axum HTTP API surface.
//!
//! Task 0.1 established a bare health check. Task 2.2 adds first-run
//! setup, login/logout, session-cookie auth, and a `require_auth`
//! middleware layer that guards every other `/api/*` route by default.
//!
//! The router is built as two groups merged together:
//!
//! - **public** (`auth_routes::public_routes()` + `/api/health`): reachable
//!   with no session at all — exactly `{/api/health, /api/setup/status,
//!   /api/setup, /api/login}`. Setup and login are how a session gets
//!   created in the first place, so they necessarily can't require one;
//!   health is an infra check that should never depend on app state.
//! - **protected**: everything else, wrapped in [`middleware::require_auth`]
//!   via `route_layer` so new routes added to this group in later tasks
//!   (entities, emissions, zones, alerts, ...) are guarded automatically —
//!   nobody has to remember to re-apply the middleware each time. This
//!   includes `POST /api/logout` (`auth_routes::protected_routes()`): an
//!   unauthenticated logout is a no-op anyway, so requiring a session costs
//!   nothing and keeps the public surface minimal.
//!
//! Task 6.5 replaces the placeholder `GET /api/entities -> []` stub this
//! module briefly carried (Task 2.2 chose `/api/entities` over a `/api/me`
//! placeholder since the task brief's own illustrative test hit it
//! directly) with the real `entities` module's CRUD + detail handlers.
//!
//! ## Session store
//!
//! Sessions are cookie-based via `tower-sessions`, backed by its in-memory
//! `MemoryStore`. That means **all sessions are lost on backend restart**
//! (container redeploy, crash, etc.) — every logged-in user, including the
//! admin, has to log in again. Acceptable for this slice per the task
//! brief; a Postgres-backed store is a drop-in upgrade (`tower-sessions`
//! stores are pluggable) if/when that restart behavior becomes annoying.
//! `FLUXFANG_SESSION_KEY` (already reserved in `docker-compose.yml`/
//! `.env.example`) is *not* used yet: `MemoryStore`'s cookie only ever
//! carries an opaque, server-generated session id (no client-editable
//! payload for a signature to protect), so cookie signing has no real
//! security payoff here today. It's left wired up in the environment for
//! whichever future task adds a persistent, signed/private-cookie store.

pub mod ai_audit;
pub mod alert_methods;
pub mod alert_rules;
pub mod auth_routes;
pub mod capture;
pub mod catalog_routes;
pub mod config_routes;
pub mod correlate;
pub mod cotravel;
pub mod data_sources;
pub mod dto;
pub mod emissions;
pub mod emitter_type_routes;
pub mod emitters;
pub mod entities;
pub mod gps_status;
pub mod ingest;
pub mod mcp;
pub mod middleware;
pub mod notifications;
pub mod notify;
pub mod sensor_listener;
pub mod sensors;
pub mod state;
pub mod system;
#[cfg(test)]
mod test_support;
pub mod ws;
pub mod zones;

use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use time::Duration as TimeDuration;
use tower_sessions::cookie::SameSite;
use tower_sessions::{Expiry, MemoryStore, SessionManagerLayer};

pub use state::AppState;

pub fn app(state: AppState) -> Router {
    let public = Router::new()
        .route(
            "/api/health",
            get(|| async { Json(json!({"status":"ok"})) }),
        )
        .merge(auth_routes::public_routes());

    // `auth_routes::protected_routes()` (currently just `POST /api/logout`)
    // is merged in here too, so it goes through `require_auth` like every
    // other protected route rather than living in the public group.
    let protected = Router::new()
        .merge(auth_routes::protected_routes())
        .merge(ai_audit::protected_routes())
        .merge(alert_methods::protected_routes())
        .merge(alert_rules::protected_routes())
        .merge(catalog_routes::protected_routes())
        .merge(config_routes::protected_routes())
        .merge(cotravel::protected_routes())
        .merge(data_sources::protected_routes())
        .merge(emissions::protected_routes())
        .merge(emitter_type_routes::protected_routes())
        .merge(emitters::protected_routes())
        .merge(entities::protected_routes())
        .merge(gps_status::protected_routes())
        .merge(notifications::protected_routes())
        .merge(sensors::protected_routes())
        .merge(system::protected_routes())
        .merge(ws::protected_routes())
        .merge(zones::protected_routes())
        .route_layer(axum::middleware::from_fn(middleware::require_auth));

    // `/mcp` is its own group, guarded by `mcp::guard::mcp_guard`
    // (loopback-only) instead of `require_auth` — it's not part of the
    // session-cookie auth surface. See `mcp` module docs.
    let mcp = mcp::routes();

    Router::new()
        .merge(public)
        .merge(protected)
        .merge(mcp)
        .layer(session_layer())
        .with_state(state)
}

/// Build the `tower-sessions` cookie-session layer. See module docs for the
/// store choice; cookie attributes here follow the brief: http-only,
/// same-site Lax, and a `secure` flag controlled by `FLUXFANG_SESSION_SECURE`
/// (default `false` — nothing in this stack terminates TLS yet, see
/// `frontend/nginx.conf`; set to `true` once it does, since browsers silently
/// drop `Secure` cookies sent over plain HTTP).
fn session_layer() -> SessionManagerLayer<MemoryStore> {
    let secure = std::env::var("FLUXFANG_SESSION_SECURE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    SessionManagerLayer::new(MemoryStore::default())
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(secure)
        .with_expiry(Expiry::OnInactivity(TimeDuration::hours(24)))
}
