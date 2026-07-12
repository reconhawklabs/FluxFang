//! `CoTravelRepo`: the Co-Travel Detection page's read model.
//!
//! This module owns two things: the per-emitter co-travel aggregate query
//! (`candidates`) and the `cotravel_ignore` list CRUD (`ignore`/`unignore`/
//! `list_ignored`). Scoring/tiering is deliberately NOT here — that's
//! `fluxfang_core::cotravel::score`, a pure function the API layer applies
//! to each `CoTravelCandidate` this repo returns.

use chrono::{DateTime, Utc};
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

    /// Safety cap on how many qualifying emitters one request materializes
    /// (the API scores/sorts/paginates the returned set in-process). Far above
    /// any realistic qualifying count even at the loosest slider settings.
    pub const MAX_CANDIDATES: i64 = 5000;

    /// Per-emitter co-travel metrics for every emitter that clears the gate
    /// (`spread >= min_distance_m AND span >= min_time_s`), excluding ignored
    /// emitters and rows with no location. Ordered by spread descending and
    /// capped at [`Self::MAX_CANDIDATES`]. See the design doc §4/§6.
    ///
    /// Points are counted by snapping each sighting to a grid whose cell size
    /// is `min_distance_m` (converted to degrees via a flat ~111320 m/degree
    /// approximation — adequate for a discrete point count). `spread_m` is the
    /// true geodesic max distance between sighting points (convex-hull
    /// diameter), so 3+-point emitters are not over-reported.
    pub async fn candidates(
        pool: &PgPool,
        filter: &CoTravelFilter,
    ) -> Result<Vec<CoTravelCandidate>, sqlx::Error> {
        // $1 time_from (nullable), $2 time_to (nullable), $3 cell size (deg),
        // $4 min_distance_m, $5 min_time_s.
        let sql = "
            WITH snapped AS (
                SELECT e.emitter_id,
                       e.observed_at,
                       ST_X(e.location::geometry) AS lon,
                       ST_Y(e.location::geometry) AS lat,
                       ST_SnapToGrid(e.location::geometry, $3, $3) AS cell
                FROM emission e
                WHERE e.location IS NOT NULL
                  AND e.emitter_id IS NOT NULL
                  AND ($1::timestamptz IS NULL OR e.observed_at >= $1)
                  AND ($2::timestamptz IS NULL OR e.observed_at <= $2)
                  AND NOT EXISTS (
                      SELECT 1 FROM cotravel_ignore ci WHERE ci.emitter_id = e.emitter_id
                  )
            ),
            agg AS (
                SELECT emitter_id,
                       COUNT(*)::bigint AS hits,
                       COUNT(DISTINCT cell)::bigint AS points,
                       EXTRACT(EPOCH FROM (MAX(observed_at) - MIN(observed_at)))::double precision AS span_s,
                       MIN(observed_at) AS first_seen,
                       MAX(observed_at) AS last_seen,
                       ST_ConvexHull(ST_Collect(ST_SetSRID(ST_MakePoint(lon, lat), 4326))) AS hull
                FROM snapped
                GROUP BY emitter_id
            ),
            metrics AS (
                SELECT emitter_id, hits, points, span_s, first_seen, last_seen,
                       -- Farthest vertex pair on the hull is picked in planar
                       -- (lon/lat degree) space, then that pair's distance is
                       -- measured geodesically via ::geography — exact, since
                       -- picking the max-distance pair is invariant to the
                       -- monotonic degree->meter mapping at these latitudes/spans.
                       COALESCE(
                           ST_Length(ST_LongestLine(hull, hull)::geography),
                           0
                       )::double precision AS spread_m
                FROM agg
            )
            SELECT m.emitter_id,
                   e.name,
                   e.emitter_type,
                   e.identity_key,
                   e.attributes,
                   m.hits,
                   m.points,
                   m.span_s,
                   m.spread_m,
                   m.first_seen,
                   m.last_seen
            FROM metrics m
            JOIN emitter e ON e.id = m.emitter_id
            WHERE m.spread_m >= $4
              AND m.span_s >= $5
              AND m.points >= 2
            ORDER BY m.spread_m DESC
            LIMIT $6
        ";

        let cell_deg = filter.min_distance_m / 111_320.0;
        sqlx::query_as::<_, CoTravelCandidate>(sql)
            .bind(filter.time_from)
            .bind(filter.time_to)
            .bind(cell_deg)
            .bind(filter.min_distance_m)
            .bind(filter.min_time_s)
            .bind(Self::MAX_CANDIDATES)
            .fetch_all(pool)
            .await
    }
}

/// Filter for [`CoTravelRepo::candidates`]. `min_distance_m` is used both as
/// the gate threshold (an emitter's sighting spread must be at least this far)
/// AND as the grid cell size for counting separated "points" (see design doc
/// §4's slider dual-role).
#[derive(Debug, Clone)]
pub struct CoTravelFilter {
    pub time_from: Option<DateTime<Utc>>,
    pub time_to: Option<DateTime<Utc>>,
    pub min_distance_m: f64,
    pub min_time_s: f64,
}

/// One emitter's raw co-travel metrics over the window (pre-scoring). The API
/// layer maps these through `fluxfang_core::cotravel::score`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CoTravelCandidate {
    pub emitter_id: Uuid,
    pub name: String,
    pub emitter_type: Option<String>,
    pub identity_key: Option<String>,
    pub attributes: serde_json::Value,
    pub hits: i64,
    pub points: i64,
    pub span_s: f64,
    pub spread_m: f64,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}
