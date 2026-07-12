//! Round-trip tests for `CoTravelRepo`'s ignore list.

mod common;

use common::fresh_pool;
use fluxfang_db::models::NewEmitter;
use fluxfang_db::{CoTravelRepo, EmitterRepo};
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
