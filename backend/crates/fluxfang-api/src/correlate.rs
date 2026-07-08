//! The periodic TPMS correlation pass (Spec B): loads candidate tpms_sensor
//! emitters (those from an auto-correlate data source), and for each
//! same-model pair not already linked, runs the pure engine
//! (`fluxfang_core::correlate`) over their recent emissions and adds an
//! `auto` association on a positive verdict. Only ever ADDS `source='auto'`.

use std::time::Duration;

use chrono::{DateTime, Utc};
use fluxfang_core::correlate::{cooccurrences, should_associate, CorrelationConfig, Reading};
use fluxfang_db::models::Emitter;
use fluxfang_db::repo::emission::EmissionFilter;
use fluxfang_db::{EmissionRepo, EmitterAssociationRepo, EmitterRepo};
use sqlx::PgPool;

/// How far back to pull emissions when correlating.
const LOOKBACK: Duration = Duration::from_secs(60 * 60 * 24); // 24h
/// Max emissions to consider per emitter per pass.
const MAX_READINGS: i64 = 1000;

/// Run one correlation pass. Returns the number of new associations added.
pub async fn run_correlation_pass(pool: &PgPool, now: DateTime<Utc>) -> anyhow::Result<usize> {
    let cfg = CorrelationConfig::default();
    let candidates = EmitterRepo::list_auto_correlate_tpms(pool).await?;
    let time_from = now - chrono::Duration::from_std(LOOKBACK)?;

    // Fetch each candidate's recent readings once.
    let mut readings: Vec<(Emitter, Vec<Reading>)> = Vec::new();
    for e in candidates {
        let filter = EmissionFilter {
            emitter_id: Some(e.id),
            time_from: Some(time_from),
            kind: Some("tpms".to_string()),
            limit: MAX_READINGS,
            ..Default::default()
        };
        let (emissions, _) = EmissionRepo::query(pool, filter).await?;
        let rs = emissions
            .into_iter()
            .map(|em| Reading {
                at: em.observed_at,
                lon: em.lon,
                lat: em.lat,
            })
            .collect();
        readings.push((e, rs));
    }

    let mut added = 0usize;
    for i in 0..readings.len() {
        for j in (i + 1)..readings.len() {
            let (ea, ra) = (&readings[i].0, &readings[i].1);
            let (eb, rb) = (&readings[j].0, &readings[j].1);

            let models_match = model_of(ea) == model_of(eb) && model_of(ea).is_some();
            if !models_match {
                continue;
            }
            if EmitterAssociationRepo::exists(pool, ea.id, eb.id).await? {
                continue;
            }
            let events = cooccurrences(ra, rb, cfg.cooccur_window);
            if let Some(confidence) = should_associate(&events, true, &cfg) {
                EmitterAssociationRepo::add(pool, ea.id, eb.id, "auto", Some(confidence)).await?;
                added += 1;
            }
        }
    }
    Ok(added)
}

fn model_of(e: &Emitter) -> Option<String> {
    e.attributes
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}
