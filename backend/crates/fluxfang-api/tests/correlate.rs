//! Task 7: `run_correlation_pass` wires the pure `fluxfang_core::correlate`
//! engine to the DB. Seeds a data source with `config.auto_correlate_tpms =
//! true`, two `tpms_sensor` emitters, and located `tpms` emissions for both
//! (co-occurring within the engine's 60s window at two locations >= 1 mile
//! apart), then asserts the pass links them bidirectionally with
//! `source = "auto"`. A sibling negative test proves a differing
//! `attributes.model` blocks the link even with identical seeding otherwise.
//!
//! Same seeding pattern `tests/emissions.rs`/`tests/data_sources.rs` use:
//! `EmissionRepo::insert`/`DataSourceRepo::insert`/`SessionRepo::open`
//! directly against the test app's own isolated pool (`common::fresh_pool_shared`),
//! not `#[sqlx::test]` — this crate's tests always isolate via a fresh
//! Postgres *schema* (see `tests/common/mod.rs`), not sqlx's own
//! transaction-per-test harness.

use chrono::{TimeZone, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_db::models::{NewDataSource, NewEmission, NewEmitter};
use fluxfang_db::{DataSourceRepo, EmissionRepo, EmitterAssociationRepo, EmitterRepo, SessionRepo};

mod common;
use common::fresh_pool_shared;

/// A `rtl_sdr`/`tpms` data source with `config.auto_correlate_tpms = true` —
/// the candidate-set gate `EmitterRepo::list_auto_correlate_tpms` filters on.
async fn seed_auto_correlate_data_source(pool: &PgPool) -> Uuid {
    DataSourceRepo::insert(
        pool,
        NewDataSource {
            kind: "rtl_sdr".to_string(),
            mode: "tpms".to_string(),
            interface: None,
            config: json!({"auto_correlate_tpms": true, "frequency": "315M"}),
        },
    )
    .await
    .expect("seed auto-correlate data source")
    .id
}

async fn seed_session(pool: &PgPool) -> Uuid {
    SessionRepo::close_active(pool)
        .await
        .expect("self-heal: close any active survey_session");
    SessionRepo::open(pool)
        .await
        .expect("seed survey_session")
        .id
}

async fn seed_tpms_emitter(pool: &PgPool, name: &str, model: &str) -> Uuid {
    EmitterRepo::insert(
        pool,
        NewEmitter {
            name: name.to_string(),
            emitter_type: Some("tpms_sensor".to_string()),
            attributes: json!({"model": model}),
            identity_key: Some(format!("tpms_sensor:{name}")),
            ..Default::default()
        },
    )
    .await
    .expect("seed tpms_sensor emitter")
    .id
}

#[allow(clippy::too_many_arguments)]
async fn insert_tpms(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    emitter: Uuid,
    at: chrono::DateTime<Utc>,
    lon: f64,
    lat: f64,
) {
    EmissionRepo::insert(
        pool,
        NewEmission {
            data_source_id: Some(ds),
            emitter_id: Some(emitter),
            session_id: Some(session),
            observed_at: at,
            signal_strength: Some(-40),
            location: Some((lon, lat)),
            location_quality: "fresh".to_string(),
            kind: "tpms".to_string(),
            payload: json!({"id": "x", "type": "TPMS"}),
            sensor_id: "local".to_string(),
        },
    )
    .await
    .expect("seed tpms emission");
}

/// Two co-occurrence locations >= 1 mile apart: `loc1` and `loc2` (~0.02deg
/// latitude north of `loc1`, roughly 2.2km — comfortably over the engine's
/// 1609.34m threshold).
fn locations() -> ((f64, f64), (f64, f64)) {
    ((-122.0, 37.0), (-122.0, 37.02))
}

#[tokio::test]
async fn correlation_pass_links_two_sensors_seen_together_a_mile_apart() {
    let pool = fresh_pool_shared().await;
    let ds = seed_auto_correlate_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let a = seed_tpms_emitter(&pool, "a", "Toyota").await;
    let b = seed_tpms_emitter(&pool, "b", "Toyota").await;
    let (loc1, loc2) = locations();

    let base = Utc.with_ymd_and_hms(2026, 7, 7, 12, 0, 0).unwrap();
    // Co-occurrence #1 at loc1 (10s apart, within the 60s window).
    insert_tpms(&pool, ds, session, a, base, loc1.0, loc1.1).await;
    insert_tpms(
        &pool,
        ds,
        session,
        b,
        base + chrono::Duration::seconds(10),
        loc1.0,
        loc1.1,
    )
    .await;
    // Co-occurrence #2 at loc2, 10 minutes later.
    let t2 = base + chrono::Duration::minutes(10);
    insert_tpms(&pool, ds, session, a, t2, loc2.0, loc2.1).await;
    insert_tpms(
        &pool,
        ds,
        session,
        b,
        t2 + chrono::Duration::seconds(10),
        loc2.0,
        loc2.1,
    )
    .await;

    let now = base + chrono::Duration::hours(1);
    let added = fluxfang_api::correlate::run_correlation_pass(&pool, now)
        .await
        .expect("correlation pass should succeed");
    assert!(added >= 1, "expected at least one new association");

    let from_a = EmitterAssociationRepo::list_for(&pool, a)
        .await
        .expect("list_for a");
    assert!(
        from_a
            .iter()
            .any(|ae| ae.emitter.id == b && ae.source == "auto"),
        "a should be auto-associated with b"
    );
    let from_b = EmitterAssociationRepo::list_for(&pool, b)
        .await
        .expect("list_for b");
    assert!(
        from_b
            .iter()
            .any(|ae| ae.emitter.id == a && ae.source == "auto"),
        "b should be auto-associated with a (bidirectional)"
    );
}

#[tokio::test]
async fn correlation_pass_does_not_link_different_models() {
    let pool = fresh_pool_shared().await;
    let ds = seed_auto_correlate_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let a = seed_tpms_emitter(&pool, "a", "Toyota").await;
    let b = seed_tpms_emitter(&pool, "b", "Honda").await;
    let (loc1, loc2) = locations();

    let base = Utc.with_ymd_and_hms(2026, 7, 7, 12, 0, 0).unwrap();
    insert_tpms(&pool, ds, session, a, base, loc1.0, loc1.1).await;
    insert_tpms(
        &pool,
        ds,
        session,
        b,
        base + chrono::Duration::seconds(10),
        loc1.0,
        loc1.1,
    )
    .await;
    let t2 = base + chrono::Duration::minutes(10);
    insert_tpms(&pool, ds, session, a, t2, loc2.0, loc2.1).await;
    insert_tpms(
        &pool,
        ds,
        session,
        b,
        t2 + chrono::Duration::seconds(10),
        loc2.0,
        loc2.1,
    )
    .await;

    let now = base + chrono::Duration::hours(1);
    let added = fluxfang_api::correlate::run_correlation_pass(&pool, now)
        .await
        .expect("correlation pass should succeed");
    assert_eq!(added, 0, "different models must never be auto-associated");
    assert!(!EmitterAssociationRepo::exists(&pool, a, b)
        .await
        .expect("exists check"));
}
