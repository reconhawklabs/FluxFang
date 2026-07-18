use sqlx::Row;

mod common;
use common::fresh_pool;

#[tokio::test]
async fn migration_0012_adds_provenance_and_audit() {
    let pool = fresh_pool().await;

    // emitter.source and entity.source/ai_confidence exist and default to 'manual'.
    let e = sqlx::query("INSERT INTO entity (name) VALUES ('t') RETURNING source, ai_confidence")
        .fetch_one(&pool).await.expect("insert entity");
    let src: String = e.get("source");
    assert_eq!(src, "manual");

    // ai_audit_log accepts a row.
    sqlx::query(
        "INSERT INTO ai_audit_log (tool, action, summary, status) \
         VALUES ('t', 'add', 's', 'ok')",
    )
    .execute(&pool).await.expect("insert audit row");

    // action/status CHECK rejects bad values.
    let bad = sqlx::query(
        "INSERT INTO ai_audit_log (tool, action, summary, status) \
         VALUES ('t', 'sideways', 's', 'ok')",
    ).execute(&pool).await;
    assert!(bad.is_err(), "action CHECK should reject 'sideways'");
}
