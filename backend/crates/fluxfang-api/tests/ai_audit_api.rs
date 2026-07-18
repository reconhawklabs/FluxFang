use axum::http::StatusCode;
use std::sync::Arc;

mod common;
use common::{
    assert_status, body_json, get, get_with_cookie, post_json, session_cookie,
    test_app_with_factory,
};
use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::models::NewAiAudit;
use fluxfang_db::AiAuditRepo;
use serde_json::json;

async fn login(app: &axum::Router) -> String {
    post_json(app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    let resp = post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    session_cookie(&resp)
}

#[tokio::test]
async fn ai_audit_endpoint_lists_rows_and_requires_auth() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    assert_status(&get(&app, "/api/ai-audit").await, StatusCode::UNAUTHORIZED);

    let cookie = login(&app).await;
    AiAuditRepo::insert(
        &pool,
        NewAiAudit {
            tool: "create_entity".into(),
            action: "add".into(),
            summary: "made one".into(),
            args: json!({}),
            result: None,
            affected_ids: vec![],
            status: "ok".into(),
            error: None,
        },
    )
    .await
    .unwrap();

    let resp = get_with_cookie(&app, "/api/ai-audit", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["tool"], "create_entity");
    assert_eq!(body["items"][0]["action"], "add");
}
