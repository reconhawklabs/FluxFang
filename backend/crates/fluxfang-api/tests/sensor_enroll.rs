mod common;

use fluxfang_api::sensor_listener::SensorListenerManager;
use fluxfang_db::{DataSourceRepo, NewDataSource, SensorRepo};

async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

#[tokio::test]
async fn enroll_during_open_window_creates_pending_and_returns_fingerprint() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
    let ds = DataSourceRepo::insert(&pool, NewDataSource {
        kind: "sensor".to_string(), mode: "listener".to_string(), interface: None,
        config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port,"enrollment_window_secs":900}),
    }).await.unwrap();

    let mgr = SensorListenerManager::new(pool.clone());
    mgr.start(ds.id).await;

    let key = fluxfang_sensor_proto::encode_key(&fluxfang_sensor_proto::generate_key());
    let url = format!("http://127.0.0.1:{port}/sensor/enroll");
    let body = serde_json::json!({ "sensor_id": "frontgate", "key": key });

    // Window closed -> 403.
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "enroll must be refused when the window is closed"
    );

    // Open window -> pending + fingerprint echoed.
    mgr.open_enrollment_window(ds.id).await;
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let j: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(j["status"], "pending");
    assert!(j["fingerprint"].as_str().unwrap().contains('-'));

    let s = SensorRepo::get_by_sensor_id(&pool, ds.id, "frontgate")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.status, "pending");
    assert!(s.source_ip.is_some());

    mgr.stop(ds.id).await;
}

#[tokio::test]
async fn enroll_rejects_bad_slug_and_bad_key() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
    let ds = DataSourceRepo::insert(&pool, NewDataSource {
        kind: "sensor".to_string(), mode: "listener".to_string(), interface: None,
        config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port,"enrollment_window_secs":900}),
    }).await.unwrap();
    let mgr = SensorListenerManager::new(pool.clone());
    mgr.start(ds.id).await;
    mgr.open_enrollment_window(ds.id).await;
    let url = format!("http://127.0.0.1:{port}/sensor/enroll");

    let good_key = fluxfang_sensor_proto::encode_key(&fluxfang_sensor_proto::generate_key());
    // bad slug
    let r = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({"sensor_id":"front gate","key":good_key}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    // bad key (not base64/32 bytes)
    let r = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({"sensor_id":"frontgate","key":"not-a-key"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);

    mgr.stop(ds.id).await;
}
