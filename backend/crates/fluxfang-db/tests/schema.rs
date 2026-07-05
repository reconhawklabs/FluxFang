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
