use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_db::models::NewAiAudit;
use fluxfang_db::AiAuditRepo;

/// Record a successful AI mutation. Best-effort: an audit write failure is
/// logged, never propagated (the mutation already happened).
pub async fn record_success(
    pool: &PgPool,
    tool: &str,
    action: &str,
    summary: String,
    args: &Value,
    result: &Value,
    affected_ids: Vec<Uuid>,
) {
    let row = NewAiAudit {
        tool: tool.to_string(),
        action: action.to_string(),
        summary,
        args: args.clone(),
        result: Some(result.clone()),
        affected_ids,
        status: "ok".to_string(),
        error: None,
    };
    if let Err(e) = AiAuditRepo::insert(pool, row).await {
        eprintln!("fluxfang-mcp: failed to write audit row for {tool}: {e}");
    }
}

pub async fn record_error(pool: &PgPool, tool: &str, action: &str, args: &Value, err_msg: &str) {
    let row = NewAiAudit {
        tool: tool.to_string(),
        action: action.to_string(),
        summary: format!("{tool} failed"),
        args: args.clone(),
        result: None,
        affected_ids: Vec::new(),
        status: "error".to_string(),
        error: Some(err_msg.to_string()),
    };
    if let Err(e) = AiAuditRepo::insert(pool, row).await {
        eprintln!("fluxfang-mcp: failed to write error-audit row for {tool}: {e}");
    }
}
