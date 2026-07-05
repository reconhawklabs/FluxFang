//! `EmitterRepo`: `emitter` — a distinct identified source (e.g. a specific
//! access point), optionally grouped under an `entity` and matched against
//! unassigned emissions via a [`fluxfang_core::Rule`] stored as
//! `match_criteria`.
//!
//! ## Matching/backfill (`attach_emissions_matching`, `count_matching`)
//!
//! Both methods translate `rule.conditions` to a parameterized SQL fragment
//! via [`fluxfang_core::conditions_to_sql_checked`] against the `"wifi"`
//! catalog (this schema currently only allows `emission.kind = 'wifi'`, see
//! `repo::emission`'s module docs for the same scoping choice), then apply
//! that fragment as an `AND`-ed clause on top of `emitter_id IS NULL AND
//! kind = 'wifi'`:
//!
//! - [`EmitterRepo::attach_emissions_matching`] runs that WHERE inside an
//!   `UPDATE emission SET emitter_id = $1 ...`, returns the number of rows
//!   it updated, then — in the same transaction — refreshes the emitter's
//!   `first_seen_at`/`last_seen_at` from `MIN`/`MAX(observed_at)` over *all*
//!   emissions now assigned to it (not just the ones just attached, so a
//!   second backfill call widens the window correctly instead of only
//!   reflecting the latest batch).
//! - [`EmitterRepo::count_matching`] runs the identical WHERE as a bare
//!   `SELECT COUNT(*)`, with no `UPDATE` at all — a preview of how many rows
//!   `attach_emissions_matching` would affect, for Task 6.4's preview
//!   endpoint.
//!
//! Binds are threaded the same way [`crate::repo::emission::EmissionRepo`]
//! does: the structured `$1` (emitter id, for `attach_emissions_matching`
//! only) is bound first, then the translator's own binds are appended, in
//! order, uniformly as text — see that module's docs for why re-deriving
//! bind "types" from the conditions after the fact is unsafe.
//!
//! A [`fluxfang_core::RuleSqlError`] from the translator (unknown field,
//! mistyped value, or op/field mismatch) is surfaced as
//! `Err(EmitterRuleError::Rule)` rather than silently skipping the backfill.

use std::fmt;

use fluxfang_core::{catalog_for, conditions_to_sql_checked, Rule, RuleSqlError};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Emitter, NewEmitter};

pub struct EmitterRepo;

/// Column list shared by every query that produces an [`Emitter`]. The `type`
/// column decodes into [`Emitter::type_`] via `#[sqlx(rename = "type")]`, so
/// no `AS` aliasing is needed here (unlike `emission.location`).
const EMITTER_COLUMNS: &str =
    "id, created_at, name, type, entity_id, match_criteria, first_seen_at, last_seen_at";

/// Error from [`EmitterRepo::attach_emissions_matching`] /
/// [`EmitterRepo::count_matching`]: either a DB error, or the rule
/// translator rejecting `rule.conditions` (unknown/mistyped field, or an
/// op/field mismatch).
#[derive(Debug)]
pub enum EmitterRuleError {
    Sql(sqlx::Error),
    Rule(RuleSqlError),
}

impl fmt::Display for EmitterRuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmitterRuleError::Sql(e) => write!(f, "database error: {e}"),
            EmitterRuleError::Rule(e) => write!(f, "invalid rule conditions: {e}"),
        }
    }
}

impl std::error::Error for EmitterRuleError {}

impl From<sqlx::Error> for EmitterRuleError {
    fn from(e: sqlx::Error) -> Self {
        EmitterRuleError::Sql(e)
    }
}

impl From<RuleSqlError> for EmitterRuleError {
    fn from(e: RuleSqlError) -> Self {
        EmitterRuleError::Rule(e)
    }
}

impl EmitterRepo {
    pub async fn insert(pool: &PgPool, new: NewEmitter) -> Result<Emitter, sqlx::Error> {
        let sql = format!(
            "INSERT INTO emitter (name, type, entity_id, match_criteria) \
             VALUES ($1, $2, $3, $4) \
             RETURNING {EMITTER_COLUMNS}"
        );
        sqlx::query_as::<_, Emitter>(&sql)
            .bind(new.name)
            .bind(new.type_)
            .bind(new.entity_id)
            .bind(new.match_criteria)
            .fetch_one(pool)
            .await
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<Emitter>, sqlx::Error> {
        let sql = format!("SELECT {EMITTER_COLUMNS} FROM emitter ORDER BY created_at ASC");
        sqlx::query_as::<_, Emitter>(&sql).fetch_all(pool).await
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<Emitter>, sqlx::Error> {
        let sql = format!("SELECT {EMITTER_COLUMNS} FROM emitter WHERE id = $1");
        sqlx::query_as::<_, Emitter>(&sql)
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// Associate `emitter_id` with `entity_id` (`Some`), or detach it
    /// (`None`).
    pub async fn set_entity(
        pool: &PgPool,
        emitter_id: Uuid,
        entity_id: Option<Uuid>,
    ) -> Result<Emitter, sqlx::Error> {
        let sql =
            format!("UPDATE emitter SET entity_id = $2 WHERE id = $1 RETURNING {EMITTER_COLUMNS}");
        sqlx::query_as::<_, Emitter>(&sql)
            .bind(emitter_id)
            .bind(entity_id)
            .fetch_one(pool)
            .await
    }

    /// Persist a new `match_criteria` rule for `emitter_id`.
    pub async fn update_rule(
        pool: &PgPool,
        emitter_id: Uuid,
        match_criteria: &serde_json::Value,
    ) -> Result<Emitter, sqlx::Error> {
        let sql = format!(
            "UPDATE emitter SET match_criteria = $2 WHERE id = $1 RETURNING {EMITTER_COLUMNS}"
        );
        sqlx::query_as::<_, Emitter>(&sql)
            .bind(emitter_id)
            .bind(match_criteria)
            .fetch_one(pool)
            .await
    }

    /// Backfill: assign every currently-unassigned `kind = 'wifi'` emission
    /// matching `rule` to `emitter_id`, then refresh the emitter's
    /// `first_seen_at`/`last_seen_at` from all emissions now assigned to it.
    /// Returns the number of emissions newly assigned. See module docs for
    /// the SQL/bind approach.
    pub async fn attach_emissions_matching(
        pool: &PgPool,
        emitter_id: Uuid,
        rule: &Rule,
    ) -> Result<u64, EmitterRuleError> {
        let catalog = catalog_for("wifi");
        // `$1` is the emitter id (bound below), so the translator's own
        // placeholders continue from `$2`.
        let (frag, binds) =
            conditions_to_sql_checked(&rule.conditions, rule.match_mode, 2, &catalog)?;

        let update_sql = format!(
            "UPDATE emission SET emitter_id = $1 \
             WHERE emitter_id IS NULL AND kind = 'wifi' AND {frag}"
        );

        let mut tx = pool.begin().await.map_err(EmitterRuleError::Sql)?;

        let mut q = sqlx::query(&update_sql).bind(emitter_id);
        for v in &binds {
            let text = match v.clone() {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            q = q.bind(text);
        }
        let result = q.execute(&mut *tx).await.map_err(EmitterRuleError::Sql)?;
        let affected = result.rows_affected();

        // Refresh first/last_seen_at from ALL emissions now assigned to this
        // emitter (not just the ones just attached), so a later, wider
        // backfill call still produces a correct min/max.
        sqlx::query(
            "UPDATE emitter SET \
                 first_seen_at = sub.min_t, \
                 last_seen_at = sub.max_t \
             FROM ( \
                 SELECT MIN(observed_at) AS min_t, MAX(observed_at) AS max_t \
                 FROM emission WHERE emitter_id = $1 \
             ) sub \
             WHERE emitter.id = $1",
        )
        .bind(emitter_id)
        .execute(&mut *tx)
        .await
        .map_err(EmitterRuleError::Sql)?;

        tx.commit().await.map_err(EmitterRuleError::Sql)?;

        Ok(affected)
    }

    /// Preview how many currently-unassigned `kind = 'wifi'` emissions
    /// `rule` would match, without assigning anything. Same WHERE as
    /// [`Self::attach_emissions_matching`], minus the `UPDATE`.
    pub async fn count_matching(pool: &PgPool, rule: &Rule) -> Result<i64, EmitterRuleError> {
        let catalog = catalog_for("wifi");
        let (frag, binds) =
            conditions_to_sql_checked(&rule.conditions, rule.match_mode, 1, &catalog)?;

        let sql = format!(
            "SELECT COUNT(*) FROM emission WHERE emitter_id IS NULL AND kind = 'wifi' AND {frag}"
        );
        let mut q = sqlx::query_as::<_, (i64,)>(&sql);
        for v in &binds {
            let text = match v.clone() {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            q = q.bind(text);
        }
        let (count,) = q.fetch_one(pool).await.map_err(EmitterRuleError::Sql)?;
        Ok(count)
    }
}
