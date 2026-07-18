//! Minimal hand-rolled MCP over JSON-RPC 2.0 (stateless; no Mcp-Session-Id).
//! Methods: initialize, notifications/initialized, tools/list, tools/call.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::mcp::tools;
use crate::state::AppState;

const PROTOCOL_VERSION: &str = "2025-06-18";

pub async fn handle(State(state): State<AppState>, Json(req): Json<Value>) -> Response {
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let id = req.get("id").cloned();

    // Notifications (no id) get a 202 with no body.
    if id.is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let id = id.unwrap();

    match method {
        "initialize" => ok(
            id,
            json!({
                "protocolVersion": req
                    .get("params").and_then(|p| p.get("protocolVersion"))
                    .and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fluxfang", "version": env!("CARGO_PKG_VERSION") }
            }),
        ),
        "tools/list" => {
            let tools: Vec<Value> = tools::tool_list()
                .into_iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema
                    })
                })
                .collect();
            ok(id, json!({ "tools": tools }))
        }
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            if name.is_empty() {
                return err(id, -32602, "missing tool name");
            }
            match tools::dispatch(&state.pool, name, args).await {
                Ok(value) => ok(id, tool_result(value, false)),
                Err(tools::ToolError::Unknown(n)) => err(id, -32602, &format!("unknown tool: {n}")),
                Err(e) => ok(id, tool_result(json!({ "error": e.message() }), true)),
            }
        }
        _ => err(id, -32601, "method not found"),
    }
}

fn tool_result(value: Value, is_error: bool) -> Value {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string());
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn ok(id: Value, result: Value) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn err(id: Value, code: i64, message: &str) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
        .into_response()
}
