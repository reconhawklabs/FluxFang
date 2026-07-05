//! `GET/POST/PATCH/DELETE /api/zones[/:id]` (Task 6.7). PROTECTED — mounted
//! in `lib.rs::app`'s protected router group, behind `require_auth`, same as
//! every other non-setup/login route. Not to be confused with
//! `crate::ingest::zones`, which recomputes `zone_membership` during ingest
//! rather than serving any HTTP route.
//!
//! ## Response shapes
//!
//! `GET /api/zones`, `POST /api/zones`, and `PATCH /api/zones/:id` all
//! return [`crate::dto::ZoneDto`] — see its doc comment for why `lon`/`lat`
//! are flattened onto the top level rather than nested under `center`.
//!
//! `GET /api/zones/:id` returns [`ZoneDetailDto`]: every `ZoneDto` field
//! (flattened) plus `emitters`/`entities` — the subjects currently "in" the
//! zone, via `ZoneRepo::subjects_in_zone` (Task 1.3d; see that method's doc
//! comment for the exact "most-recent-located-emission" membership rule).
//!
//! ## Request shape: `center` is a nested `{lon, lat}` object
//!
//! Unlike the flattened response, `POST`/`PATCH` accept `center: {lon,
//! lat}` per the task brief — [`CenterDto`] is the request-side counterpart,
//! immediately unpacked into the `(f64, f64)` tuple `fluxfang_db`'s
//! `NewZone`/`ZoneRepo::update` expect.
//!
//! ## Validation
//!
//! [`validate_zone`] rejects (`400`, before any write) a non-positive
//! `radius_m` or a `center` outside `lon ∈ [-180, 180]`/`lat ∈ [-90, 90]`.
//! Runs on both create and update (an update that omits a field validates
//! against that field's *existing* value merged with whatever the request
//! did supply, same "validate the fully-merged result" approach
//! `alert_rules.rs::update_alert_rule` uses).
//!
//! ## Delete: cascade + rule-disable, atomically
//!
//! `DELETE /api/zones/:id` calls `ZoneRepo::delete_and_disable_rules`
//! (Task 6.7), not the older `ZoneRepo::delete` — see that method's own doc
//! comment for why: `zone_membership` rows cascade via the schema's `ON
//! DELETE CASCADE`, but `alert_rule.trigger`'s `zone_id` has no FK, so any
//! rule watching this zone gets `enabled = false` (never deleted) in the
//! same transaction as the zone delete.
//!
//! ## Error mapping
//!
//! Same convention as `emitters.rs`/`entities.rs`/`alert_rules.rs`: a bad
//! `radius_m`/`center` is `400`; a missing path-`:id` resource is `404`; any
//! other `sqlx::Error` is `500`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use fluxfang_db::models::NewZone;
use fluxfang_db::ZoneRepo;

use crate::dto::{EmitterDto, EntityDto, ZoneDto};
use crate::state::AppState;

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/api/zones", get(list_zones).post(create_zone))
        .route(
            "/api/zones/:id",
            get(get_zone).patch(update_zone).delete(delete_zone),
        )
}

/// Request-side `center` shape — see module docs on why this differs from
/// [`ZoneDto`]'s flattened response shape.
#[derive(Debug, Deserialize)]
struct CenterDto {
    lon: f64,
    lat: f64,
}

/// Reject a non-positive `radius_m` or an out-of-range `center`, pure/no-I/O
/// — see module docs.
fn validate_zone(radius_m: f64, center: &CenterDto) -> Result<(), ApiError> {
    if radius_m.is_nan() || radius_m <= 0.0 {
        return Err(ApiError::BadRequest(format!(
            "radius_m must be > 0, got {radius_m}"
        )));
    }
    if !(-180.0..=180.0).contains(&center.lon) {
        return Err(ApiError::BadRequest(format!(
            "center.lon must be in [-180, 180], got {}",
            center.lon
        )));
    }
    if !(-90.0..=90.0).contains(&center.lat) {
        return Err(ApiError::BadRequest(format!(
            "center.lat must be in [-90, 90], got {}",
            center.lat
        )));
    }
    Ok(())
}

/// The standard serde "distinguish absent from explicit null" recipe (same
/// helper `emitters.rs`/`entities.rs`/`alert_rules.rs` each define for their
/// own `PATCH` handlers): a `#[serde(default, deserialize_with = "some")]`
/// field decodes to `None` when the key is missing from the JSON object, and
/// to `Some(None)` when the key is present with a JSON `null` — letting a
/// `PATCH` body tell "leave `notes` alone" (key omitted) apart from "clear
/// it" (explicit `null`).
fn some<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

async fn list_zones(State(state): State<AppState>) -> Result<Json<Vec<ZoneDto>>, ApiError> {
    let rows = ZoneRepo::list(&state.pool).await?;
    Ok(Json(rows.iter().map(ZoneDto::from).collect()))
}

#[derive(Debug, Deserialize)]
struct CreateZoneRequest {
    name: String,
    center: CenterDto,
    radius_m: f64,
    #[serde(default)]
    notes: Option<String>,
}

async fn create_zone(
    State(state): State<AppState>,
    Json(req): Json<CreateZoneRequest>,
) -> Result<(StatusCode, Json<ZoneDto>), ApiError> {
    validate_zone(req.radius_m, &req.center)?;

    let created = ZoneRepo::insert(
        &state.pool,
        NewZone {
            name: req.name,
            center: (req.center.lon, req.center.lat),
            radius_m: req.radius_m,
            notes: req.notes,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(ZoneDto::from(&created))))
}

/// `GET /api/zones/:id`'s response — see module docs.
#[derive(Debug, Serialize)]
struct ZoneDetailDto {
    #[serde(flatten)]
    zone: ZoneDto,
    emitters: Vec<EmitterDto>,
    entities: Vec<EntityDto>,
}

async fn get_zone(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ZoneDetailDto>, ApiError> {
    let zone = ZoneRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let subjects = ZoneRepo::subjects_in_zone(&state.pool, id).await?;

    Ok(Json(ZoneDetailDto {
        zone: ZoneDto::from(&zone),
        emitters: subjects.emitters.iter().map(EmitterDto::from).collect(),
        entities: subjects.entities.iter().map(EntityDto::from).collect(),
    }))
}

#[derive(Debug, Deserialize)]
struct UpdateZoneRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    center: Option<CenterDto>,
    #[serde(default)]
    radius_m: Option<f64>,
    #[serde(default, deserialize_with = "some")]
    notes: Option<Option<String>>,
}

async fn update_zone(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateZoneRequest>,
) -> Result<Json<ZoneDto>, ApiError> {
    let existing = ZoneRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let name = req.name.unwrap_or(existing.name);
    let center = req.center.unwrap_or(CenterDto {
        lon: existing.lon,
        lat: existing.lat,
    });
    let radius_m = req.radius_m.unwrap_or(existing.radius_m);
    let notes = match req.notes {
        Some(inner) => inner,
        None => existing.notes,
    };

    validate_zone(radius_m, &center)?;

    let updated = ZoneRepo::update(
        &state.pool,
        id,
        &name,
        (center.lon, center.lat),
        radius_m,
        notes.as_deref(),
    )
    .await?;
    Ok(Json(ZoneDto::from(&updated)))
}

async fn delete_zone(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (zone_existed, _rules_disabled) =
        ZoneRepo::delete_and_disable_rules(&state.pool, id).await?;
    if zone_existed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Small internal error type, same convention as `emitters::ApiError`/
/// `entities::ApiError`/`alert_rules::ApiError`: DB failures map to `500`;
/// an invalid `radius_m`/`center` is `400`; a missing `:id` is `404`.
enum ApiError {
    BadRequest(String),
    NotFound,
    Internal,
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in zones route: {err}");
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
