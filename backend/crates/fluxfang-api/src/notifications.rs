//! `GET /api/notifications` + `POST /api/notifications/:id/read` (Task 6.6).
//! PROTECTED — mounted in `lib.rs::app`'s protected router group, behind
//! `require_auth`, same as every other non-setup/login route.
//!
//! Thin wrapper over `fluxfang_db::NotificationRepo`: `list_notifications`
//! drives `NotificationRepo::list`'s `unread_only`/`limit`/`offset`
//! parameters straight off the query string (via `axum::extract::Query`,
//! not the hand-rolled `form_urlencoded` walk `emissions.rs` needs — every
//! param here is single-valued, so there's no repeated-key case to handle),
//! and additionally reports `unread_count` (a nav-bar-badge-style total,
//! independent of `unread_only`/pagination) alongside the page and its
//! total row count.
//!
//! ## Error mapping
//!
//! Same convention as `entities.rs`: a missing path-`:id` resource is `404`;
//! any other `sqlx::Error` is `500`. There's no `400` case of this module's
//! own — `limit`/`offset` are clamped rather than rejected (same convention
//! `emissions.rs`'s own `parse_limit`/`parse_offset` use), and `unread_only`
//! defaults to `false` if omitted or unparseable... actually malformed, see
//! `axum::extract::Query`'s own rejection (a `400` from axum itself, before
//! this module's handler ever runs).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use fluxfang_db::models::Notification;
use fluxfang_db::NotificationRepo;

use crate::state::AppState;

/// Default page size when `limit` is omitted — same default `emissions.rs`
/// uses for its own listing endpoint.
const DEFAULT_LIMIT: i64 = 50;
/// Hard ceiling `limit` is clamped to, regardless of what's requested.
const MAX_LIMIT: i64 = 500;

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/api/notifications", get(list_notifications))
        .route("/api/notifications/:id/read", post(mark_notification_read))
}

/// One row in `GET /api/notifications`'s `items`, and `POST
/// /api/notifications/:id/read`'s response — a thin, explicit projection of
/// `fluxfang_db::models::Notification` (same "explicit DTO, not a
/// re-export" rationale as `dto::EmissionDto`/`dto::EmitterDto`), even
/// though today it covers every field that model has.
#[derive(Debug, Clone, Serialize)]
struct NotificationDto {
    id: Uuid,
    alert_rule_id: Option<Uuid>,
    alert_method_id: Option<Uuid>,
    fired_at: DateTime<Utc>,
    payload: serde_json::Value,
    delivery_status: String,
    read_at: Option<DateTime<Utc>>,
}

impl From<&Notification> for NotificationDto {
    fn from(n: &Notification) -> Self {
        NotificationDto {
            id: n.id,
            alert_rule_id: n.alert_rule_id,
            alert_method_id: n.alert_method_id,
            fired_at: n.fired_at,
            payload: n.payload.clone(),
            delivery_status: n.delivery_status.clone(),
            read_at: n.read_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ListNotificationsQuery {
    #[serde(default)]
    unread_only: bool,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

#[derive(Debug, Serialize)]
struct NotificationsPageDto {
    items: Vec<NotificationDto>,
    total: i64,
    unread_count: i64,
}

async fn list_notifications(
    State(state): State<AppState>,
    Query(q): Query<ListNotificationsQuery>,
) -> Result<Json<NotificationsPageDto>, ApiError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);

    let (rows, total) = NotificationRepo::list(&state.pool, q.unread_only, limit, offset).await?;
    let unread_count = NotificationRepo::unread_count(&state.pool).await?;

    Ok(Json(NotificationsPageDto {
        items: rows.iter().map(NotificationDto::from).collect(),
        total,
        unread_count,
    }))
}

async fn mark_notification_read(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<NotificationDto>, ApiError> {
    let updated = NotificationRepo::mark_read(&state.pool, id)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => ApiError::NotFound,
            other => ApiError::from(other),
        })?;
    Ok(Json(NotificationDto::from(&updated)))
}

/// Small internal error type, same convention as `entities::ApiError`: DB
/// failures map to `500`; a missing `:id` (`NotificationRepo::mark_read`'s
/// `UPDATE ... RETURNING` finds no row, surfacing as `sqlx::Error::RowNotFound`)
/// is `404`.
enum ApiError {
    NotFound,
    Internal,
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in notifications route: {err}");
        ApiError::Internal
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::NotFound => StatusCode::NOT_FOUND.into_response(),
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}
