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
    vec![
        ToolSchema {
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
        },
        ToolSchema {
            name: "list_stray_emissions",
            description: "List emissions not yet assigned to any emitter (stray). Filter by kind (wifi/bluetooth/tpms), time_from/time_to (RFC3339), text. Returns full raw payload + signal_strength.",
            input_schema: json!({"type":"object","properties":{
                "kind":{"type":"string"},"time_from":{"type":"string"},"time_to":{"type":"string"},
                "text":{"type":"string"},"limit":{"type":"integer"},"offset":{"type":"integer"}}}),
        },
        ToolSchema {
            name: "list_emissions",
            description: "List emissions with full raw payload + signal. Filter by emitter_id, kind, time_from/time_to, text.",
            input_schema: json!({"type":"object","properties":{
                "emitter_id":{"type":"string"},"kind":{"type":"string"},"time_from":{"type":"string"},
                "time_to":{"type":"string"},"text":{"type":"string"},"limit":{"type":"integer"},"offset":{"type":"integer"}}}),
        },
        ToolSchema {
            name: "get_emission",
            description: "Get one emission by id, with its complete raw payload.",
            input_schema: json!({"type":"object","required":["id"],"properties":{"id":{"type":"string"}}}),
        },
        ToolSchema {
            name: "list_emitters",
            description: "List emitters with attributes/identity/match rule; filter by search, entity_id, emitter_type.",
            input_schema: json!({"type":"object","properties":{
                "search":{"type":"string"},"entity_id":{"type":"string"},"emitter_type":{"type":"string"},
                "limit":{"type":"integer"},"offset":{"type":"integer"}}}),
        },
        ToolSchema {
            name: "get_emitter",
            description: "Full emitter detail incl associations and recent located emissions.",
            input_schema: json!({"type":"object","required":["id"],"properties":{"id":{"type":"string"}}}),
        },
        ToolSchema {
            name: "get_entity",
            description: "Full entity detail incl emitters, last_seen, recent detections.",
            input_schema: json!({"type":"object","required":["id"],"properties":{"id":{"type":"string"}}}),
        },
        ToolSchema {
            name: "emitters_connected_to",
            description: "Client emitters that connected to a given ssid or bssid access point.",
            input_schema: json!({"type":"object","properties":{
                "ssid":{"type":"string"},"bssid":{"type":"string"},"limit":{"type":"integer"}}}),
        },
        ToolSchema {
            name: "list_attributes_by_type",
            description: "All attribute keys+values in use for an emitter_type.",
            input_schema: json!({"type":"object","required":["emitter_type"],"properties":{"emitter_type":{"type":"string"}}}),
        },
        ToolSchema {
            name: "signal_uniqueness",
            description: "How rare a payload field value is across all emissions.",
            input_schema: json!({"type":"object","required":["field","value"],"properties":{
                "field":{"type":"string"},"value":{"type":"string"}}}),
        },
    ]
}

/// Dispatch a `tools/call` by name. Grows in Phase 3.
pub async fn dispatch(pool: &PgPool, name: &str, args: Value) -> Result<Value, ToolError> {
    match name {
        "list_entities" => reads::list_entities(pool, args).await,
        "list_stray_emissions" => reads::list_stray_emissions(pool, args).await,
        "list_emissions" => reads::list_emissions(pool, args).await,
        "get_emission" => reads::get_emission(pool, args).await,
        "list_emitters" => reads::list_emitters(pool, args).await,
        "get_emitter" => reads::get_emitter(pool, args).await,
        "get_entity" => reads::get_entity(pool, args).await,
        "emitters_connected_to" => reads::emitters_connected_to(pool, args).await,
        "list_attributes_by_type" => reads::list_attributes_by_type(pool, args).await,
        "signal_uniqueness" => reads::signal_uniqueness(pool, args).await,
        _ => Err(ToolError::Unknown(name.to_string())),
    }
}
