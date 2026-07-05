//! Task 6.6: `GET/POST/PATCH/DELETE /api/alert-methods[/:id]` + `POST
//! /api/alert-methods/:id/test` — driven end to end through the HTTP API.
//!
//! The most important property under test is negative: `GET
//! /api/alert-methods` must never leak a secret (SMTP password, webhook
//! HMAC secret) anywhere in its response body, even though those secrets
//! are exactly what's encrypted into `config_encrypted` at creation time.

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::json;

use fluxfang_api::capture::MockCapturerFactory;

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

/// (a) Creating an email alert method with a password never echoes that
/// password back — not in the create response, and not in a subsequent
/// `GET /api/alert-methods` list. Asserted both structurally (`config` has
/// no `password` key) and as a raw substring search over the whole response
/// body, so a bug that leaked the secret under some *other* key name would
/// still be caught.
#[tokio::test]
async fn create_email_method_never_echoes_password_in_create_or_list_response() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let secret_password = "sUpErSecret_p4ssw0rd!";
    let body = json!({
        "name": "Ops email",
        "type": "email",
        "enabled": true,
        "config": {
            "host": "smtp.example.com",
            "port": 587,
            "username": "alerts@example.com",
            "password": secret_password,
            "from": "alerts@example.com",
            "to": "ops@example.com",
            "tls": true,
        }
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/alert-methods", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let created_raw = created.to_string();
    assert!(
        !created_raw.contains(secret_password),
        "create response must not contain the plaintext password: {created_raw}"
    );
    assert!(created["config"].get("password").is_none());
    assert!(created["config"].get("username").is_none());
    assert_eq!(created["config"]["host"], "smtp.example.com");
    assert_eq!(created["config"]["from"], "alerts@example.com");
    assert_eq!(created["config"]["to"], "ops@example.com");
    let id = created["id"].as_str().unwrap().to_string();

    let resp = get_with_cookie(&app, "/api/alert-methods", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let list = body_json(resp).await;
    let list_raw = list.to_string();
    assert!(
        !list_raw.contains(secret_password),
        "list response must never contain the plaintext password anywhere: {list_raw}"
    );
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1, "body: {list}");
    assert_eq!(arr[0]["id"], id);
    assert!(arr[0].get("config_encrypted").is_none());
}

/// (b) A webhook method's `secret` (HMAC signing key) is likewise never
/// returned, while its non-secret `url`/`method` are.
#[tokio::test]
async fn webhook_method_config_projection_hides_secret_but_shows_url() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let hmac_secret = "hmac-signing-secret-value";
    let body = json!({
        "name": "Webhook",
        "type": "webhook",
        "enabled": true,
        "config": {
            "url": "http://example.invalid/hook",
            "secret": hmac_secret,
        }
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/alert-methods", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    assert!(!created.to_string().contains(hmac_secret));
    assert_eq!(created["config"]["url"], "http://example.invalid/hook");
    assert!(created["config"].get("secret").is_none());
}

/// (b) `/test` on a freshly created `in_app` method dispatches with no
/// external I/O and reports `Delivered`.
#[tokio::test]
async fn test_send_on_in_app_method_is_delivered() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body =
        json!({"name": "In-app", "type": "in_app", "enabled": true, "config": {}}).to_string();
    let resp = post_json_with_cookie(&app, "/api/alert-methods", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let resp =
        common::post_with_cookie(&app, &format!("/api/alert-methods/{id}/test"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let status = body_json(resp).await;
    assert_eq!(status["status"], "delivered");
}

/// An unreachable webhook's `/test` reports `Failed`, not a `500` and not a
/// panic.
#[tokio::test]
async fn test_send_on_unreachable_webhook_reports_failed() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({
        "name": "Unreachable webhook",
        "type": "webhook",
        "enabled": true,
        "config": {"url": "http://127.0.0.1:1/hook"}
    })
    .to_string();
    let resp = post_json_with_cookie(&app, "/api/alert-methods", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let resp =
        common::post_with_cookie(&app, &format!("/api/alert-methods/{id}/test"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let status = body_json(resp).await;
    assert_eq!(status["status"], "failed");
    assert!(status["reason"].as_str().is_some());
}

/// Creating a method with an unknown `type` is a `400`.
#[tokio::test]
async fn create_method_with_unknown_type_is_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body =
        json!({"name": "x", "type": "carrier_pigeon", "enabled": true, "config": {}}).to_string();
    let resp = post_json_with_cookie(&app, "/api/alert-methods", &body, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// Creating an email method missing a required config field (`password`) is
/// a `400`, not a silently-broken method that only fails later at `/test`.
#[tokio::test]
async fn create_email_method_missing_required_field_is_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({
        "name": "Incomplete email",
        "type": "email",
        "enabled": true,
        "config": {"host": "smtp.example.com", "port": 587}
    })
    .to_string();
    let resp = post_json_with_cookie(&app, "/api/alert-methods", &body, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// `PATCH` updates `name`/`enabled` and re-encrypts a resubmitted `config`,
/// still never leaking the new secret.
#[tokio::test]
async fn patch_updates_name_enabled_and_reencrypts_config() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({
        "name": "Webhook",
        "type": "webhook",
        "enabled": true,
        "config": {"url": "http://old.invalid/hook", "secret": "old-secret"}
    })
    .to_string();
    let resp = post_json_with_cookie(&app, "/api/alert-methods", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let new_secret = "brand-new-secret";
    let patch_body = json!({
        "name": "Webhook (renamed)",
        "enabled": false,
        "config": {"url": "http://new.invalid/hook", "secret": new_secret}
    })
    .to_string();
    let resp = patch_json_with_cookie(
        &app,
        &format!("/api/alert-methods/{id}"),
        &patch_body,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    let updated = body_json(resp).await;
    assert!(!updated.to_string().contains(new_secret));
    assert_eq!(updated["name"], "Webhook (renamed)");
    assert_eq!(updated["enabled"], false);
    assert_eq!(updated["config"]["url"], "http://new.invalid/hook");
}

/// `DELETE` removes the row; a repeat delete (or an id that never existed)
/// is `404`.
#[tokio::test]
async fn delete_alert_method_then_404_on_repeat() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body =
        json!({"name": "In-app", "type": "in_app", "enabled": true, "config": {}}).to_string();
    let resp = post_json_with_cookie(&app, "/api/alert-methods", &body, &cookie).await;
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let resp = delete_with_cookie(&app, &format!("/api/alert-methods/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NO_CONTENT);

    let resp = delete_with_cookie(&app, &format!("/api/alert-methods/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NOT_FOUND);
}

/// (e) Every alert-methods endpoint is behind auth.
#[tokio::test]
async fn alert_methods_endpoints_require_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;

    assert_status(
        &get(&app, "/api/alert-methods").await,
        StatusCode::UNAUTHORIZED,
    );
    assert_status(
        &post_json(&app, "/api/alert-methods", r#"{"name":"x"}"#).await,
        StatusCode::UNAUTHORIZED,
    );
    let id = uuid::Uuid::new_v4();
    assert_status(
        &common::post_with_cookie(&app, &format!("/api/alert-methods/{id}/test"), "").await,
        StatusCode::UNAUTHORIZED,
    );
}
