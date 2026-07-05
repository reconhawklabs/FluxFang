//! Task 6.4: `GET/POST/PATCH/DELETE /api/emitters`, `POST
//! /api/emitters/:id/rule`, `POST /api/emitters/with-entity`, `GET
//! /api/emitters/preview` — driven end to end through the HTTP API. Seeds
//! emissions directly via `EmissionRepo::insert` against the test app's own
//! isolated pool, same pattern `emissions.rs`'s tests use.

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{TimeZone, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::models::{NewDataSource, NewEmission};
use fluxfang_db::{DataSourceRepo, EmissionRepo, SessionRepo};

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

async fn insert_wifi(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    bssid: &str,
    ssid: &str,
    channel: i64,
    observed_at: chrono::DateTime<Utc>,
) -> Uuid {
    let new = NewEmission {
        observed_at,
        ..NewEmission::wifi(
            ds,
            session,
            json!({"bssid": bssid, "ssid": ssid, "channel": channel}),
        )
    };
    EmissionRepo::insert(pool, new)
        .await
        .expect("insert seed emission")
        .id
}

/// (a) Creating an emitter with a `bssid eq X` rule, when 2 of 3 seeded
/// emissions match, returns the emitter + `attached_count == 2`, and those 2
/// emissions now carry the new emitter's id.
#[tokio::test]
async fn create_emitter_with_rule_backfills_matching_emissions() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let target_bssid = "aa:aa:aa:aa:aa:aa";
    let match1 = insert_wifi(&pool, ds, session, target_bssid, "Home", 1, base).await;
    let match2 = insert_wifi(
        &pool,
        ds,
        session,
        target_bssid,
        "Home",
        6,
        base + chrono::Duration::seconds(1),
    )
    .await;
    let non_match = insert_wifi(
        &pool,
        ds,
        session,
        "bb:bb:bb:bb:bb:bb",
        "Office",
        1,
        base + chrono::Duration::seconds(2),
    )
    .await;

    let body = json!({
        "name": "Bob's AP",
        "type": "Access Point",
        "match_criteria": {
            "match": "all",
            "conditions": [{"field": "bssid", "op": "eq", "value": target_bssid}]
        }
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/emitters", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let resp_body = body_json(resp).await;

    assert_eq!(resp_body["attached_count"], 2, "body: {resp_body}");
    let emitter_id = resp_body["emitter"]["id"].as_str().unwrap().to_string();
    assert_eq!(resp_body["emitter"]["name"], "Bob's AP");

    let e1 = EmissionRepo::get(&pool, match1).await.unwrap().unwrap();
    let e2 = EmissionRepo::get(&pool, match2).await.unwrap().unwrap();
    let e3 = EmissionRepo::get(&pool, non_match).await.unwrap().unwrap();
    assert_eq!(e1.emitter_id.unwrap().to_string(), emitter_id);
    assert_eq!(e2.emitter_id.unwrap().to_string(), emitter_id);
    assert!(e3.emitter_id.is_none());
}

/// (b) `POST /api/emitters` with `from_emission_id` prefills the default
/// `bssid eq` rule and attaches that emission (and any other sharing the
/// same bssid).
#[tokio::test]
async fn create_emitter_from_emission_id_prefills_default_rule_and_attaches() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let bssid = "cc:cc:cc:cc:cc:cc";
    let seed_id = insert_wifi(&pool, ds, session, bssid, "Cafe", 6, base).await;

    let body = json!({
        "name": "Cafe AP",
        "from_emission_id": seed_id,
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/emitters", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let resp_body = body_json(resp).await;

    assert_eq!(resp_body["attached_count"], 1, "body: {resp_body}");
    assert_eq!(
        resp_body["emitter"]["match_criteria"],
        json!({
            "match": "all",
            "conditions": [{"field": "bssid", "op": "eq", "value": bssid}]
        })
    );

    let seeded = EmissionRepo::get(&pool, seed_id).await.unwrap().unwrap();
    assert_eq!(
        seeded.emitter_id.unwrap().to_string(),
        resp_body["emitter"]["id"].as_str().unwrap()
    );
}

/// (c) `GET /api/emitters/preview?rule=...` returns the match count without
/// assigning anything.
#[tokio::test]
async fn preview_returns_match_count_without_assigning() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let bssid = "dd:dd:dd:dd:dd:dd";
    let e1 = insert_wifi(&pool, ds, session, bssid, "X", 1, base).await;
    let e2 = insert_wifi(
        &pool,
        ds,
        session,
        bssid,
        "X",
        1,
        base + chrono::Duration::seconds(1),
    )
    .await;

    let rule = json!({
        "match": "all",
        "conditions": [{"field": "bssid", "op": "eq", "value": bssid}]
    })
    .to_string();
    let uri = format!("/api/emitters/preview?rule={}", urlencoding_encode(&rule));

    let resp = get_with_cookie(&app, &uri, &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["match_count"], 2, "body: {body}");

    // Nothing was actually assigned.
    let r1 = EmissionRepo::get(&pool, e1).await.unwrap().unwrap();
    let r2 = EmissionRepo::get(&pool, e2).await.unwrap().unwrap();
    assert!(r1.emitter_id.is_none());
    assert!(r2.emitter_id.is_none());
}

/// (d) `POST /api/emitters/with-entity` creates an entity + emitter
/// atomically: both exist afterwards, and `emitter.entity_id == entity.id`.
#[tokio::test]
async fn with_entity_creates_entity_and_emitter_atomically() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let bssid = "ee:ee:ee:ee:ee:ee";
    let matching = insert_wifi(&pool, ds, session, bssid, "Y", 1, base).await;

    let body = json!({
        "emitter": {
            "name": "Bob's phone AP",
            "type": "Access Point",
            "match_criteria": {
                "match": "all",
                "conditions": [{"field": "bssid", "op": "eq", "value": bssid}]
            }
        },
        "entity": {
            "name": "Bob's phone",
            "notes": "seen at the office"
        }
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/emitters/with-entity", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let resp_body = body_json(resp).await;

    assert_eq!(resp_body["attached_count"], 1, "body: {resp_body}");
    let entity_id = resp_body["entity"]["id"].as_str().unwrap().to_string();
    assert_eq!(resp_body["entity"]["name"], "Bob's phone");
    assert_eq!(resp_body["emitter"]["entity_id"], entity_id);

    let updated = EmissionRepo::get(&pool, matching).await.unwrap().unwrap();
    assert_eq!(
        updated.emitter_id.unwrap().to_string(),
        resp_body["emitter"]["id"].as_str().unwrap()
    );
}

/// `with-entity` rolls back the entity insert too when the rule is invalid
/// — no orphaned entity left behind.
#[tokio::test]
async fn with_entity_invalid_rule_rolls_back_and_is_bad_request() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({
        "emitter": {
            "name": "Broken",
            "match_criteria": {
                "match": "all",
                "conditions": [{"field": "not_a_real_field", "op": "eq", "value": "x"}]
            }
        },
        "entity": {"name": "Ghost"}
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/emitters/with-entity", &body, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);

    let list_resp = get_with_cookie(&app, "/api/emitters", &cookie).await;
    let list_body = body_json(list_resp).await;
    assert_eq!(
        list_body.as_array().unwrap().len(),
        0,
        "no emitter should exist"
    );
}

/// (e) `PATCH /api/emitters/:id` with `entity_id: null` detaches.
#[tokio::test]
async fn patch_entity_id_null_detaches() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let with_entity_body = json!({
        "emitter": {"name": "Some AP"},
        "entity": {"name": "Some Entity"}
    })
    .to_string();
    let resp = post_json_with_cookie(
        &app,
        "/api/emitters/with-entity",
        &with_entity_body,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let emitter_id = created["emitter"]["id"].as_str().unwrap().to_string();
    assert!(created["emitter"]["entity_id"].is_string());

    let patch_body = json!({"entity_id": null}).to_string();
    let resp = patch_json_with_cookie(
        &app,
        &format!("/api/emitters/{emitter_id}"),
        &patch_body,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    let patched = body_json(resp).await;
    assert!(patched["entity_id"].is_null(), "body: {patched}");
}

/// PATCH with no `entity_id` key at all leaves the existing association
/// alone (as opposed to accidentally detaching).
#[tokio::test]
async fn patch_without_entity_id_key_leaves_association_untouched() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let with_entity_body = json!({
        "emitter": {"name": "Some AP"},
        "entity": {"name": "Some Entity"}
    })
    .to_string();
    let resp = post_json_with_cookie(
        &app,
        "/api/emitters/with-entity",
        &with_entity_body,
        &cookie,
    )
    .await;
    let created = body_json(resp).await;
    let emitter_id = created["emitter"]["id"].as_str().unwrap().to_string();
    let entity_id = created["emitter"]["entity_id"]
        .as_str()
        .unwrap()
        .to_string();

    let patch_body = json!({"name": "Renamed AP"}).to_string();
    let resp = patch_json_with_cookie(
        &app,
        &format!("/api/emitters/{emitter_id}"),
        &patch_body,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    let patched = body_json(resp).await;
    assert_eq!(patched["name"], "Renamed AP");
    assert_eq!(patched["entity_id"], entity_id);
}

/// (f) An invalid rule on `POST /api/emitters/:id/rule` is a `400`.
#[tokio::test]
async fn set_rule_with_invalid_field_is_bad_request() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let created = EmitterRepoInsertHelper::insert_unassigned(&pool, "Target").await;

    let body = json!({
        "match_criteria": {
            "match": "all",
            "conditions": [{"field": "not_a_field", "op": "eq", "value": "x"}]
        }
    })
    .to_string();

    let resp = post_json_with_cookie(
        &app,
        &format!("/api/emitters/{created}/rule"),
        &body,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// (f) An invalid rule on `GET /api/emitters/preview` is also a `400`.
#[tokio::test]
async fn preview_with_invalid_op_for_field_is_bad_request() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    // `ssid` is text; `gte` needs a number.
    let rule = json!({
        "match": "all",
        "conditions": [{"field": "ssid", "op": "gte", "value": 1}]
    })
    .to_string();
    let uri = format!("/api/emitters/preview?rule={}", urlencoding_encode(&rule));

    let resp = get_with_cookie(&app, &uri, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// (g) Every emitters endpoint is behind auth.
#[tokio::test]
async fn emitters_endpoints_require_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;

    assert_status(&get(&app, "/api/emitters").await, StatusCode::UNAUTHORIZED);
    assert_status(
        &get(&app, "/api/emitters/preview?rule=%7B%7D").await,
        StatusCode::UNAUTHORIZED,
    );
    let id = Uuid::new_v4();
    assert_status(
        &get(&app, &format!("/api/emitters/{id}")).await,
        StatusCode::UNAUTHORIZED,
    );
}

/// `DELETE /api/emitters/:id` removes the row; a repeat delete (or deleting
/// an id that never existed) is `404`.
#[tokio::test]
async fn delete_emitter_removes_row_then_404s() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let id = EmitterRepoInsertHelper::insert_unassigned(&pool, "Doomed").await;

    let resp = delete_with_cookie(&app, &format!("/api/emitters/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NO_CONTENT);

    let resp = get_with_cookie(&app, &format!("/api/emitters/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NOT_FOUND);

    let resp = delete_with_cookie(&app, &format!("/api/emitters/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NOT_FOUND);
}

/// Minimal percent-encoder for the one `rule=` query param these tests send
/// -- avoids pulling in a whole crate just for tests. Encodes everything
/// outside a small safe set, which is always correct (if a bit more
/// aggressive than strictly necessary).
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Test-only helper for seeding a bare, unassigned emitter directly against
/// the DB (bypassing the API) where a test only needs *an* emitter to exist,
/// not to exercise creation itself.
struct EmitterRepoInsertHelper;

impl EmitterRepoInsertHelper {
    async fn insert_unassigned(pool: &PgPool, name: &str) -> Uuid {
        use fluxfang_db::models::NewEmitter;
        use fluxfang_db::EmitterRepo;

        EmitterRepo::insert(
            pool,
            NewEmitter {
                name: name.to_string(),
                type_: None,
                entity_id: None,
                match_criteria: json!({}),
                ..Default::default()
            },
        )
        .await
        .expect("seed emitter")
        .id
    }
}
