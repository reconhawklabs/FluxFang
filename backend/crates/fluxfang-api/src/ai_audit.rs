//! `GET /api/ai-audit` (PROTECTED) — the read side of the AI Audit Log page.
//! Lists ai_audit_log rows (AI-made additions/subtractions), newest first.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use fluxfang_db::repo::ai_audit::AiAuditFilter;
use fluxfang_db::AiAuditRepo;

use crate::state::AppState;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 500;

pub fn protected_routes() -> Router<AppState> {
    Router::new().route("/api/ai-audit", get(list_audit))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    time_from: Option<DateTime<Utc>>,
    #[serde(default)]
    time_to: Option<DateTime<Utc>>,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

#[derive(Debug, Serialize)]
struct AuditPageDto {
    items: Vec<serde_json::Value>,
    total: i64,
}

async fn list_audit(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<AuditPageDto>, ApiError> {
    let filter = AiAuditFilter {
        action: q.action.filter(|a| a == "add" || a == "remove"),
        time_from: q.time_from,
        time_to: q.time_to,
        search: q.search,
        limit: q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        offset: q.offset.unwrap_or(0).max(0),
    };
    let (rows, total) = AiAuditRepo::query(&state.pool, filter).await?;
    let items = rows
        .iter()
        .map(|r| serde_json::to_value(r).unwrap_or(serde_json::Value::Null))
        .collect();
    Ok(Json(AuditPageDto { items, total }))
}

enum ApiError {
    Internal,
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in ai_audit route: {err}");
        ApiError::Internal
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}
