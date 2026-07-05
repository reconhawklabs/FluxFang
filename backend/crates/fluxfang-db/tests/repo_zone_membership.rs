//! Round-trip tests for `ZoneMembershipRepo`.

mod common;

use chrono::{Duration, Utc};
use common::fresh_pool;
use fluxfang_db::models::NewZone;
use fluxfang_db::repo::zone::ZoneRepo;
use fluxfang_db::repo::zone_membership::ZoneMembershipRepo;
use uuid::Uuid;

async fn seed_zone(pool: &sqlx::PgPool) -> Uuid {
    let z = ZoneRepo::insert(
        pool,
        NewZone {
            name: "Zone".to_string(),
            center: (-122.4194, 37.7749),
            radius_m: 1000.0,
            notes: None,
        },
    )
    .await
    .unwrap();
    z.id
}

#[tokio::test]
async fn get_returns_none_when_no_membership_row_exists() {
    let pool = fresh_pool().await;
    let zone_id = seed_zone(&pool).await;

    let got = ZoneMembershipRepo::get(&pool, "emitter", Some(Uuid::new_v4()), zone_id)
        .await
        .unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn upsert_inserts_new_membership_row() {
    let pool = fresh_pool().await;
    let zone_id = seed_zone(&pool).await;
    let subject_id = Uuid::new_v4();
    let since = Utc::now();

    let m = ZoneMembershipRepo::upsert(&pool, "emitter", Some(subject_id), zone_id, true, since)
        .await
        .unwrap();

    assert_eq!(m.subject_type, "emitter");
    assert_eq!(m.subject_id, Some(subject_id));
    assert_eq!(m.zone_id, zone_id);
    assert!(m.inside);
    assert_eq!(m.since.timestamp(), since.timestamp());
}

#[tokio::test]
async fn upsert_updates_existing_membership_row_in_place() {
    let pool = fresh_pool().await;
    let zone_id = seed_zone(&pool).await;
    let subject_id = Uuid::new_v4();
    let t1 = Utc::now() - Duration::hours(1);
    let t2 = Utc::now();

    let first = ZoneMembershipRepo::upsert(&pool, "emitter", Some(subject_id), zone_id, true, t1)
        .await
        .unwrap();
    let second = ZoneMembershipRepo::upsert(&pool, "emitter", Some(subject_id), zone_id, false, t2)
        .await
        .unwrap();

    assert_eq!(
        first.id, second.id,
        "upsert on same subject/zone should update, not insert a row"
    );
    assert!(!second.inside);
    assert_eq!(second.since.timestamp(), t2.timestamp());
}

#[tokio::test]
async fn get_finds_row_via_subject_type_and_id() {
    let pool = fresh_pool().await;
    let zone_id = seed_zone(&pool).await;
    let subject_id = Uuid::new_v4();
    let since = Utc::now();

    let inserted =
        ZoneMembershipRepo::upsert(&pool, "entity", Some(subject_id), zone_id, true, since)
            .await
            .unwrap();

    let got = ZoneMembershipRepo::get(&pool, "entity", Some(subject_id), zone_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.id, inserted.id);
}

// --- Host subject (subject_id = NULL) dedup — the schema's `NULLS NOT
// DISTINCT` unique index on (subject_type, subject_id, zone_id) means there
// can be only one 'host' row per zone. `get`/`upsert` must use
// `IS NOT DISTINCT FROM` (not `=`, which never matches NULL) to find it. ---

#[tokio::test]
async fn get_finds_host_row_via_null_subject_id() {
    let pool = fresh_pool().await;
    let zone_id = seed_zone(&pool).await;
    let since = Utc::now();

    let inserted = ZoneMembershipRepo::upsert(&pool, "host", None, zone_id, true, since)
        .await
        .unwrap();

    let got = ZoneMembershipRepo::get(&pool, "host", None, zone_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.id, inserted.id);
    assert_eq!(got.subject_id, None);
}

#[tokio::test]
async fn upsert_host_subject_twice_yields_exactly_one_row_and_updates_it() {
    let pool = fresh_pool().await;
    let zone_id = seed_zone(&pool).await;
    let t1 = Utc::now() - Duration::hours(1);
    let t2 = Utc::now();

    let first = ZoneMembershipRepo::upsert(&pool, "host", None, zone_id, true, t1)
        .await
        .unwrap();
    let second = ZoneMembershipRepo::upsert(&pool, "host", None, zone_id, false, t2)
        .await
        .unwrap();

    assert_eq!(
        first.id, second.id,
        "second host upsert should update the same row, not insert a duplicate"
    );
    assert!(!second.inside);
    assert_eq!(second.since.timestamp(), t2.timestamp());

    // Directly verify there is exactly one host row for this zone in the DB
    // (not just that the two Rust-level results happen to share an id).
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM zone_membership WHERE subject_type = 'host' AND zone_id = $1",
    )
    .bind(zone_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 1);
}
