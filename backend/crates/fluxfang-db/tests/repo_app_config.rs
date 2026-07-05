//! Round-trip tests for `AppConfigRepo`.

mod common;

use common::fresh_pool;
use fluxfang_db::AppConfigRepo;

#[tokio::test]
async fn get_returns_none_before_any_password_is_set() {
    let pool = fresh_pool().await;

    let config = AppConfigRepo::get(&pool).await.unwrap();

    assert!(config.is_none());
    assert_eq!(AppConfigRepo::password_hash(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn set_password_hash_creates_the_singleton_row() {
    let pool = fresh_pool().await;

    let config = AppConfigRepo::set_password_hash(&pool, "argon2$fake-hash-1")
        .await
        .unwrap();

    assert_eq!(config.password_hash.as_deref(), Some("argon2$fake-hash-1"));
    assert_eq!(
        AppConfigRepo::password_hash(&pool).await.unwrap().as_deref(),
        Some("argon2$fake-hash-1")
    );
}

#[tokio::test]
async fn set_password_hash_twice_updates_in_place_not_a_second_row() {
    let pool = fresh_pool().await;

    AppConfigRepo::set_password_hash(&pool, "first-hash")
        .await
        .unwrap();
    AppConfigRepo::set_password_hash(&pool, "second-hash")
        .await
        .unwrap();

    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM app_config")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "expected exactly one app_config row, got {count}");

    assert_eq!(
        AppConfigRepo::password_hash(&pool).await.unwrap().as_deref(),
        Some("second-hash")
    );
}
