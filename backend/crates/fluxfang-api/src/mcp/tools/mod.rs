use serde_json::Value;
use sqlx::PgPool;

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
    Vec::new()
}

/// Dispatch a `tools/call` by name. Grows in Phase 3.
pub async fn dispatch(_pool: &PgPool, name: &str, _args: Value) -> Result<Value, ToolError> {
    Err(ToolError::Unknown(name.to_string()))
}
