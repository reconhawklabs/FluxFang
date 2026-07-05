//! Round-trip tests for `NotificationRepo`, including the `ON DELETE SET
//! NULL` behavior of `alert_rule_id`/`alert_method_id`.

mod common;

use chrono::{Duration, Utc};
use common::fresh_pool;
use fluxfang_db::models::{NewAlertMethod, NewAlertRule, NewNotification};
use fluxfang_db::repo::alert_method::AlertMethodRepo;
use fluxfang_db::repo::alert_rule::AlertRuleRepo;
use fluxfang_db::repo::notification::NotificationRepo;
use sqlx::PgPool;
use uuid::Uuid;

async fn seed_rule(pool: &PgPool) -> Uuid {
    AlertRuleRepo::insert(
        pool,
        NewAlertRule {
            name: "Rule".to_string(),
            enabled: true,
            target_type: None,
            target_id: None,
            trigger: serde_json::json!({"on": "detected"}),
        },
    )
    .await
    .unwrap()
    .id
}

async fn seed_method(pool: &PgPool) -> Uuid {
    AlertMethodRepo::insert(
        pool,
        NewAlertMethod {
            name: "Method".to_string(),
            type_: "in_app".to_string(),
            enabled: true,
            config_encrypted: vec![],
        },
    )
    .await
    .unwrap()
    .id
}

async fn seed_notification(
    pool: &PgPool,
    alert_rule_id: Option<Uuid>,
    alert_method_id: Option<Uuid>,
    fired_at: chrono::DateTime<Utc>,
) -> Uuid {
    NotificationRepo::insert(
        pool,
        NewNotification {
            alert_rule_id,
            alert_method_id,
            fired_at,
            payload: serde_json::json!({"msg": "hello"}),
            delivery_status: "sent".to_string(),
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn insert_and_roundtrip_notification() {
    let pool = fresh_pool().await;
    let rule = seed_rule(&pool).await;
    let method = seed_method(&pool).await;
    let now = Utc::now();

    let inserted = NotificationRepo::insert(
        &pool,
        NewNotification {
            alert_rule_id: Some(rule),
            alert_method_id: Some(method),
            fired_at: now,
            payload: serde_json::json!({"msg": "hello"}),
            delivery_status: "sent".to_string(),
        },
    )
    .await
    .unwrap();

    assert_eq!(inserted.alert_rule_id, Some(rule));
    assert_eq!(inserted.alert_method_id, Some(method));
    assert_eq!(inserted.payload["msg"], "hello");
    assert_eq!(inserted.delivery_status, "sent");
    assert!(inserted.read_at.is_none());
}

#[tokio::test]
async fn insert_rejects_invalid_delivery_status_via_check_constraint() {
    let pool = fresh_pool().await;

    let bad = NewNotification {
        alert_rule_id: None,
        alert_method_id: None,
        fired_at: Utc::now(),
        payload: serde_json::json!({}),
        delivery_status: "carrier_pigeon".to_string(),
    };

    let result = NotificationRepo::insert(&pool, bad).await;
    assert!(
        result.is_err(),
        "expected the delivery_status CHECK constraint to reject an invalid value"
    );
}

#[tokio::test]
async fn list_orders_by_fired_at_desc_and_reports_total() {
    let pool = fresh_pool().await;
    let now = Utc::now();
    let oldest = seed_notification(&pool, None, None, now - Duration::hours(2)).await;
    let middle = seed_notification(&pool, None, None, now - Duration::hours(1)).await;
    let newest = seed_notification(&pool, None, None, now).await;

    let (rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
    assert_eq!(total, 3);
    assert_eq!(
        rows.iter().map(|n| n.id).collect::<Vec<_>>(),
        vec![newest, middle, oldest]
    );
}

#[tokio::test]
async fn list_respects_limit_and_offset() {
    let pool = fresh_pool().await;
    let now = Utc::now();
    seed_notification(&pool, None, None, now - Duration::hours(2)).await;
    let middle = seed_notification(&pool, None, None, now - Duration::hours(1)).await;
    seed_notification(&pool, None, None, now).await;

    let (rows, total) = NotificationRepo::list(&pool, false, 1, 1).await.unwrap();
    assert_eq!(total, 3);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, middle);
}

#[tokio::test]
async fn list_unread_only_filters_out_read_notifications() {
    let pool = fresh_pool().await;
    let now = Utc::now();
    let unread = seed_notification(&pool, None, None, now).await;
    let read = seed_notification(&pool, None, None, now - Duration::hours(1)).await;
    NotificationRepo::mark_read(&pool, read).await.unwrap();

    let (rows, total) = NotificationRepo::list(&pool, true, 10, 0).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, unread);
}

#[tokio::test]
async fn mark_read_sets_read_at() {
    let pool = fresh_pool().await;
    let id = seed_notification(&pool, None, None, Utc::now()).await;

    let updated = NotificationRepo::mark_read(&pool, id).await.unwrap();
    assert!(updated.read_at.is_some());
}

#[tokio::test]
async fn unread_count_reflects_only_unread_rows() {
    let pool = fresh_pool().await;
    let now = Utc::now();
    seed_notification(&pool, None, None, now).await;
    let read = seed_notification(&pool, None, None, now).await;
    NotificationRepo::mark_read(&pool, read).await.unwrap();

    let count = NotificationRepo::unread_count(&pool).await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn deleting_alert_rule_sets_notification_alert_rule_id_to_null() {
    let pool = fresh_pool().await;
    let rule = seed_rule(&pool).await;
    let method = seed_method(&pool).await;
    let notif_id = seed_notification(&pool, Some(rule), Some(method), Utc::now()).await;

    AlertRuleRepo::delete(&pool, rule).await.unwrap();

    let (rows, _total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
    let notif = rows.iter().find(|n| n.id == notif_id).unwrap();
    assert_eq!(
        notif.alert_rule_id, None,
        "deleting the alert_rule should SET NULL, not delete the notification"
    );
    assert_eq!(
        notif.alert_method_id,
        Some(method),
        "unrelated alert_method_id should be untouched"
    );
}

#[tokio::test]
async fn deleting_alert_method_sets_notification_alert_method_id_to_null() {
    let pool = fresh_pool().await;
    let rule = seed_rule(&pool).await;
    let method = seed_method(&pool).await;
    let notif_id = seed_notification(&pool, Some(rule), Some(method), Utc::now()).await;

    AlertMethodRepo::delete(&pool, method).await.unwrap();

    let (rows, _total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
    let notif = rows.iter().find(|n| n.id == notif_id).unwrap();
    assert_eq!(
        notif.alert_method_id, None,
        "deleting the alert_method should SET NULL, not delete the notification"
    );
    assert_eq!(
        notif.alert_rule_id,
        Some(rule),
        "unrelated alert_rule_id should be untouched"
    );
}
