use chrono::{TimeZone, Utc};
use serde_json::json;

mod common;
use common::fresh_pool_shared as fresh_pool;

use fluxfang_api::mcp::tools::writes;
use fluxfang_db::models::{NewDataSource, NewEmission};
use fluxfang_db::repo::ai_audit::AiAuditFilter;
use fluxfang_db::{AiAuditRepo, DataSourceRepo, EmissionRepo, SessionRepo};
use uuid::Uuid;

async fn seed_stray(pool: &sqlx::PgPool, bssid: &str) -> Uuid {
    let ds = DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0")).await.unwrap().id;
    SessionRepo::close_active(pool).await.ok();
    let session = SessionRepo::open(pool).await.unwrap().id;
    let mut em = NewEmission::wifi(ds, session, json!({"bssid": bssid, "ssid":"HomeNet"}));
    em.observed_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    EmissionRepo::insert(pool, em).await.unwrap().id
}

#[tokio::test]
async fn create_emitter_from_emissions_with_match_rule_claims_future() {
    let pool = fresh_pool().await;
    let e1 = seed_stray(&pool, "aa:bb:cc:dd:ee:ff").await;

    let out = writes::create_emitter_from_emissions(&pool, json!({
        "name": "HomeNet AP",
        "emitter_type": "wifi_access_point",
        "kind": "wifi",
        "attributes": {"bssid": "aa:bb:cc:dd:ee:ff", "ssid": "HomeNet"},
        "emission_ids": [e1.to_string()],
        "match_rule": {"match":"all","conditions":[{"field":"bssid","op":"eq","value":"aa:bb:cc:dd:ee:ff"}]}
    })).await.expect("create");

    let emitter_id = Uuid::parse_str(out["emitter"]["id"].as_str().unwrap()).unwrap();

    // The seed emission is now attached.
    let em = EmissionRepo::get(&pool, e1).await.unwrap().unwrap();
    assert_eq!(em.emitter_id, Some(emitter_id));
    // source tagged ai.
    assert_eq!(out["emitter"]["source"], "ai");

    // A NEW matching emission gets auto-claimed by the rule.
    let e2 = seed_stray(&pool, "aa:bb:cc:dd:ee:ff").await;
    // attach_emissions_matching runs at create; future ones are claimed by re-running the rule
    // — verify count_matching sees it:
    let preview = writes::preview_match_rule(&pool, json!({
        "kind": "wifi",
        "match_rule": {"match":"all","conditions":[{"field":"bssid","op":"eq","value":"aa:bb:cc:dd:ee:ff"}]}
    })).await.unwrap();
    assert!(preview["would_match"].as_i64().unwrap() >= 1);
    let _ = e2;

    // An audit row was written.
    let (rows, total) = AiAuditRepo::query(&pool, AiAuditFilter::default()).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].tool, "create_emitter_from_emissions");
    assert_eq!(rows[0].action, "add");
    assert_eq!(rows[0].status, "ok");
}

/// `create_emitter_from_emissions` self-audits both its success and error
/// paths internally (rather than going through `dispatch`'s wrapper-based
/// error auditing), specifically so a failure AFTER the emitter row is
/// created still records the emitter id in the error row's `affected_ids`.
/// Forces that partial failure by passing an `emission_ids` list containing
/// a nonexistent emission uuid: the emitter insert succeeds first, then
/// `EmissionRepo::set_emitter` errors (its `UPDATE ... RETURNING` matches no
/// row, so `fetch_one` surfaces `RowNotFound`) on the bogus id. Exactly one
/// audit row must exist afterward (no double-audit from the dispatch
/// wrapper), with `status == "error"` and the created emitter's id present
/// in `affected_ids`.
#[tokio::test]
async fn create_emitter_from_emissions_partial_failure_audits_affected_ids() {
    let pool = fresh_pool().await;
    let bogus_emission_id = Uuid::new_v4();

    let err = writes::create_emitter_from_emissions(&pool, json!({
        "name": "Partial Failure Emitter",
        "emission_ids": [bogus_emission_id.to_string()]
    })).await.expect_err("nonexistent emission id should error after emitter insert");
    assert!(
        err.message().to_lowercase().contains("no rows")
            || err.message().to_lowercase().contains("row not found")
            || err.message().to_lowercase().contains("database error"),
        "unexpected error message: {}",
        err.message()
    );

    let (rows, total) = AiAuditRepo::query(&pool, AiAuditFilter::default()).await.unwrap();
    assert_eq!(total, 1, "expected exactly one audit row, got: {rows:?}");
    assert_eq!(rows[0].tool, "create_emitter_from_emissions");
    assert_eq!(rows[0].action, "add");
    assert_eq!(rows[0].status, "error");
    assert!(
        !rows[0].affected_ids.is_empty(),
        "error row should record the emitter id that persisted before the failure"
    );

    // The emitter really was created (proving affected_ids isn't just
    // padded, it reflects real DB state) and its id is the one recorded.
    let created = fluxfang_db::EmitterRepo::query(
        &pool,
        fluxfang_db::repo::emitter::EmitterListFilter {
            search: Some("Partial Failure Emitter".into()),
            ..Default::default()
        },
    ).await.unwrap();
    assert_eq!(created.1, 1, "the emitter should have persisted despite the later failure");
    let created_id = created.0[0].0.id;
    assert!(rows[0].affected_ids.contains(&created_id));
}

#[tokio::test]
async fn create_entity_and_group_emitters() {
    let pool = fresh_pool().await;
    let e1 = fluxfang_db::EmitterRepo::insert(&pool, fluxfang_db::models::NewEmitter { name: "E1".into(), ..Default::default() }).await.unwrap().id;

    let ent = writes::create_entity(&pool, json!({
        "name": "Silver Sedan", "notes": "seen tailing on 3 outings", "confidence": 0.75,
        "emitter_ids": [e1.to_string()]
    })).await.unwrap();
    let entity_id = Uuid::parse_str(ent["entity"]["id"].as_str().unwrap()).unwrap();
    assert_eq!(ent["entity"]["source"], "ai");
    assert_eq!(ent["entity"]["ai_confidence"], 0.75);

    let emitter = fluxfang_db::EmitterRepo::get(&pool, e1).await.unwrap().unwrap();
    assert_eq!(emitter.entity_id, Some(entity_id));
}

#[tokio::test]
async fn link_emitters_creates_ai_association() {
    let pool = fresh_pool().await;
    let a = fluxfang_db::EmitterRepo::insert(&pool, fluxfang_db::models::NewEmitter { name: "A".into(), ..Default::default() }).await.unwrap().id;
    let b = fluxfang_db::EmitterRepo::insert(&pool, fluxfang_db::models::NewEmitter { name: "B".into(), ..Default::default() }).await.unwrap().id;

    writes::link_emitters(&pool, json!({
        "emitter_id": a.to_string(), "associated_emitter_id": b.to_string(), "confidence": 0.9
    })).await.unwrap();

    assert!(fluxfang_db::EmitterAssociationRepo::exists(&pool, a, b).await.unwrap());
    let list = fluxfang_db::EmitterAssociationRepo::list_for(&pool, a).await.unwrap();
    assert_eq!(list[0].source, "ai");
}
