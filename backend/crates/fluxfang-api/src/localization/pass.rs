//! The periodic localization pass: recompute + store every active emitter's
//! RSSI-localization estimate. Runs on the Standalone (see `main.rs`).

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

use fluxfang_db::{EmissionRepo, EmitterRepo};

use super::{localize, Obs};

/// Max age of emissions considered (spec's `MAX_BIN_AGE`).
const MAX_BIN_AGE_DAYS: i64 = 7;

/// Recompute the estimate for every emitter with emissions in the last
/// `MAX_BIN_AGE_DAYS`. Returns how many emitters received an estimate (those
/// with too few distinct observation locations are left with a null estimate,
/// so the map falls back to their latest-emission marker).
pub async fn run_localization_pass(pool: &PgPool, now: DateTime<Utc>) -> anyhow::Result<usize> {
    let since = now - Duration::days(MAX_BIN_AGE_DAYS);
    let ids = EmitterRepo::ids_with_recent_emissions(pool, since).await?;
    let mut updated = 0usize;
    for id in ids {
        let rows = EmissionRepo::located_signal_for_emitter(pool, id, since).await?;
        if rows.is_empty() {
            continue;
        }
        let obs: Vec<Obs> = rows
            .iter()
            .map(|r| Obs {
                lon: r.lon,
                lat: r.lat,
                rssi: r.rssi,
                observed_at_ms: r.observed_at.timestamp_millis(),
            })
            .collect();
        if let Some(e) = localize(&obs) {
            EmitterRepo::set_estimate(pool, id, e.lon, e.lat, e.uncertainty_m, e.bin_count as i32)
                .await?;
            updated += 1;
        }
    }
    Ok(updated)
}
