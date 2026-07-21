use axum::http::StatusCode;

mod common;
use common::{assert_status, body_json, get, get_with_cookie, post_json, session_cookie, test_app};

/// The config endpoint is protected — no cookie, no answer.
#[tokio::test]
async fn config_requires_auth() {
    let app = test_app().await;
    let resp = get(&app, "/api/config").await;
    assert_status(&resp, StatusCode::UNAUTHORIZED);
}

/// After a standalone setup, the endpoint reports the role + node id.
#[tokio::test]
async fn config_reports_standalone_role() {
    let app = test_app().await;
    let body = r#"{"password":"pw123456","role":"standalone","node_sensor_id":"base"}"#;
    let resp = post_json(&app, "/api/setup", body).await;
    let cookie = session_cookie(&resp);

    let resp = get_with_cookie(&app, "/api/config", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(
        json,
        serde_json::json!({ "role": "standalone", "node_sensor_id": "base" })
    );
}
