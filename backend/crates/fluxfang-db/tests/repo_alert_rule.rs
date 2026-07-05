//! Round-trip tests for `AlertRuleRepo`, including the `alert_rule_method`
//! join table and its cascade-delete behavior.

mod common;

use common::fresh_pool;
use fluxfang_db::models::{AlertRule, NewAlertMethod, NewAlertRule};
use fluxfang_db::repo::alert_method::AlertMethodRepo;
use fluxfang_db::repo::alert_rule::AlertRuleRepo;
use sqlx::PgPool;
use uuid::Uuid;

async fn seed_rule(pool: &PgPool) -> AlertRule {
    AlertRuleRepo::insert(
        pool,
        NewAlertRule {
            name: "Detected Rule".to_string(),
            enabled: true,
            target_type: None,
            target_id: None,
            trigger: serde_json::json!({"on": "host_enters_zone"}),
        },
    )
    .await
    .unwrap()
}

async fn seed_method(pool: &PgPool, name: &str) -> Uuid {
    AlertMethodRepo::insert(
        pool,
        NewAlertMethod {
            name: name.to_string(),
            type_: "in_app".to_string(),
            enabled: true,
            config_encrypted: vec![],
        },
    )
    .await
    .unwrap()
    .id
}

async fn alert_rule_method_count(pool: &PgPool, rule_id: Uuid) -> i64 {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM alert_rule_method WHERE alert_rule_id = $1")
            .bind(rule_id)
            .fetch_one(pool)
            .await
            .unwrap();
    count
}

#[tokio::test]
async fn insert_and_get_alert_rule_roundtrips() {
    let pool = fresh_pool().await;

    let inserted = AlertRuleRepo::insert(
        &pool,
        NewAlertRule {
            name: "Emitter Rule".to_string(),
            enabled: true,
            target_type: Some("emitter".to_string()),
            target_id: Some(Uuid::new_v4()),
            trigger: serde_json::json!({"on": "detected"}),
        },
    )
    .await
    .unwrap();

    assert_eq!(inserted.name, "Emitter Rule");
    assert_eq!(inserted.target_type.as_deref(), Some("emitter"));
    assert_eq!(inserted.trigger["on"], "detected");

    let got = AlertRuleRepo::get(&pool, inserted.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.id, inserted.id);
}

#[tokio::test]
async fn insert_allows_null_target_for_host_zone_rules() {
    let pool = fresh_pool().await;
    let rule = seed_rule(&pool).await;
    assert_eq!(rule.target_type, None);
    assert_eq!(rule.target_id, None);
}

#[tokio::test]
async fn insert_rejects_invalid_target_type_via_check_constraint() {
    let pool = fresh_pool().await;

    let bad = NewAlertRule {
        name: "Bad".to_string(),
        enabled: true,
        target_type: Some("gremlin".to_string()),
        target_id: Some(Uuid::new_v4()),
        trigger: serde_json::json!({"on": "detected"}),
    };

    let result = AlertRuleRepo::insert(&pool, bad).await;
    assert!(
        result.is_err(),
        "expected the target_type CHECK constraint to reject an invalid value"
    );
}

#[tokio::test]
async fn get_returns_none_for_unknown_id() {
    let pool = fresh_pool().await;
    let got = AlertRuleRepo::get(&pool, Uuid::new_v4()).await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn list_returns_all_rules() {
    let pool = fresh_pool().await;
    seed_rule(&pool).await;
    seed_rule(&pool).await;

    let all = AlertRuleRepo::list(&pool).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn update_replaces_all_fields() {
    let pool = fresh_pool().await;
    let rule = seed_rule(&pool).await;
    let target_id = Uuid::new_v4();

    let updated = AlertRuleRepo::update(
        &pool,
        rule.id,
        "Renamed Rule",
        false,
        Some("entity"),
        Some(target_id),
        serde_json::json!({"on": "leaves_zone"}),
    )
    .await
    .unwrap();

    assert_eq!(updated.id, rule.id);
    assert_eq!(updated.name, "Renamed Rule");
    assert!(!updated.enabled);
    assert_eq!(updated.target_type.as_deref(), Some("entity"));
    assert_eq!(updated.target_id, Some(target_id));
    assert_eq!(updated.trigger["on"], "leaves_zone");
}

#[tokio::test]
async fn delete_removes_alert_rule() {
    let pool = fresh_pool().await;
    let rule = seed_rule(&pool).await;

    let deleted = AlertRuleRepo::delete(&pool, rule.id).await.unwrap();
    assert!(deleted);
    assert!(AlertRuleRepo::get(&pool, rule.id).await.unwrap().is_none());
}

#[tokio::test]
async fn link_method_is_idempotent() {
    let pool = fresh_pool().await;
    let rule = seed_rule(&pool).await;
    let method = seed_method(&pool, "M1").await;

    AlertRuleRepo::link_method(&pool, rule.id, method)
        .await
        .unwrap();
    AlertRuleRepo::link_method(&pool, rule.id, method)
        .await
        .unwrap();

    assert_eq!(alert_rule_method_count(&pool, rule.id).await, 1);
}

#[tokio::test]
async fn unlink_method_removes_the_join_row() {
    let pool = fresh_pool().await;
    let rule = seed_rule(&pool).await;
    let method = seed_method(&pool, "M1").await;

    AlertRuleRepo::link_method(&pool, rule.id, method)
        .await
        .unwrap();
    AlertRuleRepo::unlink_method(&pool, rule.id, method)
        .await
        .unwrap();

    assert_eq!(alert_rule_method_count(&pool, rule.id).await, 0);
}

#[tokio::test]
async fn methods_for_rule_joins_through_alert_rule_method() {
    let pool = fresh_pool().await;
    let rule = seed_rule(&pool).await;
    let m1 = seed_method(&pool, "M1").await;
    let m2 = seed_method(&pool, "M2").await;

    AlertRuleRepo::link_method(&pool, rule.id, m1)
        .await
        .unwrap();
    AlertRuleRepo::link_method(&pool, rule.id, m2)
        .await
        .unwrap();

    let methods = AlertRuleRepo::methods_for_rule(&pool, rule.id)
        .await
        .unwrap();
    let mut ids: Vec<Uuid> = methods.iter().map(|m| m.id).collect();
    ids.sort();
    let mut expected = vec![m1, m2];
    expected.sort();
    assert_eq!(ids, expected);
}

#[tokio::test]
async fn set_methods_replaces_the_linked_set() {
    let pool = fresh_pool().await;
    let rule = seed_rule(&pool).await;
    let a = seed_method(&pool, "A").await;
    let b = seed_method(&pool, "B").await;
    let c = seed_method(&pool, "C").await;

    AlertRuleRepo::link_method(&pool, rule.id, a).await.unwrap();
    AlertRuleRepo::link_method(&pool, rule.id, b).await.unwrap();

    AlertRuleRepo::set_methods(&pool, rule.id, &[b, c])
        .await
        .unwrap();

    let methods = AlertRuleRepo::methods_for_rule(&pool, rule.id)
        .await
        .unwrap();
    let mut ids: Vec<Uuid> = methods.iter().map(|m| m.id).collect();
    ids.sort();
    let mut expected = vec![b, c];
    expected.sort();
    assert_eq!(ids, expected);
}

#[tokio::test]
async fn deleting_alert_rule_cascades_alert_rule_method_rows() {
    let pool = fresh_pool().await;
    let rule = seed_rule(&pool).await;
    let method = seed_method(&pool, "M1").await;
    AlertRuleRepo::link_method(&pool, rule.id, method)
        .await
        .unwrap();
    assert_eq!(alert_rule_method_count(&pool, rule.id).await, 1);

    AlertRuleRepo::delete(&pool, rule.id).await.unwrap();

    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM alert_rule_method WHERE alert_method_id = $1")
            .bind(method)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "deleting the alert_rule should cascade-delete its alert_rule_method rows"
    );
}
