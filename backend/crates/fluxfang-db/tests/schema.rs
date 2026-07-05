//! Schema coverage for Task 1.2: asserts the full FluxFang schema applies
//! cleanly and that a representative spatial round-trip works end to end
//! (data_source -> emission with a geography(Point,4326) location).

use sqlx::PgPool;

/// Connect to the DATABASE_URL and apply all migrations, returning a ready pool.
///
/// Tests must point DATABASE_URL at a *fresh* database so the full migration
/// file (including this task's DDL) applies from scratch.
async fn fresh_pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL set for tests");
    let pool = fluxfang_db::connect(&url).await.unwrap();
    fluxfang_db::run_migrations(&pool).await.unwrap();
    pool
}

/// Regression test for the zone_membership host-uniqueness fix: a plain
/// btree unique index on (subject_type, subject_id, zone_id) does NOT
/// dedupe host rows because host rows have subject_id = NULL and Postgres
/// treats NULL <> NULL, so ON CONFLICT never matches and every upsert
/// inserts a new row. The index must be declared `NULLS NOT DISTINCT` so
/// that a second upsert for the same (host, zone) updates the existing row
/// instead of inserting a duplicate — this is what makes host-zone
/// enter/leave alerts fire once per transition rather than once per
/// emission.
#[tokio::test]
async fn host_zone_membership_upsert_is_deduped_by_nulls_not_distinct_index() {
    let pool = fresh_pool().await;

    let zone: (uuid::Uuid,) = sqlx::query_as(
        "insert into zone(name, center, radius_m) \
         values('home', ST_SetSRID(ST_MakePoint(-122.4,37.7),4326)::geography, 100) \
         returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let upsert = "insert into zone_membership(subject_type, subject_id, zone_id, inside, since) \
                  values('host', null, $1, true, now()) \
                  on conflict (subject_type, subject_id, zone_id) \
                  do update set inside = excluded.inside, since = excluded.since";

    sqlx::query(upsert).bind(zone.0).execute(&pool).await.unwrap();
    sqlx::query(upsert).bind(zone.0).execute(&pool).await.unwrap();

    let count: (i64,) = sqlx::query_as(
        "select count(*) from zone_membership where subject_type = 'host' and zone_id = $1",
    )
    .bind(zone.0)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        count.0, 1,
        "expected exactly one host zone_membership row after two upserts, got {}",
        count.0
    );
}

#[tokio::test]
async fn emission_accepts_geography_point() {
    let pool = fresh_pool().await;
    let ds: (uuid::Uuid,) = sqlx::query_as(
        "insert into data_source(kind,mode,status) values('wifi','monitor','stopped') returning id")
        .fetch_one(&pool).await.unwrap();
    let row: (uuid::Uuid,) = sqlx::query_as(
        "insert into emission(data_source_id, observed_at, kind, payload, location) \
         values($1, now(), 'wifi', '{}'::jsonb, ST_SetSRID(ST_MakePoint(-122.4,37.7),4326)::geography) returning id")
        .bind(ds.0).fetch_one(&pool).await.unwrap();
    assert!(!row.0.is_nil());
}
