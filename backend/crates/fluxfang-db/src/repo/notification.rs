//! `NotificationRepo`: `notification` — fired-alert log; also the source
//! for the in-app Notifications page (`read_at: None` = unread).

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{NewNotification, Notification};

pub struct NotificationRepo;

impl NotificationRepo {
    pub async fn insert(pool: &PgPool, new: NewNotification) -> Result<Notification, sqlx::Error> {
        sqlx::query_as::<_, Notification>(
            "INSERT INTO notification \
                 (alert_rule_id, alert_method_id, fired_at, payload, delivery_status) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING *",
        )
        .bind(new.alert_rule_id)
        .bind(new.alert_method_id)
        .bind(new.fired_at)
        .bind(new.payload)
        .bind(new.delivery_status)
        .fetch_one(pool)
        .await
    }

    /// Page through notifications, newest-first. `unread_only` restricts to
    /// `read_at IS NULL`. Returns the page alongside the total row count
    /// matching the filter (ignoring `limit`/`offset`), for pagination UI.
    pub async fn list(
        pool: &PgPool,
        unread_only: bool,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<Notification>, i64), sqlx::Error> {
        let where_clause = if unread_only {
            "WHERE read_at IS NULL"
        } else {
            ""
        };

        let list_sql = format!(
            "SELECT * FROM notification {where_clause} \
             ORDER BY fired_at DESC \
             LIMIT $1 OFFSET $2"
        );
        let rows = sqlx::query_as::<_, Notification>(&list_sql)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?;

        let count_sql = format!("SELECT COUNT(*) FROM notification {where_clause}");
        let (total,): (i64,) = sqlx::query_as(&count_sql).fetch_one(pool).await?;

        Ok((rows, total))
    }

    /// Mark a notification read (`read_at = now()`).
    pub async fn mark_read(pool: &PgPool, id: Uuid) -> Result<Notification, sqlx::Error> {
        sqlx::query_as::<_, Notification>(
            "UPDATE notification SET read_at = now() WHERE id = $1 RETURNING *",
        )
        .bind(id)
        .fetch_one(pool)
        .await
    }

    /// Count of notifications with `read_at IS NULL`, for a nav-bar badge.
    pub async fn unread_count(pool: &PgPool) -> Result<i64, sqlx::Error> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM notification WHERE read_at IS NULL")
                .fetch_one(pool)
                .await?;
        Ok(count)
    }
}
