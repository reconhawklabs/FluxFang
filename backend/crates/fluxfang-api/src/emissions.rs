//! `GET /api/emissions` (Task 6.3): filter/paginate `emission` rows, driving
//! `EmissionRepo::query` off query-string parameters. PROTECTED — mounted in
//! `lib.rs::app`'s protected router group, behind `require_auth`, same as
//! every other non-setup/login route.
//!
//! ## Phase 1c: bulk-delete / clear-all
//!
//! `POST /api/emissions/bulk-delete` (`{ids: [uuid]}`) and `POST
//! /api/emissions/clear` (no body) back the emissions list page's
//! mass-select "Delete selected" and "Clear All" actions. Both return `200
//! {deleted: <u64>}`, never an error for an empty/all-unknown `ids` list —
//! see `EmissionRepo::delete_bulk`'s doc comment.
//!
//! **`POST`, not `DELETE`, and why**: a `DELETE` request with a JSON body is
//! technically legal HTTP, but some proxies/load balancers strip bodies
//! from `DELETE` requests (the semantics of a body on `DELETE` are
//! historically underspecified), which would silently turn "delete these
//! N ids" into "delete nothing" in front of such a proxy. `POST` to a
//! dedicated, distinctly-named path sidesteps that risk entirely and is
//! unambiguous about carrying a body. Both routes are static path segments
//! (`/bulk-delete`, `/clear`), so they don't collide with any future
//! `/api/emissions/:id`-shaped route.
//!
//! ## Phase A5: `emitter_type`/`emitter_category`
//!
//! Two more recognized keys, both plain scalar strings (no special parse
//! rule, unlike `cond`): `emitter_type` (exact match on the emission's
//! *emitter's* `emitter_type`, e.g. `wifi_access_point`) and
//! `emitter_category` (a coarser prefix match, e.g. `wifi` matches both
//! `wifi_access_point` and `wifi_client`) — see
//! `fluxfang_db::repo::emission::EmissionFilter`'s doc comments for the
//! exact SQL. These power the overview map's toggleable per-category
//! heatmap layers. Setting either excludes emissions with no emitter at
//! all (`emitter_id IS NULL`), since a NULL emitter can't match any
//! `emitter_type`/`emitter_category`.
//!
//! ## Why raw query-string parsing instead of `axum::extract::Query`
//!
//! `axum::extract::Query<T>` deserializes via `serde_urlencoded`, which has
//! no way to collect *repeated* keys (`cond=a&cond=b&cond=c`) into a
//! `Vec<String>` field — it's built for one-value-per-key forms. Since this
//! endpoint's field-condition filter is exactly a repeated `cond` param,
//! the query string is instead pulled out whole via `RawQuery` and walked
//! with `form_urlencoded::parse`, which yields every `(key, value)` pair
//! (including repeats) in order, undoing percent-encoding along the way.
//! `parse_filter` builds an [`EmissionFilter`] from that pair stream by
//! hand, tracking `cond` occurrences into a `Vec` and letting every other
//! recognized key overwrite (last-one-wins is fine — none of them are
//! meant to repeat). Unrecognized keys are ignored rather than rejected, so
//! adding a new query param later is backwards compatible for older
//! clients/bookmarked URLs.
//!
//! ## `cond` parse rule: `field:op:valueJson`
//!
//! Each `cond` value is split on `:` into exactly 3 parts via
//! `splitn(3, ':')` — `field`, `op`, and `value` (the value itself may
//! freely contain further `:` characters, e.g. a MAC address or a regex
//! pattern with a literal colon, since only the first two separators are
//! consumed). `op` is parsed against [`fluxfang_core::rule::Op`]'s own
//! `#[serde(rename_all = "lowercase")]` wire form (`eq`, `neq`, `matches`,
//! `in`, `gte`, `lte`); anything else is a `400`.
//!
//! `value` is parsed permissively so the same query-param syntax can carry
//! numbers, strings, and arrays without the caller having to
//! percent-encode JSON quoting for the common case:
//! 1. Try `serde_json::from_str(value)` first — this lets a bare `6` parse
//!    as the JSON *number* `6`, `["aa:..","bb:.."]` parse as a JSON array,
//!    `true`/`false`/`null` parse as their JSON literals, and an
//!    explicitly-quoted `"6"` parse as the JSON *string* `"6"` (letting a
//!    caller force string typing when needed, e.g. to match a `Text` field
//!    that happens to look numeric).
//! 2. If that fails (e.g. `^Free` or `aa:bb:cc:dd:ee:ff` — not valid JSON
//!    on their own), fall back to treating the whole raw value as a JSON
//!    *string* literal.
//!
//! This mirrors `fluxfang_core::catalog`'s field types closely enough for
//! the common cases (`channel:gte:6` → number `6`; `ssid:matches:^Free` →
//! string `"^Free"`; `bssid:in:["aa:..","bb:.."]` → array) without forcing
//! every caller to hand-quote strings. Whether the resulting JSON type
//! actually matches the field's catalog type is not checked here at all —
//! that's `EmissionRepo::query`'s job (via
//! `fluxfang_core::conditions_to_sql_checked`), and its
//! `RuleSqlError::{UnknownField, InvalidOp, InvalidValueType}` are mapped
//! to `400` below rather than `500`.
//!
//! ## Caps
//!
//! - At most [`MAX_CONDITIONS`] `cond` params are accepted per request —
//!   past that, `400`. This bounds how many extra `WHERE` clauses (and
//!   binds) one request can force `EmissionRepo::query` to build.
//! - Any `cond` whose op is `in` with more than [`MAX_IN_ELEMENTS`] array
//!   elements is rejected with `400`. Postgres has a hard ~65535-parameter
//!   ceiling per statement; without this cap, a single pathological `in`
//!   array could single-handedly exhaust that budget (and would produce a
//!   multi-megabyte `IN (...)` clause well before then).
//!
//! ## Error mapping
//!
//! Anything this handler itself rejects (malformed `cond`/`bbox`/
//! `time_from`/`time_to`/`limit`/`offset`/`match`/uuid params, or the
//! `cond`-count/`in`-size caps above) is a `400` with a human-readable
//! message. `EmissionRepo::query`'s `EmissionQueryError::Rule` (unknown
//! field, invalid op for a field, or a mistyped value — all caller
//! mistakes, not server faults) is *also* mapped to `400`, not `500`;
//! only `EmissionQueryError::Sql` (an actual database failure) becomes
//! `500`.

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use fluxfang_core::rule::{Condition, MatchMode, Op};
use fluxfang_db::repo::emission::{EmissionFilter, EmissionQueryError};
use fluxfang_db::EmissionRepo;

use crate::dto::EmissionDto;
use crate::state::AppState;

/// Default page size when `limit` is omitted.
const DEFAULT_LIMIT: i64 = 50;
/// Hard ceiling `limit` is clamped to, regardless of what's requested.
const MAX_LIMIT: i64 = 500;
/// Maximum number of `cond` params accepted per request (see module docs).
const MAX_CONDITIONS: usize = 20;
/// Maximum number of elements in a single `cond`'s `in` array (see module
/// docs on the Postgres bind-count ceiling).
const MAX_IN_ELEMENTS: usize = 1000;

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/api/emissions", get(list_emissions))
        .route("/api/emissions/points", get(list_emission_points))
        .route("/api/emissions/bulk-delete", post(bulk_delete_emissions))
        .route("/api/emissions/clear", post(clear_emissions))
}

#[derive(Debug, Serialize)]
struct EmissionsPageDto {
    items: Vec<EmissionDto>,
    total: i64,
}

async fn list_emissions(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<EmissionsPageDto>, ApiError> {
    let filter = parse_filter(raw.as_deref().unwrap_or(""))?;
    let (rows, total) = EmissionRepo::query(&state.pool, filter).await?;
    Ok(Json(EmissionsPageDto {
        items: rows.iter().map(EmissionDto::from).collect(),
        total,
    }))
}

/// `GET /api/emissions/points`'s response envelope (Task 5): only
/// coordinates, uncapped (up to `EmissionRepo::MAX_POINTS`) unlike
/// `list_emissions`'s page-sized `items` — the Dashboard/Map heatmap's
/// source, so it isn't silently missing points older than the newest page.
#[derive(Debug, Serialize)]
struct EmissionPointsDto {
    points: Vec<[f64; 2]>,
    total: i64,
    truncated: bool,
}

/// Reuses `parse_filter` (its `limit`/`offset` are simply ignored by
/// `EmissionRepo::points`, which paginates by `MAX_POINTS` instead).
async fn list_emission_points(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<EmissionPointsDto>, ApiError> {
    let filter = parse_filter(raw.as_deref().unwrap_or(""))?;
    let (points, total) = EmissionRepo::points(&state.pool, filter).await?;
    let truncated = total > points.len() as i64;
    Ok(Json(EmissionPointsDto {
        points,
        total,
        truncated,
    }))
}

/// `POST /api/emissions/bulk-delete` request body — see module docs.
#[derive(Debug, Deserialize)]
struct BulkDeleteRequest {
    ids: Vec<Uuid>,
}

/// Shared response shape for both `bulk-delete` and `clear` — see module
/// docs.
#[derive(Debug, Serialize)]
struct DeletedCountDto {
    deleted: u64,
}

async fn bulk_delete_emissions(
    State(state): State<AppState>,
    Json(req): Json<BulkDeleteRequest>,
) -> Result<Json<DeletedCountDto>, ApiError> {
    let deleted = EmissionRepo::delete_bulk(&state.pool, &req.ids).await?;
    Ok(Json(DeletedCountDto { deleted }))
}

async fn clear_emissions(State(state): State<AppState>) -> Result<Json<DeletedCountDto>, ApiError> {
    let deleted = EmissionRepo::delete_all(&state.pool).await?;
    Ok(Json(DeletedCountDto { deleted }))
}

/// Build an [`EmissionFilter`] from a raw (undecoded) query string. See the
/// module docs for why this walks `form_urlencoded::parse` by hand rather
/// than using `axum::extract::Query`.
fn parse_filter(raw: &str) -> Result<EmissionFilter, ApiError> {
    let mut data_source_id = None;
    let mut session_id = None;
    let mut emitter_id = None;
    let mut unassigned = false;
    let mut time_from = None;
    let mut time_to = None;
    let mut bbox = None;
    let mut kind = None;
    let mut text = None;
    let mut match_mode = MatchMode::All;
    let mut limit = DEFAULT_LIMIT;
    let mut offset: i64 = 0;
    let mut cond_raw: Vec<String> = Vec::new();
    let mut emitter_type = None;
    let mut emitter_category = None;
    let mut sort = None;
    let mut dir = None;

    for (key, value) in form_urlencoded::parse(raw.as_bytes()) {
        match key.as_ref() {
            "data_source_id" => data_source_id = Some(parse_uuid("data_source_id", &value)?),
            "session_id" => session_id = Some(parse_uuid("session_id", &value)?),
            "emitter_id" => emitter_id = Some(parse_uuid("emitter_id", &value)?),
            "unassigned" => unassigned = parse_bool("unassigned", &value)?,
            "time_from" => time_from = Some(parse_time("time_from", &value)?),
            "time_to" => time_to = Some(parse_time("time_to", &value)?),
            "bbox" => bbox = Some(parse_bbox(&value)?),
            "kind" => kind = Some(value.into_owned()),
            "q" => text = Some(value.into_owned()),
            "match" => match_mode = parse_match_mode(&value)?,
            "limit" => limit = parse_limit(&value)?,
            "offset" => offset = parse_offset(&value)?,
            "cond" => cond_raw.push(value.into_owned()),
            "emitter_type" => emitter_type = Some(value.into_owned()),
            "emitter_category" => emitter_category = Some(value.into_owned()),
            "sort" => sort = Some(value.into_owned()),
            "dir" => dir = Some(value.into_owned()),
            _ => {}
        }
    }

    if cond_raw.len() > MAX_CONDITIONS {
        return Err(ApiError::BadRequest(format!(
            "too many cond params: {} (max {MAX_CONDITIONS})",
            cond_raw.len()
        )));
    }

    let field_conditions = cond_raw
        .iter()
        .map(|c| parse_condition(c))
        .collect::<Result<Vec<Condition>, ApiError>>()?;

    Ok(EmissionFilter {
        data_source_id,
        session_id,
        emitter_id,
        unassigned,
        time_from,
        time_to,
        bbox,
        kind,
        field_conditions,
        match_mode,
        text,
        emitter_type,
        emitter_category,
        limit,
        offset,
        sort,
        dir,
    })
}

fn parse_uuid(param: &str, raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| ApiError::BadRequest(format!("invalid {param}: {raw:?}")))
}

fn parse_bool(param: &str, raw: &str) -> Result<bool, ApiError> {
    match raw {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(ApiError::BadRequest(format!(
            "invalid {param}: {raw:?} (expected true/false)"
        ))),
    }
}

fn parse_time(param: &str, raw: &str) -> Result<DateTime<Utc>, ApiError> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| ApiError::BadRequest(format!("invalid {param}: {raw:?} (expected RFC3339)")))
}

/// `bbox=min_lon,min_lat,max_lon,max_lat`.
fn parse_bbox(raw: &str) -> Result<(f64, f64, f64, f64), ApiError> {
    let parts: Vec<&str> = raw.split(',').collect();
    let [min_lon, min_lat, max_lon, max_lat] = parts.as_slice() else {
        return Err(ApiError::BadRequest(format!(
            "invalid bbox: {raw:?} (expected min_lon,min_lat,max_lon,max_lat)"
        )));
    };
    let parse_one = |s: &str| -> Result<f64, ApiError> {
        s.trim()
            .parse()
            .map_err(|_| ApiError::BadRequest(format!("invalid bbox: {raw:?} (bad number {s:?})")))
    };
    Ok((
        parse_one(min_lon)?,
        parse_one(min_lat)?,
        parse_one(max_lon)?,
        parse_one(max_lat)?,
    ))
}

fn parse_match_mode(raw: &str) -> Result<MatchMode, ApiError> {
    match raw {
        "all" => Ok(MatchMode::All),
        "any" => Ok(MatchMode::Any),
        _ => Err(ApiError::BadRequest(format!(
            "invalid match: {raw:?} (expected all/any)"
        ))),
    }
}

fn parse_limit(raw: &str) -> Result<i64, ApiError> {
    let value: i64 = raw.parse().map_err(|_| {
        ApiError::BadRequest(format!("invalid limit: {raw:?} (expected an integer)"))
    })?;
    Ok(value.clamp(1, MAX_LIMIT))
}

fn parse_offset(raw: &str) -> Result<i64, ApiError> {
    let value: i64 = raw.parse().map_err(|_| {
        ApiError::BadRequest(format!("invalid offset: {raw:?} (expected an integer)"))
    })?;
    Ok(value.max(0))
}

/// Parse one `cond=field:op:valueJson` param into a [`Condition`]. See the
/// module docs for the exact `value` parse rule and the `in`-size cap.
fn parse_condition(raw: &str) -> Result<Condition, ApiError> {
    let mut parts = raw.splitn(3, ':');
    let field = parts.next().unwrap_or("");
    let op_str = parts.next().ok_or_else(|| {
        ApiError::BadRequest(format!("malformed cond {raw:?} (expected field:op:value)"))
    })?;
    let value_str = parts.next().ok_or_else(|| {
        ApiError::BadRequest(format!("malformed cond {raw:?} (expected field:op:value)"))
    })?;

    if field.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "malformed cond {raw:?}: field must not be empty"
        )));
    }

    let op: Op = serde_json::from_value(serde_json::Value::String(op_str.to_string()))
        .map_err(|_| ApiError::BadRequest(format!("cond {raw:?}: unknown operator {op_str:?}")))?;

    // Parse-JSON-first, string-fallback (see module docs): lets bare
    // numbers/arrays/literals be written without caller-side JSON quoting,
    // while any non-JSON token (a bssid, a regex) is still usable as-is.
    let value: serde_json::Value = serde_json::from_str(value_str)
        .unwrap_or_else(|_| serde_json::Value::String(value_str.to_string()));

    if op == Op::In {
        if let Some(items) = value.as_array() {
            if items.len() > MAX_IN_ELEMENTS {
                return Err(ApiError::BadRequest(format!(
                    "cond {raw:?}: `in` array has {} elements (max {MAX_IN_ELEMENTS})",
                    items.len()
                )));
            }
        }
    }

    Ok(Condition {
        field: field.to_string(),
        op,
        value,
    })
}

/// Small internal error type, same convention as `data_sources::ApiError`:
/// DB failures map to `500`; deliberate/caller-input rejections (including
/// `EmissionQueryError::Rule` — see module docs) carry `400`.
enum ApiError {
    BadRequest(String),
    Internal,
}

impl From<EmissionQueryError> for ApiError {
    fn from(err: EmissionQueryError) -> Self {
        match err {
            EmissionQueryError::Rule(e) => ApiError::BadRequest(e.to_string()),
            EmissionQueryError::Sql(e) => {
                eprintln!("fluxfang-api: db error in emissions route: {e}");
                ApiError::Internal
            }
        }
    }
}

/// For `EmissionRepo::delete_bulk`/`delete_all` (Phase 1c), which return a
/// plain `sqlx::Error` rather than `EmissionQueryError` — always a `500`,
/// same as `EmissionQueryError::Sql` above.
impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in emissions route: {err}");
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
