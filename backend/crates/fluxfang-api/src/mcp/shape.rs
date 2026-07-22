use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use fluxfang_db::models::{Emission, Emitter, Entity};

use crate::mcp::tools::ToolError;

pub fn emission_json(e: &Emission) -> Value {
    json!({
        "id": e.id,
        "created_at": e.created_at,
        "observed_at": e.observed_at,
        "emitter_id": e.emitter_id,
        "session_id": e.session_id,
        "data_source_id": e.data_source_id,
        "kind": e.kind,
        "signal_strength": e.signal_strength,
        "lon": e.lon,
        "lat": e.lat,
        "location_quality": e.location_quality,
        "payload": e.payload, // full raw payload, never truncated
    })
}

pub fn emitter_json(e: &Emitter) -> Value {
    json!({
        "id": e.id,
        "created_at": e.created_at,
        "name": e.name,
        "type": e.type_,
        "emitter_type": e.emitter_type,
        "entity_id": e.entity_id,
        "identity_key": e.identity_key,
        "match_enabled": e.match_enabled,
        "match_criteria": e.match_criteria,
        "attributes": e.attributes, // full attributes, never truncated
        "first_seen_at": e.first_seen_at,
        "last_seen_at": e.last_seen_at,
        "source": e.source,
        // RSSI-localization estimate: the emitter's inferred position + an
        // uncertainty radius (metres), or null until it's localizable.
        "estimate": match (e.est_lon, e.est_lat) {
            (Some(lon), Some(lat)) => json!({
                "lon": lon,
                "lat": lat,
                "uncertainty_m": e.est_uncertainty_m,
                "bin_count": e.est_bin_count,
                "updated_at": e.est_updated_at,
            }),
            _ => Value::Null,
        },
    })
}

pub fn entity_json(e: &Entity) -> Value {
    json!({
        "id": e.id, "created_at": e.created_at, "name": e.name,
        "notes": e.notes, "source": e.source, "ai_confidence": e.ai_confidence,
    })
}

pub fn parse_uuid(args: &Value, key: &str) -> Result<Uuid, ToolError> {
    let s = args.get(key).and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidParams(format!("missing string '{key}'")))?;
    Uuid::parse_str(s).map_err(|_| ToolError::InvalidParams(format!("'{key}' is not a valid uuid")))
}

pub fn parse_uuid_list(args: &Value, key: &str) -> Result<Vec<Uuid>, ToolError> {
    let arr = args.get(key).and_then(Value::as_array)
        .ok_or_else(|| ToolError::InvalidParams(format!("missing array '{key}'")))?;
    arr.iter().map(|v| {
        let s = v.as_str().ok_or_else(|| ToolError::InvalidParams(format!("'{key}' must be uuid strings")))?;
        Uuid::parse_str(s).map_err(|_| ToolError::InvalidParams(format!("'{key}' has an invalid uuid")))
    }).collect()
}

pub fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

pub fn opt_time(args: &Value, key: &str) -> Result<Option<DateTime<Utc>>, ToolError> {
    match args.get(key).and_then(Value::as_str) {
        None => Ok(None),
        Some(s) => s.parse::<DateTime<Utc>>().map(Some)
            .map_err(|_| ToolError::InvalidParams(format!("'{key}' must be RFC3339 timestamp"))),
    }
}

pub fn clamp_limit(args: &Value) -> i64 {
    args.get("limit").and_then(Value::as_i64).unwrap_or(50).clamp(1, 500)
}

pub fn offset(args: &Value) -> i64 {
    args.get("offset").and_then(Value::as_i64).unwrap_or(0).max(0)
}
