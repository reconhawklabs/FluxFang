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

    sqlx::query(upsert)
        .bind(zone.0)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(upsert)
        .bind(zone.0)
        .execute(&pool)
        .await
        .unwrap();

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

/// Task: `0003_wifi_scan_mode.sql` widens the `data_source` kind/mode CHECK
/// to additionally allow `wifi` + `scan` (managed-mode `iw ... scan`
/// polling, alongside the existing `wifi` + `monitor` monitor-mode
/// capture). Confirms the new combination is accepted...
#[tokio::test]
async fn wifi_scan_mode_is_accepted_by_data_source_check() {
    let pool = fresh_pool().await;
    let row: (uuid::Uuid,) = sqlx::query_as(
        "insert into data_source(kind,mode,status) values('wifi','scan','stopped') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!row.0.is_nil());
}

/// ...and that an unrelated bogus mode is still rejected by the same
/// constraint (i.e. the widening didn't accidentally open it up to
/// anything).
#[tokio::test]
async fn wifi_bogus_mode_is_still_rejected_by_data_source_check() {
    let pool = fresh_pool().await;
    let result =
        sqlx::query("insert into data_source(kind,mode,status) values('wifi','bogus','stopped')")
            .execute(&pool)
            .await;
    assert!(
        result.is_err(),
        "expected wifi+bogus mode to violate the data_source kind/mode CHECK"
    );
}

/// Phase A1 (`0004_emitter_classification.sql`): confirms 0001->0004 apply
/// cleanly to a fresh schema and the four new `emitter` columns exist with
/// the documented defaults (`emitter_type`/`identity_key` NULL, `attributes`
/// `{}`, `match_enabled` true) for a plain insert that doesn't set them.
#[tokio::test]
async fn emitter_classification_columns_exist_with_documented_defaults() {
    let pool = fresh_pool().await;

    let row: (
        uuid::Uuid,
        Option<String>,
        serde_json::Value,
        bool,
        Option<String>,
    ) = sqlx::query_as(
        "insert into emitter(name) values('plain emitter') \
         returning id, emitter_type, attributes, match_enabled, identity_key",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(!row.0.is_nil());
    assert_eq!(row.1, None, "emitter_type must default to NULL");
    assert_eq!(
        row.2,
        serde_json::json!({}),
        "attributes must default to '{{}}'"
    );
    assert!(row.3, "match_enabled must default to true");
    assert_eq!(row.4, None, "identity_key must default to NULL");
}

/// The `identity_key` unique index must allow many NULL rows (every
/// user-made emitter) while still rejecting two rows that share the same
/// non-NULL key — this is exactly what makes
/// `EmitterRepo::get_or_create_by_identity`'s `ON CONFLICT (identity_key)`
/// meaningful.
///
/// This test runs against the shared `public` schema (see this module's
/// `fresh_pool`, which — unlike `tests/common::fresh_pool` — applies
/// migrations directly rather than into a per-test isolated schema), so
/// rows it inserts persist across repeated runs of this same test binary.
/// The candidate key is a fresh UUID each run specifically so re-running
/// this test never collides with a row a previous run left behind.
#[tokio::test]
async fn emitter_identity_key_unique_index_allows_many_nulls_but_rejects_duplicates() {
    let pool = fresh_pool().await;
    let key = uuid::Uuid::new_v4().to_string();

    sqlx::query("insert into emitter(name) values('a')")
        .execute(&pool)
        .await
        .expect("first NULL identity_key row must succeed");
    sqlx::query("insert into emitter(name) values('b')")
        .execute(&pool)
        .await
        .expect("second NULL identity_key row must also succeed");

    sqlx::query("insert into emitter(name, identity_key) values('c', $1)")
        .bind(&key)
        .execute(&pool)
        .await
        .expect("first row with a given non-NULL identity_key must succeed");

    let dup = sqlx::query("insert into emitter(name, identity_key) values('d', $1)")
        .bind(&key)
        .execute(&pool)
        .await;
    assert!(
        dup.is_err(),
        "a second row with the same non-NULL identity_key must violate the unique index"
    );
}

#[tokio::test]
async fn emission_accepts_geography_point() {
    let pool = fresh_pool().await;
    let ds: (uuid::Uuid,) = sqlx::query_as(
        "insert into data_source(kind,mode,status) values('wifi','monitor','stopped') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let row: (uuid::Uuid,) = sqlx::query_as(
        "insert into emission(data_source_id, observed_at, kind, payload, location) \
         values($1, now(), 'wifi', '{}'::jsonb, ST_SetSRID(ST_MakePoint(-122.4,37.7),4326)::geography) returning id")
        .bind(ds.0).fetch_one(&pool).await.unwrap();
    assert!(!row.0.is_nil());
}
