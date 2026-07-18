mod common;
use common::fresh_pool;

use fluxfang_db::models::{NewEmitter, NewEntity};
use fluxfang_db::{EmitterRepo, EntityRepo};

#[tokio::test]
async fn emitter_and_entity_carry_ai_source() {
    let pool = fresh_pool().await;

    let emitter = EmitterRepo::insert(
        &pool,
        NewEmitter { name: "AI AP".into(), source: "ai".into(), ..Default::default() },
    ).await.expect("insert emitter");
    assert_eq!(emitter.source, "ai");

    let entity = EntityRepo::insert(
        &pool,
        NewEntity { name: "AI Car".into(), notes: None, source: "ai".into(), ai_confidence: Some(0.8) },
    ).await.expect("insert entity");
    assert_eq!(entity.source, "ai");
    assert_eq!(entity.ai_confidence, Some(0.8));

    // Default construction stays 'manual'.
    let manual = EmitterRepo::insert(&pool, NewEmitter { name: "m".into(), ..Default::default() })
        .await.expect("insert manual");
    assert_eq!(manual.source, "manual");
}
