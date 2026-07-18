use chrono::{TimeZone, Utc};
use serde_json::json;

mod common;
use common::fresh_pool_shared as fresh_pool;

use fluxfang_api::mcp::tools::subtractions;
use fluxfang_db::models::{NewDataSource, NewEmission, NewEmitter};
use fluxfang_db::repo::ai_audit::AiAuditFilter;
use fluxfang_db::{AiAuditRepo, DataSourceRepo, EmissionRepo, EmitterRepo, SessionRepo};
use uuid::Uuid;

#[tokio::test]
async fn detach_emissions_returns_to_stray_and_audits_remove() {
    let pool = fresh_pool().await;
    let ds = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0")).await.unwrap().id;
    SessionRepo::close_active(&pool).await.ok();
    let session = SessionRepo::open(&pool).await.unwrap().id;
    let emitter = EmitterRepo::insert(&pool, NewEmitter { name: "E".into(), ..Default::default() }).await.unwrap().id;
    let mut em = NewEmission::wifi(ds, session, json!({"bssid":"a"}));
    em.emitter_id = Some(emitter);
    em.observed_at = Utc.with_ymd_and_hms(2026,1,1,0,0,0).unwrap();
    let eid = EmissionRepo::insert(&pool, em).await.unwrap().id;

    subtractions::detach_emissions(&pool, json!({"emission_ids":[eid.to_string()]})).await.unwrap();

    let after = EmissionRepo::get(&pool, eid).await.unwrap().unwrap();
    assert_eq!(after.emitter_id, None);

    let (rows, _) = AiAuditRepo::query(&pool, AiAuditFilter { action: Some("remove".into()), ..Default::default() }).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tool, "detach_emissions");
}

#[tokio::test]
async fn delete_emitter_removes_and_audits() {
    let pool = fresh_pool().await;
    let emitter = EmitterRepo::insert(&pool, NewEmitter { name: "Gone".into(), ..Default::default() }).await.unwrap().id;
    subtractions::delete_emitter(&pool, json!({"emitter_id": emitter.to_string()})).await.unwrap();
    assert!(EmitterRepo::get(&pool, emitter).await.unwrap().is_none());
}

#[tokio::test]
async fn unassign_emitters_from_entity_clears_entity_and_audits() {
    use fluxfang_db::models::NewEntity;
    use fluxfang_db::EntityRepo;

    let pool = fresh_pool().await;
    let entity = EntityRepo::insert(&pool, NewEntity {
        name: "Ent".into(), notes: None, source: "ai".into(), ai_confidence: None,
    }).await.unwrap().id;
    let emitter = EmitterRepo::insert(&pool, NewEmitter { name: "E2".into(), ..Default::default() }).await.unwrap().id;
    EmitterRepo::set_entity(&pool, emitter, Some(entity)).await.unwrap();

    subtractions::unassign_emitters_from_entity(&pool, json!({"emitter_ids":[emitter.to_string()]})).await.unwrap();

    let after = EmitterRepo::get(&pool, emitter).await.unwrap().unwrap();
    assert_eq!(after.entity_id, None);

    let (rows, _) = AiAuditRepo::query(&pool, AiAuditFilter { action: Some("remove".into()), ..Default::default() }).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tool, "unassign_emitters_from_entity");
}

#[tokio::test]
async fn unlink_emitters_removes_association_and_audits() {
    use fluxfang_db::EmitterAssociationRepo;

    let pool = fresh_pool().await;
    let a = EmitterRepo::insert(&pool, NewEmitter { name: "A".into(), ..Default::default() }).await.unwrap().id;
    let b = EmitterRepo::insert(&pool, NewEmitter { name: "B".into(), ..Default::default() }).await.unwrap().id;
    EmitterAssociationRepo::add(&pool, a, b, "ai", None).await.unwrap();

    subtractions::unlink_emitters(&pool, json!({"emitter_id": a.to_string(), "associated_emitter_id": b.to_string()})).await.unwrap();

    let (rows, _) = AiAuditRepo::query(&pool, AiAuditFilter { action: Some("remove".into()), ..Default::default() }).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tool, "unlink_emitters");
}

#[tokio::test]
async fn delete_entity_removes_and_audits() {
    use fluxfang_db::models::NewEntity;
    use fluxfang_db::EntityRepo;

    let pool = fresh_pool().await;
    let entity = EntityRepo::insert(&pool, NewEntity {
        name: "Gone Entity".into(), notes: None, source: "ai".into(), ai_confidence: None,
    }).await.unwrap().id;

    subtractions::delete_entity(&pool, json!({"entity_id": entity.to_string()})).await.unwrap();

    assert!(EntityRepo::get(&pool, entity).await.unwrap().is_none());
    let (rows, _) = AiAuditRepo::query(&pool, AiAuditFilter { action: Some("remove".into()), ..Default::default() }).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tool, "delete_entity");
}

#[tokio::test]
async fn delete_emitter_not_found_returns_error() {
    let pool = fresh_pool().await;
    let missing = Uuid::new_v4();
    let result = subtractions::delete_emitter(&pool, json!({"emitter_id": missing.to_string()})).await;
    assert!(result.is_err());
}

async fn seed_emission(pool: &sqlx::PgPool, kind: &str, emitter_id: Option<Uuid>, at_secs: i64) -> Uuid {
    let ds = DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0")).await.unwrap().id;
    SessionRepo::close_active(pool).await.ok();
    let session = SessionRepo::open(pool).await.unwrap().id;
    let mut em = NewEmission::wifi(ds, session, json!({"bssid":"a"}));
    em.kind = kind.to_string();
    em.emitter_id = emitter_id;
    em.observed_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap() + chrono::Duration::seconds(at_secs);
    EmissionRepo::insert(pool, em).await.unwrap().id
}

#[tokio::test]
async fn delete_emissions_by_id_removes_rows_and_audits_remove() {
    let pool = fresh_pool().await;
    let e1 = seed_emission(&pool, "wifi", None, 0).await;
    let e2 = seed_emission(&pool, "wifi", None, 1).await;

    let out = subtractions::delete_emissions(&pool, json!({"emission_ids":[e1.to_string()]}))
        .await
        .unwrap();
    assert_eq!(out["deleted"], 1);

    // e1 gone, e2 remains.
    assert!(EmissionRepo::get(&pool, e1).await.unwrap().is_none());
    assert!(EmissionRepo::get(&pool, e2).await.unwrap().is_some());

    let (rows, _) = AiAuditRepo::query(&pool, AiAuditFilter { action: Some("remove".into()), ..Default::default() }).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tool, "delete_emissions");
    assert_eq!(rows[0].affected_ids, vec![e1]);
}

#[tokio::test]
async fn delete_emissions_where_filters_and_guards_unfiltered() {
    let pool = fresh_pool().await;
    let _stray = seed_emission(&pool, "wifi", None, 0).await;
    let emitter = EmitterRepo::insert(&pool, NewEmitter { name: "E".into(), ..Default::default() }).await.unwrap().id;
    let _attached = seed_emission(&pool, "wifi", Some(emitter), 1).await;

    // Unfiltered call with no `all` flag is rejected (no accidental total wipe).
    let guarded = subtractions::delete_emissions_where(&pool, json!({})).await;
    assert!(guarded.is_err(), "unfiltered delete_emissions_where must be rejected");

    // Filter to stray only.
    let out = subtractions::delete_emissions_where(&pool, json!({"unassigned": true})).await.unwrap();
    assert_eq!(out["deleted"], 1);
    let (_, total) = EmissionRepo::query(&pool, fluxfang_db::repo::emission::EmissionFilter::default()).await.unwrap();
    assert_eq!(total, 1, "only the attached emission should remain");
}

#[tokio::test]
async fn delete_emissions_where_all_clears_everything() {
    let pool = fresh_pool().await;
    seed_emission(&pool, "wifi", None, 0).await;
    seed_emission(&pool, "tpms", None, 1).await;

    let out = subtractions::delete_emissions_where(&pool, json!({"all": true})).await.unwrap();
    assert_eq!(out["deleted"], 2);
    assert_eq!(out["all"], true);
    let (_, total) = EmissionRepo::query(&pool, fluxfang_db::repo::emission::EmissionFilter::default()).await.unwrap();
    assert_eq!(total, 0);
}
