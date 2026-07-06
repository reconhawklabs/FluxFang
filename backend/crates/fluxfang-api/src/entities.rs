//! `GET/POST/PATCH/DELETE /api/entities[/:id]` (Task 6.5). PROTECTED —
//! mounted in `lib.rs::app`'s protected router group, behind `require_auth`,
//! same as every other non-setup/login route. Replaces the placeholder
//! `GET /api/entities -> []` stub `lib.rs` carried since Task 2.2.
//!
//! ## Response shapes
//!
//! `GET /api/entities`, `POST /api/entities`, and `PATCH /api/entities/:id`
//! all return [`crate::dto::EntityDto`] — see its doc comment for why
//! `last_seen` is deliberately omitted there.
//!
//! ## Phase 1b: `GET /api/entities` search + pagination
//!
//! `GET /api/entities` accepts `search`, `limit` (default 50, clamped to a
//! max of 500, same convention as `emitters.rs`/`emissions.rs`/
//! `notifications.rs`), and `offset` query params, delegating to
//! `EntityRepo::query`/`EntityListFilter` — see that repo module's doc
//! comment for the exact search SQL. **This is a response-shape change**:
//! the endpoint used to return a bare `[EntityDto]` array; it now returns
//! `{items, total}`, the same pagination envelope the other listing
//! endpoints use. The frontend is updated in a later phase.
//!
//! `GET /api/entities/:id` returns [`EntityDetailDto`]: every `EntityDto`
//! field (flattened) plus `last_seen`, `emitters`, and `recent_detections`
//! — everything the map/tracking view needs for one entity in a single
//! round trip:
//! - `emitters`: every [`crate::dto::EmitterDto`] currently grouped under
//!   this entity, via `EmitterRepo::list_by_entity`.
//! - `last_seen`: `EntityRepo::last_seen` — the max `observed_at` across
//!   every emission belonging to any of this entity's emitters, `None` if
//!   there are none.
//! - `recent_detections`: the most recent [`RECENT_DETECTIONS_LIMIT`]
//!   geolocated emissions across *all* of this entity's emitters combined,
//!   newest first, via `EmissionRepo::recent_located` (a single `emitter_id
//!   = ANY(...)` query rather than one paginated call per emitter — see
//!   that method's doc comment).
//!
//! An entity with no emitters reports `emitters: []`, `recent_detections:
//! []`, `last_seen: null` — both repo calls handle an empty emitter set
//! (or an emitter set that isn't Any-matched) without any special-casing
//! here.
//!
//! ## Delete: `entity_id` SET NULL, not cascade
//!
//! `DELETE /api/entities/:id` removes the `entity` row; per the schema's
//! `emitter.entity_id REFERENCES entity(id) ON DELETE SET NULL`, any
//! emitters previously grouped under it survive, just detached (their own
//! emissions, `first_seen_at`/`last_seen_at`, and `match_criteria` are all
//! untouched).
//!
//! ## Error mapping
//!
//! Same convention as `emitters.rs`/`emissions.rs`: a missing path-`:id`
//! resource is `404`; any other `sqlx::Error` is `500`. There's no `400`
//! case of this module's own (unlike `emitters.rs`'s rule validation) —
//! malformed request bodies are rejected by axum's `Json` extractor itself
//! before a handler here ever runs.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use fluxfang_db::models::{Emission, NewEntity};
use fluxfang_db::repo::entity::EntityListFilter;
use fluxfang_db::{EmissionRepo, EmitterRepo, EntityRepo};

use crate::dto::{EmitterDto, EntityDto};
use crate::state::AppState;

/// Cap on `recent_detections` in the `GET /api/entities/:id` response — see
/// module docs.
const RECENT_DETECTIONS_LIMIT: i64 = 100;
/// Default page size when `limit` is omitted — same default `emitters.rs`/
/// `emissions.rs`/`notifications.rs` use for their own listing endpoints.
const DEFAULT_LIMIT: i64 = 50;
/// Hard ceiling `limit` is clamped to, regardless of what's requested.
const MAX_LIMIT: i64 = 500;

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/api/entities", get(list_entities).post(create_entity))
        .route(
            "/api/entities/:id",
            get(get_entity).patch(update_entity).delete(delete_entity),
        )
}

/// The standard serde "distinguish absent from explicit null" recipe (same
/// helper `emitters.rs` defines for its own `PATCH` handler): a
/// `#[serde(default, deserialize_with = "some")]` field decodes to `None`
/// when the key is missing from the JSON object, and to `Some(None)` when
/// the key is present with a JSON `null` — letting `PATCH` bodies tell
/// "leave `notes` alone" (key omitted) apart from "clear it" (`notes:
/// null`).
fn some<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

/// `GET /api/entities` query params (Phase 1b: search + pagination). All
/// optional; see [`EntityListFilter`] for search semantics.
#[derive(Debug, Deserialize)]
struct ListEntitiesQuery {
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

/// `GET /api/entities`' response — response-shape change from a bare
/// `[EntityDto]` array to `{items, total}`, same shape
/// `emitters.rs`/`emissions.rs`/`notifications.rs` use for their own
/// paginated listings.
#[derive(Debug, Serialize)]
struct EntitiesPageDto {
    items: Vec<EntityDto>,
    total: i64,
}

async fn list_entities(
    State(state): State<AppState>,
    Query(q): Query<ListEntitiesQuery>,
) -> Result<Json<EntitiesPageDto>, ApiError> {
    let filter = EntityListFilter {
        search: q.search,
        limit: q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        offset: q.offset.unwrap_or(0).max(0),
    };
    let (rows, total) = EntityRepo::query(&state.pool, filter).await?;
    Ok(Json(EntitiesPageDto {
        items: rows.iter().map(EntityDto::from).collect(),
        total,
    }))
}

#[derive(Debug, Deserialize)]
struct CreateEntityRequest {
    name: String,
    #[serde(default)]
    notes: Option<String>,
}

async fn create_entity(
    State(state): State<AppState>,
    Json(req): Json<CreateEntityRequest>,
) -> Result<(StatusCode, Json<EntityDto>), ApiError> {
    let created = EntityRepo::insert(
        &state.pool,
        NewEntity {
            name: req.name,
            notes: req.notes,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(EntityDto::from(&created))))
}

/// One row in `GET /api/entities/:id`'s `recent_detections` — see module
/// docs. `lat`/`lon` are plain `f64` (not `Option<f64>`, unlike
/// `fluxfang_db::models::Emission::lat`/`lon`): every row here comes from
/// `EmissionRepo::recent_located`, which only ever returns emissions with a
/// non-NULL `location`, so both are always present.
#[derive(Debug, Serialize)]
struct RecentDetectionDto {
    emitter_id: Option<Uuid>,
    lat: f64,
    lon: f64,
    observed_at: DateTime<Utc>,
}

impl From<&Emission> for RecentDetectionDto {
    fn from(e: &Emission) -> Self {
        RecentDetectionDto {
            emitter_id: e.emitter_id,
            lat: e.lat.expect("recent_located rows are always geolocated"),
            lon: e.lon.expect("recent_located rows are always geolocated"),
            observed_at: e.observed_at,
        }
    }
}

/// `GET /api/entities/:id`'s response — see module docs.
#[derive(Debug, Serialize)]
struct EntityDetailDto {
    #[serde(flatten)]
    entity: EntityDto,
    last_seen: Option<DateTime<Utc>>,
    emitters: Vec<EmitterDto>,
    recent_detections: Vec<RecentDetectionDto>,
}

async fn get_entity(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<EntityDetailDto>, ApiError> {
    let entity = EntityRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let emitters = EmitterRepo::list_by_entity(&state.pool, id).await?;
    let last_seen = EntityRepo::last_seen(&state.pool, id).await?;

    let emitter_ids: Vec<Uuid> = emitters.iter().map(|e| e.id).collect();
    let recent_detections =
        EmissionRepo::recent_located(&state.pool, &emitter_ids, RECENT_DETECTIONS_LIMIT).await?;

    Ok(Json(EntityDetailDto {
        entity: EntityDto::from(&entity),
        last_seen,
        emitters: emitters.iter().map(EmitterDto::from).collect(),
        recent_detections: recent_detections
            .iter()
            .map(RecentDetectionDto::from)
            .collect(),
    }))
}

#[derive(Debug, Deserialize)]
struct UpdateEntityRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, deserialize_with = "some")]
    notes: Option<Option<String>>,
}

async fn update_entity(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateEntityRequest>,
) -> Result<Json<EntityDto>, ApiError> {
    let existing = EntityRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let name = req.name.unwrap_or(existing.name);
    let notes = match req.notes {
        Some(inner) => inner,
        None => existing.notes,
    };

    let updated = EntityRepo::update(&state.pool, id, &name, notes.as_deref()).await?;
    Ok(Json(EntityDto::from(&updated)))
}

async fn delete_entity(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let deleted = EntityRepo::delete(&state.pool, id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Small internal error type, same convention as `emitters::ApiError`/
/// `emissions::ApiError`: DB failures map to `500`; a missing `:id` is
/// `404`. See module docs for why there's no `400` variant here.
enum ApiError {
    NotFound,
    Internal,
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in entities route: {err}");
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
