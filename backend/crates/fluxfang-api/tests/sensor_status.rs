mod common;
use common::{assert_status, body_json, get_with_cookie, post_json, session_cookie, test_app_with_factory};
use std::sync::Arc;
use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::{CachedEmissionRepo, models::NewCachedEmission};

async fn login(app: &axum::Router) -> String {
    post_json(app, "/api/setup", r#"{"password":"pw123456","role":"standalone","node_sensor_id":"local"}"#).await;
    session_cookie(&post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await)
}

#[tokio::test]
async fn sensor_status_and_cached_emissions() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    CachedEmissionRepo::insert(&pool, NewCachedEmission {
        kind:"wifi".into(), signal_strength:Some(-40), lat:Some(1.5), lon:Some(2.5),
        observed_at: chrono::Utc::now(), payload: serde_json::json!({}), data_source_id: None,
    }).await.unwrap();

    let resp = get_with_cookie(&app, "/api/sensor/status", &cookie).await;
    assert_status(&resp, axum::http::StatusCode::OK);
    assert_eq!(body_json(resp).await["cache"]["undelivered"], 1);

    let resp = get_with_cookie(&app, "/api/cached-emissions?limit=10", &cookie).await;
    assert_status(&resp, axum::http::StatusCode::OK);
    assert_eq!(body_json(resp).await.as_array().unwrap().len(), 1);
}
