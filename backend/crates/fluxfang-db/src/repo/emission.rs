//! `EmissionRepo`: `emission` — one captured observation per row.
//!
//! ## Geography read/write pattern
//!
//! `location` is `geography(Point,4326)`, nullable. sqlx cannot decode
//! `geography` directly (see `models.rs` module docs), so:
//!
//! - **Write**: bind `lon`/`lat` as plain `Option<f64>` parameters and let
//!   SQL build the point: `CASE WHEN $n IS NULL THEN NULL ELSE
//!   ST_SetSRID(ST_MakePoint($n, $n+1), 4326)::geography END`.
//! - **Read**: every `SELECT`/`RETURNING` projects `ST_X(location::geometry)
//!   AS lon, ST_Y(location::geometry) AS lat` instead of the raw column, and
//!   [`crate::models::Emission`] decodes those as `Option<f64>`.
//!
//! Never `SELECT *`/`RETURNING *` here — the explicit column list is what
//! makes the geography projection possible.
//!
//! ## `query`'s dynamic WHERE clause and bind threading
//!
//! [`EmissionRepo::query`] builds its `WHERE` clause by appending one SQL
//! clause (and 0+ positional binds) per non-empty [`EmissionFilter`] field,
//! tracking `next_bind` as a running counter so every appended clause's
//! `$N` placeholders continue exactly where the previous one left off.
//! `field_conditions` (if non-empty) is translated last, by calling
//! [`fluxfang_core::conditions_to_sql_checked`] with the *current* value of
//! `next_bind` — so its `$N`s continue after every structured filter's
//! binds, and its returned binds are appended after them, in the same
//! order the SQL text expects.
//!
//! Because the structured filters bind heterogeneous Rust types (`Uuid`,
//! `DateTime<Utc>`, `f64`, `String`) that can't share one `Vec<T>`, binds
//! are accumulated as an untyped [`BindVal`] enum and applied to a query
//! via [`bind_one`], which is generic over the `sqlx::query_as::<_, O>`
//! output type `O` — this lets the exact same bind list be replayed against
//! both the `SELECT COUNT(*)` (for `total`) and the paginated `SELECT`
//! (for the page of rows), with `LIMIT`/`OFFSET` appended as two more binds
//! only on the latter.
//!
//! ## `kind`/catalog scoping for `field_conditions`
//!
//! A `payload` filter condition (e.g. `channel gte 6`) only makes sense
//! against *one* data-source kind's field catalog (fields, types, and
//! valid operators differ per kind — see `fluxfang_core::catalog`).
//! `EmissionFilter::kind: Option<String>` is used to pick that catalog: if
//! set, `catalog_for(kind)` is used; if `field_conditions` is non-empty but
//! `kind` is `None`, this repo defaults to `catalog_for("wifi")` (the only
//! kind the schema currently allows — see `emission.kind`'s `CHECK`
//! constraint). This default is a deliberate, documented choice for this
//! slice; once more `kind`s exist, callers filtering on `field_conditions`
//! should always pass `kind` explicitly rather than relying on it.
//!
//! Any [`fluxfang_core::RuleSqlError`] from the translator (most commonly
//! an unknown/mistyped field) is surfaced as `Err(EmissionQueryError::Rule)`
//! rather than silently dropping the filter.

use std::fmt;

use chrono::{DateTime, Utc};
use fluxfang_core::{catalog_for, conditions_to_sql_checked, Condition, MatchMode, RuleSqlError};
use sqlx::postgres::PgArguments;
use sqlx::query::QueryAs;
use sqlx::{PgPool, Postgres};
use uuid::Uuid;

use crate::models::{Emission, NewEmission};

pub struct EmissionRepo;

/// Column list shared by every query that produces an [`Emission`] — see
/// the module docs on why `location` is never selected directly.
const EMISSION_COLUMNS: &str = "id, created_at, data_source_id, emitter_id, session_id, \
     observed_at, signal_strength, kind, payload, \
     ST_X(location::geometry) AS lon, ST_Y(location::geometry) AS lat";

/// Filter/paginate criteria for [`EmissionRepo::query`]. Build with
/// `EmissionFilter { data_source_id: Some(id), ..Default::default() }` —
/// every field defaults to "no constraint" (`Default::default()`'s
/// `limit`/`offset` default to a sane first page; see [`Default`] impl).
#[derive(Debug, Clone)]
pub struct EmissionFilter {
    pub data_source_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub emitter_id: Option<Uuid>,
    /// `true` -> only rows with `emitter_id IS NULL`.
    pub unassigned: bool,
    pub time_from: Option<DateTime<Utc>>,
    pub time_to: Option<DateTime<Utc>>,
    /// `(min_lon, min_lat, max_lon, max_lat)`.
    pub bbox: Option<(f64, f64, f64, f64)>,
    /// Filters `emission.kind` *and* selects which field catalog
    /// `field_conditions` is checked against (see module docs).
    pub kind: Option<String>,
    pub field_conditions: Vec<Condition>,
    pub match_mode: MatchMode,
    /// Substring search over `payload::text`.
    pub text: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl Default for EmissionFilter {
    fn default() -> Self {
        Self {
            data_source_id: None,
            session_id: None,
            emitter_id: None,
            unassigned: false,
            time_from: None,
            time_to: None,
            bbox: None,
            kind: None,
            field_conditions: Vec::new(),
            match_mode: MatchMode::All,
            text: None,
            limit: 50,
            offset: 0,
        }
    }
}

/// Error from [`EmissionRepo::query`]: either a DB error, or the
/// `field_conditions` translator rejecting a condition (unknown field, or a
/// value whose JSON type doesn't match the field's catalog type).
#[derive(Debug)]
pub enum EmissionQueryError {
    Sql(sqlx::Error),
    Rule(RuleSqlError),
}

impl fmt::Display for EmissionQueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmissionQueryError::Sql(e) => write!(f, "database error: {e}"),
            EmissionQueryError::Rule(e) => write!(f, "invalid field_conditions: {e}"),
        }
    }
}

impl std::error::Error for EmissionQueryError {}

impl From<sqlx::Error> for EmissionQueryError {
    fn from(e: sqlx::Error) -> Self {
        EmissionQueryError::Sql(e)
    }
}

impl From<RuleSqlError> for EmissionQueryError {
    fn from(e: RuleSqlError) -> Self {
        EmissionQueryError::Rule(e)
    }
}

/// One accumulated bind value of whatever concrete type a filter clause
/// needed. See the module docs on why this untyped-enum approach is used
/// instead of a single `Vec<T>`.
#[derive(Debug, Clone)]
enum BindVal {
    Uuid(Uuid),
    Time(DateTime<Utc>),
    F64(f64),
    Text(String),
}

/// Apply one [`BindVal`] to a `sqlx::query_as` builder, generic over its
/// output row type `O` so the same helper replays the same bind list
/// against both the `COUNT(*)` query and the paginated row query.
fn bind_one<'q, O>(
    q: QueryAs<'q, Postgres, O, PgArguments>,
    b: &BindVal,
) -> QueryAs<'q, Postgres, O, PgArguments> {
    match b.clone() {
        BindVal::Uuid(u) => q.bind(u),
        BindVal::Time(t) => q.bind(t),
        BindVal::F64(f) => q.bind(f),
        BindVal::Text(s) => q.bind(s),
    }
}

impl EmissionRepo {
    pub async fn insert(pool: &PgPool, new: NewEmission) -> Result<Emission, sqlx::Error> {
        let (lon, lat) = match new.location {
            Some((lon, lat)) => (Some(lon), Some(lat)),
            None => (None, None),
        };

        let sql = format!(
            "INSERT INTO emission \
                 (data_source_id, emitter_id, session_id, observed_at, signal_strength, \
                  location, kind, payload) \
             VALUES \
                 ($1, $2, $3, $4, $5, \
                  CASE WHEN $6::double precision IS NULL THEN NULL \
                       ELSE ST_SetSRID(ST_MakePoint($6::double precision, $7::double precision), 4326)::geography \
                  END, \
                  $8, $9) \
             RETURNING {EMISSION_COLUMNS}"
        );

        sqlx::query_as::<_, Emission>(&sql)
            .bind(new.data_source_id)
            .bind(new.emitter_id)
            .bind(new.session_id)
            .bind(new.observed_at)
            .bind(new.signal_strength)
            .bind(lon)
            .bind(lat)
            .bind(new.kind)
            .bind(new.payload)
            .fetch_one(pool)
            .await
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<Emission>, sqlx::Error> {
        let sql = format!("SELECT {EMISSION_COLUMNS} FROM emission WHERE id = $1");
        sqlx::query_as::<_, Emission>(&sql)
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// Assign `emitter_id` to emission `id` -- the single-row counterpart to
    /// `EmitterRepo::attach_emissions_matching`'s bulk backfill, used by
    /// `fluxfang-api::ingest`'s auto-attach: right after a fresh emission is
    /// inserted, the first emitter whose rule matches it gets stamped here.
    /// Unconditional (no `WHERE emitter_id IS NULL` guard): ingest always
    /// calls this at most once per emission, immediately after insert,
    /// before anything else could have raced to assign a different emitter.
    pub async fn set_emitter(
        pool: &PgPool,
        id: Uuid,
        emitter_id: Uuid,
    ) -> Result<Emission, sqlx::Error> {
        let sql = format!(
            "UPDATE emission SET emitter_id = $2 WHERE id = $1 RETURNING {EMISSION_COLUMNS}"
        );
        sqlx::query_as::<_, Emission>(&sql)
            .bind(id)
            .bind(emitter_id)
            .fetch_one(pool)
            .await
    }

    /// Filter/paginate emissions. Returns the requested page plus `total`,
    /// the count of matching rows ignoring `limit`/`offset` (for pagination
    /// UIs). See the module docs for the WHERE-building and catalog-scoping
    /// approach.
    pub async fn query(
        pool: &PgPool,
        filter: EmissionFilter,
    ) -> Result<(Vec<Emission>, i64), EmissionQueryError> {
        let mut clauses: Vec<String> = vec!["TRUE".to_string()];
        let mut binds: Vec<BindVal> = Vec::new();
        let mut next_bind = 1usize;

        if let Some(id) = filter.data_source_id {
            clauses.push(format!("data_source_id = ${next_bind}"));
            binds.push(BindVal::Uuid(id));
            next_bind += 1;
        }
        if let Some(id) = filter.session_id {
            clauses.push(format!("session_id = ${next_bind}"));
            binds.push(BindVal::Uuid(id));
            next_bind += 1;
        }
        if let Some(id) = filter.emitter_id {
            clauses.push(format!("emitter_id = ${next_bind}"));
            binds.push(BindVal::Uuid(id));
            next_bind += 1;
        }
        if filter.unassigned {
            clauses.push("emitter_id IS NULL".to_string());
        }
        if let Some(t) = filter.time_from {
            clauses.push(format!("observed_at >= ${next_bind}"));
            binds.push(BindVal::Time(t));
            next_bind += 1;
        }
        if let Some(t) = filter.time_to {
            clauses.push(format!("observed_at <= ${next_bind}"));
            binds.push(BindVal::Time(t));
            next_bind += 1;
        }
        if let Some((min_lon, min_lat, max_lon, max_lat)) = filter.bbox {
            clauses.push(format!(
                "ST_Intersects(location::geometry, ST_MakeEnvelope(${}, ${}, ${}, ${}, 4326))",
                next_bind,
                next_bind + 1,
                next_bind + 2,
                next_bind + 3
            ));
            binds.push(BindVal::F64(min_lon));
            binds.push(BindVal::F64(min_lat));
            binds.push(BindVal::F64(max_lon));
            binds.push(BindVal::F64(max_lat));
            next_bind += 4;
        }
        if let Some(ref kind) = filter.kind {
            clauses.push(format!("kind = ${next_bind}"));
            binds.push(BindVal::Text(kind.clone()));
            next_bind += 1;
        }
        if let Some(ref text) = filter.text {
            clauses.push(format!("payload::text ILIKE ${next_bind}"));
            binds.push(BindVal::Text(format!("%{text}%")));
            next_bind += 1;
        }
        if !filter.field_conditions.is_empty() {
            // Scoping decision (see module docs): use the filter's `kind`
            // if given, else default to "wifi" — the only kind the schema
            // currently allows.
            let kind_for_catalog = filter.kind.as_deref().unwrap_or("wifi");
            let catalog = catalog_for(kind_for_catalog);
            let (frag, cond_binds) = conditions_to_sql_checked(
                &filter.field_conditions,
                filter.match_mode,
                next_bind,
                &catalog,
            )?;
            next_bind += cond_binds.len();
            clauses.push(frag);

            // `conditions_to_sql_checked` returns one text-coerced
            // `Value::String` bind per condition (N per `Op::In`'s N array
            // elements), in the same order the SQL fragment's `$n`
            // placeholders expect. Every bind is appended here as plain
            // text, uniformly, with no per-condition op inspection: the
            // `Gte`/`Lte` SQL arms now cast *both* sides to `numeric`
            // (`(payload->>'field')::numeric >= $n::numeric`, see
            // fluxfang_core::rule_sql), so a text bind works there too --
            // there is no need to (and, critically, no reliable way to)
            // re-derive which binds are "numeric" from `field_conditions`
            // after the fact. (A prior version of this code re-walked
            // `field_conditions` guessing Gte/Lte -> numeric bind by
            // op/field-shape; that walk could desync from the translator's
            // actual bind count whenever a condition's op didn't match its
            // field's type, since `condition_clause` silently drops such a
            // condition to a bindless `FALSE` while the re-walk still
            // counted it as consuming a bind. `conditions_to_sql_checked`
            // now rejects that mismatch outright (`RuleSqlError::InvalidOp`),
            // so every condition that reaches here is guaranteed to
            // contribute exactly the binds it appears to.)
            for v in cond_binds {
                let text = match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                binds.push(BindVal::Text(text));
            }
        }

        let where_sql = clauses.join(" AND ");

        let count_sql = format!("SELECT COUNT(*) FROM emission WHERE {where_sql}");
        let mut count_q = sqlx::query_as::<_, (i64,)>(&count_sql);
        for b in &binds {
            count_q = bind_one(count_q, b);
        }
        let (total,) = count_q
            .fetch_one(pool)
            .await
            .map_err(EmissionQueryError::Sql)?;

        let limit_idx = next_bind;
        let offset_idx = next_bind + 1;
        let data_sql = format!(
            "SELECT {EMISSION_COLUMNS} FROM emission WHERE {where_sql} \
             ORDER BY observed_at DESC LIMIT ${limit_idx} OFFSET ${offset_idx}"
        );
        let mut data_q = sqlx::query_as::<_, Emission>(&data_sql);
        for b in &binds {
            data_q = bind_one(data_q, b);
        }
        let rows = data_q
            .bind(filter.limit)
            .bind(filter.offset)
            .fetch_all(pool)
            .await
            .map_err(EmissionQueryError::Sql)?;

        Ok((rows, total))
    }

    /// Recent, geolocated (`location IS NOT NULL`) emissions across a *set*
    /// of emitters, newest first, capped at `limit` — Task 6.5's `GET
    /// /api/entities/:id` uses this for `recent_detections`, the feed that
    /// drives the map/tracking view for an entity's emitters combined.
    ///
    /// A single `emitter_id = ANY($1)` query rather than one `EmissionRepo::
    /// query` call per emitter (then merging/truncating in Rust): an entity
    /// can have any number of emitters, and this keeps the work to one
    /// query and one `ORDER BY ... LIMIT` regardless of how many. Returns an
    /// empty `Vec` (not an error) for an empty `emitter_ids`, matching how
    /// an entity with no emitters should report `recent_detections: []`.
    pub async fn recent_located(
        pool: &PgPool,
        emitter_ids: &[Uuid],
        limit: i64,
    ) -> Result<Vec<Emission>, sqlx::Error> {
        if emitter_ids.is_empty() {
            return Ok(Vec::new());
        }

        let sql = format!(
            "SELECT {EMISSION_COLUMNS} FROM emission \
             WHERE emitter_id = ANY($1) AND location IS NOT NULL \
             ORDER BY observed_at DESC LIMIT $2"
        );
        sqlx::query_as::<_, Emission>(&sql)
            .bind(emitter_ids)
            .bind(limit)
            .fetch_all(pool)
            .await
    }
}
