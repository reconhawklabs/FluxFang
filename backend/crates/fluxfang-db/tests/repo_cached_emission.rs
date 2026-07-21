mod common;
use fluxfang_db::{CachedEmissionRepo, models::NewCachedEmission};

#[tokio::test]
async fn insert_list_undelivered_mark_delivered_prune() {
    let pool = common::fresh_pool().await;
    let mk = |kind: &str| NewCachedEmission {
        kind: kind.to_string(), signal_strength: Some(-40),
        lat: Some(1.5), lon: Some(2.5), observed_at: chrono::Utc::now(),
        payload: serde_json::json!({"x":1}), data_source_id: None,
    };
    let a = CachedEmissionRepo::insert(&pool, mk("wifi")).await.unwrap();
    let _b = CachedEmissionRepo::insert(&pool, mk("bluetooth")).await.unwrap();

    let undelivered = CachedEmissionRepo::list_undelivered(&pool, 100).await.unwrap();
    assert_eq!(undelivered.len(), 2);
    assert_eq!(undelivered[0].lat, Some(1.5));

    CachedEmissionRepo::mark_delivered(&pool, &[a.id]).await.unwrap();
    assert_eq!(CachedEmissionRepo::list_undelivered(&pool, 100).await.unwrap().len(), 1);

    let stats = CachedEmissionRepo::stats(&pool).await.unwrap();
    assert_eq!(stats.total, 2);
    assert_eq!(stats.undelivered, 1);

    // Prune everything older than a future cutoff → deletes both.
    let pruned = CachedEmissionRepo::prune_older_than(&pool, chrono::Utc::now() + chrono::Duration::hours(1)).await.unwrap();
    assert_eq!(pruned, 2);
}
