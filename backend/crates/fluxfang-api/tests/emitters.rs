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
use fluxfang_db::models::{NewDataSource, NewEmission, NewEntity};
use fluxfang_db::{DataSourceRepo, EmissionRepo, EntityRepo, SessionRepo};

mod common;
use common::{
    assert_status, body_json, delete_with_cookie, get, get_with_cookie, patch_json_with_cookie,
    post_json, post_json_with_cookie, post_with_cookie, session_cookie, test_app_with_factory,
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

/// `POST /api/emitters` with an `emitter_type` stores it, and the created
/// (and re-fetched) emitter's `type_label`/`category` derive from it via
/// `fluxfang_core::{emitter_type_label, emitter_category}` — the frontend's
/// dropdown-driven create flow this endpoint now supports.
#[tokio::test]
async fn create_emitter_with_emitter_type_derives_label_and_category() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({
        "name": "Bob's AP",
        "type": "Access Point",
        "emitter_type": "wifi_access_point",
        "match_criteria": {"match": "all", "conditions": []}
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/emitters", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let resp_body = body_json(resp).await;
    let emitter_id = resp_body["emitter"]["id"].as_str().unwrap().to_string();
    assert_eq!(resp_body["emitter"]["emitter_type"], "wifi_access_point");
    assert_eq!(resp_body["emitter"]["type_label"], "WiFi Access Point");
    assert_eq!(resp_body["emitter"]["category"], "wifi");

    // Re-fetch via GET to confirm it's persisted, not just echoed back.
    let get_resp = get_with_cookie(&app, &format!("/api/emitters/{emitter_id}"), &cookie).await;
    assert_status(&get_resp, StatusCode::OK);
    let get_body = body_json(get_resp).await;
    assert_eq!(get_body["emitter_type"], "wifi_access_point");
    assert_eq!(get_body["type_label"], "WiFi Access Point");
    assert_eq!(get_body["category"], "wifi");
}

/// An unknown `emitter_type` is rejected with `400` before any row is
/// inserted.
#[tokio::test]
async fn create_emitter_with_invalid_emitter_type_is_bad_request() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({
        "name": "Mystery Device",
        "emitter_type": "bluetooth_beacon",
        "match_criteria": {"match": "all", "conditions": []}
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/emitters", &body, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// Absent `emitter_type` behaves exactly as before: free-text `type` only,
/// `emitter_type` left `NULL`, `type_label` falls back to the free-text
/// `type`.
#[tokio::test]
async fn create_emitter_without_emitter_type_leaves_it_null() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({
        "name": "Plain Emitter",
        "type": "Custom Thing"
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/emitters", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let resp_body = body_json(resp).await;
    assert!(
        resp_body["emitter"]["emitter_type"].is_null(),
        "body: {resp_body}"
    );
    assert_eq!(resp_body["emitter"]["type_label"], "Custom Thing");
    assert!(resp_body["emitter"]["category"].is_null());
}

/// `POST /api/emitters/with-entity` also accepts `emitter_type`, stored the
/// same way.
#[tokio::test]
async fn with_entity_accepts_emitter_type() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({
        "emitter": {
            "name": "Bob's phone AP",
            "type": "Access Point",
            "emitter_type": "wifi_access_point",
            "match_criteria": {"match": "all", "conditions": []}
        },
        "entity": {
            "name": "Bob's phone"
        }
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/emitters/with-entity", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let resp_body = body_json(resp).await;
    assert_eq!(resp_body["emitter"]["emitter_type"], "wifi_access_point");
    assert_eq!(resp_body["emitter"]["type_label"], "WiFi Access Point");
}

/// `POST /api/emitters/with-entity` also rejects an unknown `emitter_type`
/// with `400`.
#[tokio::test]
async fn with_entity_invalid_emitter_type_is_bad_request() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({
        "emitter": {
            "name": "Mystery",
            "emitter_type": "not_a_real_type",
            "match_criteria": {"match": "all", "conditions": []}
        },
        "entity": {
            "name": "Someone"
        }
    })
    .to_string();

    let resp = post_json_with_cookie(&app, "/api/emitters/with-entity", &body, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
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
        list_body["items"].as_array().unwrap().len(),
        0,
        "no emitter should exist, body: {list_body}"
    );
    assert_eq!(list_body["total"], 0, "body: {list_body}");
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

/// Test-only helper for seeding a bare entity directly against the DB
/// (bypassing the API), for the Phase 1b `entity_id` list-filter test.
struct EntityRepoInsertHelper;

impl EntityRepoInsertHelper {
    async fn insert(pool: &PgPool, name: &str) -> Uuid {
        EntityRepo::insert(
            pool,
            NewEntity {
                name: name.to_string(),
                notes: None,
            },
        )
        .await
        .expect("seed entity")
        .id
    }
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

    /// Seed an emitter associated to `entity_id` directly, bypassing the
    /// API — used by the Phase 1b list-search/entity-filter tests.
    async fn insert_with_entity(pool: &PgPool, name: &str, entity_id: Option<Uuid>) -> Uuid {
        use fluxfang_db::models::NewEmitter;
        use fluxfang_db::EmitterRepo;

        EmitterRepo::insert(
            pool,
            NewEmitter {
                name: name.to_string(),
                type_: None,
                entity_id,
                match_criteria: json!({}),
                ..Default::default()
            },
        )
        .await
        .expect("seed emitter with entity")
        .id
    }

    /// Seed an emitter with the Phase A5 classification columns
    /// (`emitter_type`/`attributes`) set directly, bypassing the API, for
    /// tests asserting on `EmitterDto`'s derived `type_label`/`category`.
    async fn insert_classified(
        pool: &PgPool,
        name: &str,
        emitter_type: &str,
        attributes: serde_json::Value,
    ) -> Uuid {
        use fluxfang_db::models::NewEmitter;
        use fluxfang_db::EmitterRepo;

        EmitterRepo::insert(
            pool,
            NewEmitter {
                name: name.to_string(),
                type_: None,
                entity_id: None,
                match_criteria: json!({}),
                emitter_type: Some(emitter_type.to_string()),
                attributes,
                match_enabled: true,
                identity_key: None,
            },
        )
        .await
        .expect("seed classified emitter")
        .id
    }
}

// ---------------------------------------------------------------------
// Phase A5: EmitterDto's classification fields (`emitter_type`,
// `attributes`, `match_enabled`) and its derived `type_label`/`category`.
// ---------------------------------------------------------------------

/// A `wifi_access_point`-classified emitter's `GET` response exposes the raw
/// classification columns plus `type_label`/`category` derived from
/// `fluxfang_core::{emitter_type_label, emitter_category}` — not a stale
/// snapshot, computed fresh from `emitter_type` on every read.
#[tokio::test]
async fn get_emitter_shows_classification_fields_and_derived_labels_for_wifi_ap() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let attrs = json!({"ssid": "Cafe Free WiFi", "bssid": "aa:bb:cc:dd:ee:ff"});
    let id =
        EmitterRepoInsertHelper::insert_classified(&pool, "Cafe AP", "wifi_access_point", attrs)
            .await;

    let resp = get_with_cookie(&app, &format!("/api/emitters/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    assert_eq!(body["emitter_type"], "wifi_access_point", "body: {body}");
    assert_eq!(
        body["attributes"],
        json!({"ssid": "Cafe Free WiFi", "bssid": "aa:bb:cc:dd:ee:ff"})
    );
    assert_eq!(body["match_enabled"], true, "body: {body}");
    assert_eq!(body["type_label"], "WiFi Access Point", "body: {body}");
    assert_eq!(body["category"], "wifi", "body: {body}");
}

/// A plain, unclassified emitter (`emitter_type` NULL) falls back to its
/// stored free-text `type` for `type_label`, and has no `category` at all.
#[tokio::test]
async fn get_emitter_plain_type_falls_back_to_free_text_type_label_with_no_category() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let body = json!({"name": "Hand-made emitter", "type": "Custom Sensor"}).to_string();
    let resp = post_json_with_cookie(&app, "/api/emitters", &body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["emitter"]["id"].as_str().unwrap().to_string();

    let resp = get_with_cookie(&app, &format!("/api/emitters/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let fetched = body_json(resp).await;

    assert!(fetched["emitter_type"].is_null(), "body: {fetched}");
    assert_eq!(fetched["attributes"], json!({}));
    assert_eq!(fetched["match_enabled"], true, "body: {fetched}");
    assert_eq!(fetched["type_label"], "Custom Sensor", "body: {fetched}");
    assert!(fetched["category"].is_null(), "body: {fetched}");
}

// ---------------------------------------------------------------------
// Phase A5: PATCH /api/emitters/:id accepting match_enabled/attributes.
// ---------------------------------------------------------------------

/// `PATCH {match_enabled: false}` disables the emitter's auto-attach rule;
/// a subsequent `GET` reflects it.
#[tokio::test]
async fn patch_match_enabled_false_then_get_shows_false() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let id = EmitterRepoInsertHelper::insert_unassigned(&pool, "Target").await;

    let patch_body = json!({"match_enabled": false}).to_string();
    let resp =
        patch_json_with_cookie(&app, &format!("/api/emitters/{id}"), &patch_body, &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let patched = body_json(resp).await;
    assert_eq!(patched["match_enabled"], false, "body: {patched}");

    let resp = get_with_cookie(&app, &format!("/api/emitters/{id}"), &cookie).await;
    let fetched = body_json(resp).await;
    assert_eq!(fetched["match_enabled"], false, "body: {fetched}");
}

/// `PATCH {attributes: {...}}` is a full replace and round-trips through a
/// subsequent `GET` — the manual-override path (e.g. flipping
/// `randomized_mac`).
#[tokio::test]
async fn patch_attributes_round_trips() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let id = EmitterRepoInsertHelper::insert_classified(
        &pool,
        "Target",
        "wifi_client",
        json!({"src_mac": "aa:bb:cc:dd:ee:ff", "randomized_mac": true}),
    )
    .await;

    let new_attrs = json!({"src_mac": "aa:bb:cc:dd:ee:ff", "randomized_mac": false});
    let patch_body = json!({"attributes": new_attrs}).to_string();
    let resp =
        patch_json_with_cookie(&app, &format!("/api/emitters/{id}"), &patch_body, &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let patched = body_json(resp).await;
    assert_eq!(patched["attributes"], new_attrs, "body: {patched}");

    let resp = get_with_cookie(&app, &format!("/api/emitters/{id}"), &cookie).await;
    let fetched = body_json(resp).await;
    assert_eq!(fetched["attributes"], new_attrs, "body: {fetched}");
}

// ---------------------------------------------------------------------
// Phase 1b: GET /api/emitters response-shape change (bare array -> {items,
// total}) plus search/entity_id/limit/offset query params.
// ---------------------------------------------------------------------

/// `GET /api/emitters` (with no query params) returns `{items, total}`, not
/// a bare array — the response-shape change this phase makes.
#[tokio::test]
async fn list_emitters_returns_items_and_total_shape() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    EmitterRepoInsertHelper::insert_unassigned(&pool, "AP One").await;
    EmitterRepoInsertHelper::insert_unassigned(&pool, "AP Two").await;

    let resp = get_with_cookie(&app, "/api/emitters", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    assert!(body.get("items").is_some(), "body: {body}");
    assert!(body.get("total").is_some(), "body: {body}");
    assert_eq!(body["total"], 2, "body: {body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 2, "body: {body}");
}

/// `search` finds an emitter by name substring, `entity_id` scopes to just
/// that entity's emitters, and `limit`/`offset` page through results —
/// driven end to end through the HTTP query string.
#[tokio::test]
async fn list_emitters_supports_search_entity_id_and_pagination() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let entity = EntityRepoInsertHelper::insert(&pool, "Bob's Devices").await;
    EmitterRepoInsertHelper::insert_with_entity(&pool, "Bob's Cafe AP", Some(entity)).await;
    EmitterRepoInsertHelper::insert_unassigned(&pool, "Unrelated Router").await;
    EmitterRepoInsertHelper::insert_unassigned(&pool, "Another AP").await;

    // search
    let resp = get_with_cookie(&app, "/api/emitters?search=cafe", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["total"], 1, "body: {body}");
    assert_eq!(body["items"][0]["name"], "Bob's Cafe AP");

    // entity_id
    let resp = get_with_cookie(&app, &format!("/api/emitters?entity_id={entity}"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["total"], 1, "body: {body}");
    assert_eq!(body["items"][0]["name"], "Bob's Cafe AP");

    // pagination: 3 emitters total, page size 2
    let resp = get_with_cookie(&app, "/api/emitters?limit=2&offset=0", &cookie).await;
    let body = body_json(resp).await;
    assert_eq!(body["total"], 3, "body: {body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 2, "body: {body}");

    let resp = get_with_cookie(&app, "/api/emitters?limit=2&offset=2", &cookie).await;
    let body = body_json(resp).await;
    assert_eq!(body["total"], 3, "body: {body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 1, "body: {body}");
}

/// `GET /api/emitters?emitter_type=wifi_client` returns only emitters
/// classified as `wifi_client`, excluding a `wifi_access_point` one and an
/// unclassified one — the Emitters page's Type-filter dropdown.
#[tokio::test]
async fn list_emitters_supports_emitter_type_filter() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    EmitterRepoInsertHelper::insert_classified(
        &pool,
        "Some Client",
        "wifi_client",
        json!({"src_mac": "aa:bb:cc:dd:ee:ff"}),
    )
    .await;
    EmitterRepoInsertHelper::insert_classified(
        &pool,
        "Some AP",
        "wifi_access_point",
        json!({"bssid": "11:22:33:44:55:66"}),
    )
    .await;
    EmitterRepoInsertHelper::insert_unassigned(&pool, "Unclassified").await;

    let resp = get_with_cookie(&app, "/api/emitters?emitter_type=wifi_client", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["total"], 1, "body: {body}");
    assert_eq!(body["items"][0]["name"], "Some Client", "body: {body}");
}

// ---------------------------------------------------------------------
// Phase 1c: POST /api/emitters/bulk-delete and POST /api/emitters/clear.
// ---------------------------------------------------------------------

/// `POST /api/emitters/bulk-delete {ids:[a,b]}` deletes exactly those two
/// rows, leaves the third alone, and reports `{deleted: 2}`.
#[tokio::test]
async fn bulk_delete_removes_only_listed_ids_and_reports_count() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let a = EmitterRepoInsertHelper::insert_unassigned(&pool, "A").await;
    let b = EmitterRepoInsertHelper::insert_unassigned(&pool, "B").await;
    EmitterRepoInsertHelper::insert_unassigned(&pool, "Keep").await;

    let body = json!({"ids": [a, b]}).to_string();
    let resp = post_json_with_cookie(&app, "/api/emitters/bulk-delete", &body, &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let resp_body = body_json(resp).await;
    assert_eq!(resp_body["deleted"], 2, "body: {resp_body}");

    let list_resp = get_with_cookie(&app, "/api/emitters", &cookie).await;
    let list = body_json(list_resp).await;
    assert_eq!(list["total"], 1, "body: {list}");
    assert_eq!(list["items"][0]["name"], "Keep", "body: {list}");
}

/// An empty `ids` list deletes nothing and is not an error.
#[tokio::test]
async fn bulk_delete_with_empty_ids_deletes_nothing() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    EmitterRepoInsertHelper::insert_unassigned(&pool, "Survivor").await;

    let body = json!({"ids": []}).to_string();
    let resp = post_json_with_cookie(&app, "/api/emitters/bulk-delete", &body, &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let resp_body = body_json(resp).await;
    assert_eq!(resp_body["deleted"], 0, "body: {resp_body}");

    let list_resp = get_with_cookie(&app, "/api/emitters", &cookie).await;
    let list = body_json(list_resp).await;
    assert_eq!(list["total"], 1, "body: {list}");
}

/// `POST /api/emitters/clear` deletes every emitter and reports the total
/// count.
#[tokio::test]
async fn clear_deletes_all_emitters() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    EmitterRepoInsertHelper::insert_unassigned(&pool, "A").await;
    EmitterRepoInsertHelper::insert_unassigned(&pool, "B").await;

    let resp = post_with_cookie(&app, "/api/emitters/clear", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let resp_body = body_json(resp).await;
    assert_eq!(resp_body["deleted"], 2, "body: {resp_body}");

    let list_resp = get_with_cookie(&app, "/api/emitters", &cookie).await;
    let list = body_json(list_resp).await;
    assert_eq!(list["total"], 0, "body: {list}");
}

/// Both bulk-delete and clear are behind auth.
#[tokio::test]
async fn bulk_delete_and_clear_require_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;

    assert_status(
        &post_json(&app, "/api/emitters/bulk-delete", r#"{"ids":[]}"#).await,
        StatusCode::UNAUTHORIZED,
    );
    assert_status(
        &post_json(&app, "/api/emitters/clear", "").await,
        StatusCode::UNAUTHORIZED,
    );
}
