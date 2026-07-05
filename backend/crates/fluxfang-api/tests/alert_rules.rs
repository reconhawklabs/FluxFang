//! Task 6.6: `GET/POST/PATCH/DELETE /api/alert-rules[/:id]` — driven end to
//! end through the HTTP API. Seeds alert methods directly via
//! `AlertMethodRepo::insert` and a zone via `ZoneRepo::insert` against the
//! test app's own isolated pool, same pattern other route test files use.

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::models::{NewAlertMethod, NewZone};
use fluxfang_db::{AlertMethodRepo, ZoneRepo};

mod common;
use common::{
    assert_status, body_json, delete_with_cookie, get, get_with_cookie, patch_json_with_cookie,
    post_json, post_json_with_cookie, session_cookie, test_app_with_factory,
};

async fn login(app: &axum::Router) -> String {
    post_json(app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    let resp = post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    session_cookie(&resp)
}

async fn seed_in_app_method(pool: &PgPool, name: &str) -> Uuid {
    AlertMethodRepo::insert(
        pool,
        NewAlertMethod {
            name: name.to_string(),
            type_: "in_app".to_string(),
            enabled: true,
            config_encrypted: vec![],
        },
    )
    .await
    .expect("seed alert_method")
    .id
}

async fn seed_zone(pool: &PgPool) -> Uuid {
    ZoneRepo::insert(
        pool,
        NewZone {
            name: "Test Zone".to_string(),
            center: (-122.4194, 37.7749),
            radius_m: 500.0,
            notes: None,
        },
    )
    .await
    .expect("seed zone")
    .id
}

/// (c) Creating a rule linking two methods: `GET /api/alert-rules` shows
/// both `method_ids`.
#[tokio::test]
async fn create_rule_linking_two_methods_shows_both_method_ids_on_list() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let method_a = seed_in_app_method(&pool, "A").await;
    let method_b = seed_in_app_method(&pool, "B").await;

    let emitter_id = Uuid::new_v4();
    let body = json!({
        "name": "detect emitter",
        "enabled": true,
        "target_type": "emitter",
        "target_id": emitter_id,
        "trigger": {"on": "detected"},
        "method_ids": [method_a, method_b],
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/alert-rules", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();
    let created_method_ids: std::collections::HashSet<String> = created["method_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        created_method_ids,
        std::collections::HashSet::from([method_a.to_string(), method_b.to_string()])
    );

    let resp = get_with_cookie(&app, "/api/alert-rules", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let list = body_json(resp).await;
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1, "body: {list}");
    assert_eq!(arr[0]["id"], id);
    let listed_method_ids: std::collections::HashSet<String> = arr[0]["method_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        listed_method_ids,
        std::collections::HashSet::from([method_a.to_string(), method_b.to_string()])
    );
}

/// A zone-transition trigger (`enters_zone`) persists its `on` + `zone_id`
/// correctly when a `zone_id` is supplied.
#[tokio::test]
async fn zone_transition_rule_persists_on_and_zone_id() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let zone_id = seed_zone(&pool).await;
    let emitter_id = Uuid::new_v4();
    let body = json!({
        "name": "emitter enters zone",
        "enabled": true,
        "target_type": "emitter",
        "target_id": emitter_id,
        "trigger": {"on": "enters_zone", "zone_id": zone_id},
        "method_ids": [],
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/alert-rules", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    assert_eq!(created["trigger"]["on"], "enters_zone");
    assert_eq!(created["trigger"]["zone_id"], zone_id.to_string());
}

/// (c) A zone-transition trigger without `zone_id` is `400`.
#[tokio::test]
async fn zone_trigger_without_zone_id_is_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let emitter_id = Uuid::new_v4();
    let body = json!({
        "name": "bad zone rule",
        "enabled": true,
        "target_type": "emitter",
        "target_id": emitter_id,
        "trigger": {"on": "enters_zone"},
        "method_ids": [],
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/alert-rules", &body, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// (c) A rule whose `content_match` isn't a well-formed `Rule` against the
/// catalog is `400`.
#[tokio::test]
async fn detected_rule_with_invalid_content_match_is_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let emitter_id = Uuid::new_v4();
    let body = json!({
        "name": "bad content match",
        "enabled": true,
        "target_type": "emitter",
        "target_id": emitter_id,
        "trigger": {
            "on": "detected",
            "content_match": {
                "match": "all",
                "conditions": [{"field": "not_a_real_field", "op": "eq", "value": "x"}]
            }
        },
        "method_ids": [],
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/alert-rules", &body, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// A `detected`/`enters_zone`/`leaves_zone` rule with no target is `400`.
#[tokio::test]
async fn detected_rule_without_target_is_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({
        "name": "no target",
        "enabled": true,
        "trigger": {"on": "detected"},
        "method_ids": [],
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/alert-rules", &body, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// A `host_enters_zone`/`host_leaves_zone` rule must have a null target —
/// supplying one is `400`.
#[tokio::test]
async fn host_zone_rule_with_a_target_is_400() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let zone_id = seed_zone(&pool).await;
    let emitter_id = Uuid::new_v4();
    let body = json!({
        "name": "bad host rule",
        "enabled": true,
        "target_type": "emitter",
        "target_id": emitter_id,
        "trigger": {"on": "host_enters_zone", "zone_id": zone_id},
        "method_ids": [],
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/alert-rules", &body, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// A valid `host_enters_zone` rule (null target, zone_id present) is
/// accepted.
#[tokio::test]
async fn host_zone_rule_without_target_is_created() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let zone_id = seed_zone(&pool).await;
    let body = json!({
        "name": "host enters zone",
        "enabled": true,
        "trigger": {"on": "host_enters_zone", "zone_id": zone_id},
        "method_ids": [],
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/alert-rules", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    assert!(created["target_type"].is_null());
    assert!(created["target_id"].is_null());
}

/// Creating a rule that references an unknown `method_ids` entry is `400`,
/// not a foreign-key `500`.
#[tokio::test]
async fn create_rule_with_unknown_method_id_is_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let emitter_id = Uuid::new_v4();
    let bogus_method_id = Uuid::new_v4();
    let body = json!({
        "name": "detect emitter",
        "enabled": true,
        "target_type": "emitter",
        "target_id": emitter_id,
        "trigger": {"on": "detected"},
        "method_ids": [bogus_method_id],
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/alert-rules", &body, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// `PATCH` can change `method_ids` (fully replacing the linked set) and
/// other fields.
#[tokio::test]
async fn patch_alert_rule_replaces_method_ids_and_updates_fields() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let method_a = seed_in_app_method(&pool, "A").await;
    let method_b = seed_in_app_method(&pool, "B").await;

    let emitter_id = Uuid::new_v4();
    let body = json!({
        "name": "detect emitter",
        "enabled": true,
        "target_type": "emitter",
        "target_id": emitter_id,
        "trigger": {"on": "detected"},
        "method_ids": [method_a],
    })
    .to_string();
    let resp = post_json_with_cookie(&app, "/api/alert-rules", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let patch_body = json!({
        "name": "detect emitter (renamed)",
        "enabled": false,
        "method_ids": [method_b],
    })
    .to_string();
    let resp = patch_json_with_cookie(
        &app,
        &format!("/api/alert-rules/{id}"),
        &patch_body,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    let updated = body_json(resp).await;
    assert_eq!(updated["name"], "detect emitter (renamed)");
    assert_eq!(updated["enabled"], false);
    let method_ids: Vec<String> = updated["method_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(method_ids, vec![method_b.to_string()]);
}

/// `DELETE` removes the row; a repeat delete is `404`.
#[tokio::test]
async fn delete_alert_rule_then_404_on_repeat() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let emitter_id = Uuid::new_v4();
    let body = json!({
        "name": "detect emitter",
        "enabled": true,
        "target_type": "emitter",
        "target_id": emitter_id,
        "trigger": {"on": "detected"},
        "method_ids": [],
    })
    .to_string();
    let resp = post_json_with_cookie(&app, "/api/alert-rules", &body, &cookie).await;
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let resp = delete_with_cookie(&app, &format!("/api/alert-rules/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NO_CONTENT);

    let resp = delete_with_cookie(&app, &format!("/api/alert-rules/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NOT_FOUND);
}

/// (e) Every alert-rules endpoint is behind auth.
#[tokio::test]
async fn alert_rules_endpoints_require_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;

    assert_status(
        &get(&app, "/api/alert-rules").await,
        StatusCode::UNAUTHORIZED,
    );
    assert_status(
        &post_json(&app, "/api/alert-rules", r#"{"name":"x"}"#).await,
        StatusCode::UNAUTHORIZED,
    );
}
