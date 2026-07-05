//! `GET/POST/PATCH/DELETE /api/alert-rules[/:id]` (Task 6.6). PROTECTED —
//! mounted in `lib.rs::app`'s protected router group, behind `require_auth`,
//! same as every other non-setup/login route.
//!
//! ## Trigger validation matrix
//!
//! [`validate_trigger`] is the one place `trigger.on ∈ {"detected",
//! "enters_zone", "leaves_zone", "host_enters_zone", "host_leaves_zone"}`
//! (see `fluxfang_db::models::AlertRule`'s own doc comment for the full
//! enumeration) is cross-checked against the rule's `target_type`/
//! `target_id` and `trigger.zone_id`, matching exactly what
//! `ingest::alerts::evaluate_alerts`/`ingest::zones::update_subject_zones`
//! (Tasks 5.3/5.4) actually interpret at evaluation time — a rule that
//! passes this check can never silently evaluate to "never fires" because
//! of a target/trigger mismatch those modules would have skipped anyway:
//!
//! | `trigger.on`         | requires `zone_id`? | target requirement                     |
//! |-----------------------|:--------------------:|-----------------------------------------|
//! | `detected`             | no (ignored if given) | `target_type` ∈ {emitter, entity} + `target_id` |
//! | `enters_zone`          | yes                   | `target_type` ∈ {emitter, entity} + `target_id` |
//! | `leaves_zone`          | yes                   | `target_type` ∈ {emitter, entity} + `target_id` |
//! | `host_enters_zone`     | yes                   | none — `target_type`/`target_id` must both be null |
//! | `host_leaves_zone`     | yes                   | none — `target_type`/`target_id` must both be null |
//!
//! `trigger.content_match`, if present, must be a well-formed
//! [`fluxfang_core::Rule`] whose conditions type-check against the `"wifi"`
//! catalog (this schema's only supported data-source kind — same hardcoded
//! choice `emitters.rs`'s own `validate_rule` makes, via
//! `fluxfang_core::conditions_to_sql_checked`) — this is a **pure, no-I/O**
//! check, so an invalid rule is rejected with `400` before anything is
//! written, mirroring `emitters.rs`'s "validate before any mutation"
//! convention.
//!
//! ## `method_ids`: validated to exist, then set atomically
//!
//! Every `method_ids` entry (on create, and on update when the caller
//! resubmits the field) is checked against `AlertMethodRepo::get` before any
//! write happens — an unknown id is a `400`, not a `500` from a foreign-key
//! violation surfacing out of `alert_rule_method`. Once validated, the full
//! set is applied via `AlertRuleRepo::set_methods` (replace-in-a-transaction
//! — see that method's own doc comment).
//!
//! ## Response shape: `method_ids` costs one extra query per rule
//!
//! `GET /api/alert-rules` calls `AlertRuleRepo::methods_for_rule` once per
//! listed rule (not a single join across all rules) to build each row's
//! `method_ids`. Same cost trade-off this codebase already accepts
//! elsewhere at this slice's scale (e.g. `ingest::alerts::evaluate_alerts`
//! loading every `alert_rule` per emission) — documented, not optimized,
//! since a single-admin homelab deployment isn't expected to have enough
//! rules for N+1 queries here to matter.
//!
//! ## Error mapping
//!
//! Same convention as `emitters.rs`/`entities.rs`: a malformed body, an
//! invalid trigger, or an unknown `method_ids` entry is `400`; a missing
//! path-`:id` resource is `404`; any other `sqlx::Error` is `500`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_core::rule::Rule;
use fluxfang_core::{catalog_for, conditions_to_sql_checked};
use fluxfang_db::models::{AlertRule, NewAlertRule};
use fluxfang_db::{AlertMethodRepo, AlertRuleRepo};

use crate::state::AppState;

/// `trigger.on` values this schema understands — see module docs' matrix.
const VALID_TRIGGER_ON: &[&str] = &[
    "detected",
    "enters_zone",
    "leaves_zone",
    "host_enters_zone",
    "host_leaves_zone",
];

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/alert-rules",
            get(list_alert_rules).post(create_alert_rule),
        )
        .route(
            "/api/alert-rules/:id",
            axum::routing::patch(update_alert_rule).delete(delete_alert_rule),
        )
}

/// `GET /api/alert-rules`/`POST /api/alert-rules`/`PATCH
/// /api/alert-rules/:id`'s response shape.
#[derive(Debug, Clone, Serialize)]
struct AlertRuleDto {
    id: Uuid,
    name: String,
    enabled: bool,
    target_type: Option<String>,
    target_id: Option<Uuid>,
    trigger: serde_json::Value,
    method_ids: Vec<Uuid>,
    created_at: DateTime<Utc>,
}

impl AlertRuleDto {
    fn new(rule: &AlertRule, method_ids: Vec<Uuid>) -> Self {
        AlertRuleDto {
            id: rule.id,
            name: rule.name.clone(),
            enabled: rule.enabled,
            target_type: rule.target_type.clone(),
            target_id: rule.target_id,
            trigger: rule.trigger.clone(),
            method_ids,
            created_at: rule.created_at,
        }
    }
}

/// Validate `trigger`'s shape against `target_type`/`target_id` — see module
/// docs for the full matrix. Pure, no-I/O: never touches the database.
fn validate_trigger(
    trigger: &serde_json::Value,
    target_type: Option<&str>,
    target_id: Option<Uuid>,
) -> Result<(), String> {
    let on = trigger
        .get("on")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "trigger.on is required and must be a string".to_string())?;
    if !VALID_TRIGGER_ON.contains(&on) {
        return Err(format!(
            "trigger.on must be one of {VALID_TRIGGER_ON:?}, got {on:?}"
        ));
    }

    let is_zone_trigger = matches!(
        on,
        "enters_zone" | "leaves_zone" | "host_enters_zone" | "host_leaves_zone"
    );
    if is_zone_trigger {
        match trigger.get("zone_id") {
            Some(v) if !v.is_null() => {
                if serde_json::from_value::<Uuid>(v.clone()).is_err() {
                    return Err("trigger.zone_id must be a valid uuid".to_string());
                }
            }
            _ => return Err(format!("trigger.on = {on:?} requires trigger.zone_id")),
        }
    }

    let is_host_trigger = matches!(on, "host_enters_zone" | "host_leaves_zone");
    if is_host_trigger {
        if target_type.is_some() || target_id.is_some() {
            return Err(format!(
                "trigger.on = {on:?} is a host trigger and must have a null target_type/target_id"
            ));
        }
    } else {
        match target_type {
            Some("emitter") | Some("entity") => {}
            Some(other) => {
                return Err(format!(
                    "target_type must be 'emitter' or 'entity', got {other:?}"
                ))
            }
            None => {
                return Err(format!(
                    "trigger.on = {on:?} requires a target_type ('emitter' or 'entity')"
                ))
            }
        }
        if target_id.is_none() {
            return Err(format!("trigger.on = {on:?} requires a target_id"));
        }
    }

    if let Some(cm) = trigger.get("content_match") {
        if !cm.is_null() {
            let rule: Rule = serde_json::from_value(cm.clone())
                .map_err(|e| format!("invalid trigger.content_match: {e}"))?;
            let catalog = catalog_for("wifi");
            conditions_to_sql_checked(&rule.conditions, rule.match_mode, 1, &catalog)
                .map_err(|e| format!("invalid trigger.content_match: {e}"))?;
        }
    }

    Ok(())
}

/// Check every id in `method_ids` actually refers to an existing
/// `alert_method` — see module docs for why this runs before any
/// insert/`set_methods` call.
async fn validate_method_ids_exist(pool: &PgPool, method_ids: &[Uuid]) -> Result<(), ApiError> {
    for id in method_ids {
        if AlertMethodRepo::get(pool, *id).await?.is_none() {
            return Err(ApiError::BadRequest(format!("unknown method_id: {id}")));
        }
    }
    Ok(())
}

async fn list_alert_rules(
    State(state): State<AppState>,
) -> Result<Json<Vec<AlertRuleDto>>, ApiError> {
    let rules = AlertRuleRepo::list(&state.pool).await?;
    let mut out = Vec::with_capacity(rules.len());
    for rule in &rules {
        let methods = AlertRuleRepo::methods_for_rule(&state.pool, rule.id).await?;
        out.push(AlertRuleDto::new(
            rule,
            methods.iter().map(|m| m.id).collect(),
        ));
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
struct CreateAlertRuleRequest {
    name: String,
    enabled: bool,
    #[serde(default)]
    target_type: Option<String>,
    #[serde(default)]
    target_id: Option<Uuid>,
    trigger: serde_json::Value,
    #[serde(default)]
    method_ids: Vec<Uuid>,
}

async fn create_alert_rule(
    State(state): State<AppState>,
    Json(req): Json<CreateAlertRuleRequest>,
) -> Result<(StatusCode, Json<AlertRuleDto>), ApiError> {
    validate_trigger(&req.trigger, req.target_type.as_deref(), req.target_id)
        .map_err(ApiError::BadRequest)?;
    validate_method_ids_exist(&state.pool, &req.method_ids).await?;

    let rule = AlertRuleRepo::insert(
        &state.pool,
        NewAlertRule {
            name: req.name,
            enabled: req.enabled,
            target_type: req.target_type,
            target_id: req.target_id,
            trigger: req.trigger,
        },
    )
    .await?;

    AlertRuleRepo::set_methods(&state.pool, rule.id, &req.method_ids).await?;

    Ok((
        StatusCode::CREATED,
        Json(AlertRuleDto::new(&rule, req.method_ids)),
    ))
}

/// The standard serde "distinguish absent from explicit null" recipe (same
/// helper `emitters.rs`/`entities.rs` each define for their own `PATCH`
/// handlers): a `#[serde(default, deserialize_with = "some")]` field decodes
/// to `None` when the key is missing from the JSON object, and to
/// `Some(None)` when the key is present with a JSON `null` — letting a
/// `PATCH` body tell "leave `target_type`/`target_id` alone" (key omitted)
/// apart from "clear it" (explicit `null`, e.g. converting a targeted rule
/// into a host rule).
fn some<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
struct UpdateAlertRuleRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default, deserialize_with = "some")]
    target_type: Option<Option<String>>,
    #[serde(default, deserialize_with = "some")]
    target_id: Option<Option<Uuid>>,
    #[serde(default)]
    trigger: Option<serde_json::Value>,
    #[serde(default)]
    method_ids: Option<Vec<Uuid>>,
}

async fn update_alert_rule(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateAlertRuleRequest>,
) -> Result<Json<AlertRuleDto>, ApiError> {
    let existing = AlertRuleRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let name = req.name.unwrap_or_else(|| existing.name.clone());
    let enabled = req.enabled.unwrap_or(existing.enabled);
    let target_type = match req.target_type {
        Some(inner) => inner,
        None => existing.target_type.clone(),
    };
    let target_id = match req.target_id {
        Some(inner) => inner,
        None => existing.target_id,
    };
    let trigger = req.trigger.unwrap_or_else(|| existing.trigger.clone());

    validate_trigger(&trigger, target_type.as_deref(), target_id).map_err(ApiError::BadRequest)?;

    if let Some(method_ids) = &req.method_ids {
        validate_method_ids_exist(&state.pool, method_ids).await?;
    }

    let updated = AlertRuleRepo::update(
        &state.pool,
        id,
        &name,
        enabled,
        target_type.as_deref(),
        target_id,
        trigger,
    )
    .await?;

    let method_ids = if let Some(method_ids) = req.method_ids {
        AlertRuleRepo::set_methods(&state.pool, id, &method_ids).await?;
        method_ids
    } else {
        AlertRuleRepo::methods_for_rule(&state.pool, id)
            .await?
            .iter()
            .map(|m| m.id)
            .collect()
    };

    Ok(Json(AlertRuleDto::new(&updated, method_ids)))
}

async fn delete_alert_rule(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let deleted = AlertRuleRepo::delete(&state.pool, id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Small internal error type, same convention as `emitters::ApiError`/
/// `entities::ApiError`: DB failures map to `500`; deliberate rejections
/// (invalid trigger, unknown `method_ids` entry) are `400`; a missing `:id`
/// is `404`.
enum ApiError {
    BadRequest(String),
    NotFound,
    Internal,
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in alert_rules route: {err}");
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
