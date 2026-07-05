//! Round-trip tests for `ZoneRepo`.

mod common;

use chrono::{Duration, Utc};
use common::{fresh_pool, seed_session, seed_wifi_source};
use fluxfang_db::models::{NewEmission, NewEmitter, NewEntity, NewZone, Zone};
use fluxfang_db::repo::zone::ZoneRepo;
use fluxfang_db::{EmissionRepo, EmitterRepo, EntityRepo};
use sqlx::PgPool;
use uuid::Uuid;

/// Zone center: roughly downtown San Francisco.
const CENTER: (f64, f64) = (-122.4194, 37.7749);
/// Same point as `CENTER` — 0m away, always inside any positive radius.
const INSIDE: (f64, f64) = (-122.4194, 37.7749);
/// Roughly Manhattan — thousands of km from `CENTER`, always outside.
const OUTSIDE: (f64, f64) = (-73.9857, 40.7484);
const RADIUS_M: f64 = 1000.0;

async fn seed_zone(pool: &PgPool) -> Zone {
    ZoneRepo::insert(
        pool,
        NewZone {
            name: "Test Zone".to_string(),
            center: CENTER,
            radius_m: RADIUS_M,
            notes: None,
        },
    )
    .await
    .unwrap()
}

async fn seed_emitter(pool: &PgPool, name: &str, entity_id: Option<Uuid>) -> Uuid {
    let e = EmitterRepo::insert(
        pool,
        NewEmitter {
            name: name.to_string(),
            type_: None,
            entity_id,
            match_criteria: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    e.id
}

#[allow(clippy::too_many_arguments)]
async fn insert_located_emission(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    emitter_id: Uuid,
    loc: (f64, f64),
    observed_at: chrono::DateTime<Utc>,
) {
    let new = NewEmission {
        emitter_id: Some(emitter_id),
        observed_at,
        location: Some(loc),
        ..NewEmission::wifi(
            ds,
            session,
            serde_json::json!({"bssid": "aa:bb:cc:dd:ee:ff"}),
        )
    };
    EmissionRepo::insert(pool, new).await.unwrap();
}

async fn insert_unlocated_emission(pool: &PgPool, ds: Uuid, session: Uuid, emitter_id: Uuid) {
    let new = NewEmission {
        emitter_id: Some(emitter_id),
        ..NewEmission::wifi(
            ds,
            session,
            serde_json::json!({"bssid": "aa:bb:cc:dd:ee:ff"}),
        )
    };
    EmissionRepo::insert(pool, new).await.unwrap();
}

#[tokio::test]
async fn insert_and_get_zone_roundtrips() {
    let pool = fresh_pool().await;

    let z = ZoneRepo::insert(
        &pool,
        NewZone {
            name: "Home".to_string(),
            center: CENTER,
            radius_m: RADIUS_M,
            notes: Some("front yard".to_string()),
        },
    )
    .await
    .unwrap();

    assert_eq!(z.name, "Home");
    assert!((z.lon - CENTER.0).abs() < 1e-9);
    assert!((z.lat - CENTER.1).abs() < 1e-9);
    assert_eq!(z.radius_m, RADIUS_M);
    assert_eq!(z.notes.as_deref(), Some("front yard"));

    let got = ZoneRepo::get(&pool, z.id).await.unwrap().unwrap();
    assert_eq!(got.id, z.id);
    assert_eq!(got.name, "Home");
}

#[tokio::test]
async fn get_returns_none_for_unknown_id() {
    let pool = fresh_pool().await;
    let got = ZoneRepo::get(&pool, Uuid::new_v4()).await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn list_returns_all_zones() {
    let pool = fresh_pool().await;
    seed_zone(&pool).await;
    seed_zone(&pool).await;

    let all = ZoneRepo::list(&pool).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn update_replaces_all_fields() {
    let pool = fresh_pool().await;
    let z = seed_zone(&pool).await;

    let updated = ZoneRepo::update(&pool, z.id, "Renamed Zone", OUTSIDE, 250.0, Some("moved"))
        .await
        .unwrap();

    assert_eq!(updated.id, z.id);
    assert_eq!(updated.name, "Renamed Zone");
    assert!((updated.lon - OUTSIDE.0).abs() < 1e-9);
    assert!((updated.lat - OUTSIDE.1).abs() < 1e-9);
    assert_eq!(updated.radius_m, 250.0);
    assert_eq!(updated.notes.as_deref(), Some("moved"));
}

#[tokio::test]
async fn delete_removes_zone() {
    let pool = fresh_pool().await;
    let z = seed_zone(&pool).await;

    let deleted = ZoneRepo::delete(&pool, z.id).await.unwrap();
    assert!(deleted);
    assert!(ZoneRepo::get(&pool, z.id).await.unwrap().is_none());

    let deleted_again = ZoneRepo::delete(&pool, z.id).await.unwrap();
    assert!(!deleted_again);
}

#[tokio::test]
async fn subjects_in_zone_includes_emitter_whose_latest_located_emission_is_inside() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let zone = seed_zone(&pool).await;

    let emitter = seed_emitter(&pool, "inside-emitter", None).await;
    insert_located_emission(&pool, ds, session, emitter, INSIDE, Utc::now()).await;

    let subjects = ZoneRepo::subjects_in_zone(&pool, zone.id).await.unwrap();
    assert_eq!(subjects.emitters.len(), 1);
    assert_eq!(subjects.emitters[0].id, emitter);
}

#[tokio::test]
async fn subjects_in_zone_excludes_emitter_whose_latest_located_emission_is_outside() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let zone = seed_zone(&pool).await;

    let emitter = seed_emitter(&pool, "outside-emitter", None).await;
    insert_located_emission(&pool, ds, session, emitter, OUTSIDE, Utc::now()).await;

    let subjects = ZoneRepo::subjects_in_zone(&pool, zone.id).await.unwrap();
    assert!(subjects.emitters.iter().all(|e| e.id != emitter));
}

#[tokio::test]
async fn subjects_in_zone_uses_most_recent_located_emission_not_oldest() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let zone = seed_zone(&pool).await;

    let emitter = seed_emitter(&pool, "moved-emitter", None).await;
    let now = Utc::now();
    // Older observation: outside. Newer observation: inside. The emitter
    // must still be reported as "in zone" — proves membership is judged by
    // the MOST RECENT located emission, not by "any" or "oldest".
    insert_located_emission(
        &pool,
        ds,
        session,
        emitter,
        OUTSIDE,
        now - Duration::hours(1),
    )
    .await;
    insert_located_emission(&pool, ds, session, emitter, INSIDE, now).await;

    let subjects = ZoneRepo::subjects_in_zone(&pool, zone.id).await.unwrap();
    assert!(
        subjects.emitters.iter().any(|e| e.id == emitter),
        "emitter's most recent location is inside the zone, so it should count as in-zone \
         even though an older location was outside"
    );
}

#[tokio::test]
async fn subjects_in_zone_excludes_emitter_whose_latest_is_outside_even_if_older_was_inside() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let zone = seed_zone(&pool).await;

    let emitter = seed_emitter(&pool, "left-emitter", None).await;
    let now = Utc::now();
    insert_located_emission(
        &pool,
        ds,
        session,
        emitter,
        INSIDE,
        now - Duration::hours(1),
    )
    .await;
    insert_located_emission(&pool, ds, session, emitter, OUTSIDE, now).await;

    let subjects = ZoneRepo::subjects_in_zone(&pool, zone.id).await.unwrap();
    assert!(
        subjects.emitters.iter().all(|e| e.id != emitter),
        "emitter's most recent location is outside, so it should not count as in-zone \
         even though an older location was inside"
    );
}

#[tokio::test]
async fn subjects_in_zone_excludes_emitter_with_no_located_emissions() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let zone = seed_zone(&pool).await;

    let emitter = seed_emitter(&pool, "unlocated-emitter", None).await;
    insert_unlocated_emission(&pool, ds, session, emitter).await;

    let subjects = ZoneRepo::subjects_in_zone(&pool, zone.id).await.unwrap();
    assert!(subjects.emitters.iter().all(|e| e.id != emitter));
}

#[tokio::test]
async fn subjects_in_zone_includes_entity_iff_one_of_its_emitters_is_in() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let zone = seed_zone(&pool).await;

    let entity_in = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "In Entity".to_string(),
            notes: None,
        },
    )
    .await
    .unwrap();
    let entity_out = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Out Entity".to_string(),
            notes: None,
        },
    )
    .await
    .unwrap();

    // entity_in has two emitters: one inside, one outside -> entity counts
    // as in-zone because at least one emitter is in.
    let emitter_in = seed_emitter(&pool, "e-in", Some(entity_in.id)).await;
    let emitter_also = seed_emitter(&pool, "e-also-out", Some(entity_in.id)).await;
    insert_located_emission(&pool, ds, session, emitter_in, INSIDE, Utc::now()).await;
    insert_located_emission(&pool, ds, session, emitter_also, OUTSIDE, Utc::now()).await;

    // entity_out has only an outside emitter -> not in-zone.
    let emitter_out = seed_emitter(&pool, "e-out", Some(entity_out.id)).await;
    insert_located_emission(&pool, ds, session, emitter_out, OUTSIDE, Utc::now()).await;

    let subjects = ZoneRepo::subjects_in_zone(&pool, zone.id).await.unwrap();
    assert!(subjects.entities.iter().any(|e| e.id == entity_in.id));
    assert!(subjects.entities.iter().all(|e| e.id != entity_out.id));
    // No duplicate entity rows even though entity_in has an in-zone emitter
    // plus an out-of-zone one.
    assert_eq!(
        subjects
            .entities
            .iter()
            .filter(|e| e.id == entity_in.id)
            .count(),
        1
    );
}
