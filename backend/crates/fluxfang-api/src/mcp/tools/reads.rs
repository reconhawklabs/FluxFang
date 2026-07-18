use serde_json::{json, Value};
use sqlx::PgPool;

use fluxfang_db::repo::entity::EntityListFilter;
use fluxfang_db::EntityRepo;

use crate::mcp::shape;
use crate::mcp::tools::ToolError;

pub async fn list_entities(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let filter = EntityListFilter {
        search: shape::opt_str(&args, "search"),
        limit: shape::clamp_limit(&args),
        offset: shape::offset(&args),
    };
    let (rows, total) = EntityRepo::query(pool, filter).await?;
    Ok(json!({
        "items": rows.iter().map(shape::entity_json).collect::<Vec<_>>(),
        "total": total,
    }))
}
