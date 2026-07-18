use serde_json::{json, Value};
use sqlx::PgPool;

use fluxfang_db::repo::emission::DeleteEmissionFilter;
use fluxfang_db::{EmissionRepo, EmitterAssociationRepo, EmitterRepo, EntityRepo};

use crate::mcp::tools::ToolError;
use crate::mcp::{audit, shape};

/// Permanently delete specific emission rows by id. Irreversible: unlike
/// `detach_emissions` (which only unlinks an emission from its emitter,
/// returning it to stray), this removes the rows outright. Non-existent ids
/// are simply not counted.
pub async fn delete_emissions(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let ids = shape::parse_uuid_list(&args, "emission_ids")?;
    let deleted = EmissionRepo::delete_bulk(pool, &ids).await?;
    let result = json!({ "deleted": deleted });
    audit::record_success(
        pool,
        "delete_emissions",
        "remove",
        format!("Permanently deleted {deleted} emission(s) by id"),
        &args,
        &result,
        ids,
    )
    .await;
    Ok(result)
}

/// Permanently delete emissions in bulk by filter (kind / time window /
/// emitter_id / unassigned), or ALL of them when `all` is true. Irreversible.
/// Requires at least one filter field OR an explicit `all: true` — an empty,
/// unfiltered call is rejected so a total wipe can't happen by accident.
pub async fn delete_emissions_where(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    if all {
        let deleted = EmissionRepo::delete_all(pool).await?;
        let result = json!({ "deleted": deleted, "all": true });
        audit::record_success(
            pool,
            "delete_emissions_where",
            "remove",
            format!("Permanently deleted ALL {deleted} emission(s)"),
            &args,
            &result,
            Vec::new(),
        )
        .await;
        return Ok(result);
    }

    let filter = DeleteEmissionFilter {
        kind: shape::opt_str(&args, "kind"),
        time_from: shape::opt_time(&args, "time_from")?,
        time_to: shape::opt_time(&args, "time_to")?,
        emitter_id: match args.get("emitter_id").and_then(Value::as_str) {
            Some(_) => Some(shape::parse_uuid(&args, "emitter_id")?),
            None => None,
        },
        unassigned: args.get("unassigned").and_then(Value::as_bool),
    };

    // Guard: refuse an unfiltered call (which would delete everything). A full
    // clear must be requested explicitly with `all: true`.
    let has_filter = filter.kind.is_some()
        || filter.time_from.is_some()
        || filter.time_to.is_some()
        || filter.emitter_id.is_some()
        || filter.unassigned.is_some();
    if !has_filter {
        return Err(ToolError::InvalidParams(
            "provide at least one filter (kind/time_from/time_to/emitter_id/unassigned) or 'all': true to delete every emission".into(),
        ));
    }

    let deleted = EmissionRepo::delete_where(pool, filter).await?;
    let result = json!({ "deleted": deleted });
    audit::record_success(
        pool,
        "delete_emissions_where",
        "remove",
        format!("Permanently deleted {deleted} emission(s) matching filter"),
        &args,
        &result,
        Vec::new(),
    )
    .await;
    Ok(result)
}

pub async fn detach_emissions(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let ids = shape::parse_uuid_list(&args, "emission_ids")?;
    for eid in &ids {
        EmissionRepo::clear_emitter(pool, *eid).await?;
    }
    let result = json!({ "detached": ids.len() });
    audit::record_success(
        pool, "detach_emissions", "remove",
        format!("Detached {} emission(s) back to stray", ids.len()),
        &args, &result, ids,
    ).await;
    Ok(result)
}

pub async fn unassign_emitters_from_entity(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let ids = shape::parse_uuid_list(&args, "emitter_ids")?;
    for eid in &ids {
        EmitterRepo::set_entity(pool, *eid, None).await?;
    }
    let result = json!({ "unassigned": ids.len() });
    audit::record_success(
        pool, "unassign_emitters_from_entity", "remove",
        format!("Unassigned {} emitter(s) from their entity", ids.len()),
        &args, &result, ids,
    ).await;
    Ok(result)
}

pub async fn unlink_emitters(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let a = shape::parse_uuid(&args, "emitter_id")?;
    let b = shape::parse_uuid(&args, "associated_emitter_id")?;
    EmitterAssociationRepo::remove(pool, a, b).await?;
    let result = json!({ "emitter_id": a, "associated_emitter_id": b });
    audit::record_success(
        pool, "unlink_emitters", "remove",
        format!("Unlinked emitter {a} <-> {b}"), &args, &result, vec![a, b],
    ).await;
    Ok(result)
}

pub async fn delete_emitter(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let id = shape::parse_uuid(&args, "emitter_id")?;
    // Snapshot key fields for the audit trail before deletion.
    let snapshot = EmitterRepo::get(pool, id).await?
        .map(|e| shape::emitter_json(&e))
        .unwrap_or_else(|| json!({ "id": id }));
    let deleted = EmitterRepo::delete(pool, id).await?;
    if !deleted {
        return Err(ToolError::NotFound(format!("emitter {id}")));
    }
    let result = json!({ "deleted_emitter": snapshot });
    audit::record_success(
        pool, "delete_emitter", "remove",
        format!("Deleted emitter {id}"), &args, &result, vec![id],
    ).await;
    Ok(result)
}

pub async fn delete_entity(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let id = shape::parse_uuid(&args, "entity_id")?;
    let snapshot = EntityRepo::get(pool, id).await?
        .map(|e| shape::entity_json(&e))
        .unwrap_or_else(|| json!({ "id": id }));
    let deleted = EntityRepo::delete(pool, id).await?;
    if !deleted {
        return Err(ToolError::NotFound(format!("entity {id}")));
    }
    let result = json!({ "deleted_entity": snapshot });
    audit::record_success(
        pool, "delete_entity", "remove",
        format!("Deleted entity {id}"), &args, &result, vec![id],
    ).await;
    Ok(result)
}
