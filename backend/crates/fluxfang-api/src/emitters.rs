//! `GET/POST/PATCH/DELETE /api/emitters[/:id]`, `POST
//! /api/emitters/:id/rule`, `POST /api/emitters/with-entity`, `GET
//! /api/emitters/preview`, and `GET /api/emitters/types` (Task 6.4).
//! PROTECTED — mounted in `lib.rs::app`'s protected router group, behind
//! `require_auth`, same as every other non-setup/login route.
//!
//! ## Task 4: `GET /api/emitters/types`
//!
//! Returns `[{key, label}]` for every `emitter_type` that actually has at
//! least one emitter (via `EmitterRepo::distinct_types_in_use`), sorted by
//! label — the Emitters page's Type-filter dropdown's stable backend
//! source, replacing the previous "derive options from whatever rows
//! happen to be loaded" approach. See [`list_emitter_types_in_use`].
//!
//! ## Response shape
//!
//! Every handler returns [`crate::dto::EmitterDto`] (see its doc comment)
//! rather than `fluxfang_db::models::Emitter` directly.
//!
//! ## Phase 1b: `GET /api/emitters` search + entity filter + pagination
//!
//! `GET /api/emitters` accepts `search`, `entity_id`, `emitter_type`, `limit`
//! (default 50, clamped to a max of 500, same convention as `emissions.rs`/
//! `notifications.rs`), and `offset` query params, delegating to
//! `EmitterRepo::query`/`EmitterListFilter` — see that repo module's doc
//! comment for the exact search SQL. **This is a response-shape change**:
//! the endpoint used to return a bare `[EmitterDto]` array; it now returns
//! `{items, total}`, the same pagination envelope `GET /api/emissions`/`GET
//! /api/notifications` already use. The frontend is updated in a later
//! phase.
//!
//! `emitter_type` (added alongside the Type-filter dropdown) is an
//! exact-match filter on the `emitter_type` column, ANDed with `search`/
//! `entity_id` when combined — see [`EmitterListFilter`]'s doc comment.
//!
//! ## Phase A5: `PATCH` accepts `match_enabled`/`attributes`
//!
//! Alongside the existing `name`/`type`/`entity_id` fields, `PATCH
//! /api/emitters/:id` accepts `match_enabled: bool` (toggle the emitter's
//! auto-attach rule on/off, via `EmitterRepo::set_match_enabled`) and
//! `attributes: <json object>` (a manual override — e.g. flipping a
//! `randomized_mac` flag the classifier auto-detected — via
//! `EmitterRepo::set_attributes`, a **full replace**, not a merge). See
//! [`UpdateEmitterRequest`]'s own doc comment for why these two are plain
//! `Option<T>` rather than the `entity_id`/`type` "absent vs. null" dance.
//!
//! ## Rule validation happens before any mutation
//!
//! Three endpoints here (`create_emitter`, `set_rule`, `create_with_entity`)
//! accept a `match_criteria`/rule from the caller and, if given, run
//! `EmitterRepo::attach_emissions_matching`/`create_with_entity`'s backfill
//! against it. Before doing anything else, [`validate_rule`] runs the exact
//! same [`fluxfang_core::conditions_to_sql_checked`] catalog check the
//! backfill itself uses (unknown field, invalid op for a field, or a
//! mistyped value) — but as a **pure, no-I/O call**, so an invalid rule is
//! rejected with `400` before a single row is inserted or updated. This
//! means the backfill call that follows is never expected to itself fail
//! with `EmitterRuleError::Rule` in practice, but that mapping is kept
//! anyway (`RuleSqlError` -> `400`, never `500`) as defense in depth rather
//! than assumed dead code.
//!
//! ## `from_emission_id` default-rule prefill
//!
//! `POST /api/emitters` accepts `{from_emission_id, name, type?}` as an
//! alternative to an explicit `match_criteria`: the referenced emission is
//! loaded, and (for `kind = "wifi"`, the only kind this schema supports —
//! see `repo::emitter`'s module docs) a default rule
//! `{"match":"all","conditions":[{"field":"bssid","op":"eq","value":<that
//! emission's payload.bssid>}]}` is built and used as if the caller had
//! supplied it directly as `match_criteria`, backfill included. Both
//! `match_criteria` and `from_emission_id` may not be given.
//!
//! ## `with-entity`: atomic entity+emitter creation
//!
//! `POST /api/emitters/with-entity` delegates entirely to
//! `EmitterRepo::create_with_entity`, which runs the entity insert, emitter
//! insert, and optional backfill inside one transaction — see that
//! function's doc comment. This handler's own job is just request
//! parsing/validation and response shaping.
//!
//! ## Error mapping
//!
//! Same convention as `emissions.rs`/`data_sources.rs`: deliberate
//! rejections (malformed body, unknown emitter/entity id referenced by a
//! request, an invalid rule) are `400`; a missing path-`:id` resource is
//! `404`; any other `sqlx::Error` is `500`.
//!
//! ## Phase 1c: bulk-delete / clear-all
//!
//! `POST /api/emitters/bulk-delete` (`{ids: [uuid]}`) and `POST
//! /api/emitters/clear` (no body) back the emitters list page's
//! mass-select "Delete selected" and "Clear All" actions, alongside the
//! existing single-row `DELETE /api/emitters/:id`. Both return `200
//! {deleted: <u64>}` — see `emissions.rs`'s module docs for why `POST`
//! (not `DELETE`-with-a-body) is used for the two bulk/clear routes. Same
//! `emission.emitter_id` `ON DELETE SET NULL` cascade as the existing
//! single-row delete applies to every emitter removed this way.

use axum::extract::{Path, Query, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use fluxfang_core::rule::{Condition, MatchMode, Op};
use fluxfang_core::{
    catalog_for, catalog_kind_for, conditions_to_sql_checked, is_known_emitter_type, Rule,
    RuleSqlError,
};
use fluxfang_db::models::{NewEmitter, NewEntity};
use fluxfang_db::repo::emitter::{
    EmitterListFilter, EmitterQueryError, EmitterRuleError, EmitterWithEntity,
};
use fluxfang_db::{EmissionRepo, EmitterAssociationRepo, EmitterRepo};

use crate::dto::{EmitterDto, EntityDto, InUseEmitterTypeDto};
use crate::state::AppState;

/// Default page size when `limit` is omitted — same default `emissions.rs`/
/// `notifications.rs` use for their own listing endpoints.
const DEFAULT_LIMIT: i64 = 50;
/// Hard ceiling `limit` is clamped to, regardless of what's requested.
const MAX_LIMIT: i64 = 500;

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/api/emitters", get(list_emitters).post(create_emitter))
        .route("/api/emitters/preview", get(preview_emitters))
        .route("/api/emitters/types", get(list_emitter_types_in_use))
        .route("/api/emitters/with-entity", post(create_with_entity))
        .route("/api/emitters/bulk-delete", post(bulk_delete_emitters))
        .route("/api/emitters/clear", post(clear_emitters))
        .route(
            "/api/emitters/:id",
            get(get_emitter)
                .patch(update_emitter)
                .delete(delete_emitter),
        )
        .route("/api/emitters/:id/rule", post(set_rule))
        .route(
            "/api/emitters/:id/associations",
            get(list_associations).post(add_association),
        )
        .route(
            "/api/emitters/:id/associations/:other_id",
            delete(remove_association),
        )
}

/// Validate `rule.conditions` against `kind`'s catalog (Task 4: `kind` is
/// the data-source kind the emitter belongs to, e.g. `"wifi"` or
/// `"bluetooth"` — see `fluxfang_core::catalog_kind_for`) with no DB access
/// at all — see module docs for why this runs before any mutation rather
/// than relying on the backfill call itself to reject a bad rule.
fn validate_rule(rule: &Rule, kind: &str) -> Result<(), RuleSqlError> {
    let catalog = catalog_for(kind);
    conditions_to_sql_checked(&rule.conditions, rule.match_mode, 1, &catalog).map(|_| ())
}

/// Reject an unrecognized `emitter_type` (e.g. `Some("bluetooth_beacon")`)
/// with `400` before any row is inserted — same "validate before mutating"
/// convention `validate_rule` follows. `None` (the field wasn't given at
/// all) always passes: this only guards a caller-supplied value, matching
/// today's behavior when `emitter_type` is absent (free-text `type` only,
/// `emitter_type` left `NULL`).
fn validate_emitter_type(emitter_type: &Option<String>) -> Result<(), ApiError> {
    match emitter_type {
        Some(t) if !is_known_emitter_type(t) => {
            Err(ApiError::BadRequest(format!("unknown emitter_type: {t:?}")))
        }
        _ => Ok(()),
    }
}

/// Parse `raw` (a `match_criteria` JSON value) into a [`Rule`], mapping a
/// deserialize failure to `400` rather than `500`.
fn parse_rule(raw: &serde_json::Value) -> Result<Rule, ApiError> {
    serde_json::from_value(raw.clone())
        .map_err(|e| ApiError::BadRequest(format!("invalid match_criteria: {e}")))
}

/// The standard serde "distinguish absent from explicit null" recipe: a
/// `#[serde(default, deserialize_with = "some")]` field decodes to `None`
/// when the key is missing from the JSON object, and to `Some(None)` when
/// the key is present with a JSON `null` -- letting `PATCH` bodies tell
/// "leave `entity_id` alone" (key omitted) apart from "detach" (`entity_id:
/// null`).
fn some<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

/// `GET /api/emitters` query params (Phase 1b: search + entity filter +
/// pagination; this phase adds `emitter_type`). All optional; see
/// [`EmitterListFilter`] for search/`emitter_type` semantics.
#[derive(Debug, Deserialize)]
struct ListEmittersQuery {
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    entity_id: Option<Uuid>,
    /// Exact-match filter on the `emitter_type` column (e.g.
    /// `"wifi_access_point"`) — the Emitters page's Type-filter dropdown.
    /// Not validated against `fluxfang_core::is_known_emitter_type`: an
    /// unrecognized value simply matches nothing, same as any other filter
    /// value that happens not to exist in the data (unlike `POST
    /// /api/emitters`'s `emitter_type`, which is rejected up front because
    /// it's about to be persisted).
    #[serde(default)]
    emitter_type: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
    /// Sort key, allow-listed against `EMITTER_SORTS` (`name`, `identity`,
    /// `first_seen`, `last_seen`, `emissions`); an unrecognized/absent value
    /// falls back to `last_seen`. See `EmitterRepo::query`'s doc comment.
    #[serde(default)]
    sort: Option<String>,
    /// Sort direction (`"asc"`/`"desc"`, case-insensitive); anything else
    /// falls back to the default direction (`desc`).
    #[serde(default)]
    dir: Option<String>,
}

/// `GET /api/emitters`' response — response-shape change from a bare
/// `[EmitterDto]` array to `{items, total}`, same shape
/// `emissions.rs`/`notifications.rs` use for their own paginated listings.
#[derive(Debug, Serialize)]
struct EmittersPageDto {
    items: Vec<EmitterDto>,
    total: i64,
}

/// Task 3: alongside the scalar `ListEmittersQuery` params, `GET
/// /api/emitters` accepts repeated `cond=field:op:valueJson` params plus an
/// optional `match=all|any`, exactly like `GET /api/emissions` — attribute
/// filters over the emitter's `attributes` JSONB, validated against the
/// selected type's catalog. Because `axum::extract::Query` can't collect
/// repeated keys, `cond`/`match` are pulled from `RawQuery` with
/// `form_urlencoded::parse` (reusing `emissions::parse_condition`/
/// `parse_match_mode`), while the scalar params still come from
/// `Query<ListEmittersQuery>` (which simply ignores the repeated `cond`
/// key). Attribute filtering is type-specific: `cond` without an
/// `emitter_type` is a 400, since an empty catalog rejects every field.
async fn list_emitters(
    State(state): State<AppState>,
    Query(q): Query<ListEmittersQuery>,
    RawQuery(raw): RawQuery,
) -> Result<Json<EmittersPageDto>, ApiError> {
    // Collect repeated `cond` params + optional `match` from the raw query.
    let mut cond_raw: Vec<String> = Vec::new();
    let mut match_mode = MatchMode::All;
    for (key, value) in form_urlencoded::parse(raw.as_deref().unwrap_or("").as_bytes()) {
        match key.as_ref() {
            "cond" => cond_raw.push(value.into_owned()),
            "match" => {
                match_mode = crate::emissions::parse_match_mode(&value).map_err(ApiError::BadRequest)?
            }
            _ => {}
        }
    }

    if cond_raw.len() > crate::emissions::MAX_CONDITIONS {
        return Err(ApiError::BadRequest(format!(
            "too many cond params: {} (max {})",
            cond_raw.len(),
            crate::emissions::MAX_CONDITIONS
        )));
    }

    let field_conditions = cond_raw
        .iter()
        .map(|c| crate::emissions::parse_condition(c).map_err(ApiError::BadRequest))
        .collect::<Result<Vec<Condition>, ApiError>>()?;

    // Attribute filtering is type-specific: `cond` params validate against
    // the selected type's catalog, so they require an `emitter_type` (an
    // empty catalog would reject every field as unknown -> a confusing 400).
    if !field_conditions.is_empty() && q.emitter_type.is_none() {
        return Err(ApiError::BadRequest(
            "attribute filters (cond) require selecting an emitter_type".to_string(),
        ));
    }

    let filter = EmitterListFilter {
        search: q.search,
        entity_id: q.entity_id,
        emitter_type: q.emitter_type,
        field_conditions,
        match_mode,
        limit: q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        offset: q.offset.unwrap_or(0).max(0),
        sort: q.sort,
        dir: q.dir,
    };
    let (rows, total) = EmitterRepo::query(&state.pool, filter).await?;
    Ok(Json(EmittersPageDto {
        items: rows
            .iter()
            .map(|(e, count)| EmitterDto::from_parts(e, *count))
            .collect(),
        total,
    }))
}

#[derive(Debug, Deserialize)]
struct CreateEmitterRequest {
    name: String,
    #[serde(rename = "type", default)]
    type_: Option<String>,
    #[serde(default)]
    entity_id: Option<Uuid>,
    #[serde(default)]
    match_criteria: Option<serde_json::Value>,
    /// Alternative to `match_criteria`: prefill a default rule from an
    /// existing emission's payload (currently only wifi's `bssid`). See
    /// module docs.
    #[serde(default)]
    from_emission_id: Option<Uuid>,
    /// Optional machine emitter-type key (e.g. `"wifi_access_point"`),
    /// letting a frontend "create emitter" form send a dropdown selection
    /// (backed by `GET /api/emitter-types/:kind`) instead of only the
    /// free-text `type`. When given, it's validated against
    /// `fluxfang_core::is_known_emitter_type` (400 if unrecognized) and
    /// stored on `Emitter::emitter_type`, so the created emitter's
    /// `type_label`/`category` derive from it like an auto-classified one
    /// would. `type_` (the free-text label) may still be given
    /// independently — the two aren't mutually exclusive. Absent entirely
    /// (the pre-existing behavior): `emitter_type` stays `NULL`.
    #[serde(default)]
    emitter_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct EmitterAndCount {
    emitter: EmitterDto,
    attached_count: u64,
}

async fn create_emitter(
    State(state): State<AppState>,
    Json(req): Json<CreateEmitterRequest>,
) -> Result<(StatusCode, Json<EmitterAndCount>), ApiError> {
    if req.match_criteria.is_some() && req.from_emission_id.is_some() {
        return Err(ApiError::BadRequest(
            "match_criteria and from_emission_id are mutually exclusive".to_string(),
        ));
    }
    validate_emitter_type(&req.emitter_type)?;
    let kind = catalog_kind_for(req.emitter_type.as_deref());

    let (match_criteria, rule) =
        resolve_match_criteria(&state, req.match_criteria, req.from_emission_id, kind).await?;

    let new = NewEmitter {
        name: req.name,
        type_: req.type_,
        entity_id: req.entity_id,
        match_criteria,
        emitter_type: req.emitter_type,
        ..Default::default()
    };
    let created = EmitterRepo::insert(&state.pool, new).await?;

    let attached_count = if let Some(rule) = &rule {
        EmitterRepo::attach_emissions_matching(&state.pool, created.id, rule, kind).await?
    } else {
        0
    };

    let final_row = if attached_count > 0 {
        EmitterRepo::get(&state.pool, created.id)
            .await?
            .ok_or(ApiError::Internal)?
    } else {
        created
    };

    Ok((
        StatusCode::CREATED,
        Json(EmitterAndCount {
            emitter: EmitterDto::from(&final_row),
            attached_count,
        }),
    ))
}

/// Resolve a `POST /api/emitters`-style request's desired `match_criteria`
/// (the JSON to persist) and, if any was given (directly or via
/// `from_emission_id`), the parsed+validated [`Rule`] to backfill with. Both
/// `None` when the emitter should be created unassigned.
async fn resolve_match_criteria(
    state: &AppState,
    match_criteria: Option<serde_json::Value>,
    from_emission_id: Option<Uuid>,
    kind: &str,
) -> Result<(serde_json::Value, Option<Rule>), ApiError> {
    if let Some(raw) = match_criteria {
        let rule = parse_rule(&raw)?;
        validate_rule(&rule, kind).map_err(|e| ApiError::BadRequest(e.to_string()))?;
        return Ok((raw, Some(rule)));
    }

    if let Some(emission_id) = from_emission_id {
        let emission = EmissionRepo::get(&state.pool, emission_id)
            .await?
            .ok_or_else(|| {
                ApiError::BadRequest(format!("from_emission_id {emission_id} not found"))
            })?;

        if emission.kind != "wifi" {
            return Err(ApiError::BadRequest(format!(
                "cannot derive a default match rule for emission kind {:?}",
                emission.kind
            )));
        }
        let bssid = emission
            .payload
            .get("bssid")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ApiError::BadRequest("emission has no string bssid in its payload".to_string())
            })?;

        let rule = Rule {
            match_mode: MatchMode::All,
            conditions: vec![Condition {
                field: "bssid".to_string(),
                op: Op::Eq,
                value: serde_json::Value::String(bssid.to_string()),
            }],
        };
        validate_rule(&rule, kind).map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let json = serde_json::to_value(&rule).expect("Rule always serializes");
        return Ok((json, Some(rule)));
    }

    Ok((serde_json::json!({}), None))
}

async fn get_emitter(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<EmitterDto>, ApiError> {
    let emitter = EmitterRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(EmitterDto::from(&emitter)))
}

/// `match_enabled`/`attributes` (Phase A5) are plain `Option<T>` fields, not
/// the `Option<Option<T>>` "absent vs. explicit null" dance `type_`/
/// `entity_id` use: neither is nullable on the wire (`match_enabled` is a
/// bool, `attributes` is always a JSON object), so there's nothing for an
/// explicit `null` to mean — the key is either present (apply it) or
/// absent (leave it alone).
///
/// `attributes`, when present, is a **full replace**, not a merge: the
/// simplest, least-surprising semantics for a manual override (e.g.
/// setting `randomized_mac`) — a caller wanting to tweak one key reads the
/// current `GET` response's `attributes` first and posts back the whole
/// object with that key changed, same pattern as `match_criteria`
/// elsewhere in this file.
#[derive(Debug, Deserialize)]
struct UpdateEmitterRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "type", default, deserialize_with = "some")]
    type_: Option<Option<String>>,
    #[serde(default, deserialize_with = "some")]
    entity_id: Option<Option<Uuid>>,
    #[serde(default)]
    match_enabled: Option<bool>,
    #[serde(default)]
    attributes: Option<serde_json::Value>,
}

async fn update_emitter(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateEmitterRequest>,
) -> Result<Json<EmitterDto>, ApiError> {
    let existing = EmitterRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let mut current = existing;

    if req.name.is_some() || req.type_.is_some() {
        let name = req.name.unwrap_or_else(|| current.name.clone());
        let type_ = match req.type_ {
            Some(inner) => inner,
            None => current.type_.clone(),
        };
        current = EmitterRepo::update_basic(&state.pool, id, &name, type_.as_deref()).await?;
    }

    if let Some(entity_id) = req.entity_id {
        current = EmitterRepo::set_entity(&state.pool, id, entity_id).await?;
    }

    if let Some(enabled) = req.match_enabled {
        current = EmitterRepo::set_match_enabled(&state.pool, id, enabled).await?;
    }

    if let Some(attributes) = req.attributes {
        current = EmitterRepo::set_attributes(&state.pool, id, &attributes).await?;
    }

    Ok(Json(EmitterDto::from(&current)))
}

async fn delete_emitter(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let deleted = EmitterRepo::delete(&state.pool, id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// `POST /api/emitters/bulk-delete` request body — see module docs.
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

async fn bulk_delete_emitters(
    State(state): State<AppState>,
    Json(req): Json<BulkDeleteRequest>,
) -> Result<Json<DeletedCountDto>, ApiError> {
    let deleted = EmitterRepo::delete_bulk(&state.pool, &req.ids).await?;
    Ok(Json(DeletedCountDto { deleted }))
}

async fn clear_emitters(State(state): State<AppState>) -> Result<Json<DeletedCountDto>, ApiError> {
    let deleted = EmitterRepo::delete_all(&state.pool).await?;
    Ok(Json(DeletedCountDto { deleted }))
}

/// `GET /api/emitters/types` (Task 4): the distinct `emitter_type` values
/// that actually have at least one emitter, each with its human-readable
/// label, sorted by label — the Emitters page's Type-filter dropdown's
/// stable backend source, replacing the previous "derive options from
/// whatever rows happen to be loaded" approach.
async fn list_emitter_types_in_use(
    State(state): State<AppState>,
) -> Result<Json<Vec<InUseEmitterTypeDto>>, ApiError> {
    let mut keys = EmitterRepo::distinct_types_in_use(&state.pool).await?;
    let mut types: Vec<InUseEmitterTypeDto> = keys
        .drain(..)
        .map(|key| {
            let label = fluxfang_core::emitter_type_label(&key).to_string();
            InUseEmitterTypeDto { key, label }
        })
        .collect();
    types.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(Json(types))
}

#[derive(Debug, Deserialize)]
struct SetRuleRequest {
    match_criteria: serde_json::Value,
}

async fn set_rule(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SetRuleRequest>,
) -> Result<Json<EmitterAndCount>, ApiError> {
    let existing = EmitterRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let kind = catalog_kind_for(existing.emitter_type.as_deref());

    let rule = parse_rule(&req.match_criteria)?;
    validate_rule(&rule, kind).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    EmitterRepo::update_rule(&state.pool, id, &req.match_criteria).await?;
    let attached_count =
        EmitterRepo::attach_emissions_matching(&state.pool, id, &rule, kind).await?;

    let emitter = EmitterRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(EmitterAndCount {
        emitter: EmitterDto::from(&emitter),
        attached_count,
    }))
}

/// `POST /api/emitters/:id/associations` request body — see module docs.
#[derive(Debug, Deserialize)]
pub struct AddAssociationRequest {
    pub associated_emitter_id: Uuid,
}

/// GET /api/emitters/:id/associations (Spec B, Task 3): the emitters
/// currently associated with `id` ("Other Tires on the same Car"), plus
/// each link's `source`/`confidence`.
async fn list_associations(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<crate::dto::EmitterAssociationDto>>, ApiError> {
    // 404 if the emitter itself doesn't exist.
    EmitterRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let assocs = EmitterAssociationRepo::list_for(&state.pool, id).await?;
    Ok(Json(assocs.iter().map(Into::into).collect()))
}

/// POST /api/emitters/:id/associations { associated_emitter_id } (Spec B,
/// Task 3): manually link two `tpms_sensor` emitters ("same vehicle").
/// Rejects self-association and any pairing where either side isn't a
/// `tpms_sensor` emitter with `400`. Returns the resulting association list
/// for `id` (source `"manual"`).
async fn add_association(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddAssociationRequest>,
) -> Result<Json<Vec<crate::dto::EmitterAssociationDto>>, ApiError> {
    let other = req.associated_emitter_id;
    if other == id {
        return Err(ApiError::BadRequest(
            "an emitter cannot be associated with itself".to_string(),
        ));
    }
    let this = EmitterRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let that = EmitterRepo::get(&state.pool, other)
        .await?
        .ok_or_else(|| ApiError::BadRequest("associated emitter not found".to_string()))?;
    for e in [&this, &that] {
        if e.emitter_type.as_deref() != Some("tpms_sensor") {
            return Err(ApiError::BadRequest(
                "associations are only supported between TPMS Sensor emitters".to_string(),
            ));
        }
    }
    EmitterAssociationRepo::add(&state.pool, id, other, "manual", None).await?;
    let assocs = EmitterAssociationRepo::list_for(&state.pool, id).await?;
    Ok(Json(assocs.iter().map(Into::into).collect()))
}

/// DELETE /api/emitters/:id/associations/:other_id (Spec B, Task 3): remove
/// a (bidirectional) association.
async fn remove_association(
    State(state): State<AppState>,
    Path((id, other_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    EmitterAssociationRepo::remove(&state.pool, id, other_id).await?;
    Ok(StatusCode::OK)
}

#[derive(Debug, Deserialize)]
struct CreateWithEntityEmitter {
    name: String,
    #[serde(rename = "type", default)]
    type_: Option<String>,
    #[serde(default)]
    match_criteria: Option<serde_json::Value>,
    /// Same optional machine emitter-type key as `POST /api/emitters`' own
    /// `emitter_type` field — see [`CreateEmitterRequest`]'s doc comment.
    #[serde(default)]
    emitter_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateWithEntityEntity {
    name: String,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateWithEntityRequest {
    emitter: CreateWithEntityEmitter,
    entity: CreateWithEntityEntity,
}

#[derive(Debug, Serialize)]
struct EmitterEntityAndCount {
    emitter: EmitterDto,
    entity: EntityDto,
    attached_count: u64,
}

async fn create_with_entity(
    State(state): State<AppState>,
    Json(req): Json<CreateWithEntityRequest>,
) -> Result<(StatusCode, Json<EmitterEntityAndCount>), ApiError> {
    validate_emitter_type(&req.emitter.emitter_type)?;
    let kind = catalog_kind_for(req.emitter.emitter_type.as_deref());

    let (match_criteria, rule) = match req.emitter.match_criteria {
        Some(raw) => {
            let rule = parse_rule(&raw)?;
            validate_rule(&rule, kind).map_err(|e| ApiError::BadRequest(e.to_string()))?;
            (raw, Some(rule))
        }
        None => (serde_json::json!({}), None),
    };

    let result = EmitterRepo::create_with_entity(
        &state.pool,
        NewEntity {
            name: req.entity.name,
            notes: req.entity.notes,
            ..Default::default()
        },
        req.emitter.name,
        req.emitter.type_,
        req.emitter.emitter_type,
        match_criteria,
        rule.as_ref(),
        kind,
    )
    .await?;

    let EmitterWithEntity {
        emitter,
        entity,
        attached_count,
    } = result;

    Ok((
        StatusCode::CREATED,
        Json(EmitterEntityAndCount {
            emitter: EmitterDto::from(&emitter),
            entity: EntityDto::from(&entity),
            attached_count,
        }),
    ))
}

#[derive(Debug, Deserialize)]
struct PreviewQuery {
    rule: String,
    /// Data-source kind to validate/preview `rule` against (e.g. `"wifi"` or
    /// `"bluetooth"`). Defaults to `"wifi"` when omitted, preserving prior
    /// behavior for existing callers that never sent it.
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Debug, Serialize)]
struct MatchCountDto {
    match_count: i64,
}

async fn preview_emitters(
    State(state): State<AppState>,
    Query(q): Query<PreviewQuery>,
) -> Result<Json<MatchCountDto>, ApiError> {
    let kind = q.kind.as_deref().unwrap_or("wifi");
    let rule: Rule = serde_json::from_str(&q.rule)
        .map_err(|e| ApiError::BadRequest(format!("invalid rule: {e}")))?;
    validate_rule(&rule, kind).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let match_count = EmitterRepo::count_matching(&state.pool, &rule, kind).await?;
    Ok(Json(MatchCountDto { match_count }))
}

/// Small internal error type, same convention as `emissions::ApiError`/
/// `data_sources::ApiError`: DB failures map to `500`; deliberate
/// rejections (including `EmitterRuleError::Rule` -- see module docs) carry
/// their own status.
enum ApiError {
    BadRequest(String),
    NotFound,
    Internal,
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in emitters route: {err}");
        ApiError::Internal
    }
}

impl From<EmitterRuleError> for ApiError {
    fn from(err: EmitterRuleError) -> Self {
        match err {
            EmitterRuleError::Rule(e) => ApiError::BadRequest(e.to_string()),
            EmitterRuleError::Sql(e) => {
                eprintln!("fluxfang-api: db error in emitters route: {e}");
                ApiError::Internal
            }
        }
    }
}

/// Task 3: `EmitterRepo::query`'s error, mirroring the `EmissionQueryError`
/// mapping — a rejected attribute condition (unknown field, invalid op, or a
/// mistyped value: all caller mistakes) is a `400`, an actual DB failure a
/// `500`.
impl From<EmitterQueryError> for ApiError {
    fn from(err: EmitterQueryError) -> Self {
        match err {
            EmitterQueryError::Rule(e) => ApiError::BadRequest(e.to_string()),
            EmitterQueryError::Sql(e) => {
                eprintln!("fluxfang-api: db error in emitters route: {e}");
                ApiError::Internal
            }
        }
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
