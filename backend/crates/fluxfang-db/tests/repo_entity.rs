//! Round-trip tests for `EntityRepo`.

mod common;

use chrono::{Duration, Utc};
use common::{fresh_pool, seed_session, seed_wifi_source};
use fluxfang_db::models::{NewEmission, NewEmitter, NewEntity};
use fluxfang_db::repo::entity::EntityListFilter;
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
            ..Default::default()
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
            ..Default::default()
        },
    )
    .await
    .unwrap();
    EntityRepo::insert(
        &pool,
        NewEntity {
            name: "B".to_string(),
            notes: None,
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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

// ---------------------------------------------------------------------
// Phase 1b: EntityRepo::query — search (name/notes substring) + pagination.
// ---------------------------------------------------------------------

#[tokio::test]
async fn query_with_no_filter_returns_everything_and_correct_total() {
    let pool = fresh_pool().await;
    EntityRepo::insert(
        &pool,
        NewEntity {
            name: "A".to_string(),
            notes: None,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    EntityRepo::insert(
        &pool,
        NewEntity {
            name: "B".to_string(),
            notes: None,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (rows, total) = EntityRepo::query(&pool, EntityListFilter::default())
        .await
        .unwrap();
    assert_eq!(total, 2);
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn query_search_matches_by_name_case_insensitively() {
    let pool = fresh_pool().await;
    EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Bob's Phone".to_string(),
            notes: None,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Alice's Laptop".to_string(),
            notes: None,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (rows, total) = EntityRepo::query(
        &pool,
        EntityListFilter {
            search: Some("bob".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(total, 1, "rows: {rows:?}");
    assert_eq!(rows[0].name, "Bob's Phone");
}

#[tokio::test]
async fn query_search_matches_by_notes_substring() {
    let pool = fresh_pool().await;
    let matching = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Device 1".to_string(),
            notes: Some("seen loitering near the entrance".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Device 2".to_string(),
            notes: Some("nothing notable".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (rows, total) = EntityRepo::query(
        &pool,
        EntityListFilter {
            search: Some("loitering".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(total, 1, "rows: {rows:?}");
    assert_eq!(rows[0].id, matching.id);
}

#[tokio::test]
async fn query_paginates_with_correct_total_ignoring_limit_offset() {
    let pool = fresh_pool().await;
    for name in ["A", "B", "C", "D", "E"] {
        EntityRepo::insert(
            &pool,
            NewEntity {
                name: name.to_string(),
                notes: None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }

    let (page1, total1) = EntityRepo::query(
        &pool,
        EntityListFilter {
            limit: 2,
            offset: 0,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(total1, 5);
    assert_eq!(page1.len(), 2);
    assert_eq!(page1[0].name, "A");
    assert_eq!(page1[1].name, "B");

    let (page2, total2) = EntityRepo::query(
        &pool,
        EntityListFilter {
            limit: 2,
            offset: 4,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(total2, 5);
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0].name, "E");
}

// ---------------------------------------------------------------------
// Phase 1c: EntityRepo::{delete_bulk, delete_all} + the emitter.entity_id
// SET NULL cascade.
// ---------------------------------------------------------------------

async fn seed_entity(pool: &sqlx::PgPool, name: &str) -> Uuid {
    EntityRepo::insert(
        pool,
        NewEntity {
            name: name.to_string(),
            notes: None,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn delete_bulk_removes_only_the_listed_ids() {
    let pool = fresh_pool().await;
    let a = seed_entity(&pool, "A").await;
    let b = seed_entity(&pool, "B").await;
    let keep = seed_entity(&pool, "Keep").await;

    let deleted = EntityRepo::delete_bulk(&pool, &[a, b]).await.unwrap();
    assert_eq!(deleted, 2);

    assert!(EntityRepo::get(&pool, a).await.unwrap().is_none());
    assert!(EntityRepo::get(&pool, b).await.unwrap().is_none());
    assert!(
        EntityRepo::get(&pool, keep).await.unwrap().is_some(),
        "the entity not in the ids list must survive"
    );
}

#[tokio::test]
async fn delete_bulk_with_empty_ids_deletes_nothing() {
    let pool = fresh_pool().await;
    let survivor = seed_entity(&pool, "Survivor").await;

    let deleted = EntityRepo::delete_bulk(&pool, &[]).await.unwrap();
    assert_eq!(deleted, 0);
    assert!(EntityRepo::get(&pool, survivor).await.unwrap().is_some());
}

/// Deleting an entity must SET NULL its emitters' `entity_id`, not remove
/// them — same guarantee `EntityRepo::delete`'s doc comment already
/// documents for the single-row path.
#[tokio::test]
async fn delete_bulk_nulls_entity_id_on_the_entitys_emitters() {
    let pool = fresh_pool().await;
    let entity = seed_entity(&pool, "Group").await;
    let emitter = EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "AP".to_string(),
            type_: None,
            entity_id: Some(entity),
            match_criteria: serde_json::json!({}),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let deleted = EntityRepo::delete_bulk(&pool, &[entity]).await.unwrap();
    assert_eq!(deleted, 1);

    let reloaded = EmitterRepo::get(&pool, emitter.id).await.unwrap().unwrap();
    assert_eq!(
        reloaded.entity_id, None,
        "emitter must survive its entity's deletion, just detached"
    );
}

#[tokio::test]
async fn delete_all_empties_the_table() {
    let pool = fresh_pool().await;
    seed_entity(&pool, "A").await;
    seed_entity(&pool, "B").await;
    seed_entity(&pool, "C").await;

    let deleted = EntityRepo::delete_all(&pool).await.unwrap();
    assert_eq!(deleted, 3);

    let all = EntityRepo::list(&pool).await.unwrap();
    assert!(all.is_empty());
}

/// `delete_all` also SET NULLs every emitter's `entity_id`, same as
/// deleting one entity at a time would.
#[tokio::test]
async fn delete_all_nulls_entity_id_on_survivors_emitters() {
    let pool = fresh_pool().await;
    let entity = seed_entity(&pool, "Group").await;
    let emitter = EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "AP".to_string(),
            type_: None,
            entity_id: Some(entity),
            match_criteria: serde_json::json!({}),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    EntityRepo::delete_all(&pool).await.unwrap();

    let reloaded = EmitterRepo::get(&pool, emitter.id).await.unwrap().unwrap();
    assert_eq!(reloaded.entity_id, None);
}
