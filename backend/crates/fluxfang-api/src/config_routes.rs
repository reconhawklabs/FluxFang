//! `GET /api/config` — the current node's role + id, so the frontend can
//! gate nav and routes. Secret-free: it never returns the sensor `key` or
//! any other credential.
//!
//! `PATCH /api/config` — partial-merge update of the node-role config.
//! Role changes and sensor-connection edits (Settings-phase) land here: the
//! request body is merged onto the currently stored `NodeConfig`, so an
//! omitted `sensor.key` keeps the previously stored key rather than wiping
//! it out. The merged result is validated (slug shape, and a full sensor
//! block when `role == Sensor`) before it's persisted, and the response —
//! like `GET` — never echoes the key back.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use fluxfang_db::node_config::{NodeRole, SensorConfig};
use fluxfang_db::AppConfigRepo;

use crate::state::AppState;

pub fn protected_routes() -> Router<AppState> {
    Router::new().route("/api/config", get(get_config).patch(patch_config))
}

async fn get_config(State(state): State<AppState>) -> Result<Json<serde_json::Value>, StatusCode> {
    match AppConfigRepo::node_config(&state.pool).await {
        Ok(Some(node)) => {
            let sensor = node.sensor.as_ref().map(|s| {
                json!({
                    "host": s.host,
                    "port": s.port,
                    "cache_ttl_secs": s.cache_ttl_secs,
                })
            });
            Ok(Json(json!({
                "role": node.role,
                "node_sensor_id": node.node_sensor_id,
                "sensor": sensor,
            })))
        }
        // Behind require_auth this "setup not completed" case shouldn't be
        // reachable, but answer defensively rather than 500.
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            eprintln!("fluxfang-api: db error in GET /api/config: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Partial patch of the sensor-connection block — every field optional so a
/// caller can update just, say, the host without having to resend (and thus
/// risk clobbering) the stored key.
#[derive(Deserialize)]
struct SensorPatch {
    host: Option<String>,
    port: Option<u16>,
    key: Option<String>,
    cache_ttl_secs: Option<i64>,
}

/// Partial patch of the node config — every field optional, merged onto the
/// currently stored `NodeConfig` rather than replacing it wholesale.
#[derive(Deserialize)]
struct ConfigPatch {
    node_sensor_id: Option<String>,
    role: Option<NodeRole>,
    sensor: Option<SensorPatch>,
}

/// A slug fit for use as a node/sensor identifier: non-empty, bounded, and
/// restricted to characters that are safe to embed elsewhere (URLs, log
/// lines) without escaping.
fn is_valid_sensor_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

async fn patch_config(
    State(state): State<AppState>,
    Json(patch): Json<ConfigPatch>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut node = match AppConfigRepo::node_config(&state.pool).await {
        Ok(Some(n)) => n,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(e) => {
            eprintln!("fluxfang-api: db error loading config in PATCH /api/config: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if let Some(id) = patch.node_sensor_id {
        node.node_sensor_id = id;
    }
    if let Some(role) = patch.role {
        node.role = role;
    }
    if let Some(sp) = patch.sensor {
        // Merge onto the existing sensor block (or start a fresh one if
        // switching to sensor for the first time).
        let mut cur = node.sensor.take().unwrap_or(SensorConfig {
            host: String::new(),
            port: 0,
            key: String::new(),
            cache_ttl_secs: 0,
        });
        if let Some(h) = sp.host {
            cur.host = h;
        }
        if let Some(p) = sp.port {
            cur.port = p;
        }
        if let Some(k) = sp.key {
            cur.key = k; // omitted key keeps the stored one
        }
        if let Some(t) = sp.cache_ttl_secs {
            cur.cache_ttl_secs = t;
        }
        node.sensor = Some(cur);
    }

    // Validate the merged result — not just the patch — so a PATCH can
    // never leave the stored config in an invalid state.
    if !is_valid_sensor_id(&node.node_sensor_id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if node.role == NodeRole::Sensor {
        match &node.sensor {
            Some(s)
                if !s.host.is_empty()
                    && s.port >= 1
                    && !s.key.is_empty()
                    && s.cache_ttl_secs > 0 => {}
            _ => return Err(StatusCode::BAD_REQUEST),
        }
    } else {
        node.sensor = None; // standalone carries no sensor block
    }

    AppConfigRepo::set_node_config(&state.pool, &node)
        .await
        .map_err(|e| {
            eprintln!("fluxfang-api: db error persisting config in PATCH /api/config: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let sensor = node.sensor.as_ref().map(|s| {
        json!({
            "host": s.host,
            "port": s.port,
            "cache_ttl_secs": s.cache_ttl_secs,
        })
    });
    Ok(Json(json!({
        "role": node.role,
        "node_sensor_id": node.node_sensor_id,
        "sensor": sensor,
    })))
}
