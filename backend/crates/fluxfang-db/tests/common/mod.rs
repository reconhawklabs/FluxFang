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
//! disposable, non-production database, [`sweep_leftover_test_schemas`]
//! performs a best-effort, *age-gated* `DROP SCHEMA test_* CASCADE` sweep
//! once per test *binary process*, before that binary creates any schema
//! of its own. Age-gated, not merely ordering-gated: this crate's own
//! integration-test binaries, `fluxfang-api`'s integration tests, and
//! `fluxfang-api`'s in-crate unit tests (`ingest::*`) each run this same
//! sweep independently against the same database, and nothing here
//! guarantees those processes never overlap in time (e.g. `cargo-nextest`
//! runs test binaries concurrently by default, unlike plain `cargo test`).
//! So rather than assuming "any `test_*` schema found by my sweep must
//! belong to an already-exited process," each schema name embeds its own
//! creation timestamp (`test_<epoch_millis>_<uuid>`) and the sweep only
//! drops ones older than a safe threshold — see
//! [`sweep_leftover_test_schemas`]'s doc comment for the exact rule.

// Each integration test binary compiles this module fresh and only uses a
// subset of its helpers, so an unused-fn lint would fire in most of them.
#![allow(dead_code)]

use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, PgPool};
use tokio::sync::OnceCell;
use uuid::Uuid;

/// Guards [`sweep_leftover_test_schemas`] so it runs at most once per test
/// binary process, no matter how many concurrent test threads call
/// [`fresh_pool`] simultaneously (`OnceCell::get_or_init` makes every
/// caller but the first `.await` the first caller's in-flight sweep rather
/// than each running their own).
static SWEEP_DONE: OnceCell<()> = OnceCell::const_new();

/// A leftover `test_*` schema is only swept once it's at least this old.
/// No single test (let alone a whole binary's run) should take anywhere
/// near this long, so a schema still around after this window has to
/// belong to a process that already exited without cleaning up after
/// itself — never one that's still in flight.
const SWEEP_MAX_AGE_MILLIS: u128 = 15 * 60 * 1000;

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after the UNIX epoch")
        .as_millis()
}

/// Parses the creation timestamp embedded in a `test_<epoch_millis>_<uuid>`
/// schema name (see [`fresh_pool`]). Schemas that don't match this scheme
/// return `None` and are left alone by the sweep rather than guessed at.
fn parse_created_millis(schema: &str) -> Option<u128> {
    let rest = schema.strip_prefix("test_")?;
    let (millis, _uuid) = rest.split_once('_')?;
    millis.parse().ok()
}

/// Best-effort cleanup of `test_*` schemas left behind by earlier test
/// runs. Never panics — every step swallows its own errors — because this
/// is pure housekeeping and must never be allowed to break test setup.
///
/// Age-gated, not merely ordering-gated: a schema is only dropped once
/// [`parse_created_millis`] shows it's older than [`SWEEP_MAX_AGE_MILLIS`].
/// A schema younger than that could belong to a *concurrently-running*
/// sibling process — another integration-test binary in this same crate,
/// `fluxfang-api`'s integration tests, or `fluxfang-api`'s in-crate unit
/// tests — that is actively migrating/using it, so sweeping it away
/// purely because this process happened to run its sweep first would be
/// unsafe. (Previously this relied on `cargo test`'s default of running
/// one integration-test binary at a time; that's not guaranteed by any
/// tool in this workspace and doesn't hold at all for e.g. `cargo-nextest`
/// or `-j`-parallel test runners, so it was a latent race even before any
/// such tool was introduced.)
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

/// Build a pool bound to a fresh, isolated schema with all migrations
/// applied. See module docs for the isolation approach.
pub async fn fresh_pool() -> PgPool {
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for fluxfang-db tests (see task-1.3a-report.md)");

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

/// Open a `survey_session` row, returning its id. `emission.session_id`
/// (via `NewEmission::session_id`) is a required FK, so `EmissionRepo`
/// tests need a real session row to attach rows to.
///
/// Several callers (e.g. `repo_emission.rs`'s session-filter test) call
/// this more than once per test purely as an FK factory to get distinct
/// session ids — with no interest in "active session" semantics at all.
/// Since Task 5.1's `0002_single_active_session.sql` makes it a hard DB
/// error to have two rows with `ended_at IS NULL` at once, this
/// self-heals (closes whatever's currently active, exactly like
/// `SessionManager::open` does in production) before opening the next
/// one, so those FK-factory call sites keep working unmodified.
pub async fn seed_session(pool: &PgPool) -> Uuid {
    use fluxfang_db::SessionRepo;

    SessionRepo::close_active(pool)
        .await
        .expect("self-heal: close any active survey_session");
    let session = SessionRepo::open(pool).await.expect("seed survey_session");
    session.id
}
