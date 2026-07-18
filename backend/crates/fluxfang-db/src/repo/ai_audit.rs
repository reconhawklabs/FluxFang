//! `ai_audit_log`: append-only log of MCP tool calls made by the embedded
//! AI server. `insert` is called by the MCP audit wrapper after every tool
//! call; `query` backs the `GET /api/ai-audit` endpoint.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::models::{AiAuditEntry, NewAiAudit};

pub struct AiAuditRepo;

const AUDIT_COLUMNS: &str =
    "id, created_at, tool, action, summary, args, result, affected_ids, status, error";

/// Filter for [`AiAuditRepo::query`]. Every field is optional (no-op when
/// `None`) except `limit`/`offset`, which always apply — see
/// [`Default`] for the page-size default.
#[derive(Debug, Clone)]
pub struct AiAuditFilter {
    pub action: Option<String>,
    pub time_from: Option<DateTime<Utc>>,
    pub time_to: Option<DateTime<Utc>>,
    pub search: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl Default for AiAuditFilter {
    fn default() -> Self {
        Self { action: None, time_from: None, time_to: None, search: None, limit: 50, offset: 0 }
    }
}

impl AiAuditRepo {
    pub async fn insert(pool: &PgPool, new: NewAiAudit) -> Result<AiAuditEntry, sqlx::Error> {
        let sql = format!(
            "INSERT INTO ai_audit_log (tool, action, summary, args, result, affected_ids, status, error) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {AUDIT_COLUMNS}"
        );
        sqlx::query_as::<_, AiAuditEntry>(&sql)
            .bind(new.tool)
            .bind(new.action)
            .bind(new.summary)
            .bind(new.args)
            .bind(new.result)
            .bind(&new.affected_ids)
            .bind(new.status)
            .bind(new.error)
            .fetch_one(pool)
            .await
    }

    pub async fn query(
        pool: &PgPool,
        filter: AiAuditFilter,
    ) -> Result<(Vec<AiAuditEntry>, i64), sqlx::Error> {
        // Bind params in a fixed order; NULL params act as "no filter" via `($n IS NULL OR ...)`.
        let where_clause = "\
            ($1::text IS NULL OR action = $1) \
            AND ($2::timestamptz IS NULL OR created_at >= $2) \
            AND ($3::timestamptz IS NULL OR created_at <= $3) \
            AND ($4::text IS NULL OR tool ILIKE '%' || $4 || '%' OR summary ILIKE '%' || $4 || '%')";

        let count_sql = format!("SELECT count(*) FROM ai_audit_log WHERE {where_clause}");
        let total: i64 = sqlx::query_scalar(&count_sql)
            .bind(&filter.action)
            .bind(filter.time_from)
            .bind(filter.time_to)
            .bind(&filter.search)
            .fetch_one(pool)
            .await?;

        let rows_sql = format!(
            "SELECT {AUDIT_COLUMNS} FROM ai_audit_log WHERE {where_clause} \
             ORDER BY created_at DESC, id DESC LIMIT $5 OFFSET $6"
        );
        let rows = sqlx::query_as::<_, AiAuditEntry>(&rows_sql)
            .bind(&filter.action)
            .bind(filter.time_from)
            .bind(filter.time_to)
            .bind(&filter.search)
            .bind(filter.limit)
            .bind(filter.offset)
            .fetch_all(pool)
            .await?;

        Ok((rows, total))
    }
}
