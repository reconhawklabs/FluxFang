//! Round-trip tests for `EmitterRepo`.

mod common;

use chrono::{Duration, TimeZone, Utc};
use common::{fresh_pool, seed_session, seed_wifi_source};
use fluxfang_core::{Condition, MatchMode, Op, Rule};
use fluxfang_db::models::{NewEmission, NewEmitter, NewEntity};
use fluxfang_db::repo::emitter::{EmitterListFilter, EmitterRuleError};
use fluxfang_db::{EmissionRepo, EmitterRepo, EntityRepo};
use sqlx::PgPool;
use uuid::Uuid;

fn new_emitter(name: &str) -> NewEmitter {
    NewEmitter {
        name: name.to_string(),
        type_: Some("Access Point".to_string()),
        entity_id: None,
        match_criteria: serde_json::json!({}),
        ..Default::default()
    }
}

/// Builds a `NewEmitter` for the auto-create path: identity_key `Some`,
/// plus the classification fields a real classifier would set.
fn new_auto_emitter(identity_key: &str) -> NewEmitter {
    NewEmitter {
        name: format!("WiFi AP ({identity_key})"),
        type_: None,
        entity_id: None,
        match_criteria: serde_json::json!({
            "match": "all",
            "conditions": [{"field": "bssid", "op": "eq", "value": identity_key}]
        }),
        emitter_type: Some("wifi_access_point".to_string()),
        attributes: serde_json::json!({"bssid": identity_key}),
        match_enabled: true,
        identity_key: Some(identity_key.to_string()),
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

/// Like [`insert_unassigned_wifi`], but the emission is inserted already
/// assigned to `emitter_id` — simulates an emission that auto-create already
/// claimed for some other (e.g. auto-created) emitter, which the new
/// "reassign ALL matching" semantics must still pick up.
async fn insert_wifi_assigned_to(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    bssid: &str,
    emitter_id: Uuid,
) -> Uuid {
    let e = EmissionRepo::insert(
        pool,
        NewEmission {
            emitter_id: Some(emitter_id),
            ..NewEmission::wifi(
                ds,
                session,
                serde_json::json!({"bssid": bssid, "channel": 6}),
            )
        },
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
async fn attach_emissions_matching_assigns_all_matching_wifi_emissions_regardless_of_assignment() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let emitter = EmitterRepo::insert(&pool, new_emitter("AP")).await.unwrap();
    let other = EmitterRepo::insert(&pool, new_emitter("Other AP"))
        .await
        .unwrap();

    let matching_a = insert_unassigned_wifi(&pool, ds, session, "aa:bb:cc:dd:ee:ff").await;
    // Already assigned to a DIFFERENT emitter (e.g. an auto-created one) —
    // the new semantics must reassign it, not skip it.
    let matching_b =
        insert_wifi_assigned_to(&pool, ds, session, "aa:bb:cc:dd:ee:ff", other.id).await;
    let non_matching = insert_unassigned_wifi(&pool, ds, session, "00:00:00:00:00:00").await;

    let rule = bssid_rule("aa:bb:cc:dd:ee:ff");
    let affected = EmitterRepo::attach_emissions_matching(&pool, emitter.id, &rule)
        .await
        .unwrap();
    assert_eq!(
        affected, 2,
        "both matching emissions must be (re)assigned, regardless of prior assignment"
    );

    let a = EmissionRepo::get(&pool, matching_a).await.unwrap().unwrap();
    let b = EmissionRepo::get(&pool, matching_b).await.unwrap().unwrap();
    let non = EmissionRepo::get(&pool, non_matching)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.emitter_id, Some(emitter.id));
    assert_eq!(
        b.emitter_id,
        Some(emitter.id),
        "an emission already assigned to a different emitter must be reassigned"
    );
    assert_eq!(
        non.emitter_id, None,
        "non-matching emission must stay unassigned"
    );

    let refreshed = EmitterRepo::get(&pool, emitter.id).await.unwrap().unwrap();
    assert!(refreshed.first_seen_at.is_some());
    assert!(refreshed.last_seen_at.is_some());
}

#[tokio::test]
async fn touch_seen_populates_both_columns_from_null() {
    let pool = fresh_pool().await;
    let emitter = EmitterRepo::insert(&pool, new_emitter("AP")).await.unwrap();
    assert_eq!(emitter.first_seen_at, None);
    assert_eq!(emitter.last_seen_at, None);

    // A whole-second timestamp (not `Utc::now()`): `timestamptz` only has
    // microsecond precision, while `DateTime<Utc>::now()` often carries
    // sub-microsecond (nanosecond) precision that Postgres would silently
    // truncate on round-trip, making an exact `==` comparison flaky.
    let at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let touched = EmitterRepo::touch_seen(&pool, emitter.id, at)
        .await
        .unwrap();
    assert_eq!(touched.first_seen_at, Some(at));
    assert_eq!(touched.last_seen_at, Some(at));
}

#[tokio::test]
async fn touch_seen_widens_the_window_in_either_direction() {
    let pool = fresh_pool().await;
    let emitter = EmitterRepo::insert(&pool, new_emitter("AP")).await.unwrap();

    // See the precision note above for why this uses a whole-second
    // timestamp instead of `Utc::now()`.
    let mid = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    EmitterRepo::touch_seen(&pool, emitter.id, mid)
        .await
        .unwrap();

    // A later observation advances last_seen_at but must not touch
    // first_seen_at.
    let later = mid + Duration::hours(1);
    let after_later = EmitterRepo::touch_seen(&pool, emitter.id, later)
        .await
        .unwrap();
    assert_eq!(after_later.first_seen_at, Some(mid));
    assert_eq!(after_later.last_seen_at, Some(later));

    // An earlier (e.g. out-of-order/backfilled) observation pulls
    // first_seen_at back but must not touch last_seen_at.
    let earlier = mid - Duration::hours(1);
    let after_earlier = EmitterRepo::touch_seen(&pool, emitter.id, earlier)
        .await
        .unwrap();
    assert_eq!(after_earlier.first_seen_at, Some(earlier));
    assert_eq!(after_earlier.last_seen_at, Some(later));
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
async fn count_matching_counts_all_matching_regardless_of_assignment_without_assigning() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let other = EmitterRepo::insert(&pool, new_emitter("Other AP"))
        .await
        .unwrap();

    let matching_a = insert_unassigned_wifi(&pool, ds, session, "aa:bb:cc:dd:ee:ff").await;
    // Already assigned to a different emitter — must still be counted, this
    // is the exact preview-shows-0 bug the "all matching" semantics fix.
    let matching_b =
        insert_wifi_assigned_to(&pool, ds, session, "aa:bb:cc:dd:ee:ff", other.id).await;
    insert_unassigned_wifi(&pool, ds, session, "00:00:00:00:00:00").await;

    let rule = bssid_rule("aa:bb:cc:dd:ee:ff");
    let count = EmitterRepo::count_matching(&pool, &rule).await.unwrap();
    assert_eq!(
        count, 2,
        "count must include already-assigned matching emissions"
    );

    // Confirm nothing was actually assigned/reassigned by count_matching.
    let a = EmissionRepo::get(&pool, matching_a).await.unwrap().unwrap();
    let b = EmissionRepo::get(&pool, matching_b).await.unwrap().unwrap();
    assert_eq!(a.emitter_id, None);
    assert_eq!(
        b.emitter_id,
        Some(other.id),
        "count_matching must not touch existing assignment"
    );
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

// ---------------------------------------------------------------------
// Phase A1: classification columns (emitter_type/attributes/match_enabled/
// identity_key) round-trip through insert/get, and get_or_create_by_identity
// is a race-safe atomic get-or-create keyed on identity_key.
// ---------------------------------------------------------------------

#[tokio::test]
async fn insert_roundtrips_classification_columns() {
    let pool = fresh_pool().await;

    let new = NewEmitter {
        name: "WiFi AP (aa:bb:cc:dd:ee:ff)".to_string(),
        type_: None,
        entity_id: None,
        match_criteria: serde_json::json!({}),
        emitter_type: Some("wifi_access_point".to_string()),
        attributes: serde_json::json!({"bssid": "aa:bb:cc:dd:ee:ff", "ssid": "Home"}),
        match_enabled: true,
        identity_key: Some("wifi_access_point:aa:bb:cc:dd:ee:ff".to_string()),
    };
    let created = EmitterRepo::insert(&pool, new).await.unwrap();
    assert_eq!(created.emitter_type.as_deref(), Some("wifi_access_point"));
    assert_eq!(
        created.attributes,
        serde_json::json!({"bssid": "aa:bb:cc:dd:ee:ff", "ssid": "Home"})
    );
    assert!(created.match_enabled);
    assert_eq!(
        created.identity_key.as_deref(),
        Some("wifi_access_point:aa:bb:cc:dd:ee:ff")
    );

    let got = EmitterRepo::get(&pool, created.id).await.unwrap().unwrap();
    assert_eq!(got.emitter_type, created.emitter_type);
    assert_eq!(got.attributes, created.attributes);
    assert_eq!(got.match_enabled, created.match_enabled);
    assert_eq!(got.identity_key, created.identity_key);
}

#[tokio::test]
async fn insert_defaults_classification_columns_when_a_plain_user_made_emitter_is_created() {
    let pool = fresh_pool().await;
    let created = EmitterRepo::insert(&pool, new_emitter("Plain AP"))
        .await
        .unwrap();
    assert_eq!(created.emitter_type, None);
    assert_eq!(created.attributes, serde_json::json!({}));
    assert!(created.match_enabled);
    assert_eq!(created.identity_key, None);
}

#[tokio::test]
async fn get_or_create_by_identity_creates_on_first_call() {
    let pool = fresh_pool().await;

    let (emitter, created) =
        EmitterRepo::get_or_create_by_identity(&pool, new_auto_emitter("aa:bb:cc:dd:ee:ff"))
            .await
            .unwrap();

    assert!(created, "first call for a fresh identity_key must create");
    assert_eq!(emitter.identity_key.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
    assert_eq!(emitter.emitter_type.as_deref(), Some("wifi_access_point"));

    let all = EmitterRepo::list(&pool).await.unwrap();
    assert_eq!(all.len(), 1);
}

#[tokio::test]
async fn get_or_create_by_identity_gets_the_same_row_on_second_call() {
    let pool = fresh_pool().await;

    let (first, first_created) =
        EmitterRepo::get_or_create_by_identity(&pool, new_auto_emitter("aa:bb:cc:dd:ee:ff"))
            .await
            .unwrap();
    assert!(first_created);

    let (second, second_created) =
        EmitterRepo::get_or_create_by_identity(&pool, new_auto_emitter("aa:bb:cc:dd:ee:ff"))
            .await
            .unwrap();

    assert!(
        !second_created,
        "second call for the same identity_key must GET, not create"
    );
    assert_eq!(
        second.id, first.id,
        "must return the SAME row, not a new one"
    );

    let all = EmitterRepo::list(&pool).await.unwrap();
    assert_eq!(
        all.len(),
        1,
        "exactly one emitter must exist for this identity_key, not two"
    );
}

/// The atomicity guarantee that actually matters: two concurrent calls with
/// the SAME identity_key must never both create a row. `tokio::join!` runs
/// both futures concurrently against the same pool (which has more than one
/// physical connection, see `common::fresh_pool`'s `max_connections(5)`), so
/// both `INSERT ... ON CONFLICT DO NOTHING` statements can genuinely race at
/// the database level rather than being serialized by single-connection
/// pooling. Exactly one of the two calls must observe `created = true`, and
/// exactly one row must exist afterwards.
#[tokio::test]
async fn get_or_create_by_identity_is_race_safe_under_concurrent_calls() {
    let pool = fresh_pool().await;

    let (r1, r2) = tokio::join!(
        EmitterRepo::get_or_create_by_identity(&pool, new_auto_emitter("aa:bb:cc:dd:ee:ff")),
        EmitterRepo::get_or_create_by_identity(&pool, new_auto_emitter("aa:bb:cc:dd:ee:ff")),
    );
    let (e1, created1) = r1.unwrap();
    let (e2, created2) = r2.unwrap();

    assert_eq!(e1.id, e2.id, "both calls must resolve to the same row");
    assert_eq!(
        created1 as u8 + created2 as u8,
        1,
        "exactly one of the two concurrent calls must have created the row \
         (created1={created1}, created2={created2})"
    );

    let all = EmitterRepo::list(&pool).await.unwrap();
    assert_eq!(
        all.len(),
        1,
        "a concurrent pair of get_or_create_by_identity calls for the same \
         identity_key must produce exactly ONE row, got {}",
        all.len()
    );
}

#[tokio::test]
async fn get_or_create_by_identity_distinguishes_different_identity_keys() {
    let pool = fresh_pool().await;

    let (a, a_created) =
        EmitterRepo::get_or_create_by_identity(&pool, new_auto_emitter("aa:bb:cc:dd:ee:ff"))
            .await
            .unwrap();
    let (b, b_created) =
        EmitterRepo::get_or_create_by_identity(&pool, new_auto_emitter("11:22:33:44:55:66"))
            .await
            .unwrap();

    assert!(a_created);
    assert!(b_created);
    assert_ne!(a.id, b.id);

    let all = EmitterRepo::list(&pool).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn set_match_enabled_flips_the_flag() {
    let pool = fresh_pool().await;
    let emitter = EmitterRepo::insert(&pool, new_emitter("AP")).await.unwrap();
    assert!(emitter.match_enabled, "match_enabled defaults to true");

    let disabled = EmitterRepo::set_match_enabled(&pool, emitter.id, false)
        .await
        .unwrap();
    assert!(!disabled.match_enabled);

    let got = EmitterRepo::get(&pool, emitter.id).await.unwrap().unwrap();
    assert!(!got.match_enabled);

    let re_enabled = EmitterRepo::set_match_enabled(&pool, emitter.id, true)
        .await
        .unwrap();
    assert!(re_enabled.match_enabled);
}

#[tokio::test]
async fn set_attributes_roundtrips_json() {
    let pool = fresh_pool().await;
    let emitter = EmitterRepo::insert(&pool, new_emitter("AP")).await.unwrap();
    assert_eq!(emitter.attributes, serde_json::json!({}));

    let attrs = serde_json::json!({"bssid": "aa:bb:cc:dd:ee:ff", "randomized_mac": true});
    let updated = EmitterRepo::set_attributes(&pool, emitter.id, &attrs)
        .await
        .unwrap();
    assert_eq!(updated.attributes, attrs);

    let got = EmitterRepo::get(&pool, emitter.id).await.unwrap().unwrap();
    assert_eq!(got.attributes, attrs);
}

// ---------------------------------------------------------------------
// Phase 1b: create_with_entity's backfill consistency fix (drop the
// `emitter_id IS NULL` guard so it reassigns ALL matching emissions, same
// as attach_emissions_matching/count_matching).
// ---------------------------------------------------------------------

#[tokio::test]
async fn create_with_entity_reassigns_all_matching_emissions_regardless_of_prior_assignment() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let other = EmitterRepo::insert(&pool, new_emitter("Other AP"))
        .await
        .unwrap();

    let target_bssid = "aa:bb:cc:dd:ee:ff";
    let unassigned = insert_unassigned_wifi(&pool, ds, session, target_bssid).await;
    // Already claimed by a different (e.g. auto-created) emitter — the new
    // "reassign ALL matching" semantics must still pick this up when
    // creating a new entity+emitter together, exactly like
    // attach_emissions_matching does for the existing-emitter path.
    let already_assigned =
        insert_wifi_assigned_to(&pool, ds, session, target_bssid, other.id).await;
    let non_matching = insert_unassigned_wifi(&pool, ds, session, "00:00:00:00:00:00").await;

    let rule = bssid_rule(target_bssid);
    let match_criteria = serde_json::to_value(&rule).unwrap();

    let result = EmitterRepo::create_with_entity(
        &pool,
        NewEntity {
            name: "Bob's phone".to_string(),
            notes: None,
        },
        "Bob's phone AP".to_string(),
        Some("Access Point".to_string()),
        None,
        match_criteria,
        Some(&rule),
    )
    .await
    .unwrap();

    assert_eq!(
        result.attached_count, 2,
        "both matching emissions must be (re)assigned, regardless of prior assignment"
    );

    let a = EmissionRepo::get(&pool, unassigned).await.unwrap().unwrap();
    let b = EmissionRepo::get(&pool, already_assigned)
        .await
        .unwrap()
        .unwrap();
    let non = EmissionRepo::get(&pool, non_matching)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.emitter_id, Some(result.emitter.id));
    assert_eq!(
        b.emitter_id,
        Some(result.emitter.id),
        "an emission already assigned to a different emitter must be reassigned"
    );
    assert_eq!(
        non.emitter_id, None,
        "non-matching emission must stay unassigned"
    );
}

// ---------------------------------------------------------------------
// Phase 1b: EmitterRepo::query — search (name/type/attributes/
// match_criteria substring) + entity_id filter + pagination.
// ---------------------------------------------------------------------

#[tokio::test]
async fn query_with_no_filter_returns_everything_and_correct_total() {
    let pool = fresh_pool().await;
    EmitterRepo::insert(&pool, new_emitter("A")).await.unwrap();
    EmitterRepo::insert(&pool, new_emitter("B")).await.unwrap();
    EmitterRepo::insert(&pool, new_emitter("C")).await.unwrap();

    let (rows, total) = EmitterRepo::query(&pool, EmitterListFilter::default())
        .await
        .unwrap();
    assert_eq!(total, 3);
    assert_eq!(rows.len(), 3);
}

#[tokio::test]
async fn query_search_matches_by_name_case_insensitively() {
    let pool = fresh_pool().await;
    EmitterRepo::insert(&pool, new_emitter("Cafe Free WiFi"))
        .await
        .unwrap();
    EmitterRepo::insert(&pool, new_emitter("Home Router"))
        .await
        .unwrap();

    let (rows, total) = EmitterRepo::query(
        &pool,
        EmitterListFilter {
            search: Some("cafe".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(total, 1, "rows: {rows:?}");
    assert_eq!(rows[0].name, "Cafe Free WiFi");
}

/// Typing a MAC/BSSID substring that only appears inside the JSON
/// `attributes` column (not `name`/`type`) must still find the emitter —
/// the whole point of searching `attributes::text` too.
#[tokio::test]
async fn query_search_matches_by_mac_inside_attributes_json() {
    let pool = fresh_pool().await;
    let with_mac = EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "WiFi AP".to_string(),
            type_: None,
            entity_id: None,
            match_criteria: serde_json::json!({}),
            emitter_type: Some("wifi_access_point".to_string()),
            attributes: serde_json::json!({"bssid": "aa:bb:cc:dd:ee:ff"}),
            match_enabled: true,
            identity_key: None,
        },
    )
    .await
    .unwrap();
    EmitterRepo::insert(&pool, new_emitter("Unrelated AP"))
        .await
        .unwrap();

    let (rows, total) = EmitterRepo::query(
        &pool,
        EmitterListFilter {
            search: Some("aa:bb:cc:dd:ee:ff".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(total, 1, "rows: {rows:?}");
    assert_eq!(rows[0].id, with_mac.id);
}

/// Same as above but the MAC only appears in `match_criteria`, not
/// `attributes` — confirms both JSON columns are searched.
#[tokio::test]
async fn query_search_matches_by_mac_inside_match_criteria_json() {
    let pool = fresh_pool().await;
    let with_rule = EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "WiFi AP".to_string(),
            type_: Some("Access Point".to_string()),
            entity_id: None,
            match_criteria: serde_json::json!({
                "match": "all",
                "conditions": [{"field": "bssid", "op": "eq", "value": "11:22:33:44:55:66"}]
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    EmitterRepo::insert(&pool, new_emitter("Unrelated AP"))
        .await
        .unwrap();

    let (rows, total) = EmitterRepo::query(
        &pool,
        EmitterListFilter {
            search: Some("11:22:33:44:55:66".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(total, 1, "rows: {rows:?}");
    assert_eq!(rows[0].id, with_rule.id);
}

#[tokio::test]
async fn query_entity_id_filters_to_only_that_entitys_emitters() {
    let pool = fresh_pool().await;
    let entity = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Bob".to_string(),
            notes: None,
        },
    )
    .await
    .unwrap();

    let under_entity = EmitterRepo::insert(
        &pool,
        NewEmitter {
            entity_id: Some(entity.id),
            ..new_emitter("Bob's AP")
        },
    )
    .await
    .unwrap();
    EmitterRepo::insert(&pool, new_emitter("Unrelated AP"))
        .await
        .unwrap();

    let (rows, total) = EmitterRepo::query(
        &pool,
        EmitterListFilter {
            entity_id: Some(entity.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(total, 1, "rows: {rows:?}");
    assert_eq!(rows[0].id, under_entity.id);
}

#[tokio::test]
async fn query_paginates_with_correct_total_ignoring_limit_offset() {
    let pool = fresh_pool().await;
    for name in ["A", "B", "C", "D", "E"] {
        EmitterRepo::insert(&pool, new_emitter(name)).await.unwrap();
    }

    let (page1, total1) = EmitterRepo::query(
        &pool,
        EmitterListFilter {
            limit: 2,
            offset: 0,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(total1, 5);
    assert_eq!(page1.len(), 2);
    assert_eq!(page1[0].name, "A");
    assert_eq!(page1[1].name, "B");

    let (page2, total2) = EmitterRepo::query(
        &pool,
        EmitterListFilter {
            limit: 2,
            offset: 2,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(total2, 5);
    assert_eq!(page2.len(), 2);
    assert_eq!(page2[0].name, "C");
    assert_eq!(page2[1].name, "D");

    let (page3, total3) = EmitterRepo::query(
        &pool,
        EmitterListFilter {
            limit: 2,
            offset: 4,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(total3, 5);
    assert_eq!(page3.len(), 1);
    assert_eq!(page3[0].name, "E");
}
