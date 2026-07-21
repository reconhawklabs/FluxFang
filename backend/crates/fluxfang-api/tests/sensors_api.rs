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
    let s = SensorRepo::insert_pending(&pool, ds.id, "frontgate", "a2V5", "FP", Some("5.6.7.8")).await.unwrap();

    // list — key must NOT be present
    let resp = get_with_cookie(&app, "/api/sensors", &cookie).await;
    assert_status(&resp, axum::http::StatusCode::OK);
    let list = body_json(resp).await;
    assert_eq!(list[0]["sensor_id"], "frontgate");
    assert!(list[0].get("key").is_none(), "list must not leak the key");

    // approve with auto_group_emitters=false
    let resp = post_json_with_cookie(&app, &format!("/api/sensors/{}/approve", s.id), r#"{"auto_group_emitters":false}"#, &cookie).await;
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
    SensorRepo::insert_pending(&pool, ds.id, "frontgate", "k", "F", None).await.unwrap();
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
