//! Round-trip tests for `EmitterRepo`.

mod common;

use common::{fresh_pool, seed_session, seed_wifi_source};
use fluxfang_core::{Condition, MatchMode, Op, Rule};
use fluxfang_db::models::{NewEmission, NewEmitter, NewEntity};
use fluxfang_db::repo::emitter::EmitterRuleError;
use fluxfang_db::{EmissionRepo, EmitterRepo, EntityRepo};
use sqlx::PgPool;
use uuid::Uuid;

fn new_emitter(name: &str) -> NewEmitter {
    NewEmitter {
        name: name.to_string(),
        type_: Some("Access Point".to_string()),
        entity_id: None,
        match_criteria: serde_json::json!({}),
    }
}

async fn insert_unassigned_wifi(pool: &PgPool, ds: Uuid, session: Uuid, bssid: &str) -> Uuid {
    let e = EmissionRepo::insert(
        pool,
        NewEmission::wifi(
            ds,
            session,
            serde_json::json!({"bssid": bssid, "channel": 6}),
        ),
    )
    .await
    .unwrap();
    e.id
}

fn bssid_rule(bssid: &str) -> Rule {
    Rule {
        match_mode: MatchMode::All,
        conditions: vec![Condition {
            field: "bssid".to_string(),
            op: Op::Eq,
            value: serde_json::json!(bssid),
        }],
    }
}

#[tokio::test]
async fn insert_and_get_emitter_roundtrips() {
    let pool = fresh_pool().await;

    let e = EmitterRepo::insert(&pool, new_emitter("Home AP"))
        .await
        .unwrap();
    assert_eq!(e.name, "Home AP");
    assert_eq!(e.type_.as_deref(), Some("Access Point"));
    assert_eq!(e.entity_id, None);
    assert_eq!(e.first_seen_at, None);
    assert_eq!(e.last_seen_at, None);

    let got = EmitterRepo::get(&pool, e.id).await.unwrap().unwrap();
    assert_eq!(got.id, e.id);
    assert_eq!(got.name, "Home AP");
}

#[tokio::test]
async fn get_returns_none_for_unknown_id() {
    let pool = fresh_pool().await;
    let got = EmitterRepo::get(&pool, Uuid::new_v4()).await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn list_returns_all_emitters() {
    let pool = fresh_pool().await;
    EmitterRepo::insert(&pool, new_emitter("A")).await.unwrap();
    EmitterRepo::insert(&pool, new_emitter("B")).await.unwrap();

    let all = EmitterRepo::list(&pool).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn set_entity_associates_then_detaches() {
    let pool = fresh_pool().await;
    let emitter = EmitterRepo::insert(&pool, new_emitter("AP")).await.unwrap();
    let entity = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Bob".to_string(),
            notes: None,
        },
    )
    .await
    .unwrap();

    let associated = EmitterRepo::set_entity(&pool, emitter.id, Some(entity.id))
        .await
        .unwrap();
    assert_eq!(associated.entity_id, Some(entity.id));

    let detached = EmitterRepo::set_entity(&pool, emitter.id, None)
        .await
        .unwrap();
    assert_eq!(detached.entity_id, None);
}

#[tokio::test]
async fn update_rule_persists_new_match_criteria() {
    let pool = fresh_pool().await;
    let emitter = EmitterRepo::insert(&pool, new_emitter("AP")).await.unwrap();

    let rule_json = serde_json::json!({
        "match": "all",
        "conditions": [{"field": "bssid", "op": "eq", "value": "aa:bb:cc:dd:ee:ff"}]
    });
    let updated = EmitterRepo::update_rule(&pool, emitter.id, &rule_json)
        .await
        .unwrap();
    assert_eq!(updated.match_criteria, rule_json);

    let got = EmitterRepo::get(&pool, emitter.id).await.unwrap().unwrap();
    assert_eq!(got.match_criteria, rule_json);
}

#[tokio::test]
async fn attach_emissions_matching_assigns_only_matching_unassigned_wifi_emissions() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let emitter = EmitterRepo::insert(&pool, new_emitter("AP")).await.unwrap();

    let matching_a = insert_unassigned_wifi(&pool, ds, session, "aa:bb:cc:dd:ee:ff").await;
    let matching_b = insert_unassigned_wifi(&pool, ds, session, "aa:bb:cc:dd:ee:ff").await;
    let non_matching = insert_unassigned_wifi(&pool, ds, session, "00:00:00:00:00:00").await;

    let rule = bssid_rule("aa:bb:cc:dd:ee:ff");
    let affected = EmitterRepo::attach_emissions_matching(&pool, emitter.id, &rule)
        .await
        .unwrap();
    assert_eq!(affected, 2);

    let a = EmissionRepo::get(&pool, matching_a).await.unwrap().unwrap();
    let b = EmissionRepo::get(&pool, matching_b).await.unwrap().unwrap();
    let non = EmissionRepo::get(&pool, non_matching)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.emitter_id, Some(emitter.id));
    assert_eq!(b.emitter_id, Some(emitter.id));
    assert_eq!(
        non.emitter_id, None,
        "non-matching emission must stay unassigned"
    );

    let refreshed = EmitterRepo::get(&pool, emitter.id).await.unwrap().unwrap();
    assert!(refreshed.first_seen_at.is_some());
    assert!(refreshed.last_seen_at.is_some());
}

#[tokio::test]
async fn attach_emissions_matching_returns_zero_when_nothing_matches() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let emitter = EmitterRepo::insert(&pool, new_emitter("AP")).await.unwrap();

    insert_unassigned_wifi(&pool, ds, session, "00:00:00:00:00:00").await;

    let rule = bssid_rule("aa:bb:cc:dd:ee:ff");
    let affected = EmitterRepo::attach_emissions_matching(&pool, emitter.id, &rule)
        .await
        .unwrap();
    assert_eq!(affected, 0);

    let refreshed = EmitterRepo::get(&pool, emitter.id).await.unwrap().unwrap();
    assert!(refreshed.first_seen_at.is_none());
    assert!(refreshed.last_seen_at.is_none());
}

#[tokio::test]
async fn attach_emissions_matching_rejects_invalid_rule_instead_of_silently_skipping() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let emitter = EmitterRepo::insert(&pool, new_emitter("AP")).await.unwrap();
    insert_unassigned_wifi(&pool, ds, session, "aa:bb:cc:dd:ee:ff").await;

    // `bssid` is a Mac field: `Gte` is not a valid op for it (only Number
    // fields support ordering) -> the checked translator should reject this
    // with InvalidOp rather than the backfill silently matching nothing.
    let invalid_rule = Rule {
        match_mode: MatchMode::All,
        conditions: vec![Condition {
            field: "bssid".to_string(),
            op: Op::Gte,
            value: serde_json::json!("aa:bb:cc:dd:ee:ff"),
        }],
    };

    let err = EmitterRepo::attach_emissions_matching(&pool, emitter.id, &invalid_rule)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EmitterRuleError::Rule(_)),
        "expected a Rule error, got {err:?}"
    );
}

#[tokio::test]
async fn count_matching_counts_without_assigning() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let matching_a = insert_unassigned_wifi(&pool, ds, session, "aa:bb:cc:dd:ee:ff").await;
    let matching_b = insert_unassigned_wifi(&pool, ds, session, "aa:bb:cc:dd:ee:ff").await;
    insert_unassigned_wifi(&pool, ds, session, "00:00:00:00:00:00").await;

    let rule = bssid_rule("aa:bb:cc:dd:ee:ff");
    let count = EmitterRepo::count_matching(&pool, &rule).await.unwrap();
    assert_eq!(count, 2);

    // Confirm nothing was actually assigned.
    let a = EmissionRepo::get(&pool, matching_a).await.unwrap().unwrap();
    let b = EmissionRepo::get(&pool, matching_b).await.unwrap().unwrap();
    assert_eq!(a.emitter_id, None);
    assert_eq!(b.emitter_id, None);
}

#[tokio::test]
async fn count_matching_rejects_invalid_rule() {
    let pool = fresh_pool().await;
    let rule = Rule {
        match_mode: MatchMode::All,
        conditions: vec![Condition {
            field: "not_a_real_field".to_string(),
            op: Op::Eq,
            value: serde_json::json!("x"),
        }],
    };
    let err = EmitterRepo::count_matching(&pool, &rule).await.unwrap_err();
    assert!(matches!(err, EmitterRuleError::Rule(_)));
}
