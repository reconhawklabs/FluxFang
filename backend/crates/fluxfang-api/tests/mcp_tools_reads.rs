use serde_json::json;

mod common;
use common::fresh_pool_shared as fresh_pool; // re-exported by common; add if absent

use fluxfang_api::mcp::tools::reads;
use fluxfang_db::models::NewEntity;
use fluxfang_db::EntityRepo;

#[tokio::test]
async fn list_entities_returns_items_and_total() {
    let pool = fresh_pool().await;
    EntityRepo::insert(&pool, NewEntity { name: "Alpha".into(), ..Default::default() }).await.unwrap();
    EntityRepo::insert(&pool, NewEntity { name: "Beta".into(), ..Default::default() }).await.unwrap();

    let out = reads::list_entities(&pool, json!({"limit": 10})).await.expect("list");
    assert_eq!(out["total"], 2);
    assert_eq!(out["items"].as_array().unwrap().len(), 2);
    assert!(out["items"][0]["name"].is_string());
}
