use axum::http::StatusCode;

mod common;
use common::{assert_status, body_json, get, get_with_cookie, post_json, session_cookie, test_app};

/// Log in against a fresh app and return its session cookie, running setup
/// first since a fresh instance has no password configured yet. Same helper
/// pattern as `tests/catalog.rs`.
async fn login(app: &axum::Router) -> String {
    post_json(app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    let resp = post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    session_cookie(&resp)
}

/// The hardware-enumeration endpoint: authenticated callers get 200 with a
/// JSON object exposing both arrays. Contents are host-dependent (the test
/// runner may have no wireless/serial hardware at all), so only the shape —
/// both keys present as arrays — is asserted, per the task brief.
#[tokio::test]
async fn capture_devices_returns_wifi_and_serial_arrays() {
    let app = test_app().await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/system/capture-devices", &cookie).await;
    assert_status(&resp, StatusCode::OK);

    let body = body_json(resp).await;
    assert!(
        body["wifi_interfaces"].is_array(),
        "wifi_interfaces should be an array, got {body:?}"
    );
    assert!(
        body["serial_devices"].is_array(),
        "serial_devices should be an array, got {body:?}"
    );
}

/// Behind auth like every other protected route.
#[tokio::test]
async fn capture_devices_requires_auth() {
    let app = test_app().await;
    let resp = get(&app, "/api/system/capture-devices").await;
    assert_status(&resp, StatusCode::UNAUTHORIZED);
}
