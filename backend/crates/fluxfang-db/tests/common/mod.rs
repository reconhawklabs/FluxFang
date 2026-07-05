//! Shared test harness for `fluxfang-db` integration tests.
//!
//! ## Isolation strategy: one Postgres *schema* per test
//!
//! Every call to [`fresh_pool`] creates a brand-new, uniquely-named Postgres
//! schema (`test_<uuid-no-dashes>`) inside the database pointed at by
//! `DATABASE_URL`, points the returned pool's `search_path` at
//! `"<that schema>, public"`, and runs the full embedded migration set
//! *into that schema*. Because `search_path` puts the fresh schema first,
//! every unqualified table reference (`app_config`, `data_source`, ...)
//! resolves to that test's private copy of the schema — including sqlx's
//! own `_sqlx_migrations` bookkeeping table — so tests running concurrently
//! (e.g. `cargo test` with its default multi-threaded test runner) never
//! see each other's rows. `public` stays on the search_path (after the test
//! schema) so PostGIS's `geography`/`geometry` types and functions (which
//! live in `public`, created once for the whole database) keep resolving
//! everywhere.
//!
//! This was chosen over a fully separate *database* per test because
//! `CREATE SCHEMA` is far cheaper than `CREATE DATABASE` and requires no
//! superuser/template-database dance; `sqlx::migrate!` doesn't care whether
//! it's pointed at a schema or a database, it just runs DDL through
//! whatever connection (and therefore whatever `search_path`) it's given.
//!
//! The `after_connect` hook re-issues `SET search_path` on *every* new
//! physical connection sqlx opens for the pool (not just the first), since
//! `search_path` is a per-session setting and the pool may open more than
//! one connection under concurrent test bodies.
//!
//! Schemas are intentionally left behind after each test run (Postgres has
//! no "drop this schema after my session ends" primitive short of a
//! temporary-table-like mechanism, and dropping from inside the same
//! connection that's using it isn't possible). Since `fluxfang_test` is a
//! disposable, non-production database, an occasional
//! `DROP SCHEMA test_* CASCADE` sweep (or just recreating the database) is
//! sufficient housekeeping; this harness does not automate it to keep the
//! per-test path simple.

// Each integration test binary compiles this module fresh and only uses a
// subset of its helpers, so an unused-fn lint would fire in most of them.
#![allow(dead_code)]

use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, PgPool};
use uuid::Uuid;

/// Build a pool bound to a fresh, isolated schema with all migrations
/// applied. See module docs for the isolation approach.
pub async fn fresh_pool() -> PgPool {
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for fluxfang-db tests (see task-1.3a-report.md)");

    let schema = format!("test_{}", Uuid::new_v4().simple());

    // A short-lived single connection just to create the schema; the main
    // pool below (with its own after_connect hook) is what tests use.
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("connect to DATABASE_URL to create test schema");
    admin
        .execute(format!(r#"CREATE SCHEMA "{schema}""#).as_str())
        .await
        .expect("create isolated test schema");
    admin.close().await;

    let search_path = format!(r#""{schema}", public"#);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |conn, _meta| {
            let search_path = search_path.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {search_path}").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&database_url)
        .await
        .expect("connect to DATABASE_URL with isolated search_path");

    fluxfang_db::run_migrations(&pool)
        .await
        .expect("run migrations into isolated test schema");

    pool
}

/// Seed a single wifi/monitor `data_source` row, returning its id. Shared
/// by every repo test that needs *a* valid data source to attach rows to
/// (e.g. later `EmissionRepo` tests) without caring about its exact fields.
pub async fn seed_wifi_source(pool: &PgPool) -> Uuid {
    use fluxfang_db::models::NewDataSource;
    use fluxfang_db::DataSourceRepo;

    let ds = DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .expect("seed wifi data_source");
    ds.id
}

/// Seed a single gps/gpsd `data_source` row, returning its id.
pub async fn seed_gps_source(pool: &PgPool) -> Uuid {
    use fluxfang_db::models::NewDataSource;
    use fluxfang_db::DataSourceRepo;

    let ds = DataSourceRepo::insert(pool, NewDataSource::gps_gpsd())
        .await
        .expect("seed gps data_source");
    ds.id
}
