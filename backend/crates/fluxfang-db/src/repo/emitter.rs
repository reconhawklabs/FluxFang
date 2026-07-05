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
//!
//! ## Per-emission seen-window update (`touch_seen`)
//!
//! [`EmitterRepo::touch_seen`] is the single-row counterpart used by
//! `fluxfang-api::ingest`'s auto-attach (Task 5.2): when one freshly-inserted
//! emission is matched to an emitter (via in-process [`fluxfang_core::rule::eval`],
//! not the SQL translator above), this widens that emitter's
//! `first_seen_at`/`last_seen_at` by exactly that one emission's
//! `observed_at`, with the same `LEAST`/`GREATEST`-against-`COALESCE` idiom
//! `attach_emissions_matching` uses for its bulk `MIN`/`MAX` refresh.
//!
//! ## Task 6.4 additions: CRUD + atomic entity creation
//!
//! [`EmitterRepo::update_basic`] (name/type) and [`EmitterRepo::delete`]
//! round out plain CRUD alongside the already-existing `insert`/`list`/`get`.
//! [`EmitterRepo::create_with_entity`] is the one non-trivial addition: it
//! creates a new `entity`, a new `emitter` associated to it, and (if a rule
//! is given) runs the exact same backfill-and-refresh sequence as
//! `attach_emissions_matching`, all inside a single transaction — see its
//! own doc comment for why an invalid rule there rolls back the entity
//! insert too, rather than leaving an orphaned `entity` row behind.

use std::fmt;

use chrono::{DateTime, Utc};
use fluxfang_core::{catalog_for, conditions_to_sql_checked, Rule, RuleSqlError};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Emitter, Entity, NewEmitter, NewEntity};
use crate::repo::entity::ENTITY_COLUMNS;

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

    /// Every emitter grouped under `entity_id`, oldest first. Empty `Vec`
    /// (not an error) if the entity has no emitters or doesn't exist. Task
    /// 6.5's `GET /api/entities/:id` detail response uses this for its
    /// `emitters` field.
    pub async fn list_by_entity(
        pool: &PgPool,
        entity_id: Uuid,
    ) -> Result<Vec<Emitter>, sqlx::Error> {
        let sql = format!(
            "SELECT {EMITTER_COLUMNS} FROM emitter WHERE entity_id = $1 ORDER BY created_at ASC"
        );
        sqlx::query_as::<_, Emitter>(&sql)
            .bind(entity_id)
            .fetch_all(pool)
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

    /// Update `name`/`type` in place. `type_` is `Option<&str>` (not
    /// `Option<Option<&str>>`): whenever a caller wants to change either
    /// column, both are re-sent as the row's full desired value (`type_ =
    /// None` clears it), same "PATCH re-sends the fields it's touching"
    /// convention `data_sources::update_data_source` uses for `mode`. The
    /// `fluxfang-api` handler is responsible for resolving "field omitted
    /// from the request" against the existing row before calling this.
    pub async fn update_basic(
        pool: &PgPool,
        id: Uuid,
        name: &str,
        type_: Option<&str>,
    ) -> Result<Emitter, sqlx::Error> {
        let sql = format!(
            "UPDATE emitter SET name = $2, type = $3 WHERE id = $1 RETURNING {EMITTER_COLUMNS}"
        );
        sqlx::query_as::<_, Emitter>(&sql)
            .bind(id)
            .bind(name)
            .bind(type_)
            .fetch_one(pool)
            .await
    }

    /// Delete an emitter, returning whether a row was actually removed.
    /// `emission.emitter_id` is `ON DELETE SET NULL` (see
    /// `migrations/0001_init.sql`), so its emissions survive, just
    /// unassigned again.
    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM emitter WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
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

    /// Widen `emitter_id`'s `first_seen_at`/`last_seen_at` window to include
    /// a single `observed_at` -- the per-emission counterpart to
    /// `attach_emissions_matching`'s bulk `MIN`/`MAX` refresh, used by
    /// `fluxfang-api::ingest`'s auto-attach right after one new emission is
    /// assigned to this emitter. `LEAST`/`GREATEST` against
    /// `COALESCE(first_seen_at/last_seen_at, $2)` widens the existing window
    /// in either direction (an emission can arrive out of order) and also
    /// handles the first-ever-attach case, where both columns start `NULL`,
    /// in the same statement.
    pub async fn touch_seen(
        pool: &PgPool,
        emitter_id: Uuid,
        observed_at: DateTime<Utc>,
    ) -> Result<Emitter, sqlx::Error> {
        let sql = format!(
            "UPDATE emitter SET \
                 first_seen_at = LEAST(COALESCE(first_seen_at, $2), $2), \
                 last_seen_at = GREATEST(COALESCE(last_seen_at, $2), $2) \
             WHERE id = $1 RETURNING {EMITTER_COLUMNS}"
        );
        sqlx::query_as::<_, Emitter>(&sql)
            .bind(emitter_id)
            .bind(observed_at)
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

    /// Atomically create a new `entity`, create a new `emitter` associated
    /// to it, and (if `rule` is given) backfill-attach matching emissions —
    /// all inside one transaction, so a failure at any step (in particular
    /// an invalid `rule`) leaves neither row behind. Task 6.4's `POST
    /// /api/emitters/with-entity`.
    ///
    /// Mirrors `attach_emissions_matching`'s catalog-check-then-`UPDATE`-
    /// then-refresh sequence exactly, just run against `&mut *tx` instead of
    /// `pool` directly so it shares the same transaction as the two
    /// `INSERT`s. `fluxfang-api` is expected to have already validated
    /// `rule` (e.g. via a throwaway `conditions_to_sql_checked` call) before
    /// calling this, but the check here is real, not just defensive: it's
    /// the same one `attach_emissions_matching` runs, so an invalid rule
    /// still surfaces as `EmitterRuleError::Rule` and rolls back cleanly
    /// (the transaction is simply dropped, never committed).
    pub async fn create_with_entity(
        pool: &PgPool,
        new_entity: NewEntity,
        emitter_name: String,
        emitter_type: Option<String>,
        match_criteria: serde_json::Value,
        rule: Option<&Rule>,
    ) -> Result<EmitterWithEntity, EmitterRuleError> {
        let mut tx = pool.begin().await.map_err(EmitterRuleError::Sql)?;

        let entity = sqlx::query_as::<_, Entity>(&format!(
            "INSERT INTO entity (name, notes) VALUES ($1, $2) RETURNING {ENTITY_COLUMNS}"
        ))
        .bind(new_entity.name)
        .bind(new_entity.notes)
        .fetch_one(&mut *tx)
        .await
        .map_err(EmitterRuleError::Sql)?;

        let emitter_sql = format!(
            "INSERT INTO emitter (name, type, entity_id, match_criteria) \
             VALUES ($1, $2, $3, $4) \
             RETURNING {EMITTER_COLUMNS}"
        );
        let mut emitter = sqlx::query_as::<_, Emitter>(&emitter_sql)
            .bind(emitter_name)
            .bind(emitter_type)
            .bind(Some(entity.id))
            .bind(match_criteria)
            .fetch_one(&mut *tx)
            .await
            .map_err(EmitterRuleError::Sql)?;

        let mut attached_count: u64 = 0;
        if let Some(rule) = rule {
            let catalog = catalog_for("wifi");
            let (frag, binds) =
                conditions_to_sql_checked(&rule.conditions, rule.match_mode, 2, &catalog)?;

            let update_sql = format!(
                "UPDATE emission SET emitter_id = $1 \
                 WHERE emitter_id IS NULL AND kind = 'wifi' AND {frag}"
            );
            let mut q = sqlx::query(&update_sql).bind(emitter.id);
            for v in &binds {
                let text = match v.clone() {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                q = q.bind(text);
            }
            let result = q.execute(&mut *tx).await.map_err(EmitterRuleError::Sql)?;
            attached_count = result.rows_affected();

            emitter = sqlx::query_as::<_, Emitter>(&format!(
                "UPDATE emitter SET \
                     first_seen_at = sub.min_t, \
                     last_seen_at = sub.max_t \
                 FROM ( \
                     SELECT MIN(observed_at) AS min_t, MAX(observed_at) AS max_t \
                     FROM emission WHERE emitter_id = $1 \
                 ) sub \
                 WHERE emitter.id = $1 \
                 RETURNING {EMITTER_COLUMNS}"
            ))
            .bind(emitter.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(EmitterRuleError::Sql)?;
        }

        tx.commit().await.map_err(EmitterRuleError::Sql)?;

        Ok(EmitterWithEntity {
            emitter,
            entity,
            attached_count,
        })
    }
}

/// Result of [`EmitterRepo::create_with_entity`]: the freshly-created
/// emitter (with `first_seen_at`/`last_seen_at` already refreshed if a rule
/// backfilled anything), the entity it's now associated to, and how many
/// emissions the backfill attached.
#[derive(Debug, Clone)]
pub struct EmitterWithEntity {
    pub emitter: Emitter,
    pub entity: Entity,
    pub attached_count: u64,
}
