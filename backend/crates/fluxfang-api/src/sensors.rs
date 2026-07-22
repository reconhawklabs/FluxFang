//! Operator endpoints for the sensor fleet (session-authed, on :8080). The
//! per-sensor `key` is never returned by reads; `rotate` returns a freshly
//! generated key exactly once for re-provisioning.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use fluxfang_db::SensorRepo;

use crate::state::AppState;

/// A sensor is "online" if it contacted the listener within this window.
const ONLINE_THRESHOLD_SECS: i64 = 60;

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/api/sensors", get(list_sensors))
        .route("/api/sensors/:id/approve", post(approve_sensor))
        .route("/api/sensors/:id/reject", post(reject_sensor))
        .route("/api/sensors/:id/revoke", post(revoke_sensor))
        .route("/api/sensors/:id/rotate", post(rotate_sensor))
}

fn sensor_json(s: &fluxfang_db::models::Sensor, now: chrono::DateTime<chrono::Utc>, emissions_24h: i64) -> Value {
    let online = s
        .last_seen_at
        .is_some_and(|t| (now - t).num_seconds() <= ONLINE_THRESHOLD_SECS);
    json!({
        "id": s.id,
        "data_source_id": s.data_source_id,
        "sensor_id": s.sensor_id,
        "fingerprint": s.fingerprint,
        "status": s.status,
        "auto_group_emitters": s.auto_group_emitters,
        "source_ip": s.source_ip,
        "approved_at": s.approved_at,
        "last_seen_at": s.last_seen_at,
        "online": online,
        "emissions_24h": emissions_24h,
    })
}

async fn list_sensors(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let sensors = SensorRepo::list(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let now = chrono::Utc::now();
    let since = now - chrono::Duration::hours(24);
    let counts: std::collections::HashMap<String, i64> =
        fluxfang_db::EmissionRepo::counts_by_sensor_since(&state.pool, since)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .into_iter()
            .collect();
    Ok(Json(Value::Array(
        sensors
            .iter()
            .map(|s| sensor_json(s, now, counts.get(&s.sensor_id).copied().unwrap_or(0)))
            .collect(),
    )))
}

#[derive(Deserialize)]
struct ApproveBody {
    auto_group_emitters: bool,
    /// The sensor's symmetric key, typed in by the operator out-of-band (read
    /// off the sensor's own UI). It is NEVER transmitted by the sensor — the
    /// sensor only sent its one-way fingerprint at enrollment. We verify the
    /// typed key reproduces that fingerprint before trusting it.
    key: String,
}

async fn approve_sensor(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ApproveBody>,
) -> Result<Json<Value>, StatusCode> {
    let Some(sensor) = SensorRepo::get(&state.pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    else {
        return Err(StatusCode::NOT_FOUND);
    };

    // Only a `pending` sensor may be approved. Without this guard, approve
    // would happily run on a `revoked`/`rejected` row (whose key+fingerprint
    // are still on file), silently resurrecting it — contradicting the enroll
    // handler's "do not resurrect revoked/rejected" policy. 409 = wrong state.
    if sensor.status != "pending" {
        return Err(StatusCode::CONFLICT);
    }

    // The operator-supplied key must decode to a valid 32-byte key AND
    // reproduce the fingerprint the sensor claimed at enrollment. This is what
    // lets the key stay off the wire: a matching fingerprint proves the
    // operator entered the sensor's real key (not a typo, not an impostor's).
    let key = fluxfang_sensor_proto::decode_key(&body.key).map_err(|_| StatusCode::BAD_REQUEST)?;
    let computed = fluxfang_sensor_proto::fingerprint(&sensor.sensor_id, &key);
    if computed != sensor.fingerprint {
        // Wrong key: does not match this sensor's fingerprint. 400 so the UI
        // can tell the operator to re-check the key.
        return Err(StatusCode::BAD_REQUEST);
    }

    SensorRepo::set_key(&state.pool, id, &body.key, &computed)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    SensorRepo::set_auto_group(&state.pool, id, body.auto_group_emitters)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let s = SensorRepo::set_status(&state.pool, id, "approved", true)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Approving a sensor closes its listener's enrollment window (locked
    // decision): the operator is done, so stop accepting new registrations.
    state
        .sensor_listeners
        .close_enrollment_window(sensor.data_source_id)
        .await;

    Ok(Json(sensor_json(&s, chrono::Utc::now(), 0)))
}

async fn set_status_endpoint(state: &AppState, id: Uuid, status: &str) -> Result<Json<Value>, StatusCode> {
    if SensorRepo::get(&state.pool, id).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let s = SensorRepo::set_status(&state.pool, id, status, false).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(sensor_json(&s, chrono::Utc::now(), 0)))
}

async fn reject_sensor(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<Value>, StatusCode> {
    set_status_endpoint(&state, id, "rejected").await
}

async fn revoke_sensor(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<Value>, StatusCode> {
    set_status_endpoint(&state, id, "revoked").await
}

async fn rotate_sensor(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<Value>, StatusCode> {
    let Some(existing) = SensorRepo::get(&state.pool, id).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? else {
        return Err(StatusCode::NOT_FOUND);
    };
    let key = fluxfang_sensor_proto::generate_key();
    let key_b64 = fluxfang_sensor_proto::encode_key(&key);
    let fp = fluxfang_sensor_proto::fingerprint(&existing.sensor_id, &key);
    SensorRepo::set_key(&state.pool, id, &key_b64, &fp).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // Returns the new key exactly once — the operator re-provisions the sensor.
    Ok(Json(json!({ "key": key_b64, "fingerprint": fp })))
}
