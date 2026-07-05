//! `ZoneRepo`: `zone` — a user-named geofence.
//!
//! ## Geography read/write pattern
//!
//! `center` is `geography(Point,4326)`, `NOT NULL`. Same undecodable-by-sqlx
//! situation as `emission.location` (see `repo::emission`'s module docs and
//! `models.rs`): every query that produces a [`Zone`] projects `center` via
//! `ST_X(center::geometry) AS lon, ST_Y(center::geometry) AS lat` rather
//! than `SELECT *`/`RETURNING *`, and writes rebuild the point in SQL from
//! plain `f64` binds via `ST_SetSRID(ST_MakePoint(lon, lat), 4326)::geography`.
//! Unlike `emission.location`, `center` is `NOT NULL`, so [`Zone::lon`]/
//! [`Zone::lat`] are plain `f64`, not `Option<f64>`.
//!
//! ## `subjects_in_zone`: "in zone" via each emitter's most-recent located emission
//!
//! An emitter is considered "in" a zone iff its **most recent** emission
//! that has a non-NULL `location` (ignoring any unlocated emissions, which
//! carry no positional information) satisfies `ST_DWithin(location, zone.center,
//! zone.radius_m)`. This is deliberately based on the latest *located*
//! reading, not the latest reading overall and not "any" reading — an
//! emitter that was inside an hour ago and has since left (its newest
//! located emission is now outside) must NOT be reported as in-zone, while
//! one that was outside earlier and has since entered (newest located
//! emission inside) MUST be, regardless of how many older readings disagree.
//!
//! The query gets each emitter's latest located emission via
//! `DISTINCT ON (emitter_id) ... ORDER BY emitter_id, observed_at DESC`
//! (Postgres' "greatest-n-per-group" idiom), scoped to
//! `location IS NOT NULL` so emitters with zero located emissions never
//! appear in that CTE (and therefore never appear in the result, even if
//! they have plenty of unlocated emissions). That per-emitter "latest
//! location" is then joined against `zone`'s `center`/`radius_m` (fetched
//! once via a small `zone` CTE) and filtered with `ST_DWithin`.
//!
//! An entity is "in" the zone iff *any* of its emitters is — implemented by
//! joining `entity -> emitter -> latest_located` through the same CTE and
//! `SELECT DISTINCT`-ing the entity rows, so an entity with two emitters (one
//! in-zone, one not) still appears exactly once.
//!
//! Both the emitter and entity queries in [`ZoneRepo::subjects_in_zone`]
//! join against `zone`/`emitter`/`entity` unqualified-column-name tables
//! that share several column names (`id`, `created_at`, `name`, `notes`) —
//! so, unlike `EMITTER_COLUMNS`/`ENTITY_COLUMNS` in their own single-table
//! repos, the column lists here are table-qualified (`emitter.id`,
//! `entity.id`, ...) to avoid "column reference is ambiguous" errors.

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Emitter, Entity, NewZone, Zone};

pub struct ZoneRepo;

/// Column list shared by every query that produces a [`Zone`] — see the
/// module docs on why `center` is never selected directly.
const ZONE_COLUMNS: &str =
    "id, created_at, name, ST_X(center::geometry) AS lon, ST_Y(center::geometry) AS lat, \
     radius_m, notes";

/// Table-qualified [`Emitter`] column list for [`ZoneRepo::subjects_in_zone`]'s
/// joined queries (see module docs on why qualification is needed here).
const JOINED_EMITTER_COLUMNS: &str = "emitter.id, emitter.created_at, emitter.name, \
     emitter.type, emitter.entity_id, emitter.match_criteria, \
     emitter.first_seen_at, emitter.last_seen_at";

/// Table-qualified [`Entity`] column list for [`ZoneRepo::subjects_in_zone`].
const JOINED_ENTITY_COLUMNS: &str = "entity.id, entity.created_at, entity.name, entity.notes";

/// The subjects currently "in" a zone, per [`ZoneRepo::subjects_in_zone`]'s
/// membership rule (see module docs).
#[derive(Debug, Clone, Default)]
pub struct ZoneSubjects {
    pub emitters: Vec<Emitter>,
    pub entities: Vec<Entity>,
}

impl ZoneRepo {
    pub async fn insert(pool: &PgPool, new: NewZone) -> Result<Zone, sqlx::Error> {
        let (lon, lat) = new.center;
        let sql = format!(
            "INSERT INTO zone (name, center, radius_m, notes) \
             VALUES ($1, \
                 ST_SetSRID(ST_MakePoint($2::double precision, $3::double precision), 4326)::geography, \
                 $4, $5) \
             RETURNING {ZONE_COLUMNS}"
        );
        sqlx::query_as::<_, Zone>(&sql)
            .bind(new.name)
            .bind(lon)
            .bind(lat)
            .bind(new.radius_m)
            .bind(new.notes)
            .fetch_one(pool)
            .await
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<Zone>, sqlx::Error> {
        let sql = format!("SELECT {ZONE_COLUMNS} FROM zone ORDER BY created_at ASC");
        sqlx::query_as::<_, Zone>(&sql).fetch_all(pool).await
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<Zone>, sqlx::Error> {
        let sql = format!("SELECT {ZONE_COLUMNS} FROM zone WHERE id = $1");
        sqlx::query_as::<_, Zone>(&sql)
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// Full replacement of a zone's mutable fields, matching
    /// `DataSourceRepo::update`'s convention.
    pub async fn update(
        pool: &PgPool,
        id: Uuid,
        name: &str,
        center: (f64, f64),
        radius_m: f64,
        notes: Option<&str>,
    ) -> Result<Zone, sqlx::Error> {
        let (lon, lat) = center;
        let sql = format!(
            "UPDATE zone SET name = $2, \
                 center = ST_SetSRID(ST_MakePoint($3::double precision, $4::double precision), 4326)::geography, \
                 radius_m = $5, notes = $6 \
             WHERE id = $1 \
             RETURNING {ZONE_COLUMNS}"
        );
        sqlx::query_as::<_, Zone>(&sql)
            .bind(id)
            .bind(name)
            .bind(lon)
            .bind(lat)
            .bind(radius_m)
            .bind(notes)
            .fetch_one(pool)
            .await
    }

    /// Delete a zone, returning whether a row was actually removed.
    /// `zone_membership` rows referencing it cascade-delete (see the FK in
    /// `0001_init.sql`).
    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM zone WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// For every zone, whether the point `(lon, lat)` falls within it
    /// (`ST_DWithin` against each zone's own `center`/`radius_m`), in one
    /// query. Used by `fluxfang-api::ingest::zones` (Task 5.4) to
    /// recompute a subject's (emitter/entity/host) membership across
    /// every zone at once, rather than issuing one `ST_DWithin` query per
    /// zone per subject.
    ///
    /// Returns `(zone_id, inside)` pairs, one per row in `zone` — an empty
    /// `zone` table yields an empty `Vec`, not an error.
    pub async fn memberships_for_point(
        pool: &PgPool,
        lon: f64,
        lat: f64,
    ) -> Result<Vec<(Uuid, bool)>, sqlx::Error> {
        sqlx::query_as::<_, (Uuid, bool)>(
            "SELECT id, \
                 ST_DWithin( \
                     ST_SetSRID(ST_MakePoint($1::double precision, $2::double precision), 4326)::geography, \
                     center, \
                     radius_m \
                 ) AS inside \
             FROM zone",
        )
        .bind(lon)
        .bind(lat)
        .fetch_all(pool)
        .await
    }

    /// Emitters/entities currently "in" `zone_id`. See module docs for the
    /// exact membership rule (each emitter's most recent *located*
    /// emission vs. `ST_DWithin`; an entity is in iff any of its emitters
    /// is). Returns empty vectors (not an error) if `zone_id` doesn't exist.
    pub async fn subjects_in_zone(
        pool: &PgPool,
        zone_id: Uuid,
    ) -> Result<ZoneSubjects, sqlx::Error> {
        let emitter_sql = format!(
            "WITH z AS ( \
                 SELECT center, radius_m FROM zone WHERE id = $1 \
             ), latest_located AS ( \
                 SELECT DISTINCT ON (emitter_id) emitter_id, location \
                 FROM emission \
                 WHERE emitter_id IS NOT NULL AND location IS NOT NULL \
                 ORDER BY emitter_id, observed_at DESC \
             ) \
             SELECT {JOINED_EMITTER_COLUMNS} \
             FROM emitter \
             JOIN latest_located ON latest_located.emitter_id = emitter.id \
             CROSS JOIN z \
             WHERE ST_DWithin(latest_located.location, z.center, z.radius_m)"
        );
        let emitters = sqlx::query_as::<_, Emitter>(&emitter_sql)
            .bind(zone_id)
            .fetch_all(pool)
            .await?;

        let entity_sql = format!(
            "WITH z AS ( \
                 SELECT center, radius_m FROM zone WHERE id = $1 \
             ), latest_located AS ( \
                 SELECT DISTINCT ON (emitter_id) emitter_id, location \
                 FROM emission \
                 WHERE emitter_id IS NOT NULL AND location IS NOT NULL \
                 ORDER BY emitter_id, observed_at DESC \
             ) \
             SELECT DISTINCT {JOINED_ENTITY_COLUMNS} \
             FROM entity \
             JOIN emitter ON emitter.entity_id = entity.id \
             JOIN latest_located ON latest_located.emitter_id = emitter.id \
             CROSS JOIN z \
             WHERE ST_DWithin(latest_located.location, z.center, z.radius_m)"
        );
        let entities = sqlx::query_as::<_, Entity>(&entity_sql)
            .bind(zone_id)
            .fetch_all(pool)
            .await?;

        Ok(ZoneSubjects { emitters, entities })
    }
}
