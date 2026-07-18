use serde_json::{json, Value};
use sqlx::PgPool;

use fluxfang_db::{EmissionRepo, EmitterAssociationRepo, EmitterRepo, EntityRepo};

use crate::mcp::tools::ToolError;
use crate::mcp::{audit, shape};

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
