use serde_json::{json, Value};
use sqlx::PgPool;

use fluxfang_db::repo::emission::EmissionFilter;
use fluxfang_db::repo::emitter::EmitterListFilter;
use fluxfang_db::repo::entity::EntityListFilter;
use fluxfang_db::{EmissionRepo, EmitterAssociationRepo, EmitterRepo, EntityRepo};

use crate::mcp::shape;
use crate::mcp::tools::ToolError;

pub async fn list_entities(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let filter = EntityListFilter {
        search: shape::opt_str(&args, "search"),
        limit: shape::clamp_limit(&args),
        offset: shape::offset(&args),
    };
    let (rows, total) = EntityRepo::query(pool, filter).await?;
    Ok(json!({
        "items": rows.iter().map(shape::entity_json).collect::<Vec<_>>(),
        "total": total,
    }))
}

fn emission_filter(args: &Value, unassigned: bool) -> Result<EmissionFilter, ToolError> {
    Ok(EmissionFilter {
        unassigned,
        emitter_id: match args.get("emitter_id").and_then(Value::as_str) {
            Some(_) => Some(shape::parse_uuid(args, "emitter_id")?),
            None => None,
        },
        kind: shape::opt_str(args, "kind"),
        time_from: shape::opt_time(args, "time_from")?,
        time_to: shape::opt_time(args, "time_to")?,
        text: shape::opt_str(args, "text"),
        limit: shape::clamp_limit(args),
        offset: shape::offset(args),
        ..Default::default()
    })
}

async fn run_emission_query(pool: &PgPool, filter: EmissionFilter) -> Result<Value, ToolError> {
    let (rows, total) = EmissionRepo::query(pool, filter)
        .await
        .map_err(|e| ToolError::Db(format!("{e:?}")))?;
    Ok(json!({
        "items": rows.iter().map(shape::emission_json).collect::<Vec<_>>(),
        "total": total,
    }))
}

pub async fn list_stray_emissions(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    run_emission_query(pool, emission_filter(&args, true)?).await
}

pub async fn list_emissions(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    run_emission_query(pool, emission_filter(&args, false)?).await
}

pub async fn get_emission(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let id = shape::parse_uuid(&args, "id")?;
    let em = EmissionRepo::get(pool, id)
        .await?
        .ok_or_else(|| ToolError::NotFound(format!("emission {id}")))?;
    Ok(shape::emission_json(&em))
}

pub async fn list_emitters(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let filter = EmitterListFilter {
        search: shape::opt_str(&args, "search"),
        entity_id: match args.get("entity_id").and_then(Value::as_str) {
            Some(_) => Some(shape::parse_uuid(&args, "entity_id")?),
            None => None,
        },
        emitter_type: shape::opt_str(&args, "emitter_type"),
        limit: shape::clamp_limit(&args),
        offset: shape::offset(&args),
        ..Default::default()
    };
    let (rows, total) = EmitterRepo::query(pool, filter)
        .await
        .map_err(|e| ToolError::Db(format!("{e:?}")))?;
    Ok(json!({
        "items": rows.iter().map(|(e, count)| {
            let mut v = shape::emitter_json(e);
            v["attached_emission_count"] = json!(count);
            v
        }).collect::<Vec<_>>(),
        "total": total,
    }))
}

pub async fn get_emitter(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let id = shape::parse_uuid(&args, "id")?;
    let emitter = EmitterRepo::get(pool, id)
        .await?
        .ok_or_else(|| ToolError::NotFound(format!("emitter {id}")))?;
    let assocs = EmitterAssociationRepo::list_for(pool, id).await?;
    let recent = EmissionRepo::recent_located(pool, &[id], 100).await?;
    Ok(json!({
        "emitter": shape::emitter_json(&emitter),
        "associations": assocs.iter().map(|a| json!({
            "emitter": shape::emitter_json(&a.emitter),
            "source": a.source, "confidence": a.confidence,
        })).collect::<Vec<_>>(),
        "recent_emissions": recent.iter().map(shape::emission_json).collect::<Vec<_>>(),
    }))
}

pub async fn get_entity(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let id = shape::parse_uuid(&args, "id")?;
    let entity = EntityRepo::get(pool, id)
        .await?
        .ok_or_else(|| ToolError::NotFound(format!("entity {id}")))?;
    let emitters = EmitterRepo::list_by_entity(pool, id).await?;
    let last_seen = EntityRepo::last_seen(pool, id).await?;
    let emitter_ids: Vec<_> = emitters.iter().map(|e| e.id).collect();
    let recent = EmissionRepo::recent_located(pool, &emitter_ids, 100).await?;
    Ok(json!({
        "entity": shape::entity_json(&entity),
        "last_seen": last_seen,
        "emitters": emitters.iter().map(shape::emitter_json).collect::<Vec<_>>(),
        "recent_detections": recent.iter().map(shape::emission_json).collect::<Vec<_>>(),
    }))
}

pub async fn emitters_connected_to(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let ssid = shape::opt_str(&args, "ssid");
    let bssid = shape::opt_str(&args, "bssid");
    if ssid.is_none() && bssid.is_none() {
        return Err(ToolError::InvalidParams("provide 'ssid' or 'bssid'".into()));
    }
    // Client emitters whose attributes.connected_ssid/connected_bssid match.
    let rows = sqlx::query_as::<_, fluxfang_db::models::Emitter>(&format!(
        "SELECT {cols} FROM emitter \
         WHERE ($1::text IS NULL OR attributes->>'connected_ssid' = $1) \
           AND ($2::text IS NULL OR attributes->>'connected_bssid' = $2) \
         ORDER BY last_seen_at DESC NULLS LAST LIMIT $3",
        cols = fluxfang_db::repo::emitter::EMITTER_COLUMNS,
    ))
    .bind(&ssid)
    .bind(&bssid)
    .bind(shape::clamp_limit(&args))
    .fetch_all(pool)
    .await?;
    Ok(json!({ "emitters": rows.iter().map(shape::emitter_json).collect::<Vec<_>>() }))
}

pub async fn list_attributes_by_type(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let emitter_type = shape::opt_str(&args, "emitter_type")
        .ok_or_else(|| ToolError::InvalidParams("missing 'emitter_type'".into()))?;
    // Aggregate the distinct attribute keys → sample values in use for this type.
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT kv.key, jsonb_agg(DISTINCT kv.value) AS values \
         FROM emitter e, jsonb_each(e.attributes) kv \
         WHERE e.emitter_type = $1 GROUP BY kv.key ORDER BY kv.key",
    )
    .bind(&emitter_type)
    .fetch_all(pool)
    .await?;
    let attrs: serde_json::Map<String, Value> = rows.into_iter().collect();
    Ok(json!({ "emitter_type": emitter_type, "attributes": attrs }))
}

pub async fn signal_uniqueness(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let field = shape::opt_str(&args, "field")
        .ok_or_else(|| ToolError::InvalidParams("missing 'field' (e.g. ssid, bssid, id)".into()))?;
    let value = shape::opt_str(&args, "value")
        .ok_or_else(|| ToolError::InvalidParams("missing 'value'".into()))?;
    // Validate field against the safe identifier charset (no interpolation risk).
    // `field.is_empty()` must be checked explicitly: `str::chars().all(..)` is
    // vacuously true on an empty string, which would otherwise let "" through
    // and interpolate into the SQL below as `payload->>''`.
    if field.is_empty()
        || !field
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(ToolError::InvalidParams(
            "'field' must match [a-z0-9_]+".into(),
        ));
    }
    let matching: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM emission WHERE payload->>'{field}' = $1"
    ))
    .bind(&value)
    .fetch_one(pool)
    .await?;
    let distinct: i64 = sqlx::query_scalar(&format!(
        "SELECT count(DISTINCT payload->>'{field}') FROM emission WHERE payload ? '{field}'"
    ))
    .fetch_one(pool)
    .await?;
    Ok(json!({
        "field": field, "value": value,
        "matching_emissions": matching,
        "distinct_values_for_field": distinct,
        "is_unique": matching > 0 && distinct > 0,
    }))
}
