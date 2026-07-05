//! `GET/POST/PATCH/DELETE /api/alert-methods[/:id]` + `POST
//! /api/alert-methods/:id/test` (Task 6.6). PROTECTED — mounted in
//! `lib.rs::app`'s protected router group, behind `require_auth`, same as
//! every other non-setup/login route.
//!
//! ## Where the config actually lives
//!
//! `alert_method.config` (the plaintext DB column) is never written by this
//! module — `fluxfang_db::repo::alert_method::AlertMethodRepo::insert`'s own
//! doc comment already establishes that only `config_encrypted` is settable
//! at the repo layer, and stays that way here: every field the caller
//! submits under `config` (secret *and* non-secret alike — SMTP host,
//! webhook url, SMTP password, webhook HMAC secret, ...) is encrypted as one
//! JSON blob via `fluxfang_core::secrets::encrypt` under `AppState::secret_key`
//! and stored in `config_encrypted`. There is deliberately no split between
//! "goes in the plaintext `config` column" and "goes in
//! `config_encrypted`" — that would mean deciding, per field, whether it's
//! secret enough to encrypt, which is exactly the kind of mistake this
//! module's safe-projection approach (below) is designed to make impossible
//! to get wrong on the read side.
//!
//! ## Safe projection: how `GET`/`POST`/`PATCH` avoid ever echoing a secret
//!
//! Since every field lives in `config_encrypted`, building a response has to
//! decrypt it and then decide what's safe to hand back. [`safe_config`] does
//! this once, in one place, via a **positive allowlist per type** — safer
//! than a blocklist (a blocklist silently leaks any secret field a future
//! channel adds unless someone remembers to add it to the list; an allowlist
//! silently drops it instead, which is the fail-safe direction):
//!
//! - `email`: `host`, `port`, `from`, `to`, `tls` — never `username` or
//!   `password`. (`username` is excluded too, not just `password`: an SMTP
//!   username is often itself the sending mailbox's full address/login and
//!   arguably identity-bearing; the task brief's own example list for email
//!   is `host/port/from/to`, so this keeps to exactly that plus `tls`, which
//!   is a non-identifying boolean.)
//! - `webhook`: `url`, `method`, `headers` — never `secret` (the HMAC
//!   signing key).
//! - `in_app`: always `{}` (there is no config to begin with).
//! - any other/future type: always `{}` (fail safe — an unrecognized type
//!   means this module doesn't yet know which fields, if any, are safe, so
//!   it returns none rather than guessing).
//!
//! A decrypt failure (wrong/rotated key, corrupt ciphertext, or — for
//! `in_app`, whose `config_encrypted` may be empty/absent — simply nothing
//! to decrypt) collapses to `{}` rather than a `500`: the safe projection is
//! best-effort, not load-bearing for the rest of the response.
//!
//! ## `POST`/`PATCH` validation
//!
//! [`validate_config_for_type`] validates a submitted `config` by attempting
//! to deserialize it as the exact decrypted-config struct
//! `fluxfang_api::notify` already uses to dispatch that channel
//! (`notify::email::EmailConfig` / `notify::webhook::WebhookConfig`) —
//! reusing those `pub(crate)` structs instead of hand-rolling a parallel
//! "does this config have the right shape" check means the validation can
//! never drift out of sync with what `notify::dispatch` will actually
//! require at send time. `in_app` has no shape to validate (any `config` is
//! accepted, though nothing ever reads it). An unrecognized `type` is
//! rejected before any encryption happens.
//!
//! ## Error mapping
//!
//! Same convention as `emitters.rs`/`entities.rs`: a malformed body/unknown
//! `type`/invalid `config` shape is `400`; a missing path-`:id` resource is
//! `404`; any other `sqlx::Error` is `500`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use fluxfang_core::secrets::encrypt;
use fluxfang_db::models::{AlertMethod, NewAlertMethod};
use fluxfang_db::AlertMethodRepo;

use crate::notify::email::EmailConfig;
use crate::notify::webhook::WebhookConfig;
use crate::notify::{decrypt_config, dispatch, DeliveryStatus, NotificationPayload};
use crate::state::AppState;

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/alert-methods",
            get(list_alert_methods).post(create_alert_method),
        )
        .route(
            "/api/alert-methods/:id",
            patch(update_alert_method).delete(delete_alert_method),
        )
        .route("/api/alert-methods/:id/test", post(test_alert_method))
}

/// `GET /api/alert-methods`/`POST /api/alert-methods`/`PATCH
/// /api/alert-methods/:id`'s response shape — see module docs for why
/// `config` is a decrypt-then-filter safe projection rather than
/// `fluxfang_db::models::AlertMethod::config`/`config_encrypted` directly.
#[derive(Debug, Clone, Serialize)]
struct AlertMethodDto {
    id: Uuid,
    name: String,
    #[serde(rename = "type")]
    type_: String,
    enabled: bool,
    created_at: DateTime<Utc>,
    config: serde_json::Value,
}

impl AlertMethodDto {
    fn from_method(method: &AlertMethod, key: &[u8; 32]) -> Self {
        AlertMethodDto {
            id: method.id,
            name: method.name.clone(),
            type_: method.type_.clone(),
            enabled: method.enabled,
            created_at: method.created_at,
            config: safe_config(method, key),
        }
    }
}

/// The allowlisted, non-secret subset of `method`'s decrypted config safe to
/// return over the wire — see module docs for the full rationale and the
/// exact allowlist per type. Never panics; a decrypt failure (or a type this
/// module doesn't recognize) yields `{}`.
fn safe_config(method: &AlertMethod, key: &[u8; 32]) -> serde_json::Value {
    let allowed_keys: &[&str] = match method.type_.as_str() {
        "email" => &["host", "port", "from", "to", "tls"],
        "webhook" => &["url", "method", "headers"],
        _ => &[],
    };
    if allowed_keys.is_empty() {
        return serde_json::json!({});
    }

    let Ok(full) = decrypt_config::<serde_json::Value>(method, key) else {
        return serde_json::json!({});
    };
    let Some(full_obj) = full.as_object() else {
        return serde_json::json!({});
    };

    let mut safe = serde_json::Map::new();
    for key in allowed_keys {
        if let Some(v) = full_obj.get(*key) {
            safe.insert((*key).to_string(), v.clone());
        }
    }
    serde_json::Value::Object(safe)
}

/// Validate a submitted `config` against the exact decrypted-config shape
/// `notify::dispatch` will require for `type_` at send time (see module
/// docs). `in_app` has nothing to validate; an unrecognized `type_` is
/// itself the error.
fn validate_config_for_type(type_: &str, config: &serde_json::Value) -> Result<(), String> {
    match type_ {
        "email" => serde_json::from_value::<EmailConfig>(config.clone())
            .map(|_| ())
            .map_err(|e| format!("invalid email config: {e}")),
        "webhook" => serde_json::from_value::<WebhookConfig>(config.clone())
            .map(|_| ())
            .map_err(|e| format!("invalid webhook config: {e}")),
        "in_app" => Ok(()),
        other => Err(format!(
            "unknown alert method type {other:?}; expected 'email', 'in_app', or 'webhook'"
        )),
    }
}

/// Encrypt `config` (the full, unfiltered plaintext JSON — see module docs)
/// under `key`, mapping a `serde_json` serialization failure (there isn't a
/// realistic way to trigger one on an already-parsed `serde_json::Value`,
/// but the fallible path is handled rather than `.expect()`ed away) to a
/// `400` instead of a panic.
fn encrypt_config(config: &serde_json::Value, key: &[u8; 32]) -> Result<Vec<u8>, ApiError> {
    let plaintext = serde_json::to_vec(config)
        .map_err(|e| ApiError::BadRequest(format!("invalid config json: {e}")))?;
    Ok(encrypt(key, &plaintext))
}

async fn list_alert_methods(
    State(state): State<AppState>,
) -> Result<Json<Vec<AlertMethodDto>>, ApiError> {
    let rows = AlertMethodRepo::list(&state.pool).await?;
    Ok(Json(
        rows.iter()
            .map(|m| AlertMethodDto::from_method(m, &state.secret_key))
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct CreateAlertMethodRequest {
    name: String,
    #[serde(rename = "type")]
    type_: String,
    enabled: bool,
    #[serde(default = "empty_object")]
    config: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

async fn create_alert_method(
    State(state): State<AppState>,
    Json(req): Json<CreateAlertMethodRequest>,
) -> Result<(StatusCode, Json<AlertMethodDto>), ApiError> {
    validate_config_for_type(&req.type_, &req.config).map_err(ApiError::BadRequest)?;
    let config_encrypted = encrypt_config(&req.config, &state.secret_key)?;

    let created = AlertMethodRepo::insert(
        &state.pool,
        NewAlertMethod {
            name: req.name,
            type_: req.type_,
            enabled: req.enabled,
            config_encrypted,
        },
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(AlertMethodDto::from_method(&created, &state.secret_key)),
    ))
}

#[derive(Debug, Deserialize)]
struct UpdateAlertMethodRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    config: Option<serde_json::Value>,
}

async fn update_alert_method(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateAlertMethodRequest>,
) -> Result<Json<AlertMethodDto>, ApiError> {
    let existing = AlertMethodRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let name = req.name.unwrap_or_else(|| existing.name.clone());
    let enabled = req.enabled.unwrap_or(existing.enabled);

    // `type_` is immutable after creation (see `AlertMethodRepo::update`'s
    // own doc comment), so a re-submitted `config` is validated/encrypted
    // against the existing row's type, never a caller-supplied one.
    let config_encrypted = match req.config {
        Some(config) => {
            validate_config_for_type(&existing.type_, &config).map_err(ApiError::BadRequest)?;
            encrypt_config(&config, &state.secret_key)?
        }
        None => existing.config_encrypted.clone().unwrap_or_default(),
    };

    let updated =
        AlertMethodRepo::update(&state.pool, id, &name, enabled, config_encrypted).await?;
    Ok(Json(AlertMethodDto::from_method(
        &updated,
        &state.secret_key,
    )))
}

async fn delete_alert_method(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let deleted = AlertMethodRepo::delete(&state.pool, id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// `POST /api/alert-methods/:id/test`: dispatch a sample notification
/// through `method`'s real, configured channel (the exact same
/// `notify::dispatch` path a fired alert uses) and report the resulting
/// [`DeliveryStatus`] synchronously — no `notification` row is written and
/// no `Event` is broadcast, since this is an operator-triggered config check,
/// not a real alert firing.
async fn test_alert_method(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeliveryStatus>, ApiError> {
    let method = AlertMethodRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let payload = NotificationPayload {
        title: "FluxFang test notification".to_string(),
        body: format!(
            "This is a test notification from alert method \"{}\".",
            method.name
        ),
        context: serde_json::json!({"alert_method_id": method.id, "test": true}),
    };

    let status = dispatch(&method, &state.secret_key, &payload).await;
    Ok(Json(status))
}

/// Small internal error type, same convention as `emitters::ApiError`/
/// `entities::ApiError`: DB failures map to `500`; deliberate rejections
/// (unknown `type`, invalid `config` shape) are `400`; a missing `:id` is
/// `404`.
enum ApiError {
    BadRequest(String),
    NotFound,
    Internal,
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in alert_methods route: {err}");
        ApiError::Internal
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            ApiError::NotFound => StatusCode::NOT_FOUND.into_response(),
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}
