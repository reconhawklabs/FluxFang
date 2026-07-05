//! Round-trip tests for `AlertMethodRepo`.

mod common;

use common::fresh_pool;
use fluxfang_db::models::NewAlertMethod;
use fluxfang_db::repo::alert_method::AlertMethodRepo;
use sqlx::PgPool;
use uuid::Uuid;

async fn seed_email_method(pool: &PgPool) -> fluxfang_db::models::AlertMethod {
    AlertMethodRepo::insert(
        pool,
        NewAlertMethod {
            name: "Ops Email".to_string(),
            type_: "email".to_string(),
            enabled: true,
            config_encrypted: b"secret-bytes".to_vec(),
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn insert_and_get_alert_method_roundtrips() {
    let pool = fresh_pool().await;

    let inserted = seed_email_method(&pool).await;
    assert_eq!(inserted.name, "Ops Email");
    assert_eq!(inserted.type_, "email");
    assert!(inserted.enabled);
    assert_eq!(
        inserted.config_encrypted.as_deref(),
        Some(&b"secret-bytes"[..])
    );

    let got = AlertMethodRepo::get(&pool, inserted.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.id, inserted.id);
    assert_eq!(got.name, "Ops Email");
}

#[tokio::test]
async fn config_encrypted_bytea_roundtrips_exactly() {
    let pool = fresh_pool().await;

    let bytes: Vec<u8> = vec![0u8, 1, 2, 255, 254, 253, 0, 0, 42];
    let inserted = AlertMethodRepo::insert(
        &pool,
        NewAlertMethod {
            name: "Webhook".to_string(),
            type_: "webhook".to_string(),
            enabled: true,
            config_encrypted: bytes.clone(),
        },
    )
    .await
    .unwrap();

    let got = AlertMethodRepo::get(&pool, inserted.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.config_encrypted, Some(bytes));
}

#[tokio::test]
async fn insert_rejects_invalid_type_via_check_constraint() {
    let pool = fresh_pool().await;

    let bad = NewAlertMethod {
        name: "Bad".to_string(),
        type_: "carrier_pigeon".to_string(),
        enabled: true,
        config_encrypted: vec![],
    };

    let result = AlertMethodRepo::insert(&pool, bad).await;
    assert!(
        result.is_err(),
        "expected the type CHECK constraint to reject an invalid type"
    );
}

#[tokio::test]
async fn get_returns_none_for_unknown_id() {
    let pool = fresh_pool().await;
    let got = AlertMethodRepo::get(&pool, Uuid::new_v4()).await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn list_returns_all_methods() {
    let pool = fresh_pool().await;
    seed_email_method(&pool).await;
    seed_email_method(&pool).await;

    let all = AlertMethodRepo::list(&pool).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn update_replaces_name_enabled_and_config_encrypted() {
    let pool = fresh_pool().await;
    let m = seed_email_method(&pool).await;

    let updated = AlertMethodRepo::update(&pool, m.id, "Renamed", false, b"new-bytes".to_vec())
        .await
        .unwrap();

    assert_eq!(updated.id, m.id);
    assert_eq!(updated.name, "Renamed");
    assert!(!updated.enabled);
    assert_eq!(updated.config_encrypted.as_deref(), Some(&b"new-bytes"[..]));
}

#[tokio::test]
async fn delete_removes_alert_method() {
    let pool = fresh_pool().await;
    let m = seed_email_method(&pool).await;

    let deleted = AlertMethodRepo::delete(&pool, m.id).await.unwrap();
    assert!(deleted);
    assert!(AlertMethodRepo::get(&pool, m.id).await.unwrap().is_none());

    let deleted_again = AlertMethodRepo::delete(&pool, m.id).await.unwrap();
    assert!(!deleted_again);
}
