use serde_json::{json, Value};
use sqlx::PgPool;

pub mod analysis;
pub mod reads;
pub mod writes;

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
        ToolSchema {
            name: "collocation_query",
            description: "For each unordered pair among the given emitters, count co-occurring emissions within a time window.",
            input_schema: json!({"type":"object","required":["emitter_ids"],"properties":{
                "emitter_ids":{"type":"array","items":{"type":"string"},"minItems":2},
                "window_seconds":{"type":"integer"}}}),
        },
        ToolSchema {
            name: "suggest_associations",
            description: "Score candidate emitter pairs for association using co-occurrence timing/distance (returns confidence).",
            input_schema: json!({"type":"object","required":["emitter_ids"],"properties":{
                "emitter_ids":{"type":"array","items":{"type":"string"},"minItems":2}}}),
        },
        ToolSchema {
            name: "cotravel_analysis",
            description: "Rank emitters by how strongly they co-travel with the host (spread/points/span → tier).",
            input_schema: json!({"type":"object","properties":{
                "time_from":{"type":"string"},"time_to":{"type":"string"},
                "min_distance_m":{"type":"number"},"min_time_s":{"type":"number"}}}),
        },
        ToolSchema {
            name: "create_emitter_from_emissions",
            description: "Create a new AI-sourced emitter, optionally attaching explicit emission_ids and/or a match_rule (which retroactively claims all currently-matching emissions of the given kind).",
            input_schema: json!({"type":"object","required":["name"],"properties":{
                "name":{"type":"string"},"type":{"type":"string"},"emitter_type":{"type":"string"},
                "kind":{"type":"string","description":"required if match_rule is given (wifi/bluetooth/tpms)"},
                "attributes":{"type":"object"},
                "emission_ids":{"type":"array","items":{"type":"string"}},
                "match_rule":{"type":"object","description":"{match: all|any, conditions: [{field,op,value}]}"}
            }}),
        },
        ToolSchema {
            name: "set_emitter_match_rule",
            description: "Replace an emitter's match rule and retroactively claim all currently-matching emissions of the given kind.",
            input_schema: json!({"type":"object","required":["emitter_id","match_rule","kind"],"properties":{
                "emitter_id":{"type":"string"},"kind":{"type":"string"},
                "match_rule":{"type":"object"}
            }}),
        },
        ToolSchema {
            name: "preview_match_rule",
            description: "Read-only: count how many emissions of the given kind a match rule would claim, without changing anything.",
            input_schema: json!({"type":"object","required":["match_rule","kind"],"properties":{
                "kind":{"type":"string"},"match_rule":{"type":"object"}
            }}),
        },
        ToolSchema {
            name: "attach_emissions",
            description: "Attach a list of emission_ids to an emitter.",
            input_schema: json!({"type":"object","required":["emitter_id","emission_ids"],"properties":{
                "emitter_id":{"type":"string"},"emission_ids":{"type":"array","items":{"type":"string"}}
            }}),
        },
        ToolSchema {
            name: "update_emitter",
            description: "Update an emitter's name/type, and/or shallow-merge new attributes into its existing attributes.",
            input_schema: json!({"type":"object","required":["emitter_id"],"properties":{
                "emitter_id":{"type":"string"},"name":{"type":"string"},"type":{"type":"string"},
                "attributes":{"type":"object"}
            }}),
        },
        ToolSchema {
            name: "create_entity",
            description: "Create a new AI-sourced entity (tracked real-world thing), optionally grouping in a set of existing emitter_ids.",
            input_schema: json!({"type":"object","required":["name"],"properties":{
                "name":{"type":"string"},"notes":{"type":"string"},"confidence":{"type":"number"},
                "emitter_ids":{"type":"array","items":{"type":"string"}}
            }}),
        },
        ToolSchema {
            name: "update_entity",
            description: "Update an entity's name and/or notes.",
            input_schema: json!({"type":"object","required":["entity_id"],"properties":{
                "entity_id":{"type":"string"},"name":{"type":"string"},"notes":{"type":"string"}
            }}),
        },
        ToolSchema {
            name: "assign_emitters_to_entity",
            description: "Assign a list of emitter_ids to an existing entity.",
            input_schema: json!({"type":"object","required":["entity_id","emitter_ids"],"properties":{
                "entity_id":{"type":"string"},"emitter_ids":{"type":"array","items":{"type":"string"}}
            }}),
        },
        ToolSchema {
            name: "link_emitters",
            description: "Create an AI-sourced association between two emitters, with an optional confidence score.",
            input_schema: json!({"type":"object","required":["emitter_id","associated_emitter_id"],"properties":{
                "emitter_id":{"type":"string"},"associated_emitter_id":{"type":"string"},
                "confidence":{"type":"number"}
            }}),
        },
    ]
}

/// Write tools and the audit action they log on the error path. Success rows
/// are written inside each handler; this covers the complementary case where
/// a write tool errors out and still needs an `action`-tagged trail. Lists
/// every write tool name across Tasks 11-13 (some aren't dispatched yet —
/// harmless, `dispatch_inner` just returns `Unknown` for those until their
/// task lands). `preview_match_rule` is read-only and intentionally absent
/// (returns `None`, so no audit row is written for it).
fn write_action(name: &str) -> Option<&'static str> {
    match name {
        "create_emitter_from_emissions" | "set_emitter_match_rule" | "attach_emissions"
        | "update_emitter" | "create_entity" | "update_entity" | "assign_emitters_to_entity"
        | "link_emitters" => Some("add"),
        "detach_emissions" | "unassign_emitters_from_entity" | "unlink_emitters"
        | "delete_emitter" | "delete_entity" => Some("remove"),
        _ => None, // read-only / preview tools
    }
}

/// Dispatch a `tools/call` by name. Wraps [`dispatch_inner`] with error-path
/// auditing: if the tool is a write tool (per [`write_action`]) and it
/// returns `Err`, record an `action`-tagged error row so a failing mutation
/// still leaves a trail (success rows are written inside the handlers
/// themselves, since only the handler knows the affected ids/summary).
pub async fn dispatch(pool: &PgPool, name: &str, args: Value) -> Result<Value, ToolError> {
    let result = dispatch_inner(pool, name, args.clone()).await;
    if let (Some(action), Err(e)) = (write_action(name), &result) {
        crate::mcp::audit::record_error(pool, name, action, &args, &e.message()).await;
    }
    result
}

async fn dispatch_inner(pool: &PgPool, name: &str, args: Value) -> Result<Value, ToolError> {
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
        "collocation_query" => analysis::collocation_query(pool, args).await,
        "suggest_associations" => analysis::suggest_associations(pool, args).await,
        "cotravel_analysis" => analysis::cotravel_analysis(pool, args).await,
        "create_emitter_from_emissions" => writes::create_emitter_from_emissions(pool, args).await,
        "set_emitter_match_rule" => writes::set_emitter_match_rule(pool, args).await,
        "preview_match_rule" => writes::preview_match_rule(pool, args).await,
        "attach_emissions" => writes::attach_emissions(pool, args).await,
        "update_emitter" => writes::update_emitter(pool, args).await,
        "create_entity" => writes::create_entity(pool, args).await,
        "update_entity" => writes::update_entity(pool, args).await,
        "assign_emitters_to_entity" => writes::assign_emitters_to_entity(pool, args).await,
        "link_emitters" => writes::link_emitters(pool, args).await,
        _ => Err(ToolError::Unknown(name.to_string())),
    }
}
