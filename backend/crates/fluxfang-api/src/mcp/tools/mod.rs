use serde_json::{json, Value};
use sqlx::PgPool;

pub mod reads;

#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// Error surfaced to the AI. All variants render as an `isError: true` tool
/// result (except `Unknown`, which the protocol layer maps to JSON-RPC -32602).
#[derive(Debug)]
pub enum ToolError {
    Unknown(String),
    InvalidParams(String),
    NotFound(String),
    Db(String),
}

impl ToolError {
    pub fn message(&self) -> String {
        match self {
            ToolError::Unknown(m) => format!("unknown tool: {m}"),
            ToolError::InvalidParams(m) => format!("invalid params: {m}"),
            ToolError::NotFound(m) => format!("not found: {m}"),
            ToolError::Db(m) => format!("database error: {m}"),
        }
    }
}

impl From<sqlx::Error> for ToolError {
    fn from(e: sqlx::Error) -> Self {
        ToolError::Db(e.to_string())
    }
}

/// Every registered tool's schema, for `tools/list`. Grows in Phase 3.
pub fn tool_list() -> Vec<ToolSchema> {
    vec![ToolSchema {
        name: "list_entities",
        description: "List entities (tracked real-world things that own emitters). Paginated.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "search": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 500},
                "offset": {"type": "integer", "minimum": 0}
            }
        }),
    }]
}

/// Dispatch a `tools/call` by name. Grows in Phase 3.
pub async fn dispatch(pool: &PgPool, name: &str, args: Value) -> Result<Value, ToolError> {
    match name {
        "list_entities" => reads::list_entities(pool, args).await,
        _ => Err(ToolError::Unknown(name.to_string())),
    }
}
