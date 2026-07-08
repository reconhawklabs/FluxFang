//! Round-trip tests for `EmitterAssociationRepo`.

mod common;

use common::fresh_pool;
use fluxfang_db::models::NewEmitter;
use fluxfang_db::{EmitterAssociationRepo, EmitterRepo};
use sqlx::PgPool;
use uuid::Uuid;

async fn mk_emitter(pool: &PgPool, name: &str) -> Uuid {
    let (e, _) = EmitterRepo::get_or_create_by_identity(
        pool,
        NewEmitter {
            name: name.to_string(),
            emitter_type: Some("tpms_sensor".to_string()),
            identity_key: Some(format!("tpms_sensor:{name}")),
            match_enabled: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    e.id
}

#[tokio::test]
async fn add_is_bidirectional_and_listable_from_both_sides() {
    let pool = fresh_pool().await;
    let a = mk_emitter(&pool, "a").await;
    let b = mk_emitter(&pool, "b").await;
    EmitterAssociationRepo::add(&pool, a, b, "manual", None)
        .await
        .unwrap();

    let from_a = EmitterAssociationRepo::list_for(&pool, a).await.unwrap();
    assert_eq!(from_a.len(), 1);
    assert_eq!(from_a[0].emitter.id, b);
    assert_eq!(from_a[0].source, "manual");

    let from_b = EmitterAssociationRepo::list_for(&pool, b).await.unwrap();
    assert_eq!(from_b.len(), 1);
    assert_eq!(from_b[0].emitter.id, a);
}

#[tokio::test]
async fn manual_upgrades_auto_but_auto_does_not_downgrade_manual() {
    let pool = fresh_pool().await;
    let a = mk_emitter(&pool, "a").await;
    let b = mk_emitter(&pool, "b").await;
    EmitterAssociationRepo::add(&pool, a, b, "auto", Some(0.9))
        .await
        .unwrap();
    EmitterAssociationRepo::add(&pool, a, b, "manual", None)
        .await
        .unwrap();
    assert_eq!(
        EmitterAssociationRepo::list_for(&pool, a).await.unwrap()[0].source,
        "manual"
    );
    // auto add must not downgrade the manual link
    EmitterAssociationRepo::add(&pool, a, b, "auto", Some(0.5))
        .await
        .unwrap();
    assert_eq!(
        EmitterAssociationRepo::list_for(&pool, a).await.unwrap()[0].source,
        "manual"
    );
}

#[tokio::test]
async fn remove_clears_both_directions() {
    let pool = fresh_pool().await;
    let a = mk_emitter(&pool, "a").await;
    let b = mk_emitter(&pool, "b").await;
    EmitterAssociationRepo::add(&pool, a, b, "manual", None)
        .await
        .unwrap();
    EmitterAssociationRepo::remove(&pool, a, b).await.unwrap();
    assert!(EmitterAssociationRepo::list_for(&pool, a)
        .await
        .unwrap()
        .is_empty());
    assert!(EmitterAssociationRepo::list_for(&pool, b)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn exists_reflects_state() {
    let pool = fresh_pool().await;
    let a = mk_emitter(&pool, "a").await;
    let b = mk_emitter(&pool, "b").await;
    assert!(!EmitterAssociationRepo::exists(&pool, a, b).await.unwrap());
    EmitterAssociationRepo::add(&pool, a, b, "auto", Some(0.9))
        .await
        .unwrap();
    assert!(EmitterAssociationRepo::exists(&pool, a, b).await.unwrap());
}
