//! `AppConfigRepo`: the single-row `app_config` table.

use sqlx::PgPool;

use crate::models::AppConfig;
use crate::node_config::NodeConfig;

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
    ///
    /// This is an unconditional overwrite, so it must never be used to
    /// service first-run setup (two concurrent setup requests would race
    /// and the last writer would silently win). It's kept around for a
    /// future *authenticated* "change password" flow, where the caller has
    /// already proven who they are and an overwrite is exactly what's
    /// wanted. See [`Self::set_password_hash_if_unset`] for the atomic,
    /// set-once variant setup actually uses.
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

    /// Atomically set the password hash *only if none is set yet* — the
    /// set-once primitive first-run setup needs so two concurrent
    /// `POST /api/setup` requests can't both "win" (a non-atomic
    /// check-then-act of `password_hash().is_some()` followed by a separate
    /// unconditional upsert has a TOCTOU window: both requests can read
    /// `None`, both pass the check, and both upsert, with the last write
    /// silently becoming the admin password).
    ///
    /// Implemented as a single statement so Postgres serializes it for us:
    /// the `INSERT ... ON CONFLICT (id) DO UPDATE ... WHERE
    /// app_config.password_hash IS NULL` only performs the `DO UPDATE` (and
    /// therefore only matches a row for `RETURNING`) when the existing row's
    /// `password_hash` is still `NULL`. Concurrent callers serialize on the
    /// row's conflict lock, so exactly one of them observes the row with a
    /// `NULL` hash and wins.
    ///
    /// Returns `Some(config)` if this call is the one that set the hash
    /// (the row didn't exist yet, or existed with a `NULL` hash). Returns
    /// `None` if a password was already configured (by this call or an
    /// earlier one) — the caller should treat that the same as "setup
    /// already completed" (e.g. `409 Conflict`).
    pub async fn set_password_hash_if_unset(
        pool: &PgPool,
        hash: &str,
    ) -> Result<Option<AppConfig>, sqlx::Error> {
        sqlx::query_as::<_, AppConfig>(
            "INSERT INTO app_config (id, password_hash) \
             VALUES ($1, $2) \
             ON CONFLICT (id) DO UPDATE SET password_hash = EXCLUDED.password_hash \
             WHERE app_config.password_hash IS NULL \
             RETURNING *",
        )
        .bind(SINGLETON_ID)
        .bind(hash)
        .fetch_optional(pool)
        .await
    }

    /// The persisted node-role config, or `None` if setup hasn't stored one
    /// yet (a fresh singleton has `settings = '{}'`, with no `role` key).
    pub async fn node_config(pool: &PgPool) -> Result<Option<NodeConfig>, sqlx::Error> {
        let Some(cfg) = Self::get(pool).await? else {
            return Ok(None);
        };
        if cfg.settings.get("role").is_none() {
            return Ok(None);
        }
        serde_json::from_value(cfg.settings)
            .map(Some)
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))
    }

    /// First-run setup, atomic set-once: create the singleton row (or fill a
    /// still-blank one) with the admin password hash AND the node-role config
    /// in one statement. Like [`Self::set_password_hash_if_unset`], only the
    /// caller that observes `password_hash IS NULL` performs the write and
    /// gets `Some(_)`; concurrent callers serialize on the row lock and get
    /// `None` (treat as "setup already completed" → 409).
    pub async fn complete_setup(
        pool: &PgPool,
        hash: &str,
        node: &NodeConfig,
    ) -> Result<Option<AppConfig>, sqlx::Error> {
        let settings = serde_json::to_value(node).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
        sqlx::query_as::<_, AppConfig>(
            "INSERT INTO app_config (id, password_hash, settings) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (id) DO UPDATE \
                SET password_hash = EXCLUDED.password_hash, \
                    settings = EXCLUDED.settings \
             WHERE app_config.password_hash IS NULL \
             RETURNING *",
        )
        .bind(SINGLETON_ID)
        .bind(hash)
        .bind(settings)
        .fetch_optional(pool)
        .await
    }
}
