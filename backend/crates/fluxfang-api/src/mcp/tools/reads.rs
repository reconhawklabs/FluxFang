use serde_json::{json, Value};
use sqlx::PgPool;

use fluxfang_db::repo::emission::EmissionFilter;
use fluxfang_db::repo::entity::EntityListFilter;
use fluxfang_db::EmissionRepo;
use fluxfang_db::EntityRepo;

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
