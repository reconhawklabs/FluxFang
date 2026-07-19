use axum::http::StatusCode;
use serde_json::json;

mod common;
use common::{assert_status, post_json, spawn_server, test_app};

async fn rpc(base: std::net::SocketAddr, body: serde_json::Value) -> serde_json::Value {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{base}/mcp"))
        .json(&body)
        .send()
        .await
        .expect("send");
    assert!(resp.status().is_success(), "status: {}", resp.status());
    resp.json().await.expect("json")
}

#[tokio::test]
async fn initialize_then_tools_list() {
    let app = test_app().await;
    let addr = spawn_server(app).await;

    let init = rpc(addr, json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}
    })).await;
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "fluxfang");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    // The `instructions` field orients the AI. It must be present, meaningful,
    // and — critically — name EVERY registered tool, so the model always sees
    // the full surface and the roster can't drift out of sync with tool_list().
    let instructions = init["result"]["instructions"]
        .as_str()
        .expect("initialize result must include an instructions string");
    assert!(
        instructions.contains("FluxFang"),
        "instructions should describe the server"
    );
    for tool in fluxfang_api::mcp::tools::tool_list() {
        assert!(
            instructions.contains(tool.name),
            "instructions must name every tool; missing: {}",
            tool.name
        );
    }

    let list = rpc(
        addr,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    )
    .await;
    assert!(
        list["result"]["tools"].is_array(),
        "tools/list shape: {list}"
    );

    let unknown = rpc(
        addr,
        json!({"jsonrpc": "2.0", "id": 3, "method": "does/not/exist"}),
    )
    .await;
    assert_eq!(
        unknown["error"]["code"], -32601,
        "method not found: {unknown}"
    );
}

/// The `/mcp` route is guarded by `mcp_guard`, which fails closed (403) when
/// there's no `ConnectInfo<SocketAddr>` extension to prove the caller is on
/// loopback. `spawn_server` above goes over real TCP (loopback → allowed),
/// so it can't exercise the deny path -- an in-process `oneshot` call, which
/// never populates `ConnectInfo`, is the only way to drive a request through
/// the router without a peer address attached. This is the only test in the
/// suite that actually proves the guard denies rather than merely allowing.
#[tokio::test]
async fn mcp_guard_denies_without_connect_info() {
    let app = test_app().await;

    let resp = post_json(
        &app,
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    )
    .await;

    assert_status(&resp, StatusCode::FORBIDDEN);
}

/// End-to-end (real TCP, loopback → allowed) `tools/call` round trip for a
/// read-only tool against a fresh, empty DB, asserting the MCP result
/// envelope shape (`content[0].type == "text"`, `isError == false`) and that
/// the embedded `text` is itself valid JSON containing the expected
/// `list_entities` result shape (`items: []`, `total: 0`).
#[tokio::test]
async fn tools_call_list_entities_round_trip() {
    let app = test_app().await;
    let addr = spawn_server(app).await;

    let resp = rpc(
        addr,
        json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": {"name": "list_entities", "arguments": {}}
        }),
    )
    .await;

    assert_eq!(resp["result"]["content"][0]["type"], "text", "resp: {resp}");
    assert_eq!(resp["result"]["isError"], false, "resp: {resp}");

    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("content[0].text should be a string: {resp}"));
    let parsed: serde_json::Value =
        serde_json::from_str(text).expect("content[0].text should be valid JSON");
    assert!(parsed["items"].is_array(), "parsed: {parsed}");
    assert_eq!(parsed["total"], 0, "parsed: {parsed}");
}
