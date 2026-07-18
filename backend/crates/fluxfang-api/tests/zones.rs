//! Task 6.7: `GET/POST/PATCH/DELETE /api/zones[/:id]` — driven end to end
//! through the HTTP API. Seeds data sources/sessions/emitters/emissions
//! directly via their repos and alert rules via `AlertRuleRepo::insert`
//! against the test app's own isolated pool, same pattern
//! `entities.rs`'/`alert_rules.rs`' tests use.

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::Utc;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::models::{NewAlertRule, NewDataSource, NewEmission, NewEmitter, NewEntity};
use fluxfang_db::{
    AlertRuleRepo, DataSourceRepo, EmissionRepo, EmitterRepo, EntityRepo, SessionRepo,
    ZoneMembershipRepo,
};

mod common;
use common::{
    assert_status, body_json, delete_with_cookie, get, get_with_cookie, patch_json_with_cookie,
    post_json, post_json_with_cookie, session_cookie, test_app_with_factory,
};

/// Zone center: roughly downtown San Francisco.
const CENTER: (f64, f64) = (-122.4194, 37.7749);
/// Same point as `CENTER` — always inside any positive radius around it.
const INSIDE: (f64, f64) = (-122.4194, 37.7749);
/// Roughly Manhattan — thousands of km from `CENTER`, always outside.
const OUTSIDE: (f64, f64) = (-73.9857, 40.7484);
const RADIUS_M: f64 = 1000.0;

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
            type_: None,
            entity_id,
            match_criteria: json!({}),
            ..Default::default()
        },
    )
    .await
    .expect("seed emitter")
    .id
}

async fn insert_located_emission(
    pool: &PgPool,
    ds: Uuid,
    session: Uuid,
    emitter_id: Uuid,
    loc: (f64, f64),
) {
    let new = NewEmission {
        emitter_id: Some(emitter_id),
        observed_at: Utc::now(),
        location: Some(loc),
        ..NewEmission::wifi(ds, session, json!({"bssid": "aa:bb:cc:dd:ee:ff"}))
    };
    EmissionRepo::insert(pool, new)
        .await
        .expect("insert seed emission");
}

async fn create_zone_via_api(
    app: &axum::Router,
    cookie: &str,
    center: (f64, f64),
) -> serde_json::Value {
    let body = json!({
        "name": "Test Zone",
        "center": {"lon": center.0, "lat": center.1},
        "radius_m": RADIUS_M,
        "notes": "a notable place",
    })
    .to_string();
    let resp = post_json_with_cookie(app, "/api/zones", &body, cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    body_json(resp).await
}

/// (a) A zone's `GET /api/zones/:id` reports an emitter whose latest located
/// emission is inside, excludes one whose latest is outside, and reports an
/// entity iff one of its emitters is in.
#[tokio::test]
async fn get_zone_detail_includes_inside_emitter_excludes_outside_and_gates_entity_on_any_emitter()
{
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;
    let ds = seed_data_source(&pool).await;
    let session = seed_session(&pool).await;

    let created = create_zone_via_api(&app, &cookie, CENTER).await;
    let zone_id = created["id"].as_str().unwrap().to_string();

    let entity_in = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "In Entity".to_string(),
            notes: None,
            ..Default::default()
        },
    )
    .await
    .expect("seed entity_in");
    let entity_out = EntityRepo::insert(
        &pool,
        NewEntity {
            name: "Out Entity".to_string(),
            notes: None,
            ..Default::default()
        },
    )
    .await
    .expect("seed entity_out");

    let emitter_in = seed_emitter(&pool, "inside-emitter", Some(entity_in.id)).await;
    let emitter_out = seed_emitter(&pool, "outside-emitter", Some(entity_out.id)).await;
    insert_located_emission(&pool, ds, session, emitter_in, INSIDE).await;
    insert_located_emission(&pool, ds, session, emitter_out, OUTSIDE).await;

    let resp = get_with_cookie(&app, &format!("/api/zones/{zone_id}"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    assert_eq!(body["id"], zone_id);

    let emitter_ids: Vec<String> = body["emitters"]
        .as_array()
        .expect("emitters should be an array")
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        emitter_ids.contains(&emitter_in.to_string()),
        "body: {body}"
    );
    assert!(
        !emitter_ids.contains(&emitter_out.to_string()),
        "body: {body}"
    );

    let entity_ids: Vec<String> = body["entities"]
        .as_array()
        .expect("entities should be an array")
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        entity_ids.contains(&entity_in.id.to_string()),
        "body: {body}"
    );
    assert!(
        !entity_ids.contains(&entity_out.id.to_string()),
        "body: {body}"
    );
}

/// `GET /api/zones/:id` for an unknown id is `404`.
#[tokio::test]
async fn get_unknown_zone_is_404() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let id = Uuid::new_v4();
    let resp = get_with_cookie(&app, &format!("/api/zones/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NOT_FOUND);
}

/// `POST /api/zones` creates a row that then shows up in the list.
#[tokio::test]
async fn create_zone_then_lists() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let created = create_zone_via_api(&app, &cookie, CENTER).await;
    assert_eq!(created["name"], "Test Zone");
    assert_eq!(created["radius_m"], RADIUS_M);
    assert!((created["lon"].as_f64().unwrap() - CENTER.0).abs() < 1e-9);
    assert!((created["lat"].as_f64().unwrap() - CENTER.1).abs() < 1e-9);

    let resp = get_with_cookie(&app, "/api/zones", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let list = body_json(resp).await;
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1, "body: {list}");
    assert_eq!(arr[0]["id"], created["id"]);
}

/// (b) `radius_m` of `0` or negative is `400`.
#[tokio::test]
async fn create_zone_with_non_positive_radius_is_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    for radius in [0.0, -5.0] {
        let body = json!({
            "name": "Bad Radius",
            "center": {"lon": CENTER.0, "lat": CENTER.1},
            "radius_m": radius,
        })
        .to_string();
        let resp = post_json_with_cookie(&app, "/api/zones", &body, &cookie).await;
        assert_status(&resp, StatusCode::BAD_REQUEST);
    }
}

/// (b) An out-of-range `lon`/`lat` is `400`.
#[tokio::test]
async fn create_zone_with_invalid_lon_lat_is_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    for (lon, lat) in [(200.0, 0.0), (0.0, 100.0), (-200.0, 0.0), (0.0, -100.0)] {
        let body = json!({
            "name": "Bad Coords",
            "center": {"lon": lon, "lat": lat},
            "radius_m": RADIUS_M,
        })
        .to_string();
        let resp = post_json_with_cookie(&app, "/api/zones", &body, &cookie).await;
        assert_status(&resp, StatusCode::BAD_REQUEST);
    }
}

/// (b) The coordinate boundaries themselves — `lon = -180`/`180`,
/// `lat = -90`/`90` — are ACCEPTED (`201`), not just rejected just outside
/// them: `validate_zone`'s `RangeInclusive` checks already accept these, this
/// pins that behavior so it can't regress to an exclusive range.
#[tokio::test]
async fn create_zone_at_coordinate_boundaries_is_accepted() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    for (lon, lat) in [
        (-180.0, 0.0),
        (180.0, 0.0),
        (0.0, -90.0),
        (0.0, 90.0),
        (-180.0, -90.0),
        (180.0, 90.0),
    ] {
        let body = json!({
            "name": "Boundary Zone",
            "center": {"lon": lon, "lat": lat},
            "radius_m": RADIUS_M,
        })
        .to_string();
        let resp = post_json_with_cookie(&app, "/api/zones", &body, &cookie).await;
        assert_status(&resp, StatusCode::CREATED);
        let created = body_json(resp).await;
        assert!(
            (created["lon"].as_f64().unwrap() - lon).abs() < 1e-9,
            "lon={lon} lat={lat}: body {created}"
        );
        assert!(
            (created["lat"].as_f64().unwrap() - lat).abs() < 1e-9,
            "lon={lon} lat={lat}: body {created}"
        );
    }
}

/// `PATCH /api/zones/:id` updates name/radius while leaving `center` alone
/// when omitted.
#[tokio::test]
async fn patch_zone_updates_name_and_radius_leaves_center_alone() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let created = create_zone_via_api(&app, &cookie, CENTER).await;
    let id = created["id"].as_str().unwrap().to_string();

    let patch_body = json!({"name": "Renamed", "radius_m": 250.0}).to_string();
    let resp =
        patch_json_with_cookie(&app, &format!("/api/zones/{id}"), &patch_body, &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let updated = body_json(resp).await;
    assert_eq!(updated["name"], "Renamed");
    assert_eq!(updated["radius_m"], 250.0);
    assert!((updated["lon"].as_f64().unwrap() - CENTER.0).abs() < 1e-9);
    assert!((updated["lat"].as_f64().unwrap() - CENTER.1).abs() < 1e-9);
}

/// `PATCH /api/zones/:id` with a bad radius/center is `400` and leaves the
/// zone unchanged.
#[tokio::test]
async fn patch_zone_with_invalid_radius_is_400() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let created = create_zone_via_api(&app, &cookie, CENTER).await;
    let id = created["id"].as_str().unwrap().to_string();

    let patch_body = json!({"radius_m": -1.0}).to_string();
    let resp =
        patch_json_with_cookie(&app, &format!("/api/zones/{id}"), &patch_body, &cookie).await;
    assert_status(&resp, StatusCode::BAD_REQUEST);

    let reloaded = fluxfang_db::ZoneRepo::get(&pool, Uuid::parse_str(&id).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.radius_m, RADIUS_M, "zone should be unchanged");
}

/// (c) Deleting a zone a zone-alert-rule references disables that rule
/// (`enabled = false`), leaves a rule referencing a *different* zone
/// enabled, and removes the zone's `zone_membership` rows.
#[tokio::test]
async fn delete_zone_disables_only_its_own_referencing_alert_rule() {
    let (app, pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let doomed = create_zone_via_api(&app, &cookie, CENTER).await;
    let doomed_id = doomed["id"].as_str().unwrap().to_string();
    let doomed_uuid = Uuid::parse_str(&doomed_id).unwrap();

    let survivor = create_zone_via_api(&app, &cookie, OUTSIDE).await;
    let survivor_id = survivor["id"].as_str().unwrap().to_string();
    let survivor_uuid = Uuid::parse_str(&survivor_id).unwrap();

    // A zone_membership row for the doomed zone, to confirm cascade-delete.
    ZoneMembershipRepo::upsert(&pool, "host", None, doomed_uuid, true, Utc::now())
        .await
        .expect("seed zone_membership");

    let rule_on_doomed = AlertRuleRepo::insert(
        &pool,
        NewAlertRule {
            name: "watches doomed zone".to_string(),
            enabled: true,
            target_type: None,
            target_id: None,
            trigger: json!({"on": "host_enters_zone", "zone_id": doomed_uuid}),
        },
    )
    .await
    .expect("seed rule on doomed zone");
    let rule_on_survivor = AlertRuleRepo::insert(
        &pool,
        NewAlertRule {
            name: "watches surviving zone".to_string(),
            enabled: true,
            target_type: None,
            target_id: None,
            trigger: json!({"on": "host_enters_zone", "zone_id": survivor_uuid}),
        },
    )
    .await
    .expect("seed rule on surviving zone");

    let resp = delete_with_cookie(&app, &format!("/api/zones/{doomed_id}"), &cookie).await;
    assert_status(&resp, StatusCode::NO_CONTENT);

    // Zone is gone.
    let resp = get_with_cookie(&app, &format!("/api/zones/{doomed_id}"), &cookie).await;
    assert_status(&resp, StatusCode::NOT_FOUND);

    // Its referencing rule is now disabled.
    let reloaded_doomed_rule = AlertRuleRepo::get(&pool, rule_on_doomed.id)
        .await
        .unwrap()
        .expect("rule should still exist, just disabled");
    assert!(
        !reloaded_doomed_rule.enabled,
        "rule referencing the deleted zone should be disabled"
    );

    // A rule referencing a different zone is untouched.
    let reloaded_survivor_rule = AlertRuleRepo::get(&pool, rule_on_survivor.id)
        .await
        .unwrap()
        .expect("rule should still exist");
    assert!(
        reloaded_survivor_rule.enabled,
        "rule referencing a surviving zone should remain enabled"
    );

    // zone_membership rows for the deleted zone are gone.
    let membership = ZoneMembershipRepo::get(&pool, "host", None, doomed_uuid)
        .await
        .unwrap();
    assert!(
        membership.is_none(),
        "zone_membership rows for the deleted zone should cascade-delete"
    );

    // Surviving zone is still there.
    let resp = get_with_cookie(&app, &format!("/api/zones/{survivor_id}"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
}

/// `DELETE` on an unknown id (or a repeat delete) is `404`.
#[tokio::test]
async fn delete_zone_then_404_on_repeat() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let created = create_zone_via_api(&app, &cookie, CENTER).await;
    let id = created["id"].as_str().unwrap().to_string();

    let resp = delete_with_cookie(&app, &format!("/api/zones/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NO_CONTENT);

    let resp = delete_with_cookie(&app, &format!("/api/zones/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NOT_FOUND);
}

/// (d) Every zones endpoint is behind auth.
#[tokio::test]
async fn zones_endpoints_require_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;

    assert_status(&get(&app, "/api/zones").await, StatusCode::UNAUTHORIZED);
    let id = Uuid::new_v4();
    assert_status(
        &get(&app, &format!("/api/zones/{id}")).await,
        StatusCode::UNAUTHORIZED,
    );
    assert_status(
        &post_json(&app, "/api/zones", r#"{"name":"x"}"#).await,
        StatusCode::UNAUTHORIZED,
    );
}
