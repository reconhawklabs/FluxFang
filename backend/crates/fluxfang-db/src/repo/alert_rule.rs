//! `AlertRuleRepo`: `alert_rule`, plus the `alert_rule_method` join table
//! that records which `alert_method`(s) deliver each rule.
//!
//! ## `set_methods`: replace-in-a-transaction
//!
//! [`AlertRuleRepo::set_methods`] is the only method here that isn't a
//! single statement: Task 6.6 needs to replace a rule's entire linked-method
//! set atomically (e.g. the edit-rule form always submits the *complete*
//! desired method list, not a diff). It runs `DELETE FROM
//! alert_rule_method WHERE alert_rule_id = $1` followed by one `INSERT` per
//! requested method id, all inside one `pool.begin()`/`tx.commit()`
//! transaction (same pattern as
//! `EmitterRepo::attach_emissions_matching`'s `UPDATE` + first/last-seen
//! refresh) — so a caller (or a concurrent reader) never observes an
//! intermediate state where the old set has been cleared but the new one
//! hasn't landed yet.
//!
//! [`AlertRuleRepo::link_method`] is separately idempotent via `ON CONFLICT
//! DO NOTHING` (linking the same rule/method pair twice is a no-op, not an
//! error), which `set_methods` doesn't need since it always starts from a
//! clean slate.

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{AlertMethod, AlertRule, NewAlertRule};
use crate::repo::alert_method::ALERT_METHOD_COLUMNS;

pub struct AlertRuleRepo;

impl AlertRuleRepo {
    pub async fn insert(pool: &PgPool, new: NewAlertRule) -> Result<AlertRule, sqlx::Error> {
        sqlx::query_as::<_, AlertRule>(
            "INSERT INTO alert_rule (name, enabled, target_type, target_id, trigger) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING *",
        )
        .bind(new.name)
        .bind(new.enabled)
        .bind(new.target_type)
        .bind(new.target_id)
        .bind(new.trigger)
        .fetch_one(pool)
        .await
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<AlertRule>, sqlx::Error> {
        sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rule ORDER BY created_at ASC")
            .fetch_all(pool)
            .await
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<AlertRule>, sqlx::Error> {
        sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rule WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        pool: &PgPool,
        id: Uuid,
        name: &str,
        enabled: bool,
        target_type: Option<&str>,
        target_id: Option<Uuid>,
        trigger: serde_json::Value,
    ) -> Result<AlertRule, sqlx::Error> {
        sqlx::query_as::<_, AlertRule>(
            "UPDATE alert_rule \
             SET name = $2, enabled = $3, target_type = $4, target_id = $5, trigger = $6 \
             WHERE id = $1 \
             RETURNING *",
        )
        .bind(id)
        .bind(name)
        .bind(enabled)
        .bind(target_type)
        .bind(target_id)
        .bind(trigger)
        .fetch_one(pool)
        .await
    }

    /// Delete an alert rule, returning whether a row was actually removed.
    /// Cascades to `alert_rule_method` (`ON DELETE CASCADE`); SETs NULL any
    /// `notification.alert_rule_id` referencing it (`ON DELETE SET NULL`) —
    /// see `tests/repo_alert_rule.rs`'s
    /// `deleting_alert_rule_cascades_alert_rule_method_rows` and
    /// `tests/repo_notification.rs`'s
    /// `deleting_alert_rule_sets_notification_alert_rule_id_to_null`.
    pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM alert_rule WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Link `method_id` to `rule_id`. Idempotent: linking the same pair
    /// twice leaves exactly one `alert_rule_method` row.
    pub async fn link_method(
        pool: &PgPool,
        rule_id: Uuid,
        method_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO alert_rule_method (alert_rule_id, alert_method_id) \
             VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(rule_id)
        .bind(method_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Remove one rule/method link, if it exists.
    pub async fn unlink_method(
        pool: &PgPool,
        rule_id: Uuid,
        method_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM alert_rule_method WHERE alert_rule_id = $1 AND alert_method_id = $2",
        )
        .bind(rule_id)
        .bind(method_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// All `alert_method`s currently linked to `rule_id`, joined through
    /// `alert_rule_method`.
    pub async fn methods_for_rule(
        pool: &PgPool,
        rule_id: Uuid,
    ) -> Result<Vec<AlertMethod>, sqlx::Error> {
        let cols: String = ALERT_METHOD_COLUMNS
            .split(", ")
            .map(|c| format!("am.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {cols} \
             FROM alert_rule_method arm \
             JOIN alert_method am ON am.id = arm.alert_method_id \
             WHERE arm.alert_rule_id = $1 \
             ORDER BY am.created_at ASC"
        );
        sqlx::query_as::<_, AlertMethod>(&sql)
            .bind(rule_id)
            .fetch_all(pool)
            .await
    }

    /// Replace `rule_id`'s entire linked-method set with exactly
    /// `method_ids`, atomically (delete-all-then-insert-each inside one
    /// transaction — see module docs).
    pub async fn set_methods(
        pool: &PgPool,
        rule_id: Uuid,
        method_ids: &[Uuid],
    ) -> Result<(), sqlx::Error> {
        let mut tx = pool.begin().await?;

        sqlx::query("DELETE FROM alert_rule_method WHERE alert_rule_id = $1")
            .bind(rule_id)
            .execute(&mut *tx)
            .await?;

        for method_id in method_ids {
            sqlx::query(
                "INSERT INTO alert_rule_method (alert_rule_id, alert_method_id) VALUES ($1, $2)",
            )
            .bind(rule_id)
            .bind(method_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await
    }
}
