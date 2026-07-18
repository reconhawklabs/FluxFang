use std::time::Duration;

use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_core::correlate::{cooccurrences, should_associate, CorrelationConfig, Reading};
use fluxfang_core::cotravel::{score, CoTravelMetrics};
use fluxfang_db::repo::cotravel::CoTravelFilter;
use fluxfang_db::repo::emission::EmissionFilter;
use fluxfang_db::{CoTravelRepo, EmissionRepo};

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

/// Score candidate emitter pairs for association using co-occurrence
/// timing/distance. Returns only pairs `should_associate` reaches a verdict
/// on (models_match is always true here — the caller is explicitly asking us
/// to evaluate these emitters as candidates).
pub async fn suggest_associations(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let ids = shape::parse_uuid_list(&args, "emitter_ids")?;
    if ids.len() < 2 {
        return Err(ToolError::InvalidParams("need at least 2 emitter_ids".into()));
    }
    let cfg = CorrelationConfig::default();
    let mut readings = Vec::with_capacity(ids.len());
    for id in &ids {
        readings.push((*id, readings_for(pool, *id).await?));
    }
    let mut suggestions = Vec::new();
    for i in 0..readings.len() {
        for j in (i + 1)..readings.len() {
            let events = cooccurrences(&readings[i].1, &readings[j].1, cfg.cooccur_window);
            if let Some(confidence) = should_associate(&events, true, &cfg) {
                suggestions.push(json!({
                    "emitter_a": readings[i].0, "emitter_b": readings[j].0,
                    "confidence": confidence, "cooccurrences": events.len(),
                }));
            }
        }
    }
    Ok(json!({ "suggestions": suggestions }))
}

/// Rank emitters by how strongly they co-travel: `CoTravelRepo::candidates`
/// runs the PostGIS gate/aggregate query, `fluxfang_core::cotravel::score`
/// turns each row's spread/points/span into a 0-100 score and tier.
pub async fn cotravel_analysis(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    // ¼-mile / 30s defaults mirror the Co-Travel page (fluxfang-api::cotravel
    // DEFAULT_MIN_DISTANCE_M / DEFAULT_MIN_TIME_S).
    let filter = CoTravelFilter {
        time_from: shape::opt_time(&args, "time_from")?,
        time_to: shape::opt_time(&args, "time_to")?,
        min_distance_m: args.get("min_distance_m").and_then(Value::as_f64).unwrap_or(402.336),
        min_time_s: args.get("min_time_s").and_then(Value::as_f64).unwrap_or(30.0),
    };
    let candidates = CoTravelRepo::candidates(pool, &filter).await?;
    let scored: Vec<Value> = candidates
        .into_iter()
        .map(|c| {
            let s = score(&CoTravelMetrics {
                spread_m: c.spread_m,
                points: c.points,
                span_s: c.span_s,
                hits: c.hits,
            });
            json!({
                "emitter_id": c.emitter_id, "name": c.name,
                "spread_m": c.spread_m, "points": c.points, "span_s": c.span_s, "hits": c.hits,
                "score": s.score, "tier": s.tier.as_str(),
            })
        })
        .collect();
    Ok(json!({ "candidates": scored }))
}
