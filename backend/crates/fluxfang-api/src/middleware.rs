//! `require_auth`: axum middleware guarding a router group behind an
//! authenticated session. See `lib.rs` for how the public/protected route
//! groups are split and where this gets layered on.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower_sessions::Session;

/// Session key `auth_routes::{setup, login}` set to `true` once the caller
/// has proven they know the admin password. Presence + truthiness is all
/// `require_auth` checks — there's only one account, so no user id/roles
/// are needed yet.
pub const SESSION_AUTH_KEY: &str = "authenticated";

/// Reject the request with `401 Unauthorized` unless its session has
/// [`SESSION_AUTH_KEY`] set. Must run *after* `tower_sessions`'s layer
/// (which is what makes the `Session` extractor below work at all) — see
/// `lib.rs::app` for the layering order.
pub async fn require_auth(session: Session, request: Request<Body>, next: Next) -> Response {
    let authenticated = session
        .get::<bool>(SESSION_AUTH_KEY)
        .await
        .unwrap_or(None)
        .unwrap_or(false);

    if authenticated {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}
