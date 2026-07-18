//! Round-trip tests for `CoTravelRepo`'s ignore list and candidate query.

mod common;

use chrono::{Duration, Utc};
use common::{fresh_pool, seed_session, seed_wifi_source};
use fluxfang_db::models::{NewEmission, NewEmitter};
use fluxfang_db::repo::cotravel::CoTravelFilter;
use fluxfang_db::{CoTravelRepo, EmissionRepo, EmitterRepo};
use sqlx::PgPool;
use uuid::Uuid;

async fn seed_emitter(pool: &PgPool, name: &str) -> Uuid {
    EmitterRepo::insert(
        pool,
        NewEmitter {
            name: name.to_string(),
            type_: None,
            entity_id: None,
            match_criteria: serde_json::json!({}),
            emitter_type: Some("wifi_client".to_string()),
            attributes: serde_json::json!({"src_mac": "aa:bb:cc:dd:ee:ff"}),
            match_enabled: true,
            identity_key: Some(format!("wifi_client:{name}")),
            source: "manual".to_string(),
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn ignore_is_idempotent_and_listed() {
    let pool = fresh_pool().await;
    let id = seed_emitter(&pool, "a").await;

    CoTravelRepo::ignore(&pool, id).await.unwrap();
    // Ignoring the same emitter twice must not error (upsert).
    CoTravelRepo::ignore(&pool, id).await.unwrap();

    let listed = CoTravelRepo::list_ignored(&pool).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].identity_key.as_deref(), Some("wifi_client:a"));
}

#[tokio::test]
async fn unignore_removes_and_reports_count() {
    let pool = fresh_pool().await;
    let id = seed_emitter(&pool, "b").await;
    CoTravelRepo::ignore(&pool, id).await.unwrap();

    let removed = CoTravelRepo::unignore(&pool, id).await.unwrap();
    assert_eq!(removed, 1);
    assert!(CoTravelRepo::list_ignored(&pool).await.unwrap().is_empty());

    // Unignoring something not present is a no-op, not an error.
    let removed_again = CoTravelRepo::unignore(&pool, id).await.unwrap();
    assert_eq!(removed_again, 0);
}

#[tokio::test]
async fn unignore_unknown_id_is_zero() {
    let pool = fresh_pool().await;
    let removed = CoTravelRepo::unignore(&pool, Uuid::new_v4()).await.unwrap();
    assert_eq!(removed, 0);
}

/// Insert a located wifi emission for `emitter_id` at (lon,lat), `t`.
async fn insert_located(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    emitter_id: Uuid,
    lon: f64,
    lat: f64,
    t: chrono::DateTime<Utc>,
) {
    let mut new = NewEmission::wifi(ds, session, serde_json::json!({"bssid": "x"}));
    new.emitter_id = Some(emitter_id);
    new.location = Some((lon, lat));
    new.observed_at = t;
    new.location_quality = "fresh".to_string();
    EmissionRepo::insert(pool, new).await.unwrap();
}

#[tokio::test]
async fn candidate_gate_and_metrics() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let now = Utc::now();

    // Mover: two sightings ~1.5 km apart, 10 min apart -> clears a 402 m / 30 s gate.
    let mover = seed_emitter(&pool, "mover").await;
    insert_located(&pool, ds, session, mover, -84.500, 37.700, now).await;
    insert_located(&pool, ds, session, mover, -84.483, 37.700, now + Duration::minutes(10)).await;

    // Stationary: two sightings at the same point -> spread 0, fails the gate.
    let fixed = seed_emitter(&pool, "fixed").await;
    insert_located(&pool, ds, session, fixed, -84.400, 37.600, now).await;
    insert_located(&pool, ds, session, fixed, -84.400, 37.600, now + Duration::minutes(10)).await;

    let filter = CoTravelFilter {
        time_from: None,
        time_to: None,
        min_distance_m: 402.336,
        min_time_s: 30.0,
    };
    let rows = CoTravelRepo::candidates(&pool, &filter).await.unwrap();

    assert_eq!(rows.len(), 1, "only the mover should clear the gate");
    let r = &rows[0];
    assert_eq!(r.emitter_id, mover);
    assert!(r.spread_m > 1000.0 && r.spread_m < 2000.0, "spread was {}", r.spread_m);
    assert!(r.span_s >= 599.0, "span was {}", r.span_s);
    assert_eq!(r.hits, 2);
    assert_eq!(r.points, 2);
}

/// A bounding-box diagonal (MIN/MAX corner-to-corner) over-reports spread for
/// 3+-point emitters versus the true max pairwise (convex-hull diameter)
/// distance. Seed a bent path where the bbox diagonal (~123 km) is
/// meaningfully larger than the true farthest pair (~114 km, A-C), and assert
/// the query reports the true value.
#[tokio::test]
async fn candidate_spread_is_true_max_pairwise_not_bbox() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let now = Utc::now();

    let bent = seed_emitter(&pool, "bent").await;
    // A
    insert_located(&pool, ds, session, bent, -84.60, 37.00, now).await;
    // B
    insert_located(&pool, ds, session, bent, -84.00, 37.00, now + Duration::minutes(5)).await;
    // C
    insert_located(&pool, ds, session, bent, -84.30, 38.00, now + Duration::minutes(10)).await;

    let filter = CoTravelFilter {
        time_from: None,
        time_to: None,
        min_distance_m: 402.336,
        min_time_s: 30.0,
    };
    let rows = CoTravelRepo::candidates(&pool, &filter).await.unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].points, 3);
    assert!(
        rows[0].spread_m > 108_000.0 && rows[0].spread_m < 118_000.0,
        "spread was {}",
        rows[0].spread_m
    );
}

#[tokio::test]
async fn candidate_excludes_ignored() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let now = Utc::now();

    let mover = seed_emitter(&pool, "mover").await;
    insert_located(&pool, ds, session, mover, -84.500, 37.700, now).await;
    insert_located(&pool, ds, session, mover, -84.483, 37.700, now + Duration::minutes(10)).await;

    CoTravelRepo::ignore(&pool, mover).await.unwrap();

    let filter = CoTravelFilter {
        time_from: None,
        time_to: None,
        min_distance_m: 402.336,
        min_time_s: 30.0,
    };
    let rows = CoTravelRepo::candidates(&pool, &filter).await.unwrap();
    assert!(rows.is_empty(), "ignored emitter must not appear");
}

#[tokio::test]
async fn candidate_time_window_filters() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let now = Utc::now();

    let mover = seed_emitter(&pool, "mover").await;
    insert_located(&pool, ds, session, mover, -84.500, 37.700, now).await;
    insert_located(&pool, ds, session, mover, -84.483, 37.700, now + Duration::minutes(10)).await;

    // Window ending before the second sighting leaves only one point -> no gate.
    let filter = CoTravelFilter {
        time_from: None,
        time_to: Some(now + Duration::minutes(1)),
        min_distance_m: 402.336,
        min_time_s: 30.0,
    };
    let rows = CoTravelRepo::candidates(&pool, &filter).await.unwrap();
    assert!(rows.is_empty());
}
