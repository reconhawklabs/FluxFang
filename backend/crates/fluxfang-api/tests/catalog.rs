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

/// Task 6.1's core flow: the catalog is behind auth, and `bssid` (a `Mac`
/// field) exposes exactly the ops that make sense for it, mapped to their
/// plain-English labels — `eq`/`matches` present, `gte` absent (MACs aren't
/// ordered).
#[tokio::test]
async fn catalog_for_wifi_includes_bssid_with_expected_ops() {
    let app = test_app().await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/catalog/wifi", &cookie).await;
    assert_status(&resp, StatusCode::OK);

    let body = body_json(resp).await;
    let fields = body.as_array().expect("catalog should be a JSON array");
    let bssid = fields
        .iter()
        .find(|f| f["key"] == "bssid")
        .expect("wifi catalog should include a bssid field");

    assert_eq!(bssid["type"], "mac");
    let ops = bssid["ops"].as_array().expect("ops should be an array");
    assert!(ops.contains(&serde_json::json!({"code":"eq","label":"is exactly"})));
    assert!(ops.contains(&serde_json::json!({"code":"matches","label":"contains / matches"})));
    assert!(!ops.iter().any(|op| op["code"] == "gte"));
}

/// Enum-typed fields (e.g. `frame_type`) additionally expose their allowed
/// `values`.
#[tokio::test]
async fn catalog_for_wifi_enum_field_exposes_values() {
    let app = test_app().await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/catalog/wifi", &cookie).await;
    assert_status(&resp, StatusCode::OK);

    let body = body_json(resp).await;
    let fields = body.as_array().expect("catalog should be a JSON array");
    let frame_type = fields
        .iter()
        .find(|f| f["key"] == "frame_type")
        .expect("wifi catalog should include a frame_type field");

    assert_eq!(frame_type["type"], "enum");
    let values = frame_type["values"]
        .as_array()
        .expect("enum field should expose values");
    let values: Vec<&str> = values.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(values.contains(&"beacon"));
    assert!(values.contains(&"probe_request"));
}

/// An unknown kind returns 200 with an empty array, matching
/// `catalog_for`'s own "unknown kinds return an empty catalog" behavior.
#[tokio::test]
async fn catalog_for_unknown_kind_is_empty_array() {
    let app = test_app().await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/catalog/zigbee", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    assert_eq!(body_json(resp).await, serde_json::json!([]));
}

/// The endpoint is behind auth like every other protected route.
#[tokio::test]
async fn catalog_requires_auth() {
    let app = test_app().await;
    let resp = get(&app, "/api/catalog/wifi").await;
    assert_status(&resp, StatusCode::UNAUTHORIZED);
}
