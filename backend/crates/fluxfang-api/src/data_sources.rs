//! `GET/POST/PATCH/DELETE /api/data-sources[/:id]` + `POST
//! /api/data-sources/:id/{start,stop}` (Task 6.2). PROTECTED — mounted in
//! `lib.rs::app`'s protected router group, behind `require_auth`, same as
//! every other non-setup/login route.
//!
//! ## Response shape
//!
//! Every handler here returns `fluxfang_db::models::DataSource` directly
//! (it already `#[derive(Serialize)]`s) rather than a bespoke DTO — unlike
//! `catalog_routes`'s `FieldDefDto`, there's no reserved-word/enum-shape
//! trap to route around here, so a dedicated DTO would just be a
//! pointless copy of the same fields.
//!
//! ## Validation
//!
//! `create`/`update` run [`crate::capture::validate_data_source`] before
//! touching the database, rejecting a bad `kind`/`mode`/`interface`/`config`
//! combination with `400` and a human-readable message — see that
//! function's doc comment for the exact rules. The DB's own `CHECK
//! (kind, mode)` constraint (`migrations/0001_init.sql`) is a second,
//! redundant backstop that should never actually fire given this
//! pre-validation, but isn't relied upon to produce a friendly error if it
//! somehow did (an unexpected `sqlx::Error` here maps to `500`, same as
//! every other DB-error path in this module).
//!
//! ## Deleting a running source
//!
//! Chosen: **stop it first**, then delete (not a `409`). Rationale: from an
//! operator's point of view "delete this data source" is an unambiguous,
//! final instruction — refusing it with a `409` just to make them press
//! "stop" first and then "delete" again is friction with no real safety
//! payoff (nothing else references a `data_source` row in a way that a
//! still-running capturer would corrupt if the row disappeared: emissions
//! keep their `data_source_id` via `ON DELETE SET NULL`). If the stop
//! itself fails, the delete is still attempted — a stuck capturer must not
//! be able to prevent the operator from removing its configuration.
//!
//! ## `start`/`stop` and the underlying `CaptureSupervisor`
//!
//! Both endpoints call the supervisor and then re-read the row, returning
//! its current state regardless of whether the supervisor call succeeded:
//! `CaptureSupervisor::start`/`stop` already persist `status`/`last_error`
//! on every outcome (see its own doc comments), so "did the HTTP call
//! succeed" and "is the data source now running" are deliberately
//! decoupled -- the response body's `status`/`last_error` fields are the
//! single source of truth for whether capture is actually happening, not
//! the HTTP status code.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use fluxfang_db::models::{DataSource, NewDataSource};
use fluxfang_db::DataSourceRepo;

use crate::capture::validate_data_source;
use crate::state::AppState;

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/data-sources",
            get(list_data_sources).post(create_data_source),
        )
        .route(
            "/api/data-sources/:id",
            get(get_data_source)
                .patch(update_data_source)
                .delete(delete_data_source),
        )
        .route("/api/data-sources/:id/start", post(start_data_source))
        .route("/api/data-sources/:id/stop", post(stop_data_source))
        .route("/api/data-sources/:id/allow-sensors", post(allow_sensors))
}

#[derive(Debug, Deserialize)]
struct CreateDataSourceRequest {
    kind: String,
    mode: String,
    #[serde(default)]
    interface: Option<String>,
    #[serde(default = "default_config")]
    config: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct UpdateDataSourceRequest {
    #[serde(default = "default_config")]
    config: serde_json::Value,
    mode: String,
    #[serde(default)]
    interface: Option<String>,
}

fn default_config() -> serde_json::Value {
    serde_json::json!({})
}

async fn list_data_sources(
    State(state): State<AppState>,
) -> Result<Json<Vec<DataSource>>, ApiError> {
    Ok(Json(DataSourceRepo::list(&state.pool).await?))
}

async fn create_data_source(
    State(state): State<AppState>,
    Json(req): Json<CreateDataSourceRequest>,
) -> Result<(StatusCode, Json<DataSource>), ApiError> {
    validate_data_source(&req.kind, &req.mode, req.interface.as_deref(), &req.config)
        .map_err(ApiError::BadRequest)?;

    let new = NewDataSource {
        kind: req.kind,
        mode: req.mode,
        interface: req.interface,
        config: req.config,
    };
    let created = DataSourceRepo::insert(&state.pool, new).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn get_data_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DataSource>, ApiError> {
    let source = DataSourceRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(source))
}

async fn update_data_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateDataSourceRequest>,
) -> Result<Json<DataSource>, ApiError> {
    let existing = DataSourceRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // A running source's capturer is already serving the *old* config (e.g.
    // a manual-GPS host location) -- mutating the row underneath it would
    // leave the capturer silently stale until stopped/restarted, which is
    // exactly the wrong failure mode for a counter-surveillance tool. Editing
    // is only allowed while stopped; stop it first, then edit.
    if existing.status == "running" {
        return Err(ApiError::BadRequest(
            "cannot edit a running data source; stop it first".to_string(),
        ));
    }

    // `kind` is immutable (see `DataSourceRepo::update`'s own doc comment),
    // so validation is checked against the *existing* row's kind alongside
    // the proposed mode/interface/config.
    validate_data_source(
        &existing.kind,
        &req.mode,
        req.interface.as_deref(),
        &req.config,
    )
    .map_err(ApiError::BadRequest)?;

    let updated = DataSourceRepo::update(
        &state.pool,
        id,
        req.config,
        &req.mode,
        req.interface.as_deref(),
    )
    .await?;
    Ok(Json(updated))
}

async fn delete_data_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let existing = DataSourceRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    if existing.status == "running" {
        // Best-effort: see module docs on why delete proceeds regardless.
        // `sensor` datasources are network listeners driven by
        // `sensor_listeners`, not `CaptureSupervisor` -- same branch as
        // start_data_source/stop_data_source above, otherwise `capture.stop`
        // finds no in-memory handle for a running sensor, phantom-reconciles
        // the row to `stopped`, and never signals the real listener task,
        // leaking its bound `TcpListener`.
        if existing.kind == "sensor" {
            state.sensor_listeners.stop(id).await;
        } else if let Err(err) = state.capture.stop(id).await {
            eprintln!("data_sources: failed to stop {id} before delete: {err:#}");
        }
    }

    let deleted = DataSourceRepo::delete(&state.pool, id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

async fn start_data_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DataSource>, ApiError> {
    let Some(source) = DataSourceRepo::get(&state.pool, id).await? else {
        return Err(ApiError::NotFound);
    };
    // Record the user's intent before touching the supervisor -- see Task 7:
    // `desired_state` is what `CaptureSupervisor::resume_running`/
    // `SensorListenerManager::resume_running` key off after a restart, so it
    // must be persisted regardless of whether the start attempt below
    // actually succeeds.
    DataSourceRepo::set_desired_state(&state.pool, id, "running").await?;
    // A `sensor` datasource is a network listener, not a capture device --
    // see `crate::sensor_listener` module docs -- so it's driven by
    // `sensor_listeners` instead of `CaptureSupervisor`. Either way, errors
    // are reflected in the row's own status/last_error, not propagated as an
    // HTTP error.
    if source.kind == "sensor" {
        state.sensor_listeners.start(id).await;
    } else {
        let _ = state.capture.start(id).await;
    }
    let current = DataSourceRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(current))
}

async fn stop_data_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DataSource>, ApiError> {
    let Some(source) = DataSourceRepo::get(&state.pool, id).await? else {
        return Err(ApiError::NotFound);
    };
    // Same reasoning as start_data_source: record intent first, then branch
    // on kind.
    DataSourceRepo::set_desired_state(&state.pool, id, "stopped").await?;
    if source.kind == "sensor" {
        state.sensor_listeners.stop(id).await;
    } else {
        let _ = state.capture.stop(id).await;
    }
    let current = DataSourceRepo::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(current))
}

async fn allow_sensors(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(source) = DataSourceRepo::get(&state.pool, id).await? else {
        return Err(ApiError::NotFound);
    };
    if source.kind != "sensor" {
        return Err(ApiError::BadRequest("not a sensor datasource".to_string()));
    }
    let remaining = state.sensor_listeners.open_enrollment_window(id).await.unwrap_or(0);
    Ok(Json(serde_json::json!({ "remaining_secs": remaining })))
}

/// Small internal error type, same convention as `auth_routes::ApiError`:
/// DB failures map to `500`; deliberate rejections carry their own status.
enum ApiError {
    BadRequest(String),
    NotFound,
    Internal,
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in data_sources route: {err}");
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
