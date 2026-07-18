use serde_json::json;

mod common;
use common::{spawn_server, test_app};

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
