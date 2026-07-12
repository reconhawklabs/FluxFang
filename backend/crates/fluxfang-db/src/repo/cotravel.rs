//! `CoTravelRepo`: the Co-Travel Detection page's read model.
//!
//! This module owns two things: the per-emitter co-travel aggregate query
//! (`candidates`, added in a later task) and the `cotravel_ignore` list CRUD
//! (`ignore`/`unignore`/`list_ignored`). Scoring/tiering is deliberately NOT
//! here — that's `fluxfang_core::cotravel::score`, a pure function the API
//! layer applies to each `CoTravelCandidate` this repo returns.

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

pub struct CoTravelRepo;

/// One ignored emitter, projected for the Ignored panel (identity only — the
/// panel just needs to show what it is and offer Restore).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IgnoredEmitter {
    pub id: Uuid,
    pub name: String,
    pub emitter_type: Option<String>,
    pub identity_key: Option<String>,
    pub attributes: serde_json::Value,
}

impl CoTravelRepo {
    /// Add `emitter_id` to the ignore list. Idempotent: ignoring an already-
    /// ignored emitter is a no-op (`ON CONFLICT DO NOTHING`), never an error.
    pub async fn ignore(pool: &PgPool, emitter_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO cotravel_ignore (emitter_id) VALUES ($1) \
             ON CONFLICT (emitter_id) DO NOTHING",
        )
        .bind(emitter_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Remove `emitter_id` from the ignore list, returning how many rows were
    /// removed (0 if it wasn't ignored — not an error).
    pub async fn unignore(pool: &PgPool, emitter_id: Uuid) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM cotravel_ignore WHERE emitter_id = $1")
            .bind(emitter_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Every ignored emitter, newest-ignored first — the Ignored panel's source.
    pub async fn list_ignored(pool: &PgPool) -> Result<Vec<IgnoredEmitter>, sqlx::Error> {
        sqlx::query_as::<_, IgnoredEmitter>(
            "SELECT e.id, e.name, e.emitter_type, e.identity_key, e.attributes \
             FROM cotravel_ignore ci \
             JOIN emitter e ON e.id = ci.emitter_id \
             ORDER BY ci.created_at DESC",
        )
        .fetch_all(pool)
        .await
    }
}
