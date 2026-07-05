//! Task 6.6: `GET /api/notifications` + `POST /api/notifications/:id/read`
//! — driven end to end through the HTTP API. Seeds `notification` rows
//! directly via `NotificationRepo::insert` (there is no route that creates
//! one directly — they're normally produced by an alert firing, Tasks
//! 5.3/5.4), against the test app's own isolated pool.

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::models::NewNotification;
use fluxfang_db::NotificationRepo;

mod common;
use common::{
    assert_status, body_json, get, get_with_cookie, post_json, post_with_cookie, session_cookie,
    test_app_with_factory,
};

async fn login(app: &axum::Router) -> String {
    post_json(app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    let resp = post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    session_cookie(&resp)
}

async fn seed_notification(pool: &PgPool, delivery_status: &str) -> Uuid {
    NotificationRepo::insert(
        pool,
        NewNotification {
            alert_rule_id: None,
            alert_method_id: None,
            fired_at: Utc::now(),
            payload: serde_json::json!({"title": "test", "body": "test body", "context": {}}),
            delivery_status: delivery_status.to_string(),
        },
    )
    .await
    .expect("seed notification")
    .id
}

/// (d) `unread_only` filters, `total` reflects the filter, `unread_count`
/// reports the *global* unread total regardless of `unread_only`/pagination.
#[tokio::test]
async fn list_filters_unread_only_and_reports_total_and_unread_count() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let unread_a = seed_notification(&pool, "sent").await;
    let _unread_b = seed_notification(&pool, "failed").await;
    let read_one = seed_notification(&pool, "sent").await;
    NotificationRepo::mark_read(&pool, read_one).await.unwrap();

    // All three, unfiltered.
    let resp = get_with_cookie(&app, "/api/notifications", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["total"], 3, "body: {body}");
    assert_eq!(body["unread_count"], 2, "body: {body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 3, "body: {body}");

    // unread_only=true: only the two unread rows.
    let resp = get_with_cookie(&app, "/api/notifications?unread_only=true", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["total"], 2, "body: {body}");
    assert_eq!(body["unread_count"], 2, "body: {body}");
    let ids: std::collections::HashSet<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&unread_a.to_string()));
    assert!(!ids.contains(&read_one.to_string()));
}

/// (d) `POST /api/notifications/:id/read` flips `read_at` from null to set,
/// and a subsequent `unread_only` listing no longer includes it.
#[tokio::test]
async fn mark_read_flips_read_at_and_removes_from_unread_listing() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let id = seed_notification(&pool, "sent").await;

    let resp = get_with_cookie(&app, "/api/notifications?unread_only=true", &cookie).await;
    let body = body_json(resp).await;
    assert_eq!(body["total"], 1, "body: {body}");

    let resp = post_with_cookie(&app, &format!("/api/notifications/{id}/read"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let updated = body_json(resp).await;
    assert_eq!(updated["id"], id.to_string());
    assert!(!updated["read_at"].is_null(), "body: {updated}");

    let resp = get_with_cookie(&app, "/api/notifications?unread_only=true", &cookie).await;
    let body = body_json(resp).await;
    assert_eq!(body["total"], 0, "body: {body}");
    assert_eq!(body["unread_count"], 0, "body: {body}");
}

/// Marking an unknown notification id read is `404`.
#[tokio::test]
async fn mark_read_unknown_id_is_404() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let id = Uuid::new_v4();
    let resp = post_with_cookie(&app, &format!("/api/notifications/{id}/read"), &cookie).await;
    assert_status(&resp, StatusCode::NOT_FOUND);
}

/// (e) Every notifications endpoint is behind auth.
#[tokio::test]
async fn notifications_endpoints_require_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;

    assert_status(
        &get(&app, "/api/notifications").await,
        StatusCode::UNAUTHORIZED,
    );
    let id = Uuid::new_v4();
    assert_status(
        &post_with_cookie(&app, &format!("/api/notifications/{id}/read"), "").await,
        StatusCode::UNAUTHORIZED,
    );
}
