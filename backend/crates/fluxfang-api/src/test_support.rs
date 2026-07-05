//! Shared `#[cfg(test)]` schema-isolation harness for this crate's *in-crate*
//! unit tests (`ingest::{mod, session, zones, alerts}`'s `#[cfg(test)] mod
//! tests` blocks).
//!
//! Each of those modules used to carry its own copy-pasted
//! `sweep_leftover_test_schemas`/`fresh_pool`/`SWEEP_DONE` trio. That meant
//! four independent `OnceCell`s racing each other *within the same test
//! binary*: module A's sweep could run concurrently with module B's
//! `fresh_pool` and `DROP SCHEMA CASCADE` a schema B had just created and
//! was actively migrating into, since nothing coordinated the two "run
//! once per binary" guards against each other. Consolidating them here
//! gives the whole binary exactly one sweep and one `OnceCell`.
//!
//! That still isn't enough on its own, though: this crate's *integration*
//! tests (`tests/common/mod.rs`, a separate binary) and `fluxfang-db`'s
//! integration tests (another separate binary, another database) each run
//! their own independent copy of this same sweep, and nothing guarantees
//! those processes don't overlap in time (`cargo test`'s binary-at-a-time
//! default isn't guaranteed by any tool here, and e.g. `cargo-nextest` runs
//! binaries concurrently by default). So on top of "one sweep per binary",
//! every copy of this sweep is *age-gated*: see [`sweep_leftover_test_schemas`].

use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, PgPool};
use tokio::sync::OnceCell;
use uuid::Uuid;

/// A leftover `test_*` schema is only swept once it's at least this old.
/// No single test (let alone a whole binary's run) should take anywhere
/// near this long, so a schema still around after this window has to
/// belong to a process that already exited without cleaning up after
/// itself -- never one that's still in flight.
const SWEEP_MAX_AGE_MILLIS: u128 = 15 * 60 * 1000;

static SWEEP_DONE: OnceCell<()> = OnceCell::const_new();

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after the UNIX epoch")
        .as_millis()
}

/// Parses the creation timestamp embedded in a `test_<epoch_millis>_<uuid>`
/// schema name (see [`fresh_pool`]). Schemas that don't match this scheme
/// (e.g. hand-created during manual debugging) return `None` and are left
/// alone by the sweep rather than guessed at.
fn parse_created_millis(schema: &str) -> Option<u128> {
    let rest = schema.strip_prefix("test_")?;
    let (millis, _uuid) = rest.split_once('_')?;
    millis.parse().ok()
}

/// Best-effort cleanup of `test_*` schemas left behind by earlier test
/// runs. Never panics -- every step swallows its own errors -- because
/// this is pure housekeeping and must never be allowed to break test
/// setup.
///
/// Age-gated: a schema is only dropped once [`parse_created_millis`] shows
/// it's older than [`SWEEP_MAX_AGE_MILLIS`]. A schema younger than that
/// could belong to a concurrently-running test binary (this crate's own
/// integration tests, or `fluxfang-db`'s) that is actively migrating or
/// using it, so sweeping it away purely because *this* process happened
/// to start its sweep first would be unsafe.
async fn sweep_leftover_test_schemas(database_url: &str) {
    let Ok(admin) = PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
    else {
        return;
    };

    let schemas: Result<Vec<(String,)>, _> = sqlx::query_as(
        "SELECT schema_name FROM information_schema.schemata \
         WHERE schema_name LIKE 'test\\_%' ESCAPE '\\'",
    )
    .fetch_all(&admin)
    .await;

    if let Ok(schemas) = schemas {
        let now = now_millis();
        for (schema,) in schemas {
            let is_stale = matches!(
                parse_created_millis(&schema),
                Some(created) if now.saturating_sub(created) > SWEEP_MAX_AGE_MILLIS
            );
            if is_stale {
                let _ = admin
                    .execute(format!(r#"DROP SCHEMA IF EXISTS "{schema}" CASCADE"#).as_str())
                    .await;
            }
        }
    }

    admin.close().await;
}

/// Build a pool bound to a fresh, isolated schema
/// (`test_<epoch_millis>_<uuid-no-dashes>`) with all migrations applied.
/// Shared by every `#[cfg(test)] mod tests` under `ingest::*` -- see the
/// module docs for why a single shared copy (rather than the
/// per-module-copy this replaced) matters.
pub(crate) async fn fresh_pool() -> PgPool {
    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for fluxfang-api tests");

    SWEEP_DONE
        .get_or_init(|| sweep_leftover_test_schemas(&database_url))
        .await;

    let schema = format!("test_{}_{}", now_millis(), Uuid::new_v4().simple());

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
