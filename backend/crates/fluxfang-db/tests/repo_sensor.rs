mod common;

use fluxfang_db::{DataSourceRepo, NewDataSource, SensorRepo};

async fn a_sensor_datasource(pool: &sqlx::PgPool) -> uuid::Uuid {
    DataSourceRepo::insert(
        pool,
        NewDataSource {
            kind: "sensor".to_string(),
            mode: "listener".to_string(),
            interface: None,
            config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":9000,"enrollment_window_secs":900}),
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn insert_pending_then_get_and_list() {
    let pool = common::fresh_pool().await;
    let ds = a_sensor_datasource(&pool).await;

    let s = SensorRepo::insert_pending(
        &pool,
        ds,
        "frontgate",
        "a2V5",
        "4F-A2-09-EE",
        Some("5.6.7.8"),
    )
    .await
    .unwrap();
    assert_eq!(s.status, "pending");
    assert_eq!(s.sensor_id, "frontgate");
    assert!(s.auto_group_emitters, "default auto_group_emitters is true");

    let got = SensorRepo::get_by_sensor_id(&pool, ds, "frontgate")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.id, s.id);
    assert_eq!(SensorRepo::list(&pool).await.unwrap().len(), 1);
}

#[tokio::test]
async fn approve_sets_status_and_auto_group_and_timestamp() {
    let pool = common::fresh_pool().await;
    let ds = a_sensor_datasource(&pool).await;
    let s = SensorRepo::insert_pending(&pool, ds, "frontgate", "a2V5", "FP", None)
        .await
        .unwrap();

    SensorRepo::set_auto_group(&pool, s.id, false)
        .await
        .unwrap();
    let approved = SensorRepo::set_status(&pool, s.id, "approved", true)
        .await
        .unwrap();
    assert_eq!(approved.status, "approved");
    assert!(approved.approved_at.is_some());
    assert!(!approved.auto_group_emitters);
}

#[tokio::test]
async fn rotate_key_updates_key_and_fingerprint() {
    let pool = common::fresh_pool().await;
    let ds = a_sensor_datasource(&pool).await;
    let s = SensorRepo::insert_pending(&pool, ds, "frontgate", "oldkey", "OLD", None)
        .await
        .unwrap();
    let rotated = SensorRepo::set_key(&pool, s.id, "newkey", "NEW")
        .await
        .unwrap();
    assert_eq!(rotated.key, "newkey");
    assert_eq!(rotated.fingerprint, "NEW");
}

#[tokio::test]
async fn unique_sensor_id_per_datasource() {
    let pool = common::fresh_pool().await;
    let ds = a_sensor_datasource(&pool).await;
    SensorRepo::insert_pending(&pool, ds, "frontgate", "k", "F", None)
        .await
        .unwrap();
    let dup = SensorRepo::insert_pending(&pool, ds, "frontgate", "k2", "F2", None).await;
    assert!(
        dup.is_err(),
        "duplicate (data_source_id, sensor_id) must be rejected"
    );
}
