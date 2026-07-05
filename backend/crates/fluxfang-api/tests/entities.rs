//! Task 6.5: `GET/POST/PATCH/DELETE /api/entities[/:id]` — driven end to end
//! through the HTTP API. Seeds emitters directly via `EmitterRepo::insert`
//! and emissions via `EmissionRepo::insert` against the test app's own
//! isolated pool, same pattern `emitters.rs`'s tests use.

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{TimeZone, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::models::{NewDataSource, NewEmission, NewEmitter, NewEntity};
use fluxfang_db::{DataSourceRepo, EmissionRepo, EmitterRepo, EntityRepo, SessionRepo};

mod common;
use common::{
    assert_status, body_json, delete_with_cookie, get, get_with_cookie, patch_json_with_cookie,
    post_json, post_json_with_cookie, session_cookie, test_app_with_factory,
};

/// Log in against a fresh app and return its session cookie, running setup
/// first since a fresh instance has no password configured yet.
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

async fn seed_emitter(pool: &PgPool, name: &str, entity_id: Option<Uuid>) -> Uuid {
    EmitterRepo::insert(
        pool,
        NewEmitter {
            name: name.to_string(),
            type_: Some("Access Point".to_string()),
            entity_id,
            match_criteria: json!({}),
            ..Default::default()
        },
    )
    .await
    .expect("seed emitter")
    .id
}

#[allow(clippy::too_many_arguments)]
async fn insert_wifi(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    emitter_id: Option<Uuid>,
    bssid: &str,
    observed_at: chrono::DateTime<Utc>,
    location: Option<(f64, f64)>,
) -> Uuid {
    let new = NewEmission {
        emitter_id,
        observed_at,
        location,
        ..NewEmission::wifi(ds, session, json!({"bssid": bssid}))
    };
    EmissionRepo::insert(pool, new)
        .await
        .expect("insert seed emission")
        .id
}

/// (a) `GET /api/entities/:id` for an entity with two emitters returns both
/// emitters, `last_seen` == the max `observed_at` across them, and
/// `recent_detections` includes the located emissions (and excludes the
/// unlocated one).
#[tokio::test]
async fn get_entity_detail_returns_emitters_max_last_seen_and_located_detections() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;

    let entity = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Bob's phone".to_string(),
            notes: Some("seen at the office".to_string()),
        },
    )
    .await
    .expect("seed entity");

    let emitter_a = seed_emitter(&pool, "AP-A", Some(entity.id)).await;
    let emitter_b = seed_emitter(&pool, "AP-B", Some(entity.id)).await;

    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let earlier = base;
    let later = base + chrono::Duration::hours(1);

    insert_wifi(
        &pool,
        ds,
        session,
        Some(emitter_a),
        "aa:aa:aa:aa:aa:aa",
        earlier,
        Some((-122.4, 37.7)),
    )
    .await;
    let latest_id = insert_wifi(
        &pool,
        ds,
        session,
        Some(emitter_b),
        "bb:bb:bb:bb:bb:bb",
        later,
        Some((-122.5, 37.8)),
    )
    .await;
    // An unlocated emission on emitter_b, newer still — must not appear in
    // `recent_detections` (no location), but this scenario also confirms
    // `last_seen` isn't accidentally computed only from located emissions.
    insert_wifi(
        &pool,
        ds,
        session,
        Some(emitter_b),
        "bb:bb:bb:bb:bb:bb",
        later + chrono::Duration::minutes(1),
        None,
    )
    .await;

    let resp = get_with_cookie(&app, &format!("/api/entities/{}", entity.id), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    assert_eq!(body["id"], entity.id.to_string());
    assert_eq!(body["name"], "Bob's phone");

    let emitter_ids: Vec<String> = body["emitters"]
        .as_array()
        .expect("emitters should be an array")
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(emitter_ids.len(), 2, "body: {body}");
    assert!(emitter_ids.contains(&emitter_a.to_string()));
    assert!(emitter_ids.contains(&emitter_b.to_string()));

    let expected_last_seen = (later + chrono::Duration::minutes(1)).timestamp();
    let got_last_seen = chrono::DateTime::parse_from_rfc3339(body["last_seen"].as_str().unwrap())
        .unwrap()
        .timestamp();
    assert_eq!(got_last_seen, expected_last_seen, "body: {body}");

    let detections = body["recent_detections"]
        .as_array()
        .expect("recent_detections should be an array");
    assert_eq!(detections.len(), 2, "body: {body}");
    // Newest-located first.
    assert_eq!(
        detections[0]["emitter_id"].as_str().unwrap(),
        emitter_b.to_string()
    );
    let latest = EmissionRepo::get(&pool, latest_id).await.unwrap().unwrap();
    assert_eq!(detections[0]["lat"], latest.lat.unwrap());
    assert_eq!(detections[0]["lon"], latest.lon.unwrap());
}

/// (b) An entity with no emitters reports empty `emitters`/
/// `recent_detections` and a null `last_seen`.
#[tokio::test]
async fn get_entity_detail_with_no_emitters_is_empty() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let create_body = json!({"name": "Lonely"}).to_string();
    let resp = post_json_with_cookie(&app, "/api/entities", &create_body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let resp = get_with_cookie(&app, &format!("/api/entities/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    assert_eq!(
        body["emitters"].as_array().unwrap().len(),
        0,
        "body: {body}"
    );
    assert_eq!(
        body["recent_detections"].as_array().unwrap().len(),
        0,
        "body: {body}"
    );
    assert!(body["last_seen"].is_null(), "body: {body}");
}

/// (c) `PATCH /api/entities/:id` updates `name`.
#[tokio::test]
async fn patch_entity_updates_name() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let entity = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Old Name".to_string(),
            notes: Some("keep me".to_string()),
        },
    )
    .await
    .expect("seed entity");

    let patch_body = json!({"name": "New Name"}).to_string();
    let resp = patch_json_with_cookie(
        &app,
        &format!("/api/entities/{}", entity.id),
        &patch_body,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["name"], "New Name");
    // Notes untouched since the request didn't mention it.
    assert_eq!(body["notes"], "keep me");

    let reloaded = EntityRepo::get(&pool, entity.id).await.unwrap().unwrap();
    assert_eq!(reloaded.name, "New Name");
}

/// (d) `DELETE /api/entities/:id` removes the entity; an emitter previously
/// associated to it has its `entity_id` set to `null` (schema `ON DELETE SET
/// NULL`), not removed itself.
#[tokio::test]
async fn delete_entity_removes_row_and_nulls_emitter_entity_id() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let entity = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Doomed".to_string(),
            notes: None,
        },
    )
    .await
    .expect("seed entity");
    let emitter_id = seed_emitter(&pool, "Survivor AP", Some(entity.id)).await;

    let resp = delete_with_cookie(&app, &format!("/api/entities/{}", entity.id), &cookie).await;
    assert_status(&resp, StatusCode::NO_CONTENT);

    let reloaded = EntityRepo::get(&pool, entity.id).await.unwrap();
    assert!(reloaded.is_none(), "entity should be gone");

    let emitter = EmitterRepo::get(&pool, emitter_id)
        .await
        .unwrap()
        .expect("emitter should survive its entity's deletion");
    assert!(
        emitter.entity_id.is_none(),
        "emitter.entity_id should be nulled, got {:?}",
        emitter.entity_id
    );

    // Repeat delete (or an id that never existed) is 404.
    let resp = delete_with_cookie(&app, &format!("/api/entities/{}", entity.id), &cookie).await;
    assert_status(&resp, StatusCode::NOT_FOUND);
}

/// (e) Every entities endpoint is behind auth.
#[tokio::test]
async fn entities_endpoints_require_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;

    assert_status(&get(&app, "/api/entities").await, StatusCode::UNAUTHORIZED);
    let id = Uuid::new_v4();
    assert_status(
        &get(&app, &format!("/api/entities/{id}")).await,
        StatusCode::UNAUTHORIZED,
    );
    assert_status(
        &post_json(&app, "/api/entities", r#"{"name":"x"}"#).await,
        StatusCode::UNAUTHORIZED,
    );
}

/// `POST /api/entities` creates a row that then shows up in the list.
#[tokio::test]
async fn create_entity_then_lists() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({"name": "New Entity", "notes": "n1"}).to_string();
    let resp = post_json_with_cookie(&app, "/api/entities", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    assert_eq!(created["name"], "New Entity");
    assert_eq!(created["notes"], "n1");

    let resp = get_with_cookie(&app, "/api/entities", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let list = body_json(resp).await;
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1, "body: {list}");
    assert_eq!(arr[0]["name"], "New Entity");
    // List rows deliberately omit `last_seen` (see dto::EntityDto docs).
    assert!(arr[0].get("last_seen").is_none(), "body: {list}");
}

/// `GET /api/entities/:id` for an unknown id is `404`.
#[tokio::test]
async fn get_unknown_entity_is_404() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let id = Uuid::new_v4();
    let resp = get_with_cookie(&app, &format!("/api/entities/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NOT_FOUND);
}
