//! Round-trip tests for `EmissionRepo`.

mod common;

use chrono::{Duration, Utc};
use common::{fresh_pool, seed_session, seed_wifi_source};
use fluxfang_core::{Condition, MatchMode, Op};
use fluxfang_db::models::{Emission, NewEmission, NewEmitter};
use fluxfang_db::repo::emission::{EmissionFilter, EmissionQueryError};
use fluxfang_db::{EmissionRepo, EmitterRepo};
use sqlx::PgPool;
use uuid::Uuid;

/// Seed an emitter classified as `emitter_type`, for the Phase A5
/// `emitter_type`/`emitter_category` filter tests below.
async fn seed_classified_emitter(pool: &PgPool, name: &str, emitter_type: &str) -> Uuid {
    EmitterRepo::insert(
        pool,
        NewEmitter {
            name: name.to_string(),
            type_: None,
            entity_id: None,
            match_criteria: serde_json::json!({}),
            emitter_type: Some(emitter_type.to_string()),
            attributes: serde_json::json!({}),
            match_enabled: true,
            identity_key: None,
        },
    )
    .await
    .unwrap()
    .id
}

fn wifi_payload(bssid: &str, channel: i64) -> serde_json::Value {
    serde_json::json!({"bssid": bssid, "channel": channel})
}

async fn insert_wifi(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    bssid: &str,
    channel: i64,
) -> Emission {
    EmissionRepo::insert(
        pool,
        NewEmission::wifi(ds, session, wifi_payload(bssid, channel)),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn insert_and_get_emission_roundtrips() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let e = insert_wifi(&pool, ds, session, "aa:bb:cc:dd:ee:ff", 6).await;
    assert_eq!(e.kind, "wifi");
    assert_eq!(e.data_source_id, Some(ds));
    assert_eq!(e.session_id, Some(session));
    assert_eq!(e.lon, None);
    assert_eq!(e.lat, None);

    let got = EmissionRepo::get(&pool, e.id).await.unwrap().unwrap();
    assert_eq!(got.id, e.id);
    assert_eq!(got.payload["bssid"], "aa:bb:cc:dd:ee:ff");
    assert_eq!(got.payload["channel"], 6);
}

#[tokio::test]
async fn set_emitter_assigns_and_persists() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let emission = insert_wifi(&pool, ds, session, "aa:bb:cc:dd:ee:ff", 6).await;
    assert_eq!(emission.emitter_id, None);

    let emitter = fluxfang_db::EmitterRepo::insert(
        &pool,
        fluxfang_db::models::NewEmitter {
            name: "AP".to_string(),
            type_: None,
            entity_id: None,
            match_criteria: serde_json::json!({}),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let updated = EmissionRepo::set_emitter(&pool, emission.id, emitter.id)
        .await
        .unwrap();
    assert_eq!(updated.emitter_id, Some(emitter.id));

    let got = EmissionRepo::get(&pool, emission.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.emitter_id, Some(emitter.id));
}

#[tokio::test]
async fn get_returns_none_for_unknown_id() {
    let pool = fresh_pool().await;
    let got = EmissionRepo::get(&pool, Uuid::new_v4()).await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn insert_with_location_roundtrips_lon_lat() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let mut new = NewEmission::wifi(ds, session, wifi_payload("aa:bb:cc:dd:ee:ff", 6));
    new.location = Some((-122.4, 37.7)); // (lon, lat)

    let inserted = EmissionRepo::insert(&pool, new).await.unwrap();
    assert!((inserted.lon.unwrap() - -122.4).abs() < 1e-9);
    assert!((inserted.lat.unwrap() - 37.7).abs() < 1e-9);

    let got = EmissionRepo::get(&pool, inserted.id)
        .await
        .unwrap()
        .unwrap();
    assert!((got.lon.unwrap() - -122.4).abs() < 1e-9);
    assert!((got.lat.unwrap() - 37.7).abs() < 1e-9);
}

#[tokio::test]
async fn query_filters_by_data_source_id() {
    let pool = fresh_pool().await;
    let ds_a = seed_wifi_source(&pool).await;
    let ds_b = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let a = insert_wifi(&pool, ds_a, session, "aa:aa:aa:aa:aa:aa", 1).await;
    insert_wifi(&pool, ds_b, session, "bb:bb:bb:bb:bb:bb", 1).await;

    let filter = EmissionFilter {
        data_source_id: Some(ds_a),
        ..EmissionFilter::default()
    };
    let (rows, total) = EmissionRepo::query(&pool, filter).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, a.id);
}

#[tokio::test]
async fn query_filters_by_session_id() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session_a = seed_session(&pool).await;
    let session_b = seed_session(&pool).await;

    let a = insert_wifi(&pool, ds, session_a, "aa:aa:aa:aa:aa:aa", 1).await;
    insert_wifi(&pool, ds, session_b, "bb:bb:bb:bb:bb:bb", 1).await;

    let filter = EmissionFilter {
        session_id: Some(session_a),
        ..EmissionFilter::default()
    };
    let (rows, total) = EmissionRepo::query(&pool, filter).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].id, a.id);
}

#[tokio::test]
async fn query_unassigned_returns_only_rows_with_null_emitter_id() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    // No EmitterRepo yet (later sub-task), so both rows are naturally
    // unassigned (emitter_id = NULL) — this test only asserts that
    // `unassigned = false` doesn't spuriously filter anything out, and
    // that `unassigned = true` still returns them (since NULL is the only
    // state reachable without EmitterRepo).
    insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", 1).await;
    insert_wifi(&pool, ds, session, "bb:bb:bb:bb:bb:bb", 1).await;

    let filter = EmissionFilter {
        unassigned: true,
        ..EmissionFilter::default()
    };
    let (rows, total) = EmissionRepo::query(&pool, filter).await.unwrap();
    assert_eq!(total, 2);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.emitter_id.is_none()));
}

#[tokio::test]
async fn query_filters_by_time_range() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let now = Utc::now();
    let old = NewEmission {
        observed_at: now - Duration::hours(2),
        ..NewEmission::wifi(ds, session, wifi_payload("aa:aa:aa:aa:aa:aa", 1))
    };
    let recent = NewEmission {
        observed_at: now,
        ..NewEmission::wifi(ds, session, wifi_payload("bb:bb:bb:bb:bb:bb", 1))
    };
    EmissionRepo::insert(&pool, old).await.unwrap();
    let recent = EmissionRepo::insert(&pool, recent).await.unwrap();

    let filter = EmissionFilter {
        time_from: Some(now - Duration::minutes(30)),
        ..EmissionFilter::default()
    };
    let (rows, total) = EmissionRepo::query(&pool, filter).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].id, recent.id);
}

#[tokio::test]
async fn query_filters_by_bbox() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let mut inside = NewEmission::wifi(ds, session, wifi_payload("aa:aa:aa:aa:aa:aa", 1));
    inside.location = Some((-122.4, 37.7)); // San Francisco-ish
    let inside = EmissionRepo::insert(&pool, inside).await.unwrap();

    let mut outside = NewEmission::wifi(ds, session, wifi_payload("bb:bb:bb:bb:bb:bb", 1));
    outside.location = Some((-74.0, 40.7)); // New York-ish
    EmissionRepo::insert(&pool, outside).await.unwrap();

    let filter = EmissionFilter {
        bbox: Some((-123.0, 37.0, -122.0, 38.0)),
        ..EmissionFilter::default()
    };
    let (rows, total) = EmissionRepo::query(&pool, filter).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].id, inside.id);
}

#[tokio::test]
async fn query_field_conditions_numeric_gte_filters_correctly() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let low = insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", 1).await;
    let high = insert_wifi(&pool, ds, session, "bb:bb:bb:bb:bb:bb", 11).await;

    let filter = EmissionFilter {
        kind: Some("wifi".to_string()),
        field_conditions: vec![Condition {
            field: "channel".to_string(),
            op: Op::Gte,
            value: serde_json::json!(6),
        }],
        match_mode: MatchMode::All,
        ..EmissionFilter::default()
    };
    let (rows, total) = EmissionRepo::query(&pool, filter).await.unwrap();
    assert_eq!(
        total, 1,
        "expected only the channel=11 row to satisfy channel gte 6"
    );
    assert_eq!(rows[0].id, high.id);
    assert_ne!(rows[0].id, low.id);
}

// --- Regression: mixed `field_conditions` bind-order desync (Task 1.3b fix) ---
//
// `EmissionRepo::query` used to re-walk `field_conditions` after translation
// to decide (by op alone) which of `conditions_to_sql_checked`'s returned
// binds were "numeric". That re-walk could desync from the translator's
// actual bind count/order whenever a condition passed the checked variant's
// value-type check but still produced a bindless `FALSE` clause in
// `condition_clause` (e.g. `Gte` on a `Text` field: the value can be a JSON
// string, which is what `Text` expects, but `Gte` still rejects non-numbers
// downstream) -- the re-walk still counted such a condition as consuming one
// bind, so every subsequent bind was applied one `$n` short of where it
// belonged. With 2+ `field_conditions` this could bind the wrong value to
// the wrong placeholder (wrong results, or a Postgres type error). The fix
// makes this impossible in two ways: (1) `conditions_to_sql_checked` now
// rejects an op/field mismatch outright (`RuleSqlError::InvalidOp`) instead
// of letting it reach `condition_clause`'s silent `FALSE` fallback, and (2)
// `EmissionRepo::query` no longer re-derives bind types at all -- it just
// appends every returned bind as text, relying on the translator's own
// `$n::numeric` casts (fluxfang_core::rule_sql) for `Gte`/`Lte`.

#[tokio::test]
async fn query_mixed_field_conditions_filters_correctly_without_bind_desync() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    async fn insert(
        pool: &PgPool,
        ds: Uuid,
        session: Uuid,
        bssid: &str,
        ssid: &str,
        channel: i64,
    ) -> Emission {
        EmissionRepo::insert(
            pool,
            NewEmission::wifi(
                ds,
                session,
                serde_json::json!({"bssid": bssid, "ssid": ssid, "channel": channel}),
            ),
        )
        .await
        .unwrap()
    }

    // Matches both conditions: ssid starts with "Free" AND channel >= 6.
    let both = insert(&pool, ds, session, "aa:aa:aa:aa:aa:aa", "FreeWifi", 11).await;
    // ssid matches, channel doesn't.
    insert(&pool, ds, session, "bb:bb:bb:bb:bb:bb", "FreePublicWifi", 1).await;
    // channel matches, ssid doesn't.
    insert(&pool, ds, session, "cc:cc:cc:cc:cc:cc", "HomeNetwork", 11).await;

    let filter = EmissionFilter {
        kind: Some("wifi".to_string()),
        field_conditions: vec![
            Condition {
                field: "ssid".to_string(),
                op: Op::Matches,
                value: serde_json::json!("^Free"),
            },
            Condition {
                field: "channel".to_string(),
                op: Op::Gte,
                value: serde_json::json!(6),
            },
        ],
        match_mode: MatchMode::All,
        ..EmissionFilter::default()
    };

    let (rows, total) = EmissionRepo::query(&pool, filter).await.unwrap();
    assert_eq!(
        total, 1,
        "only the FreeWifi/channel=11 row should satisfy both conditions"
    );
    assert_eq!(rows[0].id, both.id);
}

#[tokio::test]
async fn query_rejects_mistyped_op_field_condition_instead_of_desyncing_binds() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", 1).await;

    // `ssid` is FieldType::Text, so `Gte` is not a valid op for it (only
    // Number fields support ordering). Before the fix, this combination
    // (mistyped op, followed by a second, well-typed condition) was the
    // shape that produced silent bind desync rather than an error.
    let filter = EmissionFilter {
        kind: Some("wifi".to_string()),
        field_conditions: vec![
            Condition {
                field: "ssid".to_string(),
                op: Op::Gte,
                value: serde_json::json!("z"),
            },
            Condition {
                field: "channel".to_string(),
                op: Op::Eq,
                value: serde_json::json!(1),
            },
        ],
        match_mode: MatchMode::All,
        ..EmissionFilter::default()
    };

    let err = EmissionRepo::query(&pool, filter).await.unwrap_err();
    assert!(
        matches!(err, EmissionQueryError::Rule(_)),
        "expected a clean Rule error, not wrong results or a DB error, got: {err:?}"
    );
}

#[tokio::test]
async fn query_text_substring_matches_payload() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let free = insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", 1).await;
    insert_wifi(&pool, ds, session, "bb:bb:bb:bb:bb:bb", 1).await;

    // Neither payload has an "ssid" field yet in these helpers, so match on
    // the bssid substring instead, which is unique to the first row.
    let filter = EmissionFilter {
        text: Some("aa:aa:aa".to_string()),
        ..EmissionFilter::default()
    };
    let (rows, total) = EmissionRepo::query(&pool, filter).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].id, free.id);
}

#[tokio::test]
async fn query_paginates_with_correct_total() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    for i in 0..5 {
        insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", i).await;
    }

    let filter = EmissionFilter {
        limit: 2,
        offset: 0,
        ..EmissionFilter::default()
    };
    let (page1, total1) = EmissionRepo::query(&pool, filter).await.unwrap();
    assert_eq!(total1, 5);
    assert_eq!(page1.len(), 2);

    let filter2 = EmissionFilter {
        limit: 2,
        offset: 4,
        ..EmissionFilter::default()
    };
    let (page3, total3) = EmissionRepo::query(&pool, filter2).await.unwrap();
    assert_eq!(total3, 5);
    assert_eq!(page3.len(), 1, "last page should have the remaining 1 row");
}

#[tokio::test]
async fn query_rejects_unknown_field_in_conditions() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", 1).await;

    let filter = EmissionFilter {
        field_conditions: vec![Condition {
            field: "not_a_real_field".to_string(),
            op: Op::Eq,
            value: serde_json::json!("x"),
        }],
        ..EmissionFilter::default()
    };
    let err = EmissionRepo::query(&pool, filter).await.unwrap_err();
    assert!(matches!(err, EmissionQueryError::Rule(_)));
}

// ---------------------------------------------------------------------
// Phase A5: `emitter_type`/`emitter_category` filters (join emission ->
// emitter, for the overview map's category-layer heatmaps).
// ---------------------------------------------------------------------

/// `emitter_category=wifi` matches every emission attached to a
/// `wifi_access_point` *or* `wifi_client` emitter (prefix match on
/// `emitter_type`), while excluding both a differently-classified emitter's
/// emission and an entirely unassigned one.
#[tokio::test]
async fn query_filters_by_emitter_category_prefix_matches_all_subtypes() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let ap = seed_classified_emitter(&pool, "AP", "wifi_access_point").await;
    let client = seed_classified_emitter(&pool, "Client", "wifi_client").await;

    let ap_emission = insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", 1).await;
    EmissionRepo::set_emitter(&pool, ap_emission.id, ap)
        .await
        .unwrap();

    let client_emission = insert_wifi(&pool, ds, session, "bb:bb:bb:bb:bb:bb", 1).await;
    EmissionRepo::set_emitter(&pool, client_emission.id, client)
        .await
        .unwrap();

    // Unassigned -- must never appear once emitter_category is set.
    insert_wifi(&pool, ds, session, "cc:cc:cc:cc:cc:cc", 1).await;

    let filter = EmissionFilter {
        emitter_category: Some("wifi".to_string()),
        ..EmissionFilter::default()
    };
    let (rows, total) = EmissionRepo::query(&pool, filter).await.unwrap();
    assert_eq!(total, 2, "rows: {rows:?}");
    let ids: Vec<Uuid> = rows.iter().map(|e| e.id).collect();
    assert!(ids.contains(&ap_emission.id));
    assert!(ids.contains(&client_emission.id));
}

/// `emitter_type=wifi_access_point` is an exact match: only the
/// AP-attached emission is returned, not the client-attached or unassigned
/// ones.
#[tokio::test]
async fn query_filters_by_emitter_type_exact_match_excludes_other_subtypes_and_unassigned() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let ap = seed_classified_emitter(&pool, "AP", "wifi_access_point").await;
    let client = seed_classified_emitter(&pool, "Client", "wifi_client").await;

    let ap_emission = insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", 1).await;
    EmissionRepo::set_emitter(&pool, ap_emission.id, ap)
        .await
        .unwrap();

    let client_emission = insert_wifi(&pool, ds, session, "bb:bb:bb:bb:bb:bb", 1).await;
    EmissionRepo::set_emitter(&pool, client_emission.id, client)
        .await
        .unwrap();

    insert_wifi(&pool, ds, session, "cc:cc:cc:cc:cc:cc", 1).await;

    let filter = EmissionFilter {
        emitter_type: Some("wifi_access_point".to_string()),
        ..EmissionFilter::default()
    };
    let (rows, total) = EmissionRepo::query(&pool, filter).await.unwrap();
    assert_eq!(total, 1, "rows: {rows:?}");
    assert_eq!(rows[0].id, ap_emission.id);
}
