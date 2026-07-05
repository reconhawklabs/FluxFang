//! `GET/POST/PATCH/DELETE /api/emitters[/:id]`, `POST
//! /api/emitters/:id/rule`, `POST /api/emitters/with-entity`, and `GET
//! /api/emitters/preview` (Task 6.4). PROTECTED — mounted in
//! `lib.rs::app`'s protected router group, behind `require_auth`, same as
//! every other non-setup/login route.
//!
//! ## Response shape
//!
//! Every handler returns [`crate::dto::EmitterDto`] (see its doc comment)
//! rather than `fluxfang_db::models::Emitter` directly.
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

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use fluxfang_core::rule::{Condition, MatchMode, Op};
use fluxfang_core::{catalog_for, conditions_to_sql_checked, Rule, RuleSqlError};
use fluxfang_db::models::{Entity, NewEmitter, NewEntity};
use fluxfang_db::repo::emitter::{EmitterRuleError, EmitterWithEntity};
use fluxfang_db::{EmissionRepo, EmitterRepo};

use crate::dto::EmitterDto;
use crate::state::AppState;

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/api/emitters", get(list_emitters).post(create_emitter))
        .route("/api/emitters/preview", get(preview_emitters))
        .route("/api/emitters/with-entity", post(create_with_entity))
        .route(
            "/api/emitters/:id",
            get(get_emitter)
                .patch(update_emitter)
                .delete(delete_emitter),
        )
        .route("/api/emitters/:id/rule", post(set_rule))
}

/// Validate `rule.conditions` against the `"wifi"` catalog (this schema's
/// only supported kind) with no DB access at all — see module docs for why
/// this runs before any mutation rather than relying on the backfill call
/// itself to reject a bad rule.
fn validate_rule(rule: &Rule) -> Result<(), RuleSqlError> {
    let catalog = catalog_for("wifi");
    conditions_to_sql_checked(&rule.conditions, rule.match_mode, 1, &catalog).map(|_| ())
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

async fn list_emitters(State(state): State<AppState>) -> Result<Json<Vec<EmitterDto>>, ApiError> {
    let rows = EmitterRepo::list(&state.pool).await?;
    Ok(Json(rows.iter().map(EmitterDto::from).collect()))
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

    let (match_criteria, rule) =
        resolve_match_criteria(&state, req.match_criteria, req.from_emission_id).await?;

    let new = NewEmitter {
        name: req.name,
        type_: req.type_,
        entity_id: req.entity_id,
        match_criteria,
    };
    let created = EmitterRepo::insert(&state.pool, new).await?;

    let attached_count = if let Some(rule) = &rule {
        EmitterRepo::attach_emissions_matching(&state.pool, created.id, rule).await?
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
) -> Result<(serde_json::Value, Option<Rule>), ApiError> {
    if let Some(raw) = match_criteria {
        let rule = parse_rule(&raw)?;
        validate_rule(&rule).map_err(|e| ApiError::BadRequest(e.to_string()))?;
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
        validate_rule(&rule).map_err(|e| ApiError::BadRequest(e.to_string()))?;
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

#[derive(Debug, Deserialize)]
struct UpdateEmitterRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "type", default, deserialize_with = "some")]
    type_: Option<Option<String>>,
    #[serde(default, deserialize_with = "some")]
    entity_id: Option<Option<Uuid>>,
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

#[derive(Debug, Deserialize)]
struct SetRuleRequest {
    match_criteria: serde_json::Value,
}

async fn set_rule(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SetRuleRequest>,
) -> Result<Json<EmitterAndCount>, ApiError> {
    if EmitterRepo::get(&state.pool, id).await?.is_none() {
        return Err(ApiError::NotFound);
    }

    let rule = parse_rule(&req.match_criteria)?;
    validate_rule(&rule).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    EmitterRepo::update_rule(&state.pool, id, &req.match_criteria).await?;
    let attached_count = EmitterRepo::attach_emissions_matching(&state.pool, id, &rule).await?;

    let emitter = EmitterRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(EmitterAndCount {
        emitter: EmitterDto::from(&emitter),
        attached_count,
    }))
}

#[derive(Debug, Deserialize)]
struct CreateWithEntityEmitter {
    name: String,
    #[serde(rename = "type", default)]
    type_: Option<String>,
    #[serde(default)]
    match_criteria: Option<serde_json::Value>,
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
    entity: Entity,
    attached_count: u64,
}

async fn create_with_entity(
    State(state): State<AppState>,
    Json(req): Json<CreateWithEntityRequest>,
) -> Result<(StatusCode, Json<EmitterEntityAndCount>), ApiError> {
    let (match_criteria, rule) = match req.emitter.match_criteria {
        Some(raw) => {
            let rule = parse_rule(&raw)?;
            validate_rule(&rule).map_err(|e| ApiError::BadRequest(e.to_string()))?;
            (raw, Some(rule))
        }
        None => (serde_json::json!({}), None),
    };

    let result = EmitterRepo::create_with_entity(
        &state.pool,
        NewEntity {
            name: req.entity.name,
            notes: req.entity.notes,
        },
        req.emitter.name,
        req.emitter.type_,
        match_criteria,
        rule.as_ref(),
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
            entity,
            attached_count,
        }),
    ))
}

#[derive(Debug, Deserialize)]
struct PreviewQuery {
    rule: String,
}

#[derive(Debug, Serialize)]
struct MatchCountDto {
    match_count: i64,
}

async fn preview_emitters(
    State(state): State<AppState>,
    Query(q): Query<PreviewQuery>,
) -> Result<Json<MatchCountDto>, ApiError> {
    let rule: Rule = serde_json::from_str(&q.rule)
        .map_err(|e| ApiError::BadRequest(format!("invalid rule: {e}")))?;
    validate_rule(&rule).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let match_count = EmitterRepo::count_matching(&state.pool, &rule).await?;
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

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            ApiError::NotFound => StatusCode::NOT_FOUND.into_response(),
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}
