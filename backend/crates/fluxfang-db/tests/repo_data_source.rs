//! Round-trip tests for `DataSourceRepo`.

mod common;

use common::fresh_pool;
use fluxfang_db::models::NewDataSource;
use fluxfang_db::DataSourceRepo;

#[tokio::test]
async fn insert_and_get_wifi_source_roundtrips() {
    let pool = fresh_pool().await;

    let inserted = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .unwrap();
    assert_eq!(inserted.kind, "wifi");
    assert_eq!(inserted.mode, "monitor");
    assert_eq!(inserted.interface.as_deref(), Some("wlan0"));
    assert_eq!(inserted.status, "stopped");

    let got = DataSourceRepo::get(&pool, inserted.id).await.unwrap();
    assert_eq!(got.unwrap().id, inserted.id);
}

#[tokio::test]
async fn insert_gps_gpsd_source_satisfies_kind_mode_check_constraint() {
    let pool = fresh_pool().await;

    let inserted = DataSourceRepo::insert(&pool, NewDataSource::gps_gpsd())
        .await
        .unwrap();
    assert_eq!(inserted.kind, "gps");
    assert_eq!(inserted.mode, "gpsd");
    assert_eq!(inserted.interface, None);
}

#[tokio::test]
async fn insert_rejects_mismatched_kind_and_mode() {
    let pool = fresh_pool().await;

    let bad = NewDataSource {
        kind: "wifi".to_string(),
        mode: "gpsd".to_string(),
        interface: None,
        config: serde_json::json!({}),
    };

    let result = DataSourceRepo::insert(&pool, bad).await;
    assert!(
        result.is_err(),
        "expected the kind/mode CHECK constraint to reject wifi+gpsd"
    );
}

#[tokio::test]
async fn get_returns_none_for_unknown_id() {
    let pool = fresh_pool().await;
    let got = DataSourceRepo::get(&pool, uuid::Uuid::new_v4())
        .await
        .unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn list_returns_every_inserted_source() {
    let pool = fresh_pool().await;

    DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .unwrap();
    DataSourceRepo::insert(&pool, NewDataSource::gps_gpsd())
        .await
        .unwrap();

    let all = DataSourceRepo::list(&pool).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn update_changes_config_mode_and_interface() {
    let pool = fresh_pool().await;
    let inserted = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .unwrap();

    let updated = DataSourceRepo::update(
        &pool,
        inserted.id,
        serde_json::json!({"channel": 6}),
        "monitor",
        Some("wlan1"),
    )
    .await
    .unwrap();

    assert_eq!(updated.interface.as_deref(), Some("wlan1"));
    assert_eq!(updated.config["channel"], 6);
    assert_eq!(updated.mode, "monitor");
}

#[tokio::test]
async fn set_status_updates_status_and_last_error() {
    let pool = fresh_pool().await;
    let inserted = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .unwrap();

    let errored = DataSourceRepo::set_status(&pool, inserted.id, "error", Some("device busy"))
        .await
        .unwrap();
    assert_eq!(errored.status, "error");
    assert_eq!(errored.last_error.as_deref(), Some("device busy"));

    let running = DataSourceRepo::set_status(&pool, inserted.id, "running", None)
        .await
        .unwrap();
    assert_eq!(running.status, "running");
    assert_eq!(running.last_error, None);
}

#[tokio::test]
async fn delete_removes_the_row() {
    let pool = fresh_pool().await;
    let inserted = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .unwrap();

    let deleted = DataSourceRepo::delete(&pool, inserted.id).await.unwrap();
    assert!(deleted);

    let got = DataSourceRepo::get(&pool, inserted.id).await.unwrap();
    assert!(got.is_none());

    let deleted_again = DataSourceRepo::delete(&pool, inserted.id).await.unwrap();
    assert!(!deleted_again);
}
