use std::time::Duration;

use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_core::correlate::{cooccurrences, Reading};
use fluxfang_db::repo::emission::EmissionFilter;
use fluxfang_db::EmissionRepo;

use crate::mcp::shape;
use crate::mcp::tools::ToolError;

async fn readings_for(pool: &PgPool, emitter_id: Uuid) -> Result<Vec<Reading>, ToolError> {
    let filter = EmissionFilter { emitter_id: Some(emitter_id), limit: 1000, ..Default::default() };
    let (emissions, _) = EmissionRepo::query(pool, filter).await
        .map_err(|e| ToolError::Db(format!("{e:?}")))?;
    Ok(emissions.into_iter()
        .map(|em| Reading { at: em.observed_at, lon: em.lon, lat: em.lat })
        .collect())
}

pub async fn collocation_query(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let ids = shape::parse_uuid_list(&args, "emitter_ids")?;
    if ids.len() < 2 {
        return Err(ToolError::InvalidParams("need at least 2 emitter_ids".into()));
    }
    let window = Duration::from_secs(
        args.get("window_seconds").and_then(Value::as_u64).unwrap_or(60),
    );

    let mut readings = Vec::with_capacity(ids.len());
    for id in &ids {
        readings.push((*id, readings_for(pool, *id).await?));
    }

    let mut pairs = Vec::new();
    for i in 0..readings.len() {
        for j in (i + 1)..readings.len() {
            let events = cooccurrences(&readings[i].1, &readings[j].1, window);
            if !events.is_empty() {
                pairs.push(json!({
                    "emitter_a": readings[i].0,
                    "emitter_b": readings[j].0,
                    "cooccurrences": events.len(),
                }));
            }
        }
    }
    Ok(json!({ "window_seconds": window.as_secs(), "pairs": pairs }))
}
