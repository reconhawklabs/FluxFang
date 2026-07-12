//! HTTP tests for the Co-Travel Detection endpoints.
//!
//! Follows the real in-process harness (`tests/common/mod.rs`) used by
//! `tests/emitters.rs`, not the invented `TestApp`/`reqwest` harness from
//! the task brief's illustrative Step 1 (that harness doesn't exist in this
//! repo).

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{TimeZone, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::models::{NewDataSource, NewEmission, NewEmitter};
use fluxfang_db::{DataSourceRepo, EmissionRepo, EmitterRepo, SessionRepo};

mod common;
use common::{
    assert_status, body_json, delete_with_cookie, get, get_with_cookie, post_json,
    post_with_cookie, session_cookie, test_app_with_factory,
};

/// Log in against a fresh app and return its session cookie, running setup
/// first since a fresh instance has no password configured yet (mirrors
/// `tests/emitters.rs::login`).
async fn login(app: &axum::Router) -> String {
    post_json(app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    let resp = post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    session_cookie(&resp)
}

async fn seed_data_source(pool: &PgPool) -> Uuid {
    DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .expect("seed wifi data_source")
        .id
}

async fn seed_session(pool: &PgPool) -> Uuid {
    SessionRepo::close_active(pool)
        .await
        .expect("self-heal: close any active survey_session");
    SessionRepo::open(pool)
        .await
        .expect("seed survey_session")
        .id
}

/// Seed a `wifi_client`-classified emitter by name, for the co-travel
/// candidate query to aggregate located emissions against.
async fn seed_emitter(pool: &PgPool, name: &str) -> Uuid {
    EmitterRepo::insert(
        pool,
        NewEmitter {
            name: name.to_string(),
            type_: None,
            entity_id: None,
            match_criteria: json!({}),
            emitter_type: Some("wifi_client".to_string()),
            attributes: json!({}),
            match_enabled: true,
            identity_key: Some(format!("wifi_client:{name}")),
        },
    )
    .await
    .expect("seed emitter")
    .id
}

/// Insert one located ("fresh") wifi emission for `emitter_id` at
/// `(lon, lat)`, `offset_minutes` after `base`.
async fn insert_located_emission(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    emitter_id: Uuid,
    lon: f64,
    lat: f64,
    base: chrono::DateTime<Utc>,
    offset_minutes: i64,
) {
    let new = NewEmission {
        observed_at: base + chrono::Duration::minutes(offset_minutes),
        emitter_id: Some(emitter_id),
        location: Some((lon, lat)),
        location_quality: "fresh".to_string(),
        ..NewEmission::wifi(ds, session, json!({"bssid": "aa:bb:cc:dd:ee:ff"}))
    };
    EmissionRepo::insert(pool, new)
        .await
        .expect("insert seed located emission");
}

/// A single emitter with two located sightings ~1.5km / 10min apart clears
/// the default (¼ mi / 30 s) gate and shows up ranked with a score/tier;
/// ignoring it removes it from the ranked page and surfaces it in the
/// Ignored panel instead.
#[tokio::test]
async fn co_travel_ranks_mover_and_ignore_hides_it() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let emitter_id = seed_emitter(&pool, "mover").await;
    insert_located_emission(&pool, ds, session, emitter_id, -84.500, 37.700, base, 0).await;
    insert_located_emission(&pool, ds, session, emitter_id, -84.483, 37.700, base, 10).await;

    // Default gate (¼ mi / 30 s) via explicit params.
    let resp = get_with_cookie(
        &app,
        "/api/co-travel?min_distance_m=402.336&min_time_s=30",
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["total"], 1, "body: {body}");
    assert_eq!(
        body["items"][0]["emitter_id"],
        emitter_id.to_string(),
        "body: {body}"
    );
    assert!(body["items"][0]["score"].is_number(), "body: {body}");
    assert!(body["items"][0]["tier"].is_string(), "body: {body}");

    // Ignore it.
    let ig = post_with_cookie(
        &app,
        &format!("/api/co-travel/ignore/{emitter_id}"),
        &cookie,
    )
    .await;
    assert_status(&ig, StatusCode::NO_CONTENT);

    // Now it's gone from the page...
    let resp2 = get_with_cookie(
        &app,
        "/api/co-travel?min_distance_m=402.336&min_time_s=30",
        &cookie,
    )
    .await;
    assert_status(&resp2, StatusCode::OK);
    let body2 = body_json(resp2).await;
    assert_eq!(body2["total"], 0, "body: {body2}");

    // ...and present in the ignored list.
    let ignored_resp = get_with_cookie(&app, "/api/co-travel/ignored", &cookie).await;
    assert_status(&ignored_resp, StatusCode::OK);
    let ignored = body_json(ignored_resp).await;
    assert_eq!(ignored.as_array().unwrap().len(), 1, "body: {ignored}");
    assert_eq!(ignored[0]["id"], emitter_id.to_string(), "body: {ignored}");
}

/// A non-numeric `min_distance_m` is rejected with `400`.
#[tokio::test]
async fn co_travel_rejects_bad_params() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/co-travel?min_distance_m=notanumber", &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// `GET /api/co-travel` is behind auth like every other protected route.
#[tokio::test]
async fn co_travel_requires_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;

    let resp = get(&app, "/api/co-travel").await;
    assert_status(&resp, StatusCode::UNAUTHORIZED);
}

/// The delete side of ignore/unignore, and its auth guard, since the ranking
/// test above only exercises `POST` (ignore) — `DELETE` should return the
/// `{removed}` count and restore visibility on the ranked page.
#[tokio::test]
async fn co_travel_unignore_restores_visibility() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let emitter_id = seed_emitter(&pool, "mover2").await;
    insert_located_emission(&pool, ds, session, emitter_id, -84.500, 37.700, base, 0).await;
    insert_located_emission(&pool, ds, session, emitter_id, -84.483, 37.700, base, 10).await;

    let ig = post_with_cookie(
        &app,
        &format!("/api/co-travel/ignore/{emitter_id}"),
        &cookie,
    )
    .await;
    assert_status(&ig, StatusCode::NO_CONTENT);

    let unig = delete_with_cookie(
        &app,
        &format!("/api/co-travel/ignore/{emitter_id}"),
        &cookie,
    )
    .await;
    assert_status(&unig, StatusCode::OK);
    let unig_body = body_json(unig).await;
    assert_eq!(unig_body["removed"], 1, "body: {unig_body}");

    let resp = get_with_cookie(
        &app,
        "/api/co-travel?min_distance_m=402.336&min_time_s=30",
        &cookie,
    )
    .await;
    let body = body_json(resp).await;
    assert_eq!(body["total"], 1, "body: {body}");
}
