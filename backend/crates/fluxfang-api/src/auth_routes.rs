//! First-run setup + login/logout routes (Task 2.2).
//!
//! All four routes here are PUBLIC (reachable with no session) — that's the
//! whole point of setup/login, and logout has to be reachable by a session
//! that might already be considered "invalid" for other purposes. They are
//! mounted in `lib.rs::app`'s public router group, *outside* the
//! `require_auth` layer.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tower_sessions::Session;

use fluxfang_core::auth::{hash_password, verify_password};
use fluxfang_db::AppConfigRepo;

use crate::middleware::SESSION_AUTH_KEY;
use crate::state::AppState;

/// Passwords longer than this are rejected before hashing. Argon2 itself
/// has no trouble with long inputs, but there's no legitimate reason for a
/// password this long, and it's a cheap guard against pathological request
/// bodies being fed into the hasher.
const MAX_PASSWORD_BYTES: usize = 1024;

#[derive(Deserialize)]
pub struct PasswordPayload {
    password: String,
}

/// The public route group. Mounted without `require_auth` — see module docs.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/api/setup/status", get(setup_status))
        .route("/api/setup", post(setup))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
}

async fn setup_status(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let hash = AppConfigRepo::password_hash(&state.pool).await?;
    Ok(Json(json!({ "needs_setup": hash.is_none() })))
}

/// Set the admin password for the first (and only) time. Rejected with
/// `409 Conflict` if a password has already been configured — this route
/// only exists to bootstrap the very first admin credential, not to change
/// it later (that's a separate, authenticated "change password" concern for
/// a future task).
async fn setup(
    State(state): State<AppState>,
    session: Session,
    Json(payload): Json<PasswordPayload>,
) -> Result<StatusCode, ApiError> {
    if AppConfigRepo::password_hash(&state.pool).await?.is_some() {
        return Err(ApiError::Status(StatusCode::CONFLICT));
    }
    if payload.password.is_empty() || payload.password.len() > MAX_PASSWORD_BYTES {
        return Err(ApiError::Status(StatusCode::BAD_REQUEST));
    }

    // Argon2 hashing is deliberately CPU-heavy; run it on a blocking-pool
    // thread so it doesn't stall the async executor handling other requests.
    let hash = tokio::task::spawn_blocking(move || hash_password(&payload.password))
        .await
        .expect("hash_password blocking task panicked");

    AppConfigRepo::set_password_hash(&state.pool, &hash).await?;

    // First-run setup doubles as a login: the person who just set the
    // password is sitting right there and shouldn't have to log in again
    // immediately after.
    session.insert(SESSION_AUTH_KEY, true).await?;

    Ok(StatusCode::OK)
}

/// Verify `password` against the stored hash and, on success, mark the
/// session authenticated. Rate-limited by `state.login_limiter` — see its
/// docs in `state.rs` for why the limiter is a single global counter rather
/// than per-client.
async fn login(
    State(state): State<AppState>,
    session: Session,
    Json(payload): Json<PasswordPayload>,
) -> Result<StatusCode, ApiError> {
    if state.login_limiter.is_limited() {
        return Err(ApiError::Status(StatusCode::TOO_MANY_REQUESTS));
    }

    let stored_hash = AppConfigRepo::password_hash(&state.pool).await?;

    let verified = match stored_hash {
        Some(hash) => {
            let candidate = payload.password;
            tokio::task::spawn_blocking(move || verify_password(&hash, &candidate))
                .await
                .expect("verify_password blocking task panicked")
        }
        // No password configured yet (setup hasn't run) — nothing can
        // possibly verify against it.
        None => false,
    };

    if verified {
        state.login_limiter.record_success();
        session.insert(SESSION_AUTH_KEY, true).await?;
        Ok(StatusCode::OK)
    } else {
        state.login_limiter.record_failure();
        Err(ApiError::Status(StatusCode::UNAUTHORIZED))
    }
}

/// Clear the session (data + backing store record + cookie).
async fn logout(session: Session) -> Result<StatusCode, ApiError> {
    session.flush().await?;
    Ok(StatusCode::OK)
}

/// Small internal error type: DB/session-store failures map to `500`;
/// deliberate rejections (already set up, bad password, rate-limited) carry
/// their own intended status via `ApiError::Status`.
enum ApiError {
    Status(StatusCode),
    Internal,
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in auth route: {err}");
        ApiError::Internal
    }
}

impl From<tower_sessions::session::Error> for ApiError {
    fn from(err: tower_sessions::session::Error) -> Self {
        eprintln!("fluxfang-api: session store error in auth route: {err}");
        ApiError::Internal
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Status(code) => code.into_response(),
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}
