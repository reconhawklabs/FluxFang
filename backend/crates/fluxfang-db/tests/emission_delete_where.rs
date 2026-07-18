mod common;
use common::fresh_pool;

use chrono::{TimeZone, Utc};
use fluxfang_db::models::{NewDataSource, NewEmission};
use fluxfang_db::repo::emission::DeleteEmissionFilter;
use fluxfang_db::{DataSourceRepo, EmissionRepo, EmitterRepo, SessionRepo};
use uuid::Uuid;

async fn seed(
    pool: &sqlx::PgPool,
    kind: &str,
    emitter_id: Option<Uuid>,
    at_secs: i64,
) -> Uuid {
    let ds = DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .unwrap()
        .id;
    SessionRepo::close_active(pool).await.ok();
    let session = SessionRepo::open(pool).await.unwrap().id;
    let mut em = NewEmission::wifi(ds, session, serde_json::json!({"bssid": "aa"}));
    em.kind = kind.to_string();
    em.emitter_id = emitter_id;
    em.observed_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
        + chrono::Duration::seconds(at_secs);
    EmissionRepo::insert(pool, em).await.unwrap().id
}

#[tokio::test]
async fn delete_where_filters_by_kind_and_unassigned() {
    let pool = fresh_pool().await;
    let emitter = EmitterRepo::insert(
        &pool,
        fluxfang_db::models::NewEmitter { name: "E".into(), ..Default::default() },
    )
    .await
    .unwrap()
    .id;

    let _stray_wifi = seed(&pool, "wifi", None, 0).await;
    let _attached_wifi = seed(&pool, "wifi", Some(emitter), 1).await;
    let _stray_tpms = seed(&pool, "tpms", None, 2).await;

    // Delete only stray wifi emissions.
    let deleted = EmissionRepo::delete_where(
        &pool,
        DeleteEmissionFilter {
            kind: Some("wifi".into()),
            unassigned: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(deleted, 1, "only the one stray wifi row should be deleted");

    // The attached wifi and the stray tpms remain.
    let (remaining, total) =
        EmissionRepo::query(&pool, fluxfang_db::repo::emission::EmissionFilter::default())
            .await
            .unwrap();
    assert_eq!(total, 2);
    assert_eq!(remaining.len(), 2);
}

#[tokio::test]
async fn delete_where_by_time_window() {
    let pool = fresh_pool().await;
    seed(&pool, "wifi", None, 0).await;
    seed(&pool, "wifi", None, 100).await;
    seed(&pool, "wifi", None, 500).await;

    let cutoff = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
        + chrono::Duration::seconds(200);
    let deleted = EmissionRepo::delete_where(
        &pool,
        DeleteEmissionFilter { time_to: Some(cutoff), ..Default::default() },
    )
    .await
    .unwrap();
    assert_eq!(deleted, 2, "the two rows at/before the cutoff should be deleted");
}
