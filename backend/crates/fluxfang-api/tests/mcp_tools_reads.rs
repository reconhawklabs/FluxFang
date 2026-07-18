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

use chrono::{TimeZone, Utc};
use fluxfang_db::models::{NewDataSource, NewEmission};
use fluxfang_db::{DataSourceRepo, EmissionRepo, SessionRepo};
use uuid::Uuid;

async fn seed_wifi_emission(pool: &sqlx::PgPool, bssid: &str, emitter_id: Option<Uuid>) -> Uuid {
    let ds = DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0")).await.unwrap().id;
    SessionRepo::close_active(pool).await.ok();
    let session = SessionRepo::open(pool).await.unwrap().id;
    let mut em = NewEmission::wifi(ds, session, serde_json::json!({"bssid": bssid, "ssid": "Net"}));
    em.emitter_id = emitter_id;
    em.observed_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    em.signal_strength = Some(-40);
    EmissionRepo::insert(pool, em).await.unwrap().id
}

#[tokio::test]
async fn list_stray_emissions_only_unassigned_and_get_emission_full_payload() {
    let pool = fresh_pool().await;
    let stray = seed_wifi_emission(&pool, "aa:aa:aa:aa:aa:aa", None).await;

    let out = reads::list_stray_emissions(&pool, serde_json::json!({"limit": 50})).await.unwrap();
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], stray.to_string());
    assert_eq!(items[0]["signal_strength"], -40);
    assert_eq!(items[0]["payload"]["bssid"], "aa:aa:aa:aa:aa:aa");

    let one = reads::get_emission(&pool, serde_json::json!({"id": stray.to_string()})).await.unwrap();
    assert_eq!(one["payload"]["ssid"], "Net");
}
