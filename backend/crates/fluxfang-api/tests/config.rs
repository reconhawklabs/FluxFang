use axum::http::StatusCode;

mod common;
use common::{
    assert_status, body_json, get, get_with_cookie, patch_json_with_cookie, post_json,
    session_cookie, test_app,
};

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
        serde_json::json!({ "role": "standalone", "node_sensor_id": "base", "sensor": null })
    );
}

#[tokio::test]
async fn patch_config_updates_node_sensor_id_and_keeps_key() {
    let app = test_app().await;
    let body = r#"{"password":"pw123456","role":"sensor","node_sensor_id":"frontgate",
        "sensor":{"host":"base","port":9000,"key":"a2V5","cache_ttl_secs":3600}}"#;
    let cookie = session_cookie(&post_json(&app, "/api/setup", body).await);

    // PATCH only the node_sensor_id + host — key omitted must be preserved.
    let resp = patch_json_with_cookie(
        &app,
        "/api/config",
        r#"{"node_sensor_id":"gate2","sensor":{"host":"newbase"}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);

    // GET reflects the merge, sensor host updated, port/ttl preserved, NO key.
    let json = body_json(get_with_cookie(&app, "/api/config", &cookie).await).await;
    assert_eq!(json["node_sensor_id"], "gate2");
    assert_eq!(json["sensor"]["host"], "newbase");
    assert_eq!(json["sensor"]["port"], 9000);
    assert!(
        json["sensor"].get("key").is_none(),
        "config must never return the key"
    );
}

#[tokio::test]
async fn patch_config_rejects_bad_slug() {
    let app = test_app().await;
    let cookie = session_cookie(
        &post_json(
            &app,
            "/api/setup",
            r#"{"password":"pw123456","role":"standalone","node_sensor_id":"local"}"#,
        )
        .await,
    );
    let resp = patch_json_with_cookie(
        &app,
        "/api/config",
        r#"{"node_sensor_id":"bad id"}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}
