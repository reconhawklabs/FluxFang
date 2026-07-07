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
//! that fragment as an `AND`-ed clause on top of just `kind = 'wifi'` —
//! **regardless of the emission's current `emitter_id`**. This is
//! deliberate, not an oversight: ingest auto-create (Phase A) assigns every
//! matching emission to an auto-created emitter immediately, so an
//! `emitter_id IS NULL` predicate here would make the "Matches X" preview
//! always show 0 and would make a manual "group these into an emitter"
//! action silently attach nothing. Both methods instead count/claim ALL
//! matching emissions of that kind, whoever currently holds them:
//!
//! - [`EmitterRepo::attach_emissions_matching`] runs that WHERE inside an
//!   `UPDATE emission SET emitter_id = $1 ...`, **reassigning** every
//!   matching emission to `emitter_id` even if it was already assigned to a
//!   different emitter (e.g. an auto-created one), returns the number of
//!   rows it updated, then — in the same transaction — refreshes the
//!   emitter's `first_seen_at`/`last_seen_at` from `MIN`/`MAX(observed_at)`
//!   over *all* emissions now assigned to it (not just the ones just
//!   attached, so a second backfill call widens the window correctly
//!   instead of only reflecting the latest batch).
//! - [`EmitterRepo::count_matching`] runs the identical WHERE as a bare
//!   `SELECT COUNT(*)`, with no `UPDATE` at all — an accurate preview of how
//!   many rows `attach_emissions_matching` would affect (including
//!   already-assigned ones it would reclaim), for Task 6.4's preview
//!   endpoint.
//!
//! (Known limitation, out of scope here: an older auto-created emitter with
//! an overlapping rule can re-claim FUTURE matching emissions right back
//! from a manually-grouped emitter unless its `match_enabled` rule is
//! disabled — see the design doc.)
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
//!
//! ## Phase A1 additions: classification columns + race-safe get-or-create
//!
//! [`EmitterRepo::get_or_create_by_identity`] is the one non-trivial
//! addition: it's what a future ingest auto-create path will call to
//! atomically look up-or-insert an emitter by `identity_key`, race-safe
//! under concurrent ingest (see its own doc comment for the `ON CONFLICT
//! ... DO NOTHING` + fallback-`SELECT` mechanics). [`EmitterRepo::insert`]
//! and `EMITTER_COLUMNS` were extended to carry the four new columns
//! (`emitter_type`/`attributes`/`match_enabled`/`identity_key`;
//! `0004_emitter_classification.sql`) through every existing query
//! unchanged. [`EmitterRepo::set_match_enabled`] and
//! [`EmitterRepo::set_attributes`] round out minimal, single-column
//! mutators for the two columns a later phase's API/UI needs to update
//! independently of everything else on the row (toggling the auto-attach
//! rule; overriding e.g. a detected `randomized_mac`). No ingest/API/
//! classification logic lives in this crate yet — see the design doc's
//! "Build order" for what's deferred to later phases.
//!
//! ## Phase 1b: `query` (list search/filter/pagination) + a backfill
//! consistency fix
//!
//! [`EmitterRepo::query`] (with [`EmitterListFilter`]) is the emitters list
//! page's search + entity filter + pagination, following the same
//! dynamic-WHERE/bind-threading shape as `repo::emission::EmissionRepo::query`
//! — see its own doc comment for the exact SQL.
//!
//! Separately: [`EmitterRepo::create_with_entity`]'s backfill previously kept
//! its own `WHERE emitter_id IS NULL AND ...` guard, left over from before
//! `attach_emissions_matching`/`count_matching` were changed (see the
//! "Matching/backfill" section above) to reassign ALL matching emissions
//! rather than only unassigned ones. That made `create_with_entity`'s
//! backfill behave differently from the other two paths — a rule that only
//! matched already-assigned emissions would silently attach nothing when
//! creating a new entity+emitter together, while `attach_emissions_matching`/
//! `count_matching` would reclaim them. The guard is now dropped here too,
//! so all three paths share identical "reassign every matching `kind =
//! 'wifi'` emission" semantics.

use std::fmt;

use chrono::{DateTime, Utc};
use fluxfang_core::{catalog_for, conditions_to_sql_checked, Rule, RuleSqlError};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Emitter, Entity, NewEmitter, NewEntity};
use crate::repo::entity::ENTITY_COLUMNS;
use crate::sort::resolve_order_by;

pub struct EmitterRepo;

/// Filter/paginate criteria for [`EmitterRepo::query`] (Phase 1b's emitter
/// list search + entity filter + pagination). Every field defaults to "no
/// constraint" via [`Default`]; `limit`/`offset` default to a sane first
/// page, same convention as `repo::emission::EmissionFilter`.
///
/// `search`, when `Some`, is a single case-insensitive substring matched
/// across `name`, `type`, `attributes::text`, and `match_criteria::text` —
/// so typing a MAC/BSSID/SSID that only appears inside the JSON
/// `attributes`/`match_criteria` columns (not the plain `name`/`type`
/// columns) still finds the emitter. See [`EmitterRepo::query`]'s own doc
/// comment for the exact SQL and why it's injection-safe.
///
/// `emitter_type`, when `Some`, is an exact match against the `emitter_type`
/// column (e.g. `"wifi_access_point"`) — the Emitters page's Type-filter
/// dropdown. A `NULL` (unclassified) `emitter_type` never matches, same as a
/// plain SQL `=` comparison against `NULL`.
#[derive(Debug, Clone)]
pub struct EmitterListFilter {
    pub search: Option<String>,
    pub entity_id: Option<Uuid>,
    pub emitter_type: Option<String>,
    pub limit: i64,
    pub offset: i64,
    pub sort: Option<String>,
    pub dir: Option<String>,
}

impl Default for EmitterListFilter {
    fn default() -> Self {
        Self {
            search: None,
            entity_id: None,
            emitter_type: None,
            limit: 50,
            offset: 0,
            sort: None,
            dir: None,
        }
    }
}

/// Column list shared by every query that produces an [`Emitter`]. The `type`
/// column decodes into [`Emitter::type_`] via `#[sqlx(rename = "type")]`, so
/// no `AS` aliasing is needed here (unlike `emission.location`). Phase A1
/// (`0004_emitter_classification.sql`) added `emitter_type`/`attributes`/
/// `match_enabled`/`identity_key`; sqlx's `FromRow` derive maps by column
/// name, not position, so appending them here is enough for every query
/// built from this constant to pick them up.
const EMITTER_COLUMNS: &str = "id, created_at, name, type, entity_id, match_criteria, \
     first_seen_at, last_seen_at, emitter_type, attributes, match_enabled, identity_key";

/// Allow-listed emitter sort keys -> SQL ordering expressions. `identity`
/// mirrors `MacIdentityCell`'s display precedence; `emissions` orders by the
/// correlated-count alias selected below.
const EMITTER_SORTS: &[(&str, &str)] = &[
    ("name", "name"),
    (
        "identity",
        "COALESCE(attributes->>'bssid', attributes->>'src_mac', attributes->>'address')",
    ),
    ("first_seen", "first_seen_at"),
    ("last_seen", "last_seen_at"),
    ("emissions", "emission_count"),
];

/// One row of the emitter list query: the emitter plus its correlated
/// emission count. `#[sqlx(flatten)]` maps the `EMITTER_COLUMNS` into
/// `emitter` and the extra `emission_count` alias into its own field.
#[derive(sqlx::FromRow)]
struct EmitterListRow {
    #[sqlx(flatten)]
    emitter: Emitter,
    emission_count: i64,
}

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
            "INSERT INTO emitter \
                 (name, type, entity_id, match_criteria, emitter_type, attributes, \
                  match_enabled, identity_key) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             RETURNING {EMITTER_COLUMNS}"
        );
        sqlx::query_as::<_, Emitter>(&sql)
            .bind(new.name)
            .bind(new.type_)
            .bind(new.entity_id)
            .bind(new.match_criteria)
            .bind(new.emitter_type)
            .bind(new.attributes)
            .bind(new.match_enabled)
            .bind(new.identity_key)
            .fetch_one(pool)
            .await
    }

    /// Atomic get-or-create keyed on `new.identity_key`, which **must** be
    /// `Some` (this is the auto-create path's entry point; a user-made
    /// emitter with `identity_key = None` should go through
    /// [`Self::insert`] instead — `None` can't be the conflict target of an
    /// `ON CONFLICT (identity_key)` clause since Postgres never considers
    /// two `NULL`s equal for that purpose, so calling this with `None`
    /// would just always insert a fresh row instead of erroring, which
    /// would be a silent footgun. Rather than return a `Result` for that
    /// caller-error case, it's a documented precondition (`debug_assert!`)
    /// — every real caller (ingest auto-create) always has a concrete
    /// identity key by construction).
    ///
    /// Race-safety: `INSERT ... ON CONFLICT (identity_key) DO NOTHING
    /// RETURNING ...` either (a) wins the race and returns the freshly
    /// inserted row, or (b) loses it (another concurrent call with the same
    /// `identity_key` committed first) and returns no row at all — Postgres
    /// guarantees exactly one of the two outcomes even under concurrent
    /// transactions targeting the same unique index, because the conflict
    /// is detected and resolved at the index level, not via a
    /// read-then-write race in application code. Case (b) falls back to a
    /// plain `SELECT` for the row the other caller created. Returns the
    /// emitter plus whether *this* call created it.
    pub async fn get_or_create_by_identity(
        pool: &PgPool,
        new: NewEmitter,
    ) -> Result<(Emitter, bool), sqlx::Error> {
        debug_assert!(
            new.identity_key.is_some(),
            "get_or_create_by_identity requires Some(identity_key); use insert() for user-made emitters"
        );

        let insert_sql = format!(
            "INSERT INTO emitter \
                 (name, type, entity_id, match_criteria, emitter_type, attributes, \
                  match_enabled, identity_key) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (identity_key) DO NOTHING \
             RETURNING {EMITTER_COLUMNS}"
        );
        let inserted = sqlx::query_as::<_, Emitter>(&insert_sql)
            .bind(&new.name)
            .bind(&new.type_)
            .bind(new.entity_id)
            .bind(&new.match_criteria)
            .bind(&new.emitter_type)
            .bind(&new.attributes)
            .bind(new.match_enabled)
            .bind(&new.identity_key)
            .fetch_optional(pool)
            .await?;

        if let Some(emitter) = inserted {
            return Ok((emitter, true));
        }

        let select_sql = format!("SELECT {EMITTER_COLUMNS} FROM emitter WHERE identity_key = $1");
        let existing = sqlx::query_as::<_, Emitter>(&select_sql)
            .bind(&new.identity_key)
            .fetch_one(pool)
            .await?;
        Ok((existing, false))
    }

    /// Flip `match_enabled` for `id` — disables/re-enables the emitter's
    /// auto-attach rule without touching anything else about it (the
    /// general "rule enable/disable" capability the design calls for).
    pub async fn set_match_enabled(
        pool: &PgPool,
        id: Uuid,
        enabled: bool,
    ) -> Result<Emitter, sqlx::Error> {
        let sql = format!(
            "UPDATE emitter SET match_enabled = $2 WHERE id = $1 RETURNING {EMITTER_COLUMNS}"
        );
        sqlx::query_as::<_, Emitter>(&sql)
            .bind(id)
            .bind(enabled)
            .fetch_one(pool)
            .await
    }

    /// Replace `attributes` wholesale (e.g. a manual `randomized_mac`
    /// override on an auto-created emitter). Full-value replace, same
    /// "PATCH re-sends the field it's touching" convention as
    /// [`Self::update_rule`]/[`Self::update_basic`] — callers that want to
    /// change one key merge against the existing JSON before calling this.
    pub async fn set_attributes(
        pool: &PgPool,
        id: Uuid,
        attributes: &serde_json::Value,
    ) -> Result<Emitter, sqlx::Error> {
        let sql =
            format!("UPDATE emitter SET attributes = $2 WHERE id = $1 RETURNING {EMITTER_COLUMNS}");
        sqlx::query_as::<_, Emitter>(&sql)
            .bind(id)
            .bind(attributes)
            .fetch_one(pool)
            .await
    }

    /// Shallow-merge `patch`'s top-level keys into a **wifi_client** emitter's
    /// `attributes` (`attributes = attributes || patch`, latest-wins per key),
    /// leaving every other key intact. A no-op for a missing id or any emitter
    /// whose `emitter_type` isn't `wifi_client` — the type guard lives in the
    /// SQL, so no prior load/round-trip is needed. Used by ingest to record a
    /// client's latest connected AP from an association/reassociation frame
    /// (see the wifi-association design doc); deliberately narrow, unlike the
    /// wholesale [`Self::set_attributes`].
    pub async fn merge_client_attributes(
        pool: &PgPool,
        id: Uuid,
        patch: &serde_json::Value,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE emitter SET attributes = attributes || $2 \
             WHERE id = $1 AND emitter_type = 'wifi_client'",
        )
        .bind(id)
        .bind(patch)
        .execute(pool)
        .await?;
        Ok(())
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

    /// Filter/paginate emitters for the emitters list page (Phase 1b):
    /// `filter.search` (case-insensitive substring over `name`, `type`,
    /// `attributes::text`, and `match_criteria::text` — so a MAC/BSSID/SSID
    /// typed into search finds an emitter even when it only appears inside
    /// the JSON columns, not the plain `name`/`type` columns), `filter.entity_id`
    /// (exact match), and `filter.emitter_type` (exact match against the
    /// `emitter_type` column, e.g. `"wifi_access_point"` — the Type-filter
    /// dropdown) are ANDed together when given. `search` is parameterized as
    /// `'%' || $n || '%'` bound as a plain string parameter — never
    /// interpolated into the SQL text — so it's not susceptible to SQL
    /// injection (same approach `repo::emission::EmissionRepo::query`'s
    /// `text`/ILIKE filter uses); `emitter_type` is likewise bound as a plain
    /// string parameter, never interpolated.
    ///
    /// `filter.sort`/`filter.dir` select an `ORDER BY` via
    /// [`crate::sort::resolve_order_by`] against the [`EMITTER_SORTS`]
    /// allow-list (`name`, `identity`, `first_seen`, `last_seen`,
    /// `emissions`), defaulting to `last_seen DESC` (most-recently-seen
    /// first) when unset or unrecognized — a change from the previous
    /// unconditional `created_at ASC`.
    ///
    /// Every row also carries a correlated `emission_count` — the number of
    /// `emission` rows currently assigned to that emitter, `0` for an
    /// emitter with none — selected via a `(SELECT COUNT(*) ...)`
    /// subquery aliased `emission_count`, which is also what `sort:
    /// "emissions"` orders by. Returns `(Emitter, emission_count)` pairs
    /// plus `total`, the count of matching rows ignoring `limit`/`offset`
    /// (for pagination UIs), same shape as `repo::emission::EmissionRepo::query`.
    pub async fn query(
        pool: &PgPool,
        filter: EmitterListFilter,
    ) -> Result<(Vec<(Emitter, i64)>, i64), sqlx::Error> {
        let mut clauses: Vec<String> = vec!["TRUE".to_string()];
        let mut next_bind = 1usize;

        if filter.search.is_some() {
            clauses.push(format!(
                "(name ILIKE ${next_bind} OR type ILIKE ${next_bind} \
                 OR attributes::text ILIKE ${next_bind} \
                 OR match_criteria::text ILIKE ${next_bind})"
            ));
            next_bind += 1;
        }
        if filter.entity_id.is_some() {
            clauses.push(format!("entity_id = ${next_bind}"));
            next_bind += 1;
        }
        if filter.emitter_type.is_some() {
            clauses.push(format!("emitter_type = ${next_bind}"));
            next_bind += 1;
        }

        let where_sql = clauses.join(" AND ");

        macro_rules! bind_shared {
            ($q:expr) => {{
                let mut q = $q;
                if let Some(ref search) = filter.search {
                    q = q.bind(format!("%{search}%"));
                }
                if let Some(entity_id) = filter.entity_id {
                    q = q.bind(entity_id);
                }
                if let Some(ref emitter_type) = filter.emitter_type {
                    q = q.bind(emitter_type.clone());
                }
                q
            }};
        }

        let count_sql = format!("SELECT COUNT(*) FROM emitter WHERE {where_sql}");
        let count_q = bind_shared!(sqlx::query_as::<_, (i64,)>(&count_sql));
        let (total,) = count_q.fetch_one(pool).await?;

        let limit_idx = next_bind;
        let offset_idx = next_bind + 1;
        let order_by = resolve_order_by(
            filter.sort.as_deref(),
            filter.dir.as_deref(),
            EMITTER_SORTS,
            "last_seen",
            "DESC",
        );
        let data_sql = format!(
            "SELECT {EMITTER_COLUMNS}, \
             (SELECT COUNT(*) FROM emission WHERE emission.emitter_id = emitter.id) \
                 AS emission_count \
             FROM emitter WHERE {where_sql} \
             ORDER BY {order_by} LIMIT ${limit_idx} OFFSET ${offset_idx}"
        );
        let data_q = bind_shared!(sqlx::query_as::<_, EmitterListRow>(&data_sql))
            .bind(filter.limit)
            .bind(filter.offset);
        let rows = data_q.fetch_all(pool).await?;
        let rows = rows
            .into_iter()
            .map(|r| (r.emitter, r.emission_count))
            .collect();

        Ok((rows, total))
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

    /// Delete every `emitter` row whose `id` is in `ids`, returning how many
    /// rows were actually removed. Phase 1c's bulk-delete for the emitters
    /// list page's mass-select action. Same `id = ANY($1)`/empty-`ids`-
    /// short-circuit shape as `repo::emission::EmissionRepo::delete_bulk` —
    /// see its doc comment for why this is injection-safe and why an empty
    /// `ids` returns `Ok(0)` without a round trip. `emission.emitter_id`'s
    /// `ON DELETE SET NULL` applies per deleted emitter, same as
    /// [`Self::delete`]: every emission previously assigned to any of these
    /// emitters survives, just unassigned again.
    pub async fn delete_bulk(pool: &PgPool, ids: &[Uuid]) -> Result<u64, sqlx::Error> {
        if ids.is_empty() {
            return Ok(0);
        }
        let result = sqlx::query("DELETE FROM emitter WHERE id = ANY($1)")
            .bind(ids)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete every `emitter` row, returning how many were removed. Phase
    /// 1c's "Clear All Emitters" action — an unconditional `DELETE`, no
    /// `WHERE` clause, no confirmation of its own (the caller/UI gates this
    /// with a confirm dialog). Every emission previously assigned to any
    /// emitter survives, just unassigned (`ON DELETE SET NULL`).
    pub async fn delete_all(pool: &PgPool) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM emitter").execute(pool).await?;
        Ok(result.rows_affected())
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

    /// Backfill: (re)assign every `kind = <kind>` emission matching `rule` to
    /// `emitter_id` — regardless of whether it's currently unassigned or
    /// already belongs to a different emitter — then refresh the emitter's
    /// `first_seen_at`/`last_seen_at` from all emissions now assigned to it.
    /// `rule` is validated/translated against `catalog_for(kind)` (Task 4:
    /// `kind` is the data-source kind the emitter belongs to, e.g. `"wifi"`
    /// or `"bluetooth"` — see `fluxfang_core::catalog_kind_for`). Returns the
    /// number of emissions (re)assigned. See module docs for the SQL/bind
    /// approach and why this reassigns rather than skipping already-assigned
    /// rows.
    pub async fn attach_emissions_matching(
        pool: &PgPool,
        emitter_id: Uuid,
        rule: &Rule,
        kind: &str,
    ) -> Result<u64, EmitterRuleError> {
        let catalog = catalog_for(kind);
        // `$1` is the emitter id and `$2` is `kind` (both bound below), so
        // the translator's own placeholders continue from `$3`.
        let (frag, binds) =
            conditions_to_sql_checked(&rule.conditions, rule.match_mode, 3, &catalog)?;

        let update_sql = format!("UPDATE emission SET emitter_id = $1 WHERE kind = $2 AND {frag}");

        let mut tx = pool.begin().await.map_err(EmitterRuleError::Sql)?;

        let mut q = sqlx::query(&update_sql).bind(emitter_id).bind(kind);
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

    /// Preview how many `kind = <kind>` emissions `rule` would match —
    /// including ones already assigned to a different emitter, since
    /// [`Self::attach_emissions_matching`] would reclaim those too — without
    /// assigning anything. Same WHERE as `attach_emissions_matching`, minus
    /// the `UPDATE`.
    pub async fn count_matching(
        pool: &PgPool,
        rule: &Rule,
        kind: &str,
    ) -> Result<i64, EmitterRuleError> {
        let catalog = catalog_for(kind);
        let (frag, binds) =
            conditions_to_sql_checked(&rule.conditions, rule.match_mode, 2, &catalog)?;

        let sql = format!("SELECT COUNT(*) FROM emission WHERE kind = $1 AND {frag}");
        let mut q = sqlx::query_as::<_, (i64,)>(&sql).bind(kind);
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
    ///
    /// `type_` is the free-text `type` column value; `emitter_type` is the
    /// machine emitter-type key (e.g. `"wifi_access_point"`) stored on the
    /// `emitter_type` DB column — same naming as `NewEmitter`'s own fields.
    /// `fluxfang-api` is expected to have already validated `emitter_type`
    /// against `fluxfang_core::is_known_emitter_type` before calling this,
    /// same as `rule`.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_with_entity(
        pool: &PgPool,
        new_entity: NewEntity,
        emitter_name: String,
        type_: Option<String>,
        emitter_type: Option<String>,
        match_criteria: serde_json::Value,
        rule: Option<&Rule>,
        kind: &str,
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
            "INSERT INTO emitter (name, type, entity_id, match_criteria, emitter_type) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING {EMITTER_COLUMNS}"
        );
        let mut emitter = sqlx::query_as::<_, Emitter>(&emitter_sql)
            .bind(emitter_name)
            .bind(type_)
            .bind(Some(entity.id))
            .bind(match_criteria)
            .bind(emitter_type)
            .fetch_one(&mut *tx)
            .await
            .map_err(EmitterRuleError::Sql)?;

        let mut attached_count: u64 = 0;
        if let Some(rule) = rule {
            let catalog = catalog_for(kind);
            // `$1` is the emitter id and `$2` is `kind` (both bound below),
            // so the translator's own placeholders continue from `$3`.
            let (frag, binds) =
                conditions_to_sql_checked(&rule.conditions, rule.match_mode, 3, &catalog)?;

            // No `emitter_id IS NULL` guard here — consistent with
            // `attach_emissions_matching`/`count_matching` (see module
            // docs): reassign ALL matching `kind = <kind>` emissions to the
            // freshly-created emitter, regardless of any prior assignment.
            let update_sql =
                format!("UPDATE emission SET emitter_id = $1 WHERE kind = $2 AND {frag}");
            let mut q = sqlx::query(&update_sql).bind(emitter.id).bind(kind);
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
