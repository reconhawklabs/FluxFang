//! `GET /api/gps/status` (Phase 5): the Dashboard GPS block's + map's data
//! source. Driven end-to-end through the HTTP API with a
//! `MockCapturerFactory` injected, same convention as `tests/data_sources.rs`
//! — no real GPS hardware is touched anywhere in this file.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use chrono::Utc;
use serde_json::json;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_capture::GpsFix;

mod common;
use common::{
    assert_status, body_json, get, get_with_cookie, post_json, post_json_with_cookie,
    post_with_cookie, session_cookie, test_app_with_factory,
};

/// Log in against a fresh app and return its session cookie, running setup
/// first since a fresh instance has no password configured yet. Same
/// pattern as `tests/data_sources.rs`.
async fn login(app: &axum::Router) -> String {
    post_json(app, "/api/setup", r#"{"password":"pw123456"}"#).await;
    let resp = post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    assert_status(&resp, StatusCode::OK);
    session_cookie(&resp)
}

/// Poll `f` (bounded, so a regression fails loudly instead of hanging the
/// suite) until the future it produces resolves `true`, or the timeout
/// elapses -- same helper as `tests/data_sources.rs`, duplicated here since
/// test-support helpers aren't shared across integration-test binaries.
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

/// No gps data source configured at all -> `disabled`, no fix.
#[tokio::test]
async fn no_gps_source_reports_disabled() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = get_with_cookie(&app, "/api/gps/status", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    assert_eq!(body["source_running"], false, "body: {body}");
    assert_eq!(body["has_fix"], false, "body: {body}");
    assert!(body["lat"].is_null(), "body: {body}");
    assert!(body["lon"].is_null(), "body: {body}");
    assert_eq!(body["status"], "disabled", "body: {body}");
}

/// A running gps mock source with a fresh, good-quality fix -> `active`,
/// with lat/lon populated.
#[tokio::test]
async fn running_gps_source_with_fresh_fix_reports_active() {
    // `at` must be near real wall-clock "now" (not a fixed historical
    // date like other tests in this crate use) -- the handler computes
    // `fix_age_seconds` against `Utc::now()` at request time, and this test
    // asserts on that freshness.
    let now = Utc::now();
    let fixes = vec![
        GpsFix {
            at: now,
            lon: -122.4,
            lat: 37.7,
            altitude: None,
            speed: None,
            heading: None,
            quality: 1,
        },
        GpsFix {
            at: now + chrono::Duration::seconds(1),
            lon: -122.41,
            lat: 37.71,
            altitude: None,
            speed: None,
            heading: None,
            quality: 1,
        },
    ];
    // Loop the fixes so `latest_fix()` stays populated for as long as this
    // test needs to poll, instead of the finite track draining and the
    // session self-closing almost immediately (see `MockCapturerFactory::
    // looping_gps`'s doc comment).
    let factory = Arc::new(MockCapturerFactory::with_gps_fixes(fixes).looping_gps());
    let (app, _pool) = test_app_with_factory(factory).await;
    let cookie = login(&app).await;

    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"gps","mode":"gpsd","config":{"host":"127.0.0.1","port":2947}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let resp = post_with_cookie(&app, &format!("/api/data-sources/{id}/start"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let started = body_json(resp).await;
    assert_eq!(started["status"], "running", "body: {started}");

    // The mock's fixes arrive asynchronously (a spawned task) -- poll the
    // status endpoint itself until it reports a fix.
    let has_fix = wait_until(Duration::from_secs(5), || {
        let app = app.clone();
        let cookie = cookie.clone();
        async move {
            let resp = get_with_cookie(&app, "/api/gps/status", &cookie).await;
            let body = body_json(resp).await;
            body["has_fix"] == json!(true)
        }
    })
    .await;
    assert!(has_fix, "expected has_fix to become true");

    let resp = get_with_cookie(&app, "/api/gps/status", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let body = body_json(resp).await;

    assert_eq!(body["source_running"], true, "body: {body}");
    assert_eq!(body["has_fix"], true, "body: {body}");
    assert!(body["lat"].as_f64().is_some(), "body: {body}");
    assert!(body["lon"].as_f64().is_some(), "body: {body}");
    assert_eq!(body["quality"], 1, "body: {body}");
    assert!(body["fix_age_seconds"].as_f64().is_some(), "body: {body}");
    assert_eq!(body["status"], "active", "body: {body}");
}

/// Behind auth like every other protected route.
#[tokio::test]
async fn gps_status_requires_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let resp = get(&app, "/api/gps/status").await;
    assert_status(&resp, StatusCode::UNAUTHORIZED);
}
