//! `AppConfigRepo`: the single-row `app_config` table.

use sqlx::PgPool;

use crate::models::AppConfig;

/// `app_config` has exactly one logical row (app-wide settings + the admin
/// password hash). Rather than relying on "whatever row happens to exist"
/// or a separate "has anyone ever inserted a row" check, the singleton is
/// pinned to a fixed, well-known id (the nil UUID). `set_password_hash`
/// upserts on that id, so the first call creates the row and every
/// subsequent call updates it in place; `get`/`password_hash` look it up by
/// that same id and return `None` if it hasn't been created yet (e.g.
/// before first-run setup).
const SINGLETON_ID: uuid::Uuid = uuid::Uuid::nil();

pub struct AppConfigRepo;

impl AppConfigRepo {
    /// The singleton app_config row, or `None` if it hasn't been created
    /// yet (no admin password has ever been set).
    pub async fn get(pool: &PgPool) -> Result<Option<AppConfig>, sqlx::Error> {
        sqlx::query_as::<_, AppConfig>("SELECT * FROM app_config WHERE id = $1")
            .bind(SINGLETON_ID)
            .fetch_optional(pool)
            .await
    }

    /// The stored password hash, or `None` if the singleton row doesn't
    /// exist yet or exists without a hash set. Convenience for the Auth
    /// module, which only cares about this one column.
    pub async fn password_hash(pool: &PgPool) -> Result<Option<String>, sqlx::Error> {
        Ok(Self::get(pool).await?.and_then(|c| c.password_hash))
    }

    /// Create the singleton row with this password hash if it doesn't
    /// exist yet, or overwrite the existing row's hash if it does.
    pub async fn set_password_hash(pool: &PgPool, hash: &str) -> Result<AppConfig, sqlx::Error> {
        sqlx::query_as::<_, AppConfig>(
            "INSERT INTO app_config (id, password_hash) \
             VALUES ($1, $2) \
             ON CONFLICT (id) DO UPDATE SET password_hash = EXCLUDED.password_hash \
             RETURNING *",
        )
        .bind(SINGLETON_ID)
        .bind(hash)
        .fetch_one(pool)
        .await
    }
}
