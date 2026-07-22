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

#[tokio::test]
async fn set_node_config_overwrites_settings() {
    let pool = common::fresh_pool().await;
    // A row must exist first (settings starts '{}'); complete_setup creates it.
    let node0 = fluxfang_db::node_config::NodeConfig {
        role: fluxfang_db::NodeRole::Standalone,
        node_sensor_id: "local".into(),
        sensor: None,
    };
    fluxfang_db::AppConfigRepo::complete_setup(&pool, "hash", &node0)
        .await
        .unwrap();

    let node1 = fluxfang_db::node_config::NodeConfig {
        role: fluxfang_db::NodeRole::Sensor,
        node_sensor_id: "frontgate".into(),
        sensor: Some(fluxfang_db::node_config::SensorConfig {
            host: "base".into(),
            port: 9000,
            key: "a2V5".into(),
            cache_ttl_secs: 3600,
        }),
    };
    fluxfang_db::AppConfigRepo::set_node_config(&pool, &node1)
        .await
        .unwrap();
    let got = fluxfang_db::AppConfigRepo::node_config(&pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, node1);
    // password_hash must be UNTOUCHED by a settings update.
    assert_eq!(
        fluxfang_db::AppConfigRepo::password_hash(&pool)
            .await
            .unwrap()
            .as_deref(),
        Some("hash")
    );
}

/// Regression for the legacy-DB upgrade bug: an install that completed setup
/// before the node-role feature has `settings` with no `role`, so
/// `node_config` returns `None` and `GET /api/config` 404s. Migration 0017's
/// backfill (identical SQL below) marks such a row Standalone/local without
/// touching the password hash, so the upgraded app loads.
#[tokio::test]
async fn backfill_node_role_migration_sql_heals_a_legacy_row() {
    let pool = fresh_pool().await;

    // Simulate a pre-feature install: password set, settings left at default
    // '{}' (no role) — the state `complete_setup` never produces but older
    // `set_password_hash`-based setup did.
    AppConfigRepo::set_password_hash(&pool, "legacy-hash").await.unwrap();
    assert!(
        AppConfigRepo::node_config(&pool).await.unwrap().is_none(),
        "a role-less settings row should read as no node config (the bug)"
    );

    // Apply the exact backfill statement migration 0017 runs at startup.
    sqlx::query(
        "UPDATE app_config \
         SET settings = settings || '{\"role\": \"standalone\", \"node_sensor_id\": \"local\"}'::jsonb \
         WHERE password_hash IS NOT NULL AND NOT (settings ? 'role')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let node = AppConfigRepo::node_config(&pool).await.unwrap().unwrap();
    assert_eq!(node.role, NodeRole::Standalone);
    assert_eq!(node.node_sensor_id, "local");
    assert!(node.sensor.is_none());
    // The backfill must not disturb the admin password.
    assert_eq!(
        AppConfigRepo::password_hash(&pool).await.unwrap().as_deref(),
        Some("legacy-hash")
    );
}
