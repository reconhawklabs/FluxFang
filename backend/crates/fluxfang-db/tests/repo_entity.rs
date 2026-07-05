//! Round-trip tests for `EntityRepo`.

mod common;

use chrono::{Duration, Utc};
use common::{fresh_pool, seed_session, seed_wifi_source};
use fluxfang_db::models::{NewEmission, NewEmitter, NewEntity};
use fluxfang_db::{EmissionRepo, EmitterRepo, EntityRepo};
use uuid::Uuid;

#[tokio::test]
async fn insert_and_get_entity_roundtrips() {
    let pool = fresh_pool().await;

    let e = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Bob's phone".to_string(),
            notes: Some("seen at the office".to_string()),
        },
    )
    .await
    .unwrap();

    assert_eq!(e.name, "Bob's phone");
    assert_eq!(e.notes.as_deref(), Some("seen at the office"));

    let got = EntityRepo::get(&pool, e.id).await.unwrap().unwrap();
    assert_eq!(got.id, e.id);
    assert_eq!(got.name, "Bob's phone");
}

#[tokio::test]
async fn get_returns_none_for_unknown_id() {
    let pool = fresh_pool().await;
    let got = EntityRepo::get(&pool, Uuid::new_v4()).await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn list_returns_all_entities() {
    let pool = fresh_pool().await;
    EntityRepo::insert(
        &pool,
        NewEntity {
            name: "A".to_string(),
            notes: None,
        },
    )
    .await
    .unwrap();
    EntityRepo::insert(
        &pool,
        NewEntity {
            name: "B".to_string(),
            notes: None,
        },
    )
    .await
    .unwrap();

    let all = EntityRepo::list(&pool).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn last_seen_is_none_when_entity_has_no_emitters_or_emissions() {
    let pool = fresh_pool().await;
    let entity = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Lonely".to_string(),
            notes: None,
        },
    )
    .await
    .unwrap();

    let last_seen = EntityRepo::last_seen(&pool, entity.id).await.unwrap();
    assert!(last_seen.is_none());
}

#[tokio::test]
async fn last_seen_returns_max_observed_at_across_entitys_emitters_emissions() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let entity = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Group".to_string(),
            notes: None,
        },
    )
    .await
    .unwrap();

    let emitter_a = EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "AP-A".to_string(),
            type_: Some("Access Point".to_string()),
            entity_id: Some(entity.id),
            match_criteria: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    let emitter_b = EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "AP-B".to_string(),
            type_: Some("Access Point".to_string()),
            entity_id: Some(entity.id),
            match_criteria: serde_json::json!({}),
        },
    )
    .await
    .unwrap();

    let now = Utc::now();
    let earlier = NewEmission {
        emitter_id: Some(emitter_a.id),
        observed_at: now - Duration::hours(1),
        ..NewEmission::wifi(
            ds,
            session,
            serde_json::json!({"bssid": "aa:aa:aa:aa:aa:aa"}),
        )
    };
    let later = NewEmission {
        emitter_id: Some(emitter_b.id),
        observed_at: now,
        ..NewEmission::wifi(
            ds,
            session,
            serde_json::json!({"bssid": "bb:bb:bb:bb:bb:bb"}),
        )
    };
    EmissionRepo::insert(&pool, earlier).await.unwrap();
    EmissionRepo::insert(&pool, later).await.unwrap();

    let last_seen = EntityRepo::last_seen(&pool, entity.id)
        .await
        .unwrap()
        .unwrap();
    // Compare at second resolution to sidestep any driver-level sub-µs drift.
    assert_eq!(last_seen.timestamp(), now.timestamp());
}
