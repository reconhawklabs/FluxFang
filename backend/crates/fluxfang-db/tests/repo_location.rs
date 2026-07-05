//! Round-trip tests for `LocationRepo`.

mod common;

use chrono::{TimeZone, Utc};
use common::{fresh_pool, seed_session};
use fluxfang_db::models::NewLocationFix;
use fluxfang_db::LocationRepo;

fn new_fix(session_id: uuid::Uuid, seq: i64) -> NewLocationFix {
    NewLocationFix {
        session_id,
        observed_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
            + chrono::Duration::seconds(seq),
        location: (-122.0 + seq as f64 * 0.001, 37.0 + seq as f64 * 0.001),
        altitude: Some(10.0),
        speed: Some(1.5),
        heading: Some(90.0),
        fix_quality: Some("1".to_string()),
    }
}

#[tokio::test]
async fn insert_fix_round_trips_geography_and_fields() {
    let pool = fresh_pool().await;
    let session_id = seed_session(&pool).await;

    let new = new_fix(session_id, 0);
    let fix = LocationRepo::insert_fix(&pool, new.clone()).await.unwrap();

    assert_eq!(fix.session_id, session_id);
    assert_eq!(fix.observed_at, new.observed_at);
    assert_eq!(fix.altitude, Some(10.0));
    assert_eq!(fix.speed, Some(1.5));
    assert_eq!(fix.heading, Some(90.0));
    assert_eq!(fix.fix_quality.as_deref(), Some("1"));
    assert!((fix.lon - new.location.0).abs() < 1e-9);
    assert!((fix.lat - new.location.1).abs() < 1e-9);
}

#[tokio::test]
async fn insert_fix_tolerates_all_optional_fields_null() {
    let pool = fresh_pool().await;
    let session_id = seed_session(&pool).await;

    let new = NewLocationFix {
        session_id,
        observed_at: Utc::now(),
        location: (0.0, 0.0),
        altitude: None,
        speed: None,
        heading: None,
        fix_quality: None,
    };
    let fix = LocationRepo::insert_fix(&pool, new).await.unwrap();
    assert!(fix.altitude.is_none());
    assert!(fix.speed.is_none());
    assert!(fix.heading.is_none());
    assert!(fix.fix_quality.is_none());
}

#[tokio::test]
async fn list_for_session_returns_fixes_oldest_first_scoped_to_session() {
    let pool = fresh_pool().await;
    let session_a = seed_session(&pool).await;
    let session_b = seed_session(&pool).await;

    // Insert out of chronological order to prove ORDER BY observed_at, not
    // insertion order.
    LocationRepo::insert_fix(&pool, new_fix(session_a, 2))
        .await
        .unwrap();
    LocationRepo::insert_fix(&pool, new_fix(session_a, 0))
        .await
        .unwrap();
    LocationRepo::insert_fix(&pool, new_fix(session_a, 1))
        .await
        .unwrap();
    // Different session — must not show up in session_a's list.
    LocationRepo::insert_fix(&pool, new_fix(session_b, 0))
        .await
        .unwrap();

    let rows = LocationRepo::list_for_session(&pool, session_a)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows
        .windows(2)
        .all(|w| w[0].observed_at <= w[1].observed_at));
}

#[tokio::test]
async fn location_fix_cascades_on_session_delete() {
    let pool = fresh_pool().await;
    let session_id = seed_session(&pool).await;
    LocationRepo::insert_fix(&pool, new_fix(session_id, 0))
        .await
        .unwrap();

    sqlx::query("DELETE FROM survey_session WHERE id = $1")
        .bind(session_id)
        .execute(&pool)
        .await
        .unwrap();

    let rows = LocationRepo::list_for_session(&pool, session_id)
        .await
        .unwrap();
    assert!(rows.is_empty());
}
