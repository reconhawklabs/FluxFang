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
            source: "manual".to_string(),
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
async fn insert_persists_location_quality() {
    let pool = fresh_pool().await;
    let session = seed_session(&pool).await;

    let new = NewEmission {
        data_source_id: None,
        emitter_id: None,
        session_id: Some(session),
        observed_at: Utc::now(),
        signal_strength: None,
        location: None,
        location_quality: "stale".to_string(),
        kind: "wifi".to_string(),
        payload: serde_json::json!({}),
        sensor_id: "local".to_string(),
    };
    let e = EmissionRepo::insert(&pool, new).await.unwrap();
    assert_eq!(e.location_quality, "stale");
    assert!(e.lon.is_none());
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

// ---------------------------------------------------------------------
// Phase 1c: EmissionRepo::{delete_bulk, delete_all}.
// ---------------------------------------------------------------------

#[tokio::test]
async fn delete_bulk_removes_only_the_listed_ids() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let a = insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", 1).await;
    let b = insert_wifi(&pool, ds, session, "bb:bb:bb:bb:bb:bb", 1).await;
    let keep = insert_wifi(&pool, ds, session, "cc:cc:cc:cc:cc:cc", 1).await;

    let deleted = EmissionRepo::delete_bulk(&pool, &[a.id, b.id])
        .await
        .unwrap();
    assert_eq!(deleted, 2);

    assert!(EmissionRepo::get(&pool, a.id).await.unwrap().is_none());
    assert!(EmissionRepo::get(&pool, b.id).await.unwrap().is_none());
    assert!(
        EmissionRepo::get(&pool, keep.id).await.unwrap().is_some(),
        "the emission not in the ids list must survive"
    );
}

#[tokio::test]
async fn delete_bulk_with_empty_ids_deletes_nothing() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let survivor = insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", 1).await;

    let deleted = EmissionRepo::delete_bulk(&pool, &[]).await.unwrap();
    assert_eq!(deleted, 0);
    assert!(
        EmissionRepo::get(&pool, survivor.id)
            .await
            .unwrap()
            .is_some(),
        "an empty ids list must not delete anything"
    );
}

#[tokio::test]
async fn delete_bulk_ignores_unknown_ids() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    let real = insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", 1).await;
    let unknown = Uuid::new_v4();

    let deleted = EmissionRepo::delete_bulk(&pool, &[real.id, unknown])
        .await
        .unwrap();
    assert_eq!(deleted, 1, "only the real id should be counted as deleted");
}

#[tokio::test]
async fn delete_all_empties_the_table() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;
    insert_wifi(&pool, ds, session, "aa:aa:aa:aa:aa:aa", 1).await;
    insert_wifi(&pool, ds, session, "bb:bb:bb:bb:bb:bb", 1).await;
    insert_wifi(&pool, ds, session, "cc:cc:cc:cc:cc:cc", 1).await;

    let deleted = EmissionRepo::delete_all(&pool).await.unwrap();
    assert_eq!(deleted, 3);

    let (rows, total) = EmissionRepo::query(&pool, EmissionFilter::default())
        .await
        .unwrap();
    assert_eq!(total, 0);
    assert!(rows.is_empty());
}

#[tokio::test]
async fn delete_all_on_empty_table_returns_zero() {
    let pool = fresh_pool().await;
    let deleted = EmissionRepo::delete_all(&pool).await.unwrap();
    assert_eq!(deleted, 0);
}

// ---------------------------------------------------------------------
// Task 2: allow-listed `sort`/`dir` (observed_at/rssi) via resolve_order_by.
// ---------------------------------------------------------------------

#[tokio::test]
async fn query_sorts_by_signal_strength_asc_and_desc() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let e_hi = EmissionRepo::insert(
        &pool,
        NewEmission {
            signal_strength: Some(-30),
            ..NewEmission::wifi(ds, session, wifi_payload("aa:aa:aa:aa:aa:aa", 1))
        },
    )
    .await
    .unwrap();
    let e_lo = EmissionRepo::insert(
        &pool,
        NewEmission {
            signal_strength: Some(-70),
            ..NewEmission::wifi(ds, session, wifi_payload("bb:bb:bb:bb:bb:bb", 1))
        },
    )
    .await
    .unwrap();
    let e_mid = EmissionRepo::insert(
        &pool,
        NewEmission {
            signal_strength: Some(-50),
            ..NewEmission::wifi(ds, session, wifi_payload("cc:cc:cc:cc:cc:cc", 1))
        },
    )
    .await
    .unwrap();
    let _ = (&e_hi, &e_lo, &e_mid);

    let asc = EmissionFilter {
        sort: Some("rssi".to_string()),
        dir: Some("asc".to_string()),
        ..Default::default()
    };
    let (rows, _total) = EmissionRepo::query(&pool, asc).await.unwrap();
    let strengths: Vec<Option<i32>> = rows.iter().map(|e| e.signal_strength).collect();
    // Ascending by signal_strength: -70, -50, -30.
    assert_eq!(strengths, vec![Some(-70), Some(-50), Some(-30)]);

    let desc = EmissionFilter {
        sort: Some("rssi".to_string()),
        dir: Some("desc".to_string()),
        ..Default::default()
    };
    let (rows, _total) = EmissionRepo::query(&pool, desc).await.unwrap();
    let strengths: Vec<Option<i32>> = rows.iter().map(|e| e.signal_strength).collect();
    assert_eq!(strengths, vec![Some(-30), Some(-50), Some(-70)]);
}

#[tokio::test]
async fn query_default_sort_is_observed_at_desc() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    let now = Utc::now();
    let t1 = NewEmission {
        observed_at: now - Duration::hours(2),
        ..NewEmission::wifi(ds, session, wifi_payload("aa:aa:aa:aa:aa:aa", 1))
    };
    let t2 = NewEmission {
        observed_at: now,
        ..NewEmission::wifi(ds, session, wifi_payload("bb:bb:bb:bb:bb:bb", 1))
    };
    EmissionRepo::insert(&pool, t1).await.unwrap();
    EmissionRepo::insert(&pool, t2).await.unwrap();

    let (rows, _total) = EmissionRepo::query(&pool, EmissionFilter::default())
        .await
        .unwrap();
    // Newest first (unchanged default).
    assert!(rows[0].observed_at >= rows[rows.len() - 1].observed_at);
}

// ---------------------------------------------------------------------
// Task 5: EmissionRepo::points -- uncapped heatmap points (build_where
// refactor shared with `query`).
// ---------------------------------------------------------------------

/// Insert a wifi emission with a location set (lon/lat), mirroring
/// `insert_with_location_roundtrips_lon_lat`'s pattern.
async fn insert_wifi_located(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    bssid: &str,
    lon: f64,
    lat: f64,
) -> Emission {
    let mut new = NewEmission::wifi(ds, session, wifi_payload(bssid, 1));
    new.location = Some((lon, lat));
    EmissionRepo::insert(pool, new).await.unwrap()
}

#[tokio::test]
async fn points_returns_all_located_emissions_past_the_old_500_cap() {
    let pool = fresh_pool().await;
    let ds = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    for i in 0..600 {
        insert_wifi_located(
            &pool,
            ds,
            session,
            &format!(
                "aa:aa:aa:{:02x}:{:02x}:{:02x}",
                i / 256,
                (i / 16) % 16,
                i % 16
            ),
            -122.4 + (i as f64) * 0.0001,
            37.7,
        )
        .await;
    }
    // A handful of emissions with no location set, which must be excluded.
    for i in 0..5 {
        insert_wifi(&pool, ds, session, &format!("bb:bb:bb:bb:bb:{i:02x}"), 1).await;
    }

    let (points, total) = EmissionRepo::points(&pool, EmissionFilter::default())
        .await
        .unwrap();
    assert_eq!(total, 600, "null-location rows excluded");
    assert_eq!(
        points.len(),
        600,
        "no 500 cap -- all located points returned"
    );
    for p in &points {
        assert!(p[0].is_finite() && p[1].is_finite());
    }
}

#[tokio::test]
async fn points_respects_data_source_filter() {
    let pool = fresh_pool().await;
    let ds_a = seed_wifi_source(&pool).await;
    let ds_b = seed_wifi_source(&pool).await;
    let session = seed_session(&pool).await;

    insert_wifi_located(&pool, ds_a, session, "aa:aa:aa:aa:aa:01", -122.4, 37.7).await;
    insert_wifi_located(&pool, ds_a, session, "aa:aa:aa:aa:aa:02", -122.5, 37.8).await;
    insert_wifi_located(&pool, ds_a, session, "aa:aa:aa:aa:aa:03", -122.6, 37.9).await;
    insert_wifi_located(&pool, ds_b, session, "bb:bb:bb:bb:bb:01", -74.0, 40.7).await;
    insert_wifi_located(&pool, ds_b, session, "bb:bb:bb:bb:bb:02", -74.1, 40.8).await;

    let filter = EmissionFilter {
        data_source_id: Some(ds_a),
        ..EmissionFilter::default()
    };
    let (points, total) = EmissionRepo::points(&pool, filter).await.unwrap();
    assert_eq!(total, 3);
    assert_eq!(points.len(), 3);
}

#[tokio::test]
async fn insert_tags_sensor_id_and_allows_null_session() {
    let pool = common::fresh_pool().await;
    let ds = fluxfang_db::DataSourceRepo::insert(
        &pool,
        fluxfang_db::NewDataSource::wifi_monitor("wlan0"),
    )
    .await
    .unwrap();
    let em = fluxfang_db::EmissionRepo::insert(
        &pool,
        fluxfang_db::models::NewEmission {
            data_source_id: Some(ds.id),
            emitter_id: None,
            session_id: None,
            observed_at: chrono::Utc::now(),
            signal_strength: Some(-40),
            location: Some((2.5, 1.5)),
            location_quality: "fresh".to_string(),
            kind: "wifi".to_string(),
            payload: serde_json::json!({"bssid":"aa:bb:cc:dd:ee:ff"}),
            sensor_id: "frontgate".to_string(),
        },
    )
    .await
    .unwrap();
    assert_eq!(em.sensor_id, "frontgate");
    assert!(em.session_id.is_none());
    assert_eq!(em.lat, Some(1.5));
    assert_eq!(em.lon, Some(2.5));
}
