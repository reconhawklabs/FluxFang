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

    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;

    // The key never leaves the sensor: it transmits only the one-way fingerprint.
    let key = fluxfang_sensor_proto::generate_key();
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key);
    let url = format!("http://127.0.0.1:{port}/sensor/enroll");
    let body = serde_json::json!({ "sensor_id": "frontgate", "fingerprint": fp });

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
    assert_eq!(s.key, "", "a freshly enrolled sensor has no key stored");
    assert_eq!(s.fingerprint, fp, "the claimed fingerprint is stored");
    assert!(s.source_ip.is_some());

    mgr.stop(ds.id).await;
}

#[tokio::test]
async fn enroll_rejects_bad_slug_and_bad_fingerprint() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
    let ds = DataSourceRepo::insert(&pool, NewDataSource {
        kind: "sensor".to_string(), mode: "listener".to_string(), interface: None,
        config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port,"enrollment_window_secs":900}),
    }).await.unwrap();
    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;
    mgr.open_enrollment_window(ds.id).await;
    let url = format!("http://127.0.0.1:{port}/sensor/enroll");

    let good_fp = fluxfang_sensor_proto::fingerprint(
        "frontgate",
        &fluxfang_sensor_proto::generate_key(),
    );
    // bad slug
    let r = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({"sensor_id":"front gate","fingerprint":good_fp}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    // malformed fingerprint (not four uppercase hex bytes joined by dashes)
    let r = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({"sensor_id":"frontgate","fingerprint":"zz-zz"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);

    mgr.stop(ds.id).await;
}

/// Common setup for the security-refusal-branch tests below: a fresh sensor
/// datasource with its listener started and enrollment window open.
async fn setup_open_listener() -> (
    sqlx::PgPool,
    SensorListenerManager,
    fluxfang_db::models::DataSource,
    String,
) {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
    let ds = DataSourceRepo::insert(&pool, NewDataSource {
        kind: "sensor".to_string(), mode: "listener".to_string(), interface: None,
        config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port,"enrollment_window_secs":900}),
    }).await.unwrap();
    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;
    mgr.open_enrollment_window(ds.id).await;
    let url = format!("http://127.0.0.1:{port}/sensor/enroll");
    (pool, mgr, ds, url)
}

#[tokio::test]
async fn enroll_approved_with_different_key_is_refused_409() {
    let (pool, mgr, ds, url) = setup_open_listener().await;

    let key_a = fluxfang_sensor_proto::generate_key();
    let key_b = fluxfang_sensor_proto::generate_key();
    let fp_a = fluxfang_sensor_proto::fingerprint("frontgate", &key_a);
    let fp_b = fluxfang_sensor_proto::fingerprint("frontgate", &key_b);
    let sensor = SensorRepo::insert_pending(&pool, ds.id, "frontgate", &fp_a, None)
        .await
        .unwrap();
    SensorRepo::set_status(&pool, sensor.id, "approved", true)
        .await
        .unwrap();

    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "sensor_id": "frontgate", "fingerprint": fp_b }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        409,
        "an approved sensor re-enrolling with a different fingerprint must be refused"
    );

    mgr.stop(ds.id).await;
}

#[tokio::test]
async fn enroll_approved_with_same_key_returns_200_approved() {
    let (pool, mgr, ds, url) = setup_open_listener().await;

    let key_a = fluxfang_sensor_proto::generate_key();
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key_a);
    let sensor = SensorRepo::insert_pending(&pool, ds.id, "frontgate", &fp, None)
        .await
        .unwrap();
    SensorRepo::set_status(&pool, sensor.id, "approved", true)
        .await
        .unwrap();

    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "sensor_id": "frontgate", "fingerprint": fp }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let j: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(j["status"], "approved");

    mgr.stop(ds.id).await;
}

#[tokio::test]
async fn enroll_revoked_is_refused_403_and_not_resurrected() {
    let (pool, mgr, ds, url) = setup_open_listener().await;

    let key_a = fluxfang_sensor_proto::generate_key();
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key_a);
    let sensor = SensorRepo::insert_pending(&pool, ds.id, "frontgate", &fp, None)
        .await
        .unwrap();
    SensorRepo::set_status(&pool, sensor.id, "revoked", false)
        .await
        .unwrap();

    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "sensor_id": "frontgate", "fingerprint": fp }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    let s = SensorRepo::get_by_sensor_id(&pool, ds.id, "frontgate")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        s.status, "revoked",
        "a revoked sensor must never be resurrected to pending"
    );

    mgr.stop(ds.id).await;
}

#[tokio::test]
async fn enroll_rejected_is_refused_403_and_not_resurrected() {
    let (pool, mgr, ds, url) = setup_open_listener().await;

    let key_a = fluxfang_sensor_proto::generate_key();
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key_a);
    let sensor = SensorRepo::insert_pending(&pool, ds.id, "frontgate", &fp, None)
        .await
        .unwrap();
    SensorRepo::set_status(&pool, sensor.id, "rejected", false)
        .await
        .unwrap();

    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "sensor_id": "frontgate", "fingerprint": fp }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    let s = SensorRepo::get_by_sensor_id(&pool, ds.id, "frontgate")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        s.status, "rejected",
        "a rejected sensor must never be resurrected to pending"
    );

    mgr.stop(ds.id).await;
}

#[tokio::test]
async fn enroll_pending_reenroll_updates_fingerprint() {
    let (pool, mgr, ds, url) = setup_open_listener().await;

    let key_a = fluxfang_sensor_proto::generate_key();
    let key_b = fluxfang_sensor_proto::generate_key();
    let fp_a = fluxfang_sensor_proto::fingerprint("frontgate", &key_a);
    let fp_b = fluxfang_sensor_proto::fingerprint("frontgate", &key_b);

    // First enroll creates a pending sensor claiming fp_a.
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "sensor_id": "frontgate", "fingerprint": fp_a }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // Re-enroll with fp_b while still pending -> fingerprint gets updated.
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "sensor_id": "frontgate", "fingerprint": fp_b }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let j: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(j["status"], "pending");

    let s = SensorRepo::get_by_sensor_id(&pool, ds.id, "frontgate")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.status, "pending");
    assert_eq!(s.key, "", "the key is never transmitted, so it stays empty");
    assert_eq!(
        s.fingerprint, fp_b,
        "pending re-enrollment must update the stored fingerprint"
    );

    mgr.stop(ds.id).await;
}
