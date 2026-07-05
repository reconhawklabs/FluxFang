//! Task 10.1: mock-capture end-to-end test.
//!
//! One comprehensive test that boots the full `fluxfang-api` app (real,
//! schema-isolated Postgres + `MockCapturerFactory`) and walks the *entire*
//! backend pipeline purely through the HTTP API: first-run setup -> login ->
//! configure a wifi + gps data source -> start capture -> mock emissions
//! flow through `ingest` -> backfill-assign them to an emitter -> associate
//! an entity -> draw a zone around the emitter's (mock-gps-supplied)
//! location -> wire up alert methods/rules -> a second batch of matching,
//! located emissions fires both a `detected` (with `content_match`) and an
//! `enters_zone` alert exactly the expected number of times -> the
//! resulting `notification` rows are visible via `GET /api/notifications`.
//!
//! ## Why two wifi "rounds" through two data sources
//!
//! The brief's own ordering constraint forces this shape, not a stylistic
//! choice:
//!
//! - The emitter-backfill assertion (`POST /api/emitters` returning a
//!   nonzero `attached_count`) only means something if matching emissions
//!   already exist, *unassigned*, before the emitter does.
//! - The alert-rule assertions only mean something if the rules exist
//!   *before* the emissions that should trigger them are ingested (`ingest`
//!   evaluates alerts/zone-transitions once, at insert time -- it never
//!   retroactively re-evaluates older rows against a rule created later).
//!
//! Those two requirements are contradictory for a single batch, so this
//! test stages two: **round 1** (five wifi emissions, three sharing one
//! target bssid, ingested with no emitter/zone/rule in existence yet) is
//! used for the backfill + entity-association + zone-drawing steps; **round
//! 2** (three more emissions on the same bssid, staged onto the factory via
//! [`MockCapturerFactory::set_wifi_observations`] and replayed through a
//! *second* wifi data source created only after every rule already exists)
//! is what actually fires the alerts. Round 2 still goes through the real
//! start -> `MockCapturer` -> `ingest` path -- it's a second data source,
//! not a direct `ingest()` call -- so the full HTTP-driven chain is
//! exercised for the alerting half too, just like the backfill half.
//!
//! ## How every emission gets a location
//!
//! An emission's `location` comes from the *session's current gps fix*
//! (`ctx.sessions.latest_fix()`), never from the observation itself (see
//! `ingest`'s own doc comment) -- so getting a located wifi emission means a
//! gps source has to be feeding the same shared `survey_session` a wifi
//! source is ingesting into. `CaptureSupervisor::ensure_wifi_session` only
//! reuses an *already-open* session rather than opening its own, so this
//! test starts its gps data source **first** (opening a real-gps-backed
//! session) and only then starts each wifi data source, which reuses it.
//!
//! The gps data source is configured with
//! [`MockCapturerFactory::looping_gps`]: a plain (non-looping) `MockGps`
//! has no artificial pacing between fixes at all (unlike `MockCapturer`,
//! which sleeps a real `interval` between sends), so a finite fix list
//! drains -- and therefore self-closes the shared `survey_session` as
//! "source exhausted" -- within a handful of milliseconds of starting,
//! long before this test's many subsequent HTTP round trips are done with
//! it. Looping keeps exactly one fixed point "fresh" in the session for as
//! long as this test needs, so every wifi emission across both rounds lands
//! at the same location -- which is exactly what lets a single zone,
//! centered on that point, contain every located emission. This is the one
//! app-code change this task's brief anticipated ("fixing wiring gaps
//! found"): see `capture.rs`'s `MockCapturerFactory::looping_gps`/
//! `set_wifi_observations` doc comments for the full rationale.
//!
//! ## Zone-transition "fires once" proof
//!
//! Round 2's three emissions are, in order: (channel=6, matches
//! `content_match`), (channel=11, does not), (channel=6, matches again) --
//! all at the same in-zone location. This exercises both "fires once per
//! transition, not per emission" (the `enters_zone` rule only fires on the
//! *first* round-2 emission; the third, still-inside one-does not re-fire)
//! and content-match selectivity (the `detected` rule fires for the first
//! and third emissions, not the second) in one batch.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::Router;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{json, Value};

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_capture::{GpsFix, RawObservation};
use fluxfang_db::{LocationRepo, SessionRepo};

mod common;
use common::{
    assert_status, body_json, get_with_cookie, patch_json_with_cookie, post_json,
    post_json_with_cookie, post_with_cookie, session_cookie, test_app_with_factory,
};

/// Log in against a fresh app and return its session cookie, running setup
/// first since a fresh instance has no password configured yet -- same
/// pattern every other route-test file in this crate uses.
async fn login(app: &Router) -> String {
    let resp = post_json(app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    let resp = post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    session_cookie(&resp)
}

/// Poll `f` (bounded, so a regression fails loudly instead of hanging the
/// suite) until it resolves `true`, or the timeout elapses. Identical
/// rationale/shape to `tests/data_sources.rs`'s own copy -- duplicated
/// rather than shared, since neither file exposes test-only helpers to the
/// other.
async fn wait_until<F, Fut>(timeout: Duration, mut f: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if f().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Poll `GET /api/emissions` until its `items` array has exactly
/// `expected` rows, returning the final response body. Never a bare sleep:
/// a regression (ingest wiring broken, count wrong) fails loudly via the
/// panic below instead of the suite hanging or silently under-asserting.
async fn wait_for_emissions_total(app: &Router, cookie: &str, expected: usize) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let resp = get_with_cookie(app, "/api/emissions?limit=100", cookie).await;
        assert_status(&resp, StatusCode::OK);
        let body = body_json(resp).await;
        let got = body["items"].as_array().map(Vec::len).unwrap_or(0);
        if got == expected {
            return body;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for {expected} emissions (got {got}); last body: {body}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Poll `GET /api/notifications` until its `items` array has exactly
/// `expected` rows, returning the final response body.
async fn wait_for_notifications_total(app: &Router, cookie: &str, expected: usize) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let resp = get_with_cookie(app, "/api/notifications?limit=100", cookie).await;
        assert_status(&resp, StatusCode::OK);
        let body = body_json(resp).await;
        let got = body["items"].as_array().map(Vec::len).unwrap_or(0);
        if got == expected {
            return body;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for {expected} notifications (got {got}); last body: {body}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn wifi_obs(bssid: &str, channel: i64, observed_at: DateTime<Utc>) -> RawObservation {
    RawObservation {
        kind: "wifi".to_string(),
        observed_at,
        signal_strength: Some(-50),
        payload: json!({"bssid": bssid, "channel": channel}),
    }
}

/// The point every located emission in this test lands at, and the center
/// of the zone drawn around it -- see module docs' "How every emission gets
/// a location" section for why every emission shares one fixed point.
const ZONE_CENTER: (f64, f64) = (-122.4194, 37.7749);
const ZONE_RADIUS_M: f64 = 500.0;

#[tokio::test]
async fn mock_capture_flows_end_to_end_through_the_http_api() {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    const TARGET_BSSID: &str = "aa:bb:cc:dd:ee:01";
    const OTHER_BSSID_1: &str = "aa:bb:cc:dd:ee:02";
    const OTHER_BSSID_2: &str = "aa:bb:cc:dd:ee:03";

    // Round 1: five emissions, three sharing TARGET_BSSID -- ingested with
    // no emitter/entity/zone/alert-rule in existence yet, to set up the
    // backfill assertion below.
    let round1 = vec![
        wifi_obs(OTHER_BSSID_1, 1, base),
        wifi_obs(TARGET_BSSID, 6, base + chrono::Duration::seconds(1)),
        wifi_obs(OTHER_BSSID_2, 1, base + chrono::Duration::seconds(2)),
        wifi_obs(TARGET_BSSID, 6, base + chrono::Duration::seconds(3)),
        wifi_obs(TARGET_BSSID, 6, base + chrono::Duration::seconds(4)),
    ];

    // Round 2: three more TARGET_BSSID emissions, staged onto the factory
    // *after* every alert rule exists (see module docs). channel=6 matches
    // this test's `content_match`; channel=11 deliberately doesn't, proving
    // selectivity. All three land at the same in-zone point: the first
    // fires `enters_zone` (a fresh transition), the third does not (already
    // inside) -- proving "once per transition, not per emission".
    let round2 = vec![
        wifi_obs(TARGET_BSSID, 6, base + chrono::Duration::seconds(10)),
        wifi_obs(TARGET_BSSID, 11, base + chrono::Duration::seconds(11)),
        wifi_obs(TARGET_BSSID, 6, base + chrono::Duration::seconds(12)),
    ];

    // One gps fix, looped forever (see module docs) so the shared session's
    // location never goes stale across this test's many HTTP round trips.
    let fix = GpsFix {
        at: base,
        lon: ZONE_CENTER.0,
        lat: ZONE_CENTER.1,
        altitude: None,
        speed: None,
        heading: None,
        quality: 1,
    };
    let factory = MockCapturerFactory::with_gps_fixes(vec![fix]).looping_gps();
    factory.set_wifi_observations(round1.clone());
    let factory = Arc::new(factory);

    let (app, pool) = test_app_with_factory(factory.clone()).await;
    let cookie = login(&app).await;

    // ---- Step 1 (setup/login): done above. ----

    // ---- Step 2a: start the gps source first, so its session is real-gps
    // backed and any wifi source that starts afterward reuses it (see
    // module docs). ----
    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"gps","mode":"gpsd","config":{"host":"127.0.0.1","port":2947}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let gps_source = body_json(resp).await;
    let gps_id = gps_source["id"].as_str().unwrap().to_string();

    let resp = post_with_cookie(&app, &format!("/api/data-sources/{gps_id}/start"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    assert_eq!(body_json(resp).await["status"], "running");

    let session_opened = wait_until(Duration::from_secs(5), || {
        let pool = pool.clone();
        async move { SessionRepo::active(&pool).await.unwrap().is_some() }
    })
    .await;
    assert!(
        session_opened,
        "expected the gps start to open a survey_session"
    );
    let session_id = SessionRepo::active(&pool).await.unwrap().unwrap().id;

    let fix_written = wait_until(Duration::from_secs(5), || {
        let pool = pool.clone();
        async move {
            !LocationRepo::list_for_session(&pool, session_id)
                .await
                .unwrap()
                .is_empty()
        }
    })
    .await;
    assert!(
        fix_written,
        "expected at least one location_fix row before starting wifi capture"
    );

    // ---- Step 2b: start the wifi source; it reuses the already-open,
    // gps-backed session, so its emissions come out located. ----
    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"wifi","mode":"monitor","interface":"wlan0","config":{}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let wifi1_source = body_json(resp).await;
    let wifi1_id = wifi1_source["id"].as_str().unwrap().to_string();

    let resp = post_with_cookie(
        &app,
        &format!("/api/data-sources/{wifi1_id}/start"),
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    assert_eq!(body_json(resp).await["status"], "running");

    // ---- Step 3: emissions present -- poll (no bare sleep) until all 5
    // round-1 emissions have flowed through ingest. ----
    let round1_body = wait_for_emissions_total(&app, &cookie, round1.len()).await;
    let round1_items = round1_body["items"].as_array().unwrap();
    assert!(
        round1_items
            .iter()
            .all(|e| e["lon"].is_number() && e["lat"].is_number()),
        "every round-1 emission should be located (a gps fix was already present); body: \
         {round1_body}"
    );
    let target_count_in_round1 = round1_items
        .iter()
        .filter(|e| e["payload"]["bssid"] == TARGET_BSSID)
        .count();
    assert_eq!(target_count_in_round1, 3);
    assert!(round1_items.iter().all(|e| e["emitter_id"].is_null()));

    // Stop the round-1 wifi source (tidy state; the gps source keeps
    // running, so the shared session stays open -- see
    // `CaptureSupervisor::stop`'s "last-stop closes" rule).
    let resp = post_with_cookie(&app, &format!("/api/data-sources/{wifi1_id}/stop"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    assert_eq!(body_json(resp).await["status"], "stopped");

    // ---- Step 4: assign to emitter (backfill). ----
    let create_emitter_body = json!({
        "name": "Target AP",
        "match_criteria": {
            "match": "all",
            "conditions": [{"field": "bssid", "op": "eq", "value": TARGET_BSSID}]
        }
    })
    .to_string();
    let resp = post_json_with_cookie(&app, "/api/emitters", &create_emitter_body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let created_emitter = body_json(resp).await;
    assert_eq!(
        created_emitter["attached_count"], 3,
        "backfill should attach exactly the 3 pre-existing TARGET_BSSID emissions; body: \
         {created_emitter}"
    );
    let emitter_id = created_emitter["emitter"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = get_with_cookie(
        &app,
        &format!("/api/emissions?emitter_id={emitter_id}"),
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    let by_emitter = body_json(resp).await;
    assert_eq!(by_emitter["total"], 3, "body: {by_emitter}");

    // ---- Step 5: associate entity. ----
    let resp =
        post_json_with_cookie(&app, "/api/entities", r#"{"name":"Bob's Phone"}"#, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let entity = body_json(resp).await;
    let entity_id = entity["id"].as_str().unwrap().to_string();

    let patch_body = json!({"entity_id": entity_id}).to_string();
    let resp = patch_json_with_cookie(
        &app,
        &format!("/api/emitters/{emitter_id}"),
        &patch_body,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    let patched_emitter = body_json(resp).await;
    assert_eq!(patched_emitter["entity_id"], entity_id);

    let resp = get_with_cookie(&app, &format!("/api/entities/{entity_id}"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let entity_detail = body_json(resp).await;
    assert!(
        entity_detail["emitters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == emitter_id),
        "body: {entity_detail}"
    );
    let expected_last_seen = base + chrono::Duration::seconds(4);
    let last_seen: DateTime<Utc> = entity_detail["last_seen"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(last_seen, expected_last_seen, "body: {entity_detail}");

    // ---- Step 6: zone, centered exactly on the located emissions' point. ----
    let create_zone_body = json!({
        "name": "Home Base",
        "center": {"lon": ZONE_CENTER.0, "lat": ZONE_CENTER.1},
        "radius_m": ZONE_RADIUS_M
    })
    .to_string();
    let resp = post_json_with_cookie(&app, "/api/zones", &create_zone_body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let zone = body_json(resp).await;
    let zone_id = zone["id"].as_str().unwrap().to_string();

    // ---- Step 7: alert method + two entity-targeted rules. ----
    let resp = post_json_with_cookie(
        &app,
        "/api/alert-methods",
        r#"{"name":"In-App","type":"in_app","enabled":true,"config":{}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let method = body_json(resp).await;
    let method_id = method["id"].as_str().unwrap().to_string();

    let detected_rule_body = json!({
        "name": "Bob detected on channel 6",
        "enabled": true,
        "target_type": "entity",
        "target_id": entity_id,
        "trigger": {
            "on": "detected",
            "content_match": {
                "match": "all",
                "conditions": [{"field": "channel", "op": "eq", "value": 6}]
            }
        },
        "method_ids": [method_id]
    })
    .to_string();
    let resp = post_json_with_cookie(&app, "/api/alert-rules", &detected_rule_body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let detected_rule = body_json(resp).await;
    let detected_rule_id = detected_rule["id"].as_str().unwrap().to_string();

    let zone_rule_body = json!({
        "name": "Bob enters Home Base",
        "enabled": true,
        "target_type": "entity",
        "target_id": entity_id,
        "trigger": {"on": "enters_zone", "zone_id": zone_id},
        "method_ids": [method_id]
    })
    .to_string();
    let resp = post_json_with_cookie(&app, "/api/alert-rules", &zone_rule_body, &cookie).await;
    assert_status(&resp, StatusCode::CREATED);
    let zone_rule = body_json(resp).await;
    let zone_rule_id = zone_rule["id"].as_str().unwrap().to_string();

    // ---- Round 2: stage the new observations, then start a *second* wifi
    // data source (reusing the still-open, gps-backed session) so these
    // emissions are ingested through the real start-capture path, only now
    // that both rules already exist. ----
    factory.set_wifi_observations(round2.clone());

    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"wifi","mode":"monitor","interface":"wlan1","config":{}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let wifi2_source = body_json(resp).await;
    let wifi2_id = wifi2_source["id"].as_str().unwrap().to_string();

    let resp = post_with_cookie(
        &app,
        &format!("/api/data-sources/{wifi2_id}/start"),
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    assert_eq!(body_json(resp).await["status"], "running");

    let total_after_round2 = round1.len() + round2.len();
    wait_for_emissions_total(&app, &cookie, total_after_round2).await;

    // ---- Step 8: notifications. ----
    // Expected: the `detected` rule fires for round 2's 1st and 3rd
    // emissions (channel=6) but not the 2nd (channel=11) -- 2 notifications.
    // The `enters_zone` rule fires exactly once, on the 1st (a fresh
    // outside->inside transition); the 3rd, still inside, does not re-fire.
    let notifications_body = wait_for_notifications_total(&app, &cookie, 3).await;
    let items = notifications_body["items"].as_array().unwrap();

    let detected_count = items
        .iter()
        .filter(|n| n["alert_rule_id"] == detected_rule_id)
        .count();
    assert_eq!(
        detected_count, 2,
        "detected rule should fire for the 2 channel=6 round-2 emissions; body: \
         {notifications_body}"
    );

    let zone_count = items
        .iter()
        .filter(|n| n["alert_rule_id"] == zone_rule_id)
        .count();
    assert_eq!(
        zone_count, 1,
        "enters_zone rule should fire exactly once (first entry only, not the second still-\
         inside emission); body: {notifications_body}"
    );

    assert_eq!(
        notifications_body["unread_count"], 3,
        "body: {notifications_body}"
    );
    assert!(
        items.iter().all(|n| n["delivery_status"] == "sent"),
        "every in_app notification should deliver successfully; body: {notifications_body}"
    );

    // ---- Full-chain sanity: the entity/emitter now show up as "in zone",
    // and the entity's last_seen has advanced to round 2's latest emission. ----
    let resp = get_with_cookie(&app, &format!("/api/zones/{zone_id}"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let zone_detail = body_json(resp).await;
    assert!(
        zone_detail["entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == entity_id),
        "body: {zone_detail}"
    );
    assert!(
        zone_detail["emitters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == emitter_id),
        "body: {zone_detail}"
    );

    let resp = get_with_cookie(&app, &format!("/api/entities/{entity_id}"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let final_entity_detail = body_json(resp).await;
    let final_last_seen: DateTime<Utc> = final_entity_detail["last_seen"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        final_last_seen,
        base + chrono::Duration::seconds(12),
        "body: {final_entity_detail}"
    );
    // `recent_detections` is scoped to this entity's own emitter(s), not
    // every emission in the system -- so it's the 3 round-1 + 3 round-2
    // TARGET_BSSID emissions (6), not `total_after_round2` (8), which also
    // includes round 1's two other-bssid, never-attached emissions.
    assert_eq!(
        final_entity_detail["recent_detections"]
            .as_array()
            .unwrap()
            .len(),
        6,
        "body: {final_entity_detail}"
    );

    // ---- Teardown: stop both remaining running sources so the looping gps
    // background task doesn't keep hammering this test's pool after the
    // test function returns (see module docs on `looping_gps`). Stopping
    // wifi2 first, then gps last, is what actually closes the shared
    // session (`CaptureSupervisor::stop`'s "last-stop closes" rule). ----
    let resp = post_with_cookie(&app, &format!("/api/data-sources/{wifi2_id}/stop"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    assert_eq!(body_json(resp).await["status"], "stopped");

    let resp = post_with_cookie(&app, &format!("/api/data-sources/{gps_id}/stop"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    assert_eq!(body_json(resp).await["status"], "stopped");

    assert!(
        SessionRepo::active(&pool).await.unwrap().is_none(),
        "stopping the last running source should close the shared survey_session"
    );
}
