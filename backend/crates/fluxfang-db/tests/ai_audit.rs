mod common;
use common::fresh_pool;

use fluxfang_db::models::NewAiAudit;
use fluxfang_db::repo::ai_audit::AiAuditFilter;
use fluxfang_db::AiAuditRepo;
use serde_json::json;
use uuid::Uuid;

fn new_audit(tool: &str, action: &str) -> NewAiAudit {
    NewAiAudit {
        tool: tool.into(),
        action: action.into(),
        summary: format!("{tool} did a thing"),
        args: json!({"x": 1}),
        result: Some(json!({"ok": true})),
        affected_ids: vec![Uuid::new_v4()],
        status: "ok".into(),
        error: None,
    }
}

#[tokio::test]
async fn insert_then_query_newest_first_and_filter_by_action() {
    let pool = fresh_pool().await;

    AiAuditRepo::insert(&pool, new_audit("create_entity", "add")).await.unwrap();
    AiAuditRepo::insert(&pool, new_audit("delete_entity", "remove")).await.unwrap();

    let (all, total) = AiAuditRepo::query(&pool, AiAuditFilter::default()).await.unwrap();
    assert_eq!(total, 2);
    assert_eq!(all.len(), 2);
    // newest first: delete_entity was inserted last.
    assert_eq!(all[0].tool, "delete_entity");

    let (adds, add_total) = AiAuditRepo::query(
        &pool,
        AiAuditFilter { action: Some("add".into()), ..Default::default() },
    ).await.unwrap();
    assert_eq!(add_total, 1);
    assert_eq!(adds[0].tool, "create_entity");
}
