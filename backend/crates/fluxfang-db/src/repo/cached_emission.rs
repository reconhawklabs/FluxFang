//! `CachedEmissionRepo`: a Sensor node's local forward queue.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{CachedEmission, NewCachedEmission};

/// Cache counters for the sensor status UI.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct CacheStats {
    pub total: i64,
    pub undelivered: i64,
}

pub struct CachedEmissionRepo;

impl CachedEmissionRepo {
    pub async fn insert(pool: &PgPool, new: NewCachedEmission) -> Result<CachedEmission, sqlx::Error> {
        sqlx::query_as::<_, CachedEmission>(
            "INSERT INTO cached_emission \
                (kind, signal_strength, lat, lon, observed_at, payload, data_source_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING *",
        )
        .bind(new.kind).bind(new.signal_strength).bind(new.lat).bind(new.lon)
        .bind(new.observed_at).bind(new.payload).bind(new.data_source_id)
        .fetch_one(pool).await
    }

    /// Oldest-first batch of not-yet-delivered rows.
    pub async fn list_undelivered(pool: &PgPool, limit: i64) -> Result<Vec<CachedEmission>, sqlx::Error> {
        sqlx::query_as::<_, CachedEmission>(
            "SELECT * FROM cached_emission WHERE delivered = false ORDER BY created_at LIMIT $1",
        ).bind(limit).fetch_all(pool).await
    }

    pub async fn mark_delivered(pool: &PgPool, ids: &[Uuid]) -> Result<u64, sqlx::Error> {
        if ids.is_empty() { return Ok(0); }
        // Stamp delivered_at only on the transition, so re-marking an already
        // delivered row doesn't move its delivery time (keeps the "last hour"
        // throughput metric honest).
        let res = sqlx::query(
            "UPDATE cached_emission SET delivered = true, \
                delivered_at = COALESCE(delivered_at, now()) \
             WHERE id = ANY($1)",
        )
        .bind(ids)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Count of emissions forwarded (delivered) at or after `since` — the
    /// Sensor Dashboard's recent-throughput metric.
    pub async fn delivered_count_since(
        pool: &PgPool,
        since: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM cached_emission WHERE delivered = true AND delivered_at >= $1",
        )
        .bind(since)
        .fetch_one(pool)
        .await?;
        Ok(row.0)
    }

    /// Delete rows created before `cutoff` (TTL prune). Returns deleted count.
    pub async fn prune_older_than(pool: &PgPool, cutoff: DateTime<Utc>) -> Result<u64, sqlx::Error> {
        let res = sqlx::query("DELETE FROM cached_emission WHERE created_at < $1")
            .bind(cutoff).execute(pool).await?;
        Ok(res.rows_affected())
    }

    /// Most-recent rows for the Emissions UI.
    pub async fn list_recent(pool: &PgPool, limit: i64) -> Result<Vec<CachedEmission>, sqlx::Error> {
        sqlx::query_as::<_, CachedEmission>(
            "SELECT * FROM cached_emission ORDER BY observed_at DESC LIMIT $1",
        ).bind(limit).fetch_all(pool).await
    }

    pub async fn stats(pool: &PgPool) -> Result<CacheStats, sqlx::Error> {
        let row: (i64, i64) = sqlx::query_as(
            "SELECT count(*), count(*) FILTER (WHERE delivered = false) FROM cached_emission",
        ).fetch_one(pool).await?;
        Ok(CacheStats { total: row.0, undelivered: row.1 })
    }
}
