use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::json;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::models::NewEmitter;
use fluxfang_db::EmitterRepo;

mod common;
use common::{
    assert_status, body_json, get, get_with_cookie, post_json, session_cookie, test_app,
    test_app_with_factory,
};

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

// ---------------------------------------------------------------------
// Task 4: GET /api/emitters/types — the distinct emitter types that
// actually have emitters, with labels, sorted by label. The stable
// Type-filter dropdown's backend source.
// ---------------------------------------------------------------------

/// `GET /api/emitters/types` returns one entry per distinct `emitter_type`
/// actually in use, each with its machine `key` and human-readable `label`.
#[tokio::test]
async fn emitters_types_lists_in_use_types_with_labels() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "BT Device".to_string(),
            emitter_type: Some("bluetooth_device".to_string()),
            match_criteria: json!({}),
            ..Default::default()
        },
    )
    .await
    .expect("seed bluetooth_device emitter");
    EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "WiFi Client".to_string(),
            emitter_type: Some("wifi_client".to_string()),
            match_criteria: json!({}),
            ..Default::default()
        },
    )
    .await
    .expect("seed wifi_client emitter");
    // Unclassified emitter — must not appear in the response.
    EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "Unclassified".to_string(),
            match_criteria: json!({}),
            ..Default::default()
        },
    )
    .await
    .expect("seed unclassified emitter");

    let resp = get_with_cookie(&app, "/api/emitters/types", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    let arr = body.as_array().expect("body should be a JSON array");
    assert_eq!(arr.len(), 2, "body: {body}");
    assert!(arr
        .iter()
        .any(|t| t["key"] == "bluetooth_device" && t["label"] == "Bluetooth Device"));
    assert!(arr
        .iter()
        .any(|t| t["key"] == "wifi_client" && t["label"] == "WiFi Client"));
}

/// The endpoint is behind auth like every other protected route.
#[tokio::test]
async fn emitters_types_requires_auth() {
    let app = test_app().await;
    let resp = get(&app, "/api/emitters/types").await;
    assert_status(&resp, StatusCode::UNAUTHORIZED);
}
