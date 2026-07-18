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

use fluxfang_db::models::NewEmitter;
use fluxfang_db::EmitterRepo;

#[tokio::test]
async fn get_emitter_includes_attributes_and_associations() {
    let pool = fresh_pool().await;
    let ap = EmitterRepo::insert(&pool, NewEmitter {
        name: "AP".into(), emitter_type: Some("wifi_access_point".into()),
        attributes: serde_json::json!({"bssid":"aa:bb:cc:dd:ee:ff","ssid":"HomeNet"}),
        ..Default::default()
    }).await.unwrap();

    let out = reads::get_emitter(&pool, serde_json::json!({"id": ap.id.to_string()})).await.unwrap();
    assert_eq!(out["emitter"]["attributes"]["ssid"], "HomeNet");
    assert!(out["associations"].is_array());
    assert!(out["recent_emissions"].is_array());
}

#[tokio::test]
async fn emitters_connected_to_matches_connected_ssid() {
    let pool = fresh_pool().await;
    EmitterRepo::insert(&pool, NewEmitter {
        name: "Client".into(), emitter_type: Some("wifi_client".into()),
        attributes: serde_json::json!({"src_mac":"11:22:33:44:55:66","connected_ssid":"HomeNet"}),
        ..Default::default()
    }).await.unwrap();

    let out = reads::emitters_connected_to(&pool, serde_json::json!({"ssid":"HomeNet"})).await.unwrap();
    assert_eq!(out["emitters"].as_array().unwrap().len(), 1);
}
