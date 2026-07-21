//! Round-trip tests for `AppConfigRepo`.

mod common;

use common::fresh_pool;
use fluxfang_db::{AppConfigRepo, NodeConfig, NodeRole, SensorConfig};

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
        AppConfigRepo::password_hash(&pool)
            .await
            .unwrap()
            .as_deref(),
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
        AppConfigRepo::password_hash(&pool)
            .await
            .unwrap()
            .as_deref(),
        Some("second-hash")
    );
}

#[tokio::test]
async fn complete_setup_first_call_returns_some_with_hash_and_settings() {
    let pool = fresh_pool().await;

    let node = NodeConfig {
        role: NodeRole::Standalone,
        node_sensor_id: "local".to_string(),
        sensor: None,
    };

    let config = AppConfigRepo::complete_setup(&pool, "argon2$setup-hash", &node)
        .await
        .unwrap()
        .expect("first complete_setup call should win and return Some");

    assert_eq!(config.password_hash.as_deref(), Some("argon2$setup-hash"));
    assert_eq!(AppConfigRepo::node_config(&pool).await.unwrap(), Some(node));
}

#[tokio::test]
async fn complete_setup_second_call_returns_none_and_does_not_overwrite() {
    let pool = fresh_pool().await;

    let first_node = NodeConfig {
        role: NodeRole::Sensor,
        node_sensor_id: "frontgate".to_string(),
        sensor: Some(SensorConfig {
            host: "base.example".to_string(),
            port: 9000,
            key: "a2V5".to_string(),
            cache_ttl_secs: 604_800,
        }),
    };
    AppConfigRepo::complete_setup(&pool, "first-hash", &first_node)
        .await
        .unwrap()
        .expect("first complete_setup call should win");

    let second_node = NodeConfig {
        role: NodeRole::Standalone,
        node_sensor_id: "local".to_string(),
        sensor: None,
    };
    let result = AppConfigRepo::complete_setup(&pool, "second-hash", &second_node)
        .await
        .unwrap();

    assert!(
        result.is_none(),
        "second complete_setup call should lose once setup already completed"
    );
    assert_eq!(
        AppConfigRepo::password_hash(&pool)
            .await
            .unwrap()
            .as_deref(),
        Some("first-hash"),
        "the losing call must not overwrite the already-set hash"
    );
    assert_eq!(
        AppConfigRepo::node_config(&pool).await.unwrap(),
        Some(first_node),
        "the losing call must not overwrite the already-set settings"
    );
}

#[tokio::test]
async fn node_config_returns_none_before_any_setup() {
    let pool = fresh_pool().await;

    let node = AppConfigRepo::node_config(&pool).await.unwrap();

    assert!(node.is_none());
}

#[tokio::test]
async fn node_config_returns_stored_config_after_complete_setup() {
    let pool = fresh_pool().await;

    let node = NodeConfig {
        role: NodeRole::Sensor,
        node_sensor_id: "backdoor".to_string(),
        sensor: Some(SensorConfig {
            host: "base.internal".to_string(),
            port: 9001,
            key: "a2V5Mg==".to_string(),
            cache_ttl_secs: 3600,
        }),
    };
    AppConfigRepo::complete_setup(&pool, "argon2$another-hash", &node)
        .await
        .unwrap()
        .expect("complete_setup should succeed on a fresh pool");

    assert_eq!(AppConfigRepo::node_config(&pool).await.unwrap(), Some(node));
}
