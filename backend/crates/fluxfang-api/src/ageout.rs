//! Background sweep for the per-data-source "Age Out Ephemeral-class
//! emitters" option.
//!
//! Ephemeral-class addresses (Wi-Fi probe-request MACs, BLE resolvable
//! private addresses) rotate on the order of minutes, so an emitter keyed on
//! one is dead the moment its device re-randomizes: it can never be seen
//! again, and it sits in `emitter` forever behind the unique
//! `identity_key` index. This sweep removes those rows -- and their
//! emissions -- once they've gone unseen for
//! [`fluxfang_core::retention::AGE_OUT_AFTER_SECS`].
//!
//! Opt-in per data source, and only ever applied to emitters whose
//! emissions *all* come from opted-in sources; the eligibility rules live
//! with the SQL in `EmitterRepo::age_out_ephemeral`. This is destructive and
//! irreversible, which is why the flag defaults to off and why a single
//! emission from a non-opted-in source protects the whole emitter.

use fluxfang_core::retention::AGE_OUT_AFTER_SECS;
use fluxfang_db::repo::emitter::EmitterRepo;
use sqlx::PgPool;
use std::time::Duration;

/// How often the sweep runs. Well under the one-hour age-out window, so an
/// emitter is removed within a few minutes of becoming eligible without the
/// sweep itself being hot.
const SWEEP_INTERVAL: Duration = Duration::from_secs(300);

/// Background loop: every 5 min, delete ephemeral-class emitters (and their
/// emissions) that no opted-in data source has seen for the last hour.
///
/// Runs unconditionally and re-reads eligibility from the data-source rows
/// on every tick, so toggling the checkbox takes effect within one cycle
/// with no restart -- and does nothing at all while no source has opted in.
/// Errors are logged and the loop continues: a failed sweep must not take
/// down a long-running node.
pub fn spawn_ageout(pool: PgPool) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        loop {
            ticker.tick().await;
            let cutoff = chrono::Utc::now() - chrono::Duration::seconds(AGE_OUT_AFTER_SECS);
            match EmitterRepo::age_out_ephemeral(&pool, cutoff).await {
                Ok(n) if n > 0 => {
                    eprintln!(
                        "AgeOut: removed {n} ephemeral-class emitter(s) unseen since {cutoff}"
                    )
                }
                Ok(_) => {}
                Err(e) => eprintln!("AgeOut: sweep failed: {e}"),
            }
        }
    });
}
