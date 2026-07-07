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
use crate::resolve_order_by;

pub struct EmissionRepo;

/// Column list shared by every query that produces an [`Emission`] — see
/// the module docs on why `location` is never selected directly.
const EMISSION_COLUMNS: &str = "id, created_at, data_source_id, emitter_id, session_id, \
     observed_at, signal_strength, kind, payload, \
     ST_X(location::geometry) AS lon, ST_Y(location::geometry) AS lat";

/// Allow-listed sort keys for the emissions list -> SQL ordering expressions.
/// Only real columns (cheap/indexed); JSONB payload keys and the emitter join
/// are intentionally excluded.
const EMISSION_SORTS: &[(&str, &str)] =
    &[("observed_at", "observed_at"), ("rssi", "signal_strength")];

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
    /// Phase A5: exact match on the emission's *emitter's* `emitter_type`
    /// (e.g. `"wifi_access_point"`) — a subquery join, since `emitter_type`
    /// lives on `emitter`, not `emission`. `None`/NULL-`emitter_id` rows are
    /// excluded whenever this is `Some` (see module docs on `query`).
    pub emitter_type: Option<String>,
    /// Phase A5: matches emitters whose `emitter_type LIKE '<category>_%'`
    /// (e.g. `"wifi"` -> `wifi_access_point`/`wifi_client`) — a coarser,
    /// prefix-based sibling of `emitter_type` for the map's category
    /// layers. Independent of `emitter_type`: both may be set at once
    /// (ANDed), though in practice a caller sends one or the other.
    pub emitter_category: Option<String>,
    pub limit: i64,
    pub offset: i64,
    /// Public sort key (see `EMISSION_SORTS`); unknown/None -> default.
    pub sort: Option<String>,
    /// `"asc"`/`"desc"`; other/None -> default direction.
    pub dir: Option<String>,
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
            emitter_type: None,
            emitter_category: None,
            limit: 50,
            offset: 0,
            sort: None,
            dir: None,
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

/// Build the shared `WHERE` body + ordered bind list for an
/// [`EmissionFilter`]. Returns `(where_sql, binds)`; the caller appends its
/// own `ORDER BY`/`LIMIT`/extra clauses and replays `binds` in order. Shared
/// by [`EmissionRepo::query`] and [`EmissionRepo::points`] so the filter
/// semantics (and their bind-order bugs, or lack thereof) can't drift
/// between the two.
fn build_where(filter: &EmissionFilter) -> Result<(String, Vec<BindVal>), EmissionQueryError> {
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
    if let Some(ref emitter_type) = filter.emitter_type {
        // Subquery join (see module docs): `emitter_type` lives on
        // `emitter`, not `emission`. A NULL `emitter_id` can never
        // appear in an `IN (SELECT ...)` result set, so this
        // automatically excludes unassigned emissions once this
        // filter is set, as intended.
        clauses.push(format!(
            "emitter_id IN (SELECT id FROM emitter WHERE emitter_type = ${next_bind})"
        ));
        binds.push(BindVal::Text(emitter_type.clone()));
        next_bind += 1;
    }
    if let Some(ref category) = filter.emitter_category {
        // Prefix match on the `<category>_<subtype>` naming convention
        // (e.g. `wifi` -> `wifi_access_point`/`wifi_client`) rather
        // than a Rust-side enumeration of every `emitter_type` in that
        // category: this stays correct as new subtypes are added to
        // the classification registry with no code change here. The
        // category value is bound as a plain parameter (concatenated
        // to `'_%'` in SQL, not in the Rust format string), so this
        // is not susceptible to SQL injection; a category value that
        // itself contains `%`/`_` wildcard characters only affects
        // match precision; it's not a security concern since it's the
        // same trust boundary as every other filter this endpoint
        // accepts from an authenticated caller.
        clauses.push(format!(
            "emitter_id IN (SELECT id FROM emitter WHERE emitter_type LIKE ${next_bind} || '_%')"
        ));
        binds.push(BindVal::Text(category.clone()));
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

    Ok((clauses.join(" AND "), binds))
}

/// Server-side safety cap for the heatmap points endpoint. High enough to
/// cover any realistic survey (~16 bytes/point => under ~1.5 MB JSON);
/// beyond it the response is marked truncated rather than silently dropped.
pub const MAX_POINTS: i64 = 50_000;

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
        let (where_sql, binds) = build_where(&filter)?;

        let count_sql = format!("SELECT COUNT(*) FROM emission WHERE {where_sql}");
        let mut count_q = sqlx::query_as::<_, (i64,)>(&count_sql);
        for b in &binds {
            count_q = bind_one(count_q, b);
        }
        let (total,) = count_q
            .fetch_one(pool)
            .await
            .map_err(EmissionQueryError::Sql)?;

        let order_by = resolve_order_by(
            filter.sort.as_deref(),
            filter.dir.as_deref(),
            EMISSION_SORTS,
            "observed_at",
            "DESC",
        );
        let limit_idx = binds.len() + 1;
        let offset_idx = binds.len() + 2;
        let data_sql = format!(
            "SELECT {EMISSION_COLUMNS} FROM emission WHERE {where_sql} \
             ORDER BY {order_by} LIMIT ${limit_idx} OFFSET ${offset_idx}"
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

    /// Located emission coordinates matching `filter` (ignoring its
    /// `limit`/`offset`), capped at [`MAX_POINTS`], plus the total matched
    /// located-row count (so the caller can report truncation). Only rows
    /// with a non-null `location` are returned — the heatmap source for the
    /// Dashboard/Map, deliberately uncapped (up to `MAX_POINTS`) unlike
    /// `query`'s page-sized `limit`, since a heatmap silently missing older
    /// points because they scrolled past `query`'s default page is worse
    /// than one big response.
    pub async fn points(
        pool: &PgPool,
        filter: EmissionFilter,
    ) -> Result<(Vec<[f64; 2]>, i64), EmissionQueryError> {
        let (where_sql, binds) = build_where(&filter)?;
        let where_located = format!("({where_sql}) AND location IS NOT NULL");

        let count_sql = format!("SELECT COUNT(*) FROM emission WHERE {where_located}");
        let mut count_q = sqlx::query_as::<_, (i64,)>(&count_sql);
        for b in &binds {
            count_q = bind_one(count_q, b);
        }
        let (total,) = count_q
            .fetch_one(pool)
            .await
            .map_err(EmissionQueryError::Sql)?;

        // MAX_POINTS is a compile-time constant integer, safe to interpolate.
        let data_sql = format!(
            "SELECT ST_X(location::geometry) AS lon, ST_Y(location::geometry) AS lat \
             FROM emission WHERE {where_located} \
             ORDER BY observed_at DESC LIMIT {MAX_POINTS}"
        );
        let mut data_q = sqlx::query_as::<_, (f64, f64)>(&data_sql);
        for b in &binds {
            data_q = bind_one(data_q, b);
        }
        let rows = data_q
            .fetch_all(pool)
            .await
            .map_err(EmissionQueryError::Sql)?;
        let points = rows.into_iter().map(|(lon, lat)| [lon, lat]).collect();
        Ok((points, total))
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

    /// Delete every `emission` row whose `id` is in `ids`, returning how
    /// many rows were actually removed. Phase 1c's bulk-delete for the
    /// emissions list page's mass-select action.
    ///
    /// `ids` is bound as a single `Uuid` array parameter (`id = ANY($1)`),
    /// never interpolated into the SQL text, so this is not susceptible to
    /// SQL injection regardless of how many/what ids are passed. An empty
    /// `ids` short-circuits to `Ok(0)` without touching the database at
    /// all: `ANY('{}')` would be a valid, always-false clause anyway, but
    /// skipping the round trip is both cheaper and makes the "nothing to
    /// delete" case explicit. An id that doesn't exist (already deleted, or
    /// never existed) is simply not counted — this is not an error.
    pub async fn delete_bulk(pool: &PgPool, ids: &[Uuid]) -> Result<u64, sqlx::Error> {
        if ids.is_empty() {
            return Ok(0);
        }
        let result = sqlx::query("DELETE FROM emission WHERE id = ANY($1)")
            .bind(ids)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete every `emission` row, returning how many were removed.
    /// Phase 1c's "Clear All Emissions" action — an unconditional `DELETE`,
    /// no `WHERE` clause, no confirmation of its own (the caller/UI gates
    /// this with a confirm dialog).
    pub async fn delete_all(pool: &PgPool) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM emission").execute(pool).await?;
        Ok(result.rows_affected())
    }
}
