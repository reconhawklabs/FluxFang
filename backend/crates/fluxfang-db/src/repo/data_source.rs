//! `DataSourceRepo`: configured wifi/gps capture devices.

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{DataSource, NewDataSource};

pub struct DataSourceRepo;

impl DataSourceRepo {
    /// Create a new data source. Always starts in `status = 'stopped'`;
    /// use `set_status` to transition it once capture actually begins.
    pub async fn insert(pool: &PgPool, new: NewDataSource) -> Result<DataSource, sqlx::Error> {
        sqlx::query_as::<_, DataSource>(
            "INSERT INTO data_source (kind, mode, interface, status, config) \
             VALUES ($1, $2, $3, 'stopped', $4) \
             RETURNING *",
        )
        .bind(new.kind)
        .bind(new.mode)
        .bind(new.interface)
        .bind(new.config)
        .fetch_one(pool)
        .await
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<DataSource>, sqlx::Error> {
        sqlx::query_as::<_, DataSource>("SELECT * FROM data_source ORDER BY created_at")
            .fetch_all(pool)
            .await
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<DataSource>, sqlx::Error> {
        sqlx::query_as::<_, DataSource>("SELECT * FROM data_source WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// Update the mutable configuration of a data source (its `config`
    /// JSON blob, `mode`, and `interface`). `kind` never changes after
    /// creation, so the caller is responsible for passing a `mode` that
    /// still satisfies the `kind`/`mode` CHECK constraint for this row.
    pub async fn update(
        pool: &PgPool,
        id: Uuid,
        config: serde_json::Value,
        mode: &str,
        interface: Option<&str>,
    ) -> Result<DataSource, sqlx::Error> {
        sqlx::query_as::<_, DataSource>(
            "UPDATE data_source \
             SET config = $2, mode = $3, interface = $4 \
             WHERE id = $1 \
             RETURNING *",
        )
        .bind(id)
        .bind(config)
        .bind(mode)
        .bind(interface)
        .fetch_one(pool)
        .await
    }

    /// Transition a data source's runtime `status` (e.g. on start/stop/
    /// error from the capture supervisor). `last_error` is cleared to NULL
    /// when not provided. A transition to `'running'` also stamps
    /// `last_ok_at = now()`, recording the last time the source was
    /// confirmed healthy (see migration
    /// `0009_location_quality_and_datasource_health.sql`).
    pub async fn set_status(
        pool: &PgPool,
        id: Uuid,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<DataSource, sqlx::Error> {
        sqlx::query_as::<_, DataSource>(
            "UPDATE data_source \
                SET status = $2, last_error = $3, \
                    last_ok_at = CASE WHEN $2 = 'running' THEN now() ELSE last_ok_at END \
                WHERE id = $1 RETURNING *",
        )
        .bind(id)
        .bind(status)
        .bind(last_error)
        .fetch_one(pool)
        .await
    }

    /// Record the user's *intent* for a source (`'running'` | `'stopped'`),
    /// independent of its actual `status`. The reconciler drives `status`
    /// toward this and retries while it is `'running'`.
    pub async fn set_desired_state(
        pool: &PgPool,
        id: Uuid,
        desired: &str,
    ) -> Result<DataSource, sqlx::Error> {
        sqlx::query_as::<_, DataSource>(
            "UPDATE data_source SET desired_state = $2 WHERE id = $1 RETURNING *",
        )
        .bind(id)
        .bind(desired)
        .fetch_one(pool)
        .await
    }

    /// Delete a data source, returning whether a row was actually removed.
    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM data_source WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
