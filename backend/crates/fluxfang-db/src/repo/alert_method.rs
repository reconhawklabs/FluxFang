//! `AlertMethodRepo`: `alert_method` — a reusable, user-configured delivery
//! channel (email / in-app / webhook).
//!
//! `type` is a Rust keyword, so it's renamed to [`AlertMethod::type_`]
//! (same pattern as `Emitter::type_`) — every query here spells out an
//! explicit column list rather than relying on `SELECT *`/`RETURNING *`.
//!
//! `config_encrypted` is an opaque ciphertext blob: Phase 8 wires up the
//! actual encryption/decryption, this repo only stores/returns the bytes
//! unchanged (see `tests/repo_alert_method.rs`'s
//! `config_encrypted_bytea_roundtrips_exactly` for the round-trip proof).

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{AlertMethod, NewAlertMethod};

pub struct AlertMethodRepo;

/// Column list shared by every query that produces an [`AlertMethod`].
pub(crate) const ALERT_METHOD_COLUMNS: &str =
    "id, created_at, name, type, enabled, config, config_encrypted";

impl AlertMethodRepo {
    /// Create a new alert method. `config` (non-secret settings) is left at
    /// its DB default (`{}`); only `config_encrypted` is settable here, per
    /// this repo's interface — nothing above this layer produces plaintext
    /// `config` yet.
    pub async fn insert(pool: &PgPool, new: NewAlertMethod) -> Result<AlertMethod, sqlx::Error> {
        let sql = format!(
            "INSERT INTO alert_method (name, type, enabled, config_encrypted) \
             VALUES ($1, $2, $3, $4) \
             RETURNING {ALERT_METHOD_COLUMNS}"
        );
        sqlx::query_as::<_, AlertMethod>(&sql)
            .bind(new.name)
            .bind(new.type_)
            .bind(new.enabled)
            .bind(new.config_encrypted)
            .fetch_one(pool)
            .await
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<AlertMethod>, sqlx::Error> {
        let sql =
            format!("SELECT {ALERT_METHOD_COLUMNS} FROM alert_method ORDER BY created_at ASC");
        sqlx::query_as::<_, AlertMethod>(&sql).fetch_all(pool).await
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<AlertMethod>, sqlx::Error> {
        let sql = format!("SELECT {ALERT_METHOD_COLUMNS} FROM alert_method WHERE id = $1");
        sqlx::query_as::<_, AlertMethod>(&sql)
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// Replace `name`, `enabled`, and `config_encrypted`. `type_` never
    /// changes after creation (same convention as `DataSourceRepo::update`
    /// leaving `kind` immutable).
    pub async fn update(
        pool: &PgPool,
        id: Uuid,
        name: &str,
        enabled: bool,
        config_encrypted: Vec<u8>,
    ) -> Result<AlertMethod, sqlx::Error> {
        let sql = format!(
            "UPDATE alert_method SET name = $2, enabled = $3, config_encrypted = $4 \
             WHERE id = $1 RETURNING {ALERT_METHOD_COLUMNS}"
        );
        sqlx::query_as::<_, AlertMethod>(&sql)
            .bind(id)
            .bind(name)
            .bind(enabled)
            .bind(config_encrypted)
            .fetch_one(pool)
            .await
    }

    /// Delete an alert method, returning whether a row was actually removed.
    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM alert_method WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
