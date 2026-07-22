mod common;
use std::sync::Arc;
use common::{assert_status, body_json, get_with_cookie, post_json, post_json_with_cookie, post_with_cookie, session_cookie, test_app_with_factory};
use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::{DataSourceRepo, EmissionRepo, NewDataSource, SensorRepo};

async fn login(app: &axum::Router) -> String {
    post_json(app, "/api/setup", r#"{"password":"pw123456","role":"standalone","node_sensor_id":"local"}"#).await;
    let resp = post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    session_cookie(&resp)
}

#[tokio::test]
async fn list_and_approve_and_rotate_sensor() {
    // `test_app_with_factory` returns the app AND its pool over ONE schema, so
    // rows inserted through `pool` are visible to the app's endpoints.
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    // (Insert a datasource + a pending sensor directly for the operator flow.)
    let ds = DataSourceRepo::insert(&pool, NewDataSource {
        kind:"sensor".into(), mode:"listener".into(), interface:None,
        config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":9000,"enrollment_window_secs":900}),
    }).await.unwrap();
    // A pending sensor stores only its fingerprint; the operator supplies the
    // key at approval, so we compute the matching fingerprint here.
    let key = fluxfang_sensor_proto::generate_key();
    let key_b64 = fluxfang_sensor_proto::encode_key(&key);
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key);
    let s = SensorRepo::insert_pending(&pool, ds.id, "frontgate", &fp, Some("5.6.7.8")).await.unwrap();

    // list — key must NOT be present
    let resp = get_with_cookie(&app, "/api/sensors", &cookie).await;
    assert_status(&resp, axum::http::StatusCode::OK);
    let list = body_json(resp).await;
    assert_eq!(list[0]["sensor_id"], "frontgate");
    assert!(list[0].get("key").is_none(), "list must not leak the key");

    // approve with auto_group_emitters=false — the operator-supplied key must
    // reproduce the stored fingerprint.
    let body = serde_json::json!({"auto_group_emitters": false, "key": key_b64}).to_string();
    let resp = post_json_with_cookie(&app, &format!("/api/sensors/{}/approve", s.id), &body, &cookie).await;
    assert_status(&resp, axum::http::StatusCode::OK);
    assert_eq!(body_json(resp).await["status"], "approved");

    // rotate returns a fresh key once
    let resp = post_with_cookie(&app, &format!("/api/sensors/{}/rotate", s.id), &cookie).await;
    assert_status(&resp, axum::http::StatusCode::OK);
    let rotated = body_json(resp).await;
    assert!(rotated["key"].as_str().unwrap().len() > 0);
}

#[tokio::test]
async fn list_sensors_includes_24h_emission_count() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = DataSourceRepo::insert(&pool, NewDataSource {
        kind:"sensor".into(), mode:"listener".into(), interface:None,
        config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":9000,"enrollment_window_secs":900}),
    }).await.unwrap();
    SensorRepo::insert_pending(&pool, ds.id, "frontgate", "F", None).await.unwrap();
    // two emissions tagged 'frontgate' in the last 24h
    let eds = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0")).await.unwrap();
    for _ in 0..2 {
        EmissionRepo::insert(&pool, fluxfang_db::models::NewEmission {
            data_source_id: Some(eds.id), emitter_id: None, session_id: None,
            observed_at: chrono::Utc::now(), signal_strength: None, location: None,
            location_quality: "none".into(), kind: "wifi".into(), payload: serde_json::json!({}),
            sensor_id: "frontgate".into(),
        }).await.unwrap();
    }
    let json = body_json(get_with_cookie(&app, "/api/sensors", &cookie).await).await;
    assert_eq!(json[0]["sensor_id"], "frontgate");
    assert_eq!(json[0]["emissions_24h"], 2);
}

#[tokio::test]
async fn approve_with_wrong_key_returns_400_and_stays_pending() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = DataSourceRepo::insert(&pool, NewDataSource {
        kind:"sensor".into(), mode:"listener".into(), interface:None,
        config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":9000,"enrollment_window_secs":900}),
    }).await.unwrap();

    // Sensor enrolled claiming the fingerprint of key_a.
    let key_a = fluxfang_sensor_proto::generate_key();
    let fp_a = fluxfang_sensor_proto::fingerprint("frontgate", &key_a);
    let s = SensorRepo::insert_pending(&pool, ds.id, "frontgate", &fp_a, None).await.unwrap();

    // Operator types the WRONG key (key_b) -> fingerprint mismatch -> 400.
    let key_b = fluxfang_sensor_proto::encode_key(&fluxfang_sensor_proto::generate_key());
    let body = serde_json::json!({"auto_group_emitters": true, "key": key_b}).to_string();
    let resp = post_json_with_cookie(&app, &format!("/api/sensors/{}/approve", s.id), &body, &cookie).await;
    assert_status(&resp, axum::http::StatusCode::BAD_REQUEST);

    // The sensor must remain pending with no key stored.
    let got = SensorRepo::get(&pool, s.id).await.unwrap().unwrap();
    assert_eq!(got.status, "pending", "a wrong-key approval must not approve the sensor");
    assert_eq!(got.key, "", "no key may be stored on a rejected approval");
    assert_eq!(got.fingerprint, fp_a, "fingerprint must be unchanged");
}

#[tokio::test]
async fn approve_with_correct_key_stores_the_key() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = DataSourceRepo::insert(&pool, NewDataSource {
        kind:"sensor".into(), mode:"listener".into(), interface:None,
        config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":9000,"enrollment_window_secs":900}),
    }).await.unwrap();

    let key = fluxfang_sensor_proto::generate_key();
    let key_b64 = fluxfang_sensor_proto::encode_key(&key);
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key);
    let s = SensorRepo::insert_pending(&pool, ds.id, "frontgate", &fp, None).await.unwrap();

    let body = serde_json::json!({"auto_group_emitters": true, "key": key_b64}).to_string();
    let resp = post_json_with_cookie(&app, &format!("/api/sensors/{}/approve", s.id), &body, &cookie).await;
    assert_status(&resp, axum::http::StatusCode::OK);
    assert_eq!(body_json(resp).await["status"], "approved");

    // The key is now stored and decodes to a usable 32-byte key.
    let got = SensorRepo::get(&pool, s.id).await.unwrap().unwrap();
    assert_eq!(got.status, "approved");
    assert!(!got.key.is_empty(), "approval must store the operator-supplied key");
    assert!(
        fluxfang_sensor_proto::decode_key(&got.key).is_ok(),
        "the stored key must decode to a valid 32-byte key"
    );
}
