//! fluxfang-db: Postgres connection pool + embedded migration runner.
//!
//! This crate is intentionally minimal for Task 1.1: it provides a pool
//! constructor and a migration runner. Application schema/tables are added
//! in later tasks (see `backend/migrations`).

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub mod models;
pub mod repo;
pub mod sort;

pub use repo::{
    AlertMethodRepo, AlertRuleRepo, AppConfigRepo, DataSourceRepo, EmissionRepo, EmitterRepo,
    EntityRepo, LocationRepo, NotificationRepo, SessionRepo, ZoneMembershipRepo, ZoneRepo,
};
pub use sort::resolve_order_by;

/// Connect to Postgres and return a ready-to-use connection pool.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
}

/// Apply all embedded migrations (idempotent).
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}
