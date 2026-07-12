//! Co-Travel Detection endpoints. PROTECTED — merged into `lib.rs::app`'s
//! protected group, so auth is applied by that group's `route_layer`.
//!
//! `GET /api/co-travel` runs `CoTravelRepo::candidates` (the PostGIS gate +
//! metrics), scores each row via `fluxfang_core::cotravel::score`, sorts by
//! score descending (spread as tiebreak, matching the repo's own order), then
//! applies `offset`/`limit` in-process and returns the `{items,total}`
//! envelope. `total` is the full qualifying count, ignoring pagination.
//!
//! Ignore/unignore are `POST`/`DELETE /api/co-travel/ignore/:emitter_id`;
//! `GET /api/co-travel/ignored` backs the Ignored panel.

use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use fluxfang_core::cotravel::{score, CoTravelMetrics};
use fluxfang_db::repo::cotravel::{CoTravelCandidate, CoTravelFilter, IgnoredEmitter};
use fluxfang_db::CoTravelRepo;

use crate::state::AppState;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 500;
const DEFAULT_MIN_DISTANCE_M: f64 = 402.336; // ¼ mile
const DEFAULT_MIN_TIME_S: f64 = 30.0;

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/api/co-travel", get(list_co_travel))
        .route("/api/co-travel/ignored", get(list_ignored))
        .route(
            "/api/co-travel/ignore/:emitter_id",
            post(ignore_emitter).delete(unignore_emitter),
        )
}

/// One ranked row on the Co-Travel page.
#[derive(Debug, Serialize)]
struct CoTravelDto {
    emitter_id: Uuid,
    name: String,
    emitter_type: Option<String>,
    identity_key: Option<String>,
    attributes: serde_json::Value,
    hits: i64,
    points: i64,
    span_s: f64,
    spread_m: f64,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    score: i32,
    tier: &'static str,
}

impl CoTravelDto {
    fn from_candidate(c: CoTravelCandidate) -> Self {
        let s = score(&CoTravelMetrics {
            spread_m: c.spread_m,
            points: c.points,
            span_s: c.span_s,
            hits: c.hits,
        });
        CoTravelDto {
            emitter_id: c.emitter_id,
            name: c.name,
            emitter_type: c.emitter_type,
            identity_key: c.identity_key,
            attributes: c.attributes,
            hits: c.hits,
            points: c.points,
            span_s: c.span_s,
            spread_m: c.spread_m,
            first_seen: c.first_seen,
            last_seen: c.last_seen,
            score: s.score,
            tier: s.tier.as_str(),
        }
    }
}

#[derive(Debug, Serialize)]
struct CoTravelPageDto {
    items: Vec<CoTravelDto>,
    total: i64,
}

async fn list_co_travel(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<CoTravelPageDto>, ApiError> {
    let params = parse_params(raw.as_deref().unwrap_or(""))?;
    let filter = CoTravelFilter {
        time_from: params.time_from,
        time_to: params.time_to,
        min_distance_m: params.min_distance_m,
        min_time_s: params.min_time_s,
    };

    let candidates = CoTravelRepo::candidates(&state.pool, &filter).await?;

    let mut scored: Vec<CoTravelDto> = candidates.into_iter().map(CoTravelDto::from_candidate).collect();
    // Highest score first; spread as a stable tiebreak.
    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.spread_m.partial_cmp(&a.spread_m).unwrap_or(std::cmp::Ordering::Equal))
    });

    let total = scored.len() as i64;
    let start = (params.offset.max(0) as usize).min(scored.len());
    let end = (start + params.limit as usize).min(scored.len());
    let items = scored.drain(start..end).collect();

    Ok(Json(CoTravelPageDto { items, total }))
}

async fn list_ignored(State(state): State<AppState>) -> Result<Json<Vec<IgnoredEmitter>>, ApiError> {
    let rows = CoTravelRepo::list_ignored(&state.pool).await?;
    Ok(Json(rows))
}

async fn ignore_emitter(
    State(state): State<AppState>,
    Path(emitter_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    CoTravelRepo::ignore(&state.pool, emitter_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
struct RemovedDto {
    removed: u64,
}

async fn unignore_emitter(
    State(state): State<AppState>,
    Path(emitter_id): Path<Uuid>,
) -> Result<Json<RemovedDto>, ApiError> {
    let removed = CoTravelRepo::unignore(&state.pool, emitter_id).await?;
    Ok(Json(RemovedDto { removed }))
}

struct Params {
    time_from: Option<DateTime<Utc>>,
    time_to: Option<DateTime<Utc>>,
    min_distance_m: f64,
    min_time_s: f64,
    limit: i64,
    offset: i64,
}

fn parse_params(raw: &str) -> Result<Params, ApiError> {
    let mut time_from = None;
    let mut time_to = None;
    let mut min_distance_m = DEFAULT_MIN_DISTANCE_M;
    let mut min_time_s = DEFAULT_MIN_TIME_S;
    let mut limit = DEFAULT_LIMIT;
    let mut offset: i64 = 0;

    for (key, value) in form_urlencoded::parse(raw.as_bytes()) {
        match key.as_ref() {
            "from" => time_from = Some(parse_time("from", &value)?),
            "to" => time_to = Some(parse_time("to", &value)?),
            "min_distance_m" => min_distance_m = parse_pos_f64("min_distance_m", &value)?,
            "min_time_s" => min_time_s = parse_pos_f64("min_time_s", &value)?,
            "limit" => {
                limit = value
                    .parse::<i64>()
                    .map_err(|_| ApiError::BadRequest(format!("invalid limit: {value:?}")))?
                    .clamp(1, MAX_LIMIT)
            }
            "offset" => {
                offset = value
                    .parse::<i64>()
                    .map_err(|_| ApiError::BadRequest(format!("invalid offset: {value:?}")))?
                    .max(0)
            }
            _ => {}
        }
    }

    Ok(Params {
        time_from,
        time_to,
        min_distance_m,
        min_time_s,
        limit,
        offset,
    })
}

fn parse_time(param: &str, raw: &str) -> Result<DateTime<Utc>, ApiError> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| ApiError::BadRequest(format!("invalid {param}: {raw:?} (expected RFC3339)")))
}

fn parse_pos_f64(param: &str, raw: &str) -> Result<f64, ApiError> {
    let v: f64 = raw
        .parse()
        .map_err(|_| ApiError::BadRequest(format!("invalid {param}: {raw:?} (expected a number)")))?;
    if !(v.is_finite() && v >= 0.0) {
        return Err(ApiError::BadRequest(format!(
            "invalid {param}: {raw:?} (must be a non-negative number)"
        )));
    }
    Ok(v)
}

enum ApiError {
    BadRequest(String),
    Internal,
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in co-travel route: {err}");
        ApiError::Internal
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}
