use chrono::{TimeZone, Utc};
use serde_json::json;

mod common;
use common::fresh_pool_shared as fresh_pool;

use fluxfang_api::mcp::tools::analysis;
use fluxfang_db::models::{NewDataSource, NewEmission, NewEmitter};
use fluxfang_db::{DataSourceRepo, EmissionRepo, EmitterRepo, SessionRepo};

#[tokio::test]
async fn collocation_counts_cooccurring_pairs() {
    let pool = fresh_pool().await;
    let ds = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0")).await.unwrap().id;
    SessionRepo::close_active(&pool).await.ok();
    let session = SessionRepo::open(&pool).await.unwrap().id;

    let a = EmitterRepo::insert(&pool, NewEmitter { name: "A".into(), ..Default::default() }).await.unwrap().id;
    let b = EmitterRepo::insert(&pool, NewEmitter { name: "B".into(), ..Default::default() }).await.unwrap().id;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    // Two near-simultaneous observations per emitter, in two clusters.
    for (secs, emitter) in [(0i64, a), (1, b), (300, a), (301, b)] {
        let mut em = NewEmission::wifi(ds, session, json!({"x": 1}));
        em.emitter_id = Some(emitter);
        em.observed_at = base + chrono::Duration::seconds(secs);
        EmissionRepo::insert(&pool, em).await.unwrap();
    }

    let out = analysis::collocation_query(&pool, json!({
        "emitter_ids": [a.to_string(), b.to_string()], "window_seconds": 60
    })).await.unwrap();
    let pairs = out["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 1);
    assert!(pairs[0]["cooccurrences"].as_i64().unwrap() >= 2);
}

#[tokio::test]
async fn suggest_associations_returns_verdicts() {
    let pool = fresh_pool().await;
    let ds = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0")).await.unwrap().id;
    SessionRepo::close_active(&pool).await.ok();
    let session = SessionRepo::open(&pool).await.unwrap().id;
    let a = EmitterRepo::insert(&pool, NewEmitter { name: "A".into(), ..Default::default() }).await.unwrap().id;
    let b = EmitterRepo::insert(&pool, NewEmitter { name: "B".into(), ..Default::default() }).await.unwrap().id;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    // Co-located far-apart events → geographic verdict (0.9).
    for (secs, emitter, lon, lat) in [
        (0i64, a, 0.0, 0.0), (1, b, 0.0, 0.0),
        (600, a, 0.2, 0.0), (601, b, 0.2, 0.0),
    ] {
        let mut em = NewEmission::wifi(ds, session, json!({}));
        em.emitter_id = Some(emitter);
        em.observed_at = base + chrono::Duration::seconds(secs);
        em.location = Some((lon, lat));
        EmissionRepo::insert(&pool, em).await.unwrap();
    }

    let out = analysis::suggest_associations(&pool, json!({
        "emitter_ids": [a.to_string(), b.to_string()]
    })).await.unwrap();
    let s = out["suggestions"].as_array().unwrap();
    assert_eq!(s.len(), 1);
    assert!(s[0]["confidence"].as_f64().unwrap() > 0.0);
}
