use axum::http::StatusCode;

mod common;
use common::{assert_status, body_json, get, get_with_cookie, post_json, session_cookie, test_app};

/// Log in against a fresh app and return its session cookie, running setup
/// first since a fresh instance has no password configured yet.
async fn login(app: &axum::Router) -> String {
    post_json(app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    let resp = post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    session_cookie(&resp)
}

/// `GET /api/emitter-types/wifi` returns the two known wifi emitter types,
/// each with a machine `key` and a human-readable `label` — the dropdown
/// data a frontend "create emitter" form needs instead of a free-text
/// field.
#[tokio::test]
async fn emitter_types_for_wifi_lists_access_point_and_client() {
    let app = test_app().await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/emitter-types/wifi", &cookie).await;
    assert_status(&resp, StatusCode::OK);

    let body = body_json(resp).await;
    let types = body
        .as_array()
        .expect("emitter-types should be a JSON array");
    assert_eq!(types.len(), 2, "body: {body}");

    assert!(types.contains(&serde_json::json!({
        "key": "wifi_access_point",
        "label": "WiFi Access Point"
    })));
    assert!(types.contains(&serde_json::json!({
        "key": "wifi_client",
        "label": "WiFi Client"
    })));
}

/// An unknown kind returns 200 with an empty array, matching
/// `emitter_types_for_kind`'s own "unknown kind has no types" behavior.
#[tokio::test]
async fn emitter_types_for_unknown_kind_is_empty_array() {
    let app = test_app().await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/emitter-types/zigbee", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    assert_eq!(body_json(resp).await, serde_json::json!([]));
}

/// The endpoint is behind auth like every other protected route.
#[tokio::test]
async fn emitter_types_requires_auth() {
    let app = test_app().await;
    let resp = get(&app, "/api/emitter-types/wifi").await;
    assert_status(&resp, StatusCode::UNAUTHORIZED);
}
