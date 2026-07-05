//! Task 6.2: `data_source` CRUD + start/stop control, driven end-to-end
//! through the HTTP API with a `MockCapturerFactory` injected — no real
//! wifi/gps hardware is touched anywhere in this file.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use chrono::{TimeZone, Utc};
use serde_json::json;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_capture::{GpsFix, RawObservation};
use fluxfang_db::repo::emission::EmissionFilter;
use fluxfang_db::{EmissionRepo, LocationRepo, SessionRepo};

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

fn wifi_obs(bssid: &str, observed_at: chrono::DateTime<Utc>) -> RawObservation {
    RawObservation {
        kind: "wifi".to_string(),
        observed_at,
        signal_strength: Some(-42),
        payload: json!({"bssid": bssid, "channel": 6}),
    }
}

/// Poll `f` (bounded, so a regression fails loudly instead of hanging the
/// suite) until the future it produces resolves `true`, or the timeout
/// elapses -- used because the `MockCapturer`/`MockGps` pipelines run on
/// their own spawned tasks, asynchronously from the HTTP request that
/// triggered `start`.
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

/// (a) Core RED/GREEN flow: create a wifi data source, start it, observe a
/// mock-capturer-emitted observation flow all the way through `ingest` and
/// become queryable via `EmissionRepo::query`, then stop it.
#[tokio::test]
async fn wifi_data_source_start_flows_mock_emission_through_ingest_then_stop() {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let bssid = "aa:bb:cc:dd:ee:ff";
    let factory = Arc::new(MockCapturerFactory::with_wifi_observations(vec![wifi_obs(
        bssid, base,
    )]));
    let (app, pool) = test_app_with_factory(factory).await;
    let cookie = login(&app).await;

    // Create: starts out stopped.
    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"wifi","mode":"monitor","interface":"wlan0","config":{}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    assert_eq!(created["status"], "stopped");
    let id = created["id"].as_str().unwrap().to_string();

    // Start: status flips to running.
    let resp = post_with_cookie(&app, &format!("/api/data-sources/{id}/start"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let started = body_json(resp).await;
    assert_eq!(started["status"], "running", "body: {started}");

    // The MockCapturer emits asynchronously (a spawned task, a few ms
    // apart) -- poll until the resulting emission lands and is queryable.
    let data_source_id: uuid::Uuid = id.parse().unwrap();
    let found = wait_until(Duration::from_secs(5), || {
        let pool = pool.clone();
        async move {
            let (rows, total) = EmissionRepo::query(
                &pool,
                EmissionFilter {
                    data_source_id: Some(data_source_id),
                    ..Default::default()
                },
            )
            .await
            .expect("query should succeed");
            total == 1 && rows[0].payload["bssid"] == bssid
        }
    })
    .await;
    assert!(
        found,
        "expected exactly one emission for this data source with the mock's bssid"
    );

    // Stop: status flips back to stopped.
    let resp = post_with_cookie(&app, &format!("/api/data-sources/{id}/stop"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let stopped = body_json(resp).await;
    assert_eq!(stopped["status"], "stopped", "body: {stopped}");
}

/// (b) Starting a gps data source opens a `survey_session` and writes
/// `location_fix` rows from the mock's fixes; stopping it closes the
/// session.
#[tokio::test]
async fn gps_data_source_start_opens_session_and_writes_fixes_then_stop_closes_it() {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let fixes = vec![
        GpsFix {
            at: base,
            lon: -122.4,
            lat: 37.7,
            altitude: None,
            speed: None,
            heading: None,
            quality: 1,
        },
        GpsFix {
            at: base + chrono::Duration::seconds(1),
            lon: -122.41,
            lat: 37.71,
            altitude: None,
            speed: None,
            heading: None,
            quality: 1,
        },
    ];
    let factory = Arc::new(MockCapturerFactory::with_gps_fixes(fixes));
    let (app, pool) = test_app_with_factory(factory).await;
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

    // A survey_session should now be open.
    let opened = wait_until(Duration::from_secs(5), || {
        let pool = pool.clone();
        async move { SessionRepo::active(&pool).await.unwrap().is_some() }
    })
    .await;
    assert!(opened, "expected an active survey_session after start");

    let session_id = SessionRepo::active(&pool).await.unwrap().unwrap().id;

    // The mock's fixes (a finite, non-looping track) should drain into
    // location_fix rows.
    let wrote_fixes = wait_until(Duration::from_secs(5), || {
        let pool = pool.clone();
        async move {
            LocationRepo::list_for_session(&pool, session_id)
                .await
                .unwrap()
                .len()
                >= 2
        }
    })
    .await;
    assert!(wrote_fixes, "expected at least 2 location_fix rows");

    let resp = post_with_cookie(&app, &format!("/api/data-sources/{id}/stop"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let stopped = body_json(resp).await;
    assert_eq!(stopped["status"], "stopped", "body: {stopped}");

    assert!(
        SessionRepo::active(&pool).await.unwrap().is_none(),
        "session should be closed after the last (and only) source stops"
    );
}

/// (c) Config validation: an invalid serial baud rate is rejected at
/// create-time with 400, without ever reaching the database's own CHECK
/// constraint.
#[tokio::test]
async fn create_gps_serial_with_invalid_baud_is_rejected_with_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"gps","mode":"serial","config":{"device":"/dev/ttyUSB0","baud":1200}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// (c) A gpsd source missing `host` is rejected with 400.
#[tokio::test]
async fn create_gps_gpsd_missing_host_is_rejected_with_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"gps","mode":"gpsd","config":{"port":2947}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// (c) The same validation applies to PATCH (update).
#[tokio::test]
async fn update_with_invalid_serial_baud_is_rejected_with_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"gps","mode":"serial","config":{"device":"/dev/ttyUSB0","baud":9600}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let resp = patch_json_with_cookie(
        &app,
        &format!("/api/data-sources/{id}"),
        r#"{"mode":"serial","config":{"device":"/dev/ttyUSB0","baud":31250}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::BAD_REQUEST);
}

/// Basic CRUD roundtrip (create/list/get/patch/delete), all under auth.
#[tokio::test]
async fn crud_roundtrip() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"wifi","mode":"monitor","interface":"wlan0","config":{}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let resp = get_with_cookie(&app, "/api/data-sources", &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let list = body_json(resp).await;
    assert!(list.as_array().unwrap().iter().any(|d| d["id"] == id));

    let resp = get_with_cookie(&app, &format!("/api/data-sources/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::OK);

    let resp = patch_json_with_cookie(
        &app,
        &format!("/api/data-sources/{id}"),
        r#"{"mode":"monitor","interface":"wlan1","config":{"channel":6}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::OK);
    let updated = body_json(resp).await;
    assert_eq!(updated["interface"], "wlan1");
    assert_eq!(updated["config"]["channel"], 6);

    let resp = delete_with_cookie(&app, &format!("/api/data-sources/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NO_CONTENT);

    let resp = get_with_cookie(&app, &format!("/api/data-sources/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NOT_FOUND);
}

/// Deleting a running source stops it first rather than erroring — see
/// `data_sources.rs` module docs for the rationale.
#[tokio::test]
async fn deleting_a_running_source_stops_it_first() {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let factory = Arc::new(MockCapturerFactory::with_wifi_observations(vec![wifi_obs(
        "aa:bb:cc:dd:ee:ff",
        base,
    )]));
    let (app, _pool) = test_app_with_factory(factory).await;
    let cookie = login(&app).await;

    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"wifi","mode":"monitor","interface":"wlan0","config":{}}"#,
        &cookie,
    )
    .await;
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let resp = post_with_cookie(&app, &format!("/api/data-sources/{id}/start"), &cookie).await;
    assert_status(&resp, StatusCode::OK);

    let resp = delete_with_cookie(&app, &format!("/api/data-sources/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::NO_CONTENT);
}

/// (d) Every data-source endpoint is behind auth.
#[tokio::test]
async fn data_source_endpoints_require_auth() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;

    assert_status(
        &get(&app, "/api/data-sources").await,
        StatusCode::UNAUTHORIZED,
    );
    assert_status(
        &get(
            &app,
            "/api/data-sources/00000000-0000-0000-0000-000000000000",
        )
        .await,
        StatusCode::UNAUTHORIZED,
    );
    assert_status(
        &post_json(
            &app,
            "/api/data-sources",
            r#"{"kind":"wifi","mode":"monitor","interface":"wlan0","config":{}}"#,
        )
        .await,
        StatusCode::UNAUTHORIZED,
    );
    assert_status(
        &post_json(
            &app,
            "/api/data-sources/00000000-0000-0000-0000-000000000000/start",
            "",
        )
        .await,
        StatusCode::UNAUTHORIZED,
    );
    assert_status(
        &post_json(
            &app,
            "/api/data-sources/00000000-0000-0000-0000-000000000000/stop",
            "",
        )
        .await,
        StatusCode::UNAUTHORIZED,
    );
}
