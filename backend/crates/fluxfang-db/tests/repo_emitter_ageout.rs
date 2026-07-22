//! `EmitterRepo::age_out_ephemeral` — the destructive sweep behind the
//! per-data-source "Age Out Ephemeral-class emitters" option.
//!
//! The eligibility rules matter more than the happy path here: this deletes
//! emissions permanently, so the tests below pin down every case where it
//! must *not* fire.

mod common;

use chrono::{Duration, Utc};
use common::{fresh_pool, seed_session};
use fluxfang_db::models::{NewDataSource, NewEmission, NewEmitter};
use fluxfang_db::{DataSourceRepo, EmissionRepo, EmitterRepo};
use sqlx::PgPool;
use uuid::Uuid;

/// A wifi data source with `age_out_ephemeral` set to `opted_in`.
async fn seed_source(pool: &PgPool, iface: &str, opted_in: bool) -> Uuid {
    let mut ds = NewDataSource::wifi_monitor(iface);
    ds.config = serde_json::json!({ "age_out_ephemeral": opted_in });
    DataSourceRepo::insert(pool, ds)
        .await
        .expect("seed wifi data_source")
        .id
}

/// An auto-created emitter of persistence class `class`, last seen
/// `minutes_ago` minutes ago.
async fn seed_emitter(pool: &PgPool, identity: &str, class: &str, minutes_ago: i64) -> Uuid {
    let seen = Utc::now() - Duration::minutes(minutes_ago);
    let emitter = EmitterRepo::insert(
        pool,
        NewEmitter {
            name: format!("WiFi Client {identity}"),
            emitter_type: Some("wifi_client".to_string()),
            attributes: serde_json::json!({
                "src_mac": identity,
                "mac_persistence": class,
            }),
            identity_key: Some(format!("wifi_client:{identity}")),
            match_criteria: serde_json::json!({}),
            ..Default::default()
        },
    )
    .await
    .expect("insert emitter");
    EmitterRepo::touch_seen(pool, emitter.id, seen)
        .await
        .expect("touch_seen");
    emitter.id
}

/// Attach one emission from `source` to `emitter`.
async fn seed_emission(pool: &PgPool, source: Uuid, session: Uuid, emitter: Uuid) -> Uuid {
    let em = EmissionRepo::insert(
        pool,
        NewEmission {
            data_source_id: Some(source),
            emitter_id: Some(emitter),
            session_id: Some(session),
            observed_at: Utc::now() - Duration::minutes(90),
            signal_strength: Some(-60),
            location: None,
            location_quality: "none".to_string(),
            kind: "wifi".to_string(),
            payload: serde_json::json!({"frame_type": "probe_request"}),
            sensor_id: "local".to_string(),
        },
    )
    .await
    .expect("insert emission");
    em.id
}

fn cutoff() -> chrono::DateTime<Utc> {
    Utc::now() - Duration::seconds(fluxfang_core::retention::AGE_OUT_AFTER_SECS)
}

#[tokio::test]
async fn removes_stale_ephemeral_emitter_and_its_emissions() {
    let pool = fresh_pool().await;
    let session = seed_session(&pool).await;
    let source = seed_source(&pool, "wlan0", true).await;
    let emitter = seed_emitter(&pool, "3a:de:ad:be:ef:00", "ephemeral", 90).await;
    let emission = seed_emission(&pool, source, session, emitter).await;

    let removed = EmitterRepo::age_out_ephemeral(&pool, cutoff())
        .await
        .expect("age out");

    assert_eq!(removed, 1);
    assert!(EmitterRepo::get(&pool, emitter).await.unwrap().is_none());
    assert!(
        EmissionRepo::get(&pool, emission).await.unwrap().is_none(),
        "the emission must go with the emitter, not survive as a stray"
    );
}

#[tokio::test]
async fn keeps_ephemeral_emitter_seen_within_the_window() {
    let pool = fresh_pool().await;
    let session = seed_session(&pool).await;
    let source = seed_source(&pool, "wlan0", true).await;
    // 30 minutes ago is inside the one-hour window.
    let emitter = seed_emitter(&pool, "3a:de:ad:be:ef:01", "ephemeral", 30).await;
    seed_emission(&pool, source, session, emitter).await;

    let removed = EmitterRepo::age_out_ephemeral(&pool, cutoff())
        .await
        .expect("age out");

    assert_eq!(removed, 0);
    assert!(EmitterRepo::get(&pool, emitter).await.unwrap().is_some());
}

#[tokio::test]
async fn keeps_more_persistent_classes_however_stale() {
    let pool = fresh_pool().await;
    let session = seed_session(&pool).await;
    let source = seed_source(&pool, "wlan0", true).await;

    // Only `ephemeral` is swept -- a static-random BLE address or a
    // per-SSID Wi-Fi MAC is still worth tracking days later.
    for (i, class) in ["stable", "per_network", "session", "unlinkable"]
        .iter()
        .enumerate()
    {
        let mac = format!("3a:de:ad:be:ef:{i:02x}");
        let emitter = seed_emitter(&pool, &mac, class, 600).await;
        seed_emission(&pool, source, session, emitter).await;
    }

    let removed = EmitterRepo::age_out_ephemeral(&pool, cutoff())
        .await
        .expect("age out");

    assert_eq!(removed, 0, "only the ephemeral class may be aged out");
}

#[tokio::test]
async fn keeps_emitter_when_any_emission_came_from_a_source_that_did_not_opt_in() {
    let pool = fresh_pool().await;
    let session = seed_session(&pool).await;
    let opted_in = seed_source(&pool, "wlan0", true).await;
    let not_opted_in = seed_source(&pool, "wlan1", false).await;

    let emitter = seed_emitter(&pool, "3a:de:ad:be:ef:02", "ephemeral", 90).await;
    seed_emission(&pool, opted_in, session, emitter).await;
    let protected = seed_emission(&pool, not_opted_in, session, emitter).await;

    let removed = EmitterRepo::age_out_ephemeral(&pool, cutoff())
        .await
        .expect("age out");

    assert_eq!(
        removed, 0,
        "a single emission from a source that didn't opt in must protect the whole emitter"
    );
    assert!(EmitterRepo::get(&pool, emitter).await.unwrap().is_some());
    assert!(EmissionRepo::get(&pool, protected).await.unwrap().is_some());
}

#[tokio::test]
async fn keeps_emitter_with_no_emissions_at_all() {
    let pool = fresh_pool().await;
    seed_source(&pool, "wlan0", true).await;
    // No emissions -> no data source consented on this emitter's behalf.
    let emitter = seed_emitter(&pool, "3a:de:ad:be:ef:03", "ephemeral", 600).await;

    let removed = EmitterRepo::age_out_ephemeral(&pool, cutoff())
        .await
        .expect("age out");

    assert_eq!(removed, 0);
    assert!(EmitterRepo::get(&pool, emitter).await.unwrap().is_some());
}

#[tokio::test]
async fn does_nothing_while_no_source_has_opted_in() {
    let pool = fresh_pool().await;
    let session = seed_session(&pool).await;
    let source = seed_source(&pool, "wlan0", false).await;
    let emitter = seed_emitter(&pool, "3a:de:ad:be:ef:04", "ephemeral", 600).await;
    seed_emission(&pool, source, session, emitter).await;

    let removed = EmitterRepo::age_out_ephemeral(&pool, cutoff())
        .await
        .expect("age out");

    assert_eq!(removed, 0, "the sweep must be inert until opted into");
}

#[tokio::test]
async fn ignores_emitters_predating_the_persistence_attribute() {
    let pool = fresh_pool().await;
    let session = seed_session(&pool).await;
    let source = seed_source(&pool, "wlan0", true).await;

    // An emitter classified before `mac_persistence` existed carries only
    // the legacy boolean. It has no class, so it isn't swept.
    let emitter = EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "legacy client".to_string(),
            emitter_type: Some("wifi_client".to_string()),
            attributes: serde_json::json!({"src_mac": "3a:00:00:00:00:01", "randomized_mac": true}),
            identity_key: Some("wifi_client:3a:00:00:00:00:01".to_string()),
            match_criteria: serde_json::json!({}),
            ..Default::default()
        },
    )
    .await
    .expect("insert emitter")
    .id;
    EmitterRepo::touch_seen(&pool, emitter, Utc::now() - Duration::minutes(600))
        .await
        .expect("touch_seen");
    seed_emission(&pool, source, session, emitter).await;

    let removed = EmitterRepo::age_out_ephemeral(&pool, cutoff())
        .await
        .expect("age out");

    assert_eq!(removed, 0);
    assert!(EmitterRepo::get(&pool, emitter).await.unwrap().is_some());
}
