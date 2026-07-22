//! Integration test for the RSSI localization pass: seed cross-sensor located
//! emissions around a target, run the pass, assert the emitter's stored
//! estimate lands near it.

mod common;

use chrono::Utc;
use fluxfang_api::localization::pass::run_localization_pass;
use fluxfang_db::models::{NewDataSource, NewEmission, NewEmitter};
use fluxfang_db::{DataSourceRepo, EmissionRepo, EmitterRepo, SessionRepo};
use sqlx::PgPool;
use uuid::Uuid;

const LON0: f64 = -71.06;
const LAT0: f64 = 42.36;

fn m_per_deg_lon() -> f64 {
    LAT0.to_radians().cos() * 111_320.0
}

async fn seed_ds(pool: &PgPool) -> Uuid {
    DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .unwrap()
        .id
}

async fn seed_session(pool: &PgPool) -> Uuid {
    SessionRepo::close_active(pool).await.ok();
    SessionRepo::open(pool).await.unwrap().id
}

/// Insert a located+signal emission for `emitter` at a metric offset from the
/// target, tagged with `sensor_id` (models a distinct capturing node).
async fn insert_at(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    emitter: Uuid,
    east_m: f64,
    north_m: f64,
    rssi: i32,
    sensor_id: &str,
) {
    let lon = LON0 + east_m / m_per_deg_lon();
    let lat = LAT0 + north_m / 111_320.0;
    EmissionRepo::insert(
        pool,
        NewEmission {
            data_source_id: Some(ds),
            emitter_id: Some(emitter),
            session_id: Some(session),
            observed_at: Utc::now(),
            signal_strength: Some(rssi),
            location: Some((lon, lat)),
            location_quality: "fresh".to_string(),
            kind: "wifi".to_string(),
            payload: serde_json::json!({}),
            sensor_id: sensor_id.to_string(),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn pass_estimates_ringed_emitter_and_skips_single_location() {
    let pool = common::fresh_pool_shared().await;
    let ds = seed_ds(&pool).await;
    let session = seed_session(&pool).await;

    // Emitter heard from four separate sensor nodes ringing the target.
    let e = EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "AP".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    insert_at(&pool, ds, session, e.id, 0.0, 30.0, -60, "local").await;
    insert_at(&pool, ds, session, e.id, 30.0, 0.0, -60, "s2").await;
    insert_at(&pool, ds, session, e.id, 0.0, -30.0, -60, "s3").await;
    insert_at(&pool, ds, session, e.id, -30.0, 0.0, -60, "s4").await;

    // A second emitter heard from only ONE location -> not localizable.
    let solo = EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "solo".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for _ in 0..5 {
        insert_at(&pool, ds, session, solo.id, 100.0, 100.0, -50, "local").await;
    }

    let n = run_localization_pass(&pool, Utc::now()).await.unwrap();
    assert!(n >= 1, "at least the ringed emitter should be estimated");

    let got = EmitterRepo::get(&pool, e.id).await.unwrap().unwrap();
    let elon = got.est_lon.expect("ringed emitter should have an estimate");
    let elat = got.est_lat.unwrap();
    let off = ((elon - LON0) * m_per_deg_lon()).hypot((elat - LAT0) * 111_320.0);
    assert!(off < 12.0, "estimate {off:.1} m from the ringed target");
    assert!(got.est_uncertainty_m.unwrap() > 0.0);
    assert_eq!(got.est_bin_count, Some(4));

    let got_solo = EmitterRepo::get(&pool, solo.id).await.unwrap().unwrap();
    assert!(
        got_solo.est_lon.is_none(),
        "a single-location emitter must not be localized"
    );
}
