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
use fluxfang_db::models::NewDataSource;
use fluxfang_db::repo::emission::EmissionFilter;
use fluxfang_db::{DataSourceRepo, EmissionRepo, EmitterRepo, LocationRepo, SessionRepo};

mod common;
use common::{
    assert_status, body_json, delete_with_cookie, fresh_pool_shared, get, get_with_cookie,
    patch_json_with_cookie, post_json, post_json_with_cookie, post_with_cookie, session_cookie,
    state_with_factory, test_app_with_factory,
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

/// (a2) Task 6: starting a `bluetooth`/`scan` data source through the same
/// `CaptureSupervisor`/mock-factory path routes its `BuiltCapture::Bluetooth`
/// through `start_wifi` (the generic capturer + inert-gps-session path) —
/// the mock's replayed advertisement flows through `ingest` into an emission
/// *and*, with `auto_create_emitters: true`, auto-creates the
/// `bluetooth_device` emitter Task 3's ingest path builds for it.
#[tokio::test]
async fn bluetooth_data_source_start_flows_mock_emission_through_ingest_then_stop() {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    // Build the factory with one bluetooth advertisement to replay.
    let factory = Arc::new(MockCapturerFactory::with_wifi_observations(vec![
        RawObservation {
            kind: "bluetooth".to_string(),
            observed_at: base,
            signal_strength: Some(-50),
            payload: json!({
                "frame_type": "advertisement",
                "address": "3c:15:c2:aa:bb:cc",
                "address_type": "public",
                "name": "Study Speaker",
                "company_id": 76
            }),
        },
    ]));
    let (app, pool) = test_app_with_factory(factory).await;
    let cookie = login(&app).await;

    // Create: starts out stopped.
    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"bluetooth","mode":"scan","interface":"hci0","config":{"auto_create_emitters":true}}"#,
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
            total == 1 && rows[0].kind == "bluetooth" && rows[0].emitter_id.is_some()
        }
    })
    .await;
    assert!(
        found,
        "expected exactly one bluetooth emission for this data source, attached to an emitter"
    );

    let (rows, _total) = EmissionRepo::query(
        &pool,
        EmissionFilter {
            data_source_id: Some(data_source_id),
            ..Default::default()
        },
    )
    .await
    .expect("query should succeed");
    let emitter_id = rows[0].emitter_id.expect("auto-created + attached");
    let emitter = EmitterRepo::get(&pool, emitter_id)
        .await
        .expect("query should succeed")
        .expect("emitter should exist");
    assert_eq!(
        emitter.name, "BT Client \"Study Speaker\" (3c:15:c2:aa:bb:cc)",
        "auto-created emitter should be named per Task 3's bluetooth naming convention"
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

/// Regression test for the session-leak review finding on Task 6.2:
/// `ensure_wifi_session` used to open (and record in `self.session`) a
/// fresh `survey_session` *before* `capturer.start(tx)` ran, with nothing
/// rolling it back if `start()` then failed (the realistic hardware
/// failure -- bad interface, no monitor mode, permissions). That left
/// `self.session` permanently `Some(...)` with nothing in the running map:
/// the DB's `survey_session` row never closed, and every subsequent gps
/// start was wrongly rejected by `ensure_gps_session` ("session already
/// open"). This test starts a wifi source whose mock capturer is
/// configured to fail `start()`, then proves the session wasn't left
/// stuck: no orphaned open `survey_session`, and a gps source can still be
/// started (and actually writes fixes) right afterward. Fails against the
/// pre-fix code (the gps start below is rejected); passes once
/// `start_wifi` rolls back a session it just opened on a failed
/// `capturer.start`.
#[tokio::test]
async fn failed_wifi_capturer_start_does_not_leak_session_or_block_gps() {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let fixes = vec![GpsFix {
        at: base,
        lon: -122.4,
        lat: 37.7,
        altitude: None,
        speed: None,
        heading: None,
        quality: 1,
    }];
    // One factory serving both: wifi builds always fail `start()`, gps
    // builds replay `fixes`.
    let factory = Arc::new(MockCapturerFactory::with_gps_fixes(fixes).failing_wifi_start());
    let (app, pool) = test_app_with_factory(factory).await;
    let cookie = login(&app).await;

    // Create + start a wifi source. Its capturer's `start()` always
    // errors, so the endpoint (which never surfaces capture errors as an
    // HTTP error -- see `data_sources.rs`'s `start_data_source`) should
    // still return 200 with the row now `error`.
    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"wifi","mode":"monitor","interface":"wlan0","config":{}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let wifi_created = body_json(resp).await;
    let wifi_id = wifi_created["id"].as_str().unwrap().to_string();

    let resp = post_with_cookie(&app, &format!("/api/data-sources/{wifi_id}/start"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let after_failed_start = body_json(resp).await;
    assert_eq!(
        after_failed_start["status"], "error",
        "body: {after_failed_start}"
    );
    assert!(
        after_failed_start["last_error"].as_str().is_some(),
        "body: {after_failed_start}"
    );

    // The failed start must not have left a dangling `survey_session` --
    // there's no running source, so nothing should be open.
    assert!(
        SessionRepo::active(&pool).await.unwrap().is_none(),
        "a failed capturer start must not leave an orphaned open survey_session"
    );

    // Proof this wasn't just luck: a gps source must be able to start
    // right afterward. Under the pre-fix bug, `ensure_gps_session` would
    // reject this ("cannot start a gps source while a session is already
    // open...") because the wifi start's session was never rolled back.
    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"gps","mode":"gpsd","config":{"host":"127.0.0.1","port":2947}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let gps_created = body_json(resp).await;
    let gps_id = gps_created["id"].as_str().unwrap().to_string();

    let resp = post_with_cookie(&app, &format!("/api/data-sources/{gps_id}/start"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let gps_started = body_json(resp).await;
    assert_eq!(
        gps_started["status"], "running",
        "gps start should succeed once the failed wifi start's session is rolled back; \
         body: {gps_started}"
    );

    let opened = wait_until(Duration::from_secs(5), || {
        let pool = pool.clone();
        async move { SessionRepo::active(&pool).await.unwrap().is_some() }
    })
    .await;
    assert!(
        opened,
        "expected a fresh active survey_session for the gps start"
    );

    let session_id = SessionRepo::active(&pool).await.unwrap().unwrap().id;
    let wrote_fixes = wait_until(Duration::from_secs(5), || {
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
        wrote_fixes,
        "expected the gps source's fix to actually be written"
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

/// A wifi data source with the new `scan` mode (managed-mode `iw ... scan`
/// polling, see `fluxfang_capture::wifi::scan`) is accepted at create-time,
/// same as the existing `monitor` mode.
#[tokio::test]
async fn create_wifi_scan_mode_is_accepted_with_201() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"wifi","mode":"scan","interface":"wlan0","config":{}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    assert_eq!(created["mode"], "scan");
    assert_eq!(created["status"], "stopped");
}

/// Phase A5: `config.auto_create_emitters` (the Add-Source "automatically
/// create emitters" toggle) is arbitrary JSON inside `config` — no new
/// column, no special-cased validation — so it must simply survive
/// create -> `GET` unchanged, same as any other `config` key.
#[tokio::test]
async fn create_wifi_with_auto_create_emitters_config_round_trips_through_get() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"wifi","mode":"monitor","interface":"wlan0","config":{"auto_create_emitters":true}}"#,
        &cookie,
    )
    .await;
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    assert_eq!(created["config"]["auto_create_emitters"], true);
    let id = created["id"].as_str().unwrap().to_string();

    let resp = get_with_cookie(&app, &format!("/api/data-sources/{id}"), &cookie).await;
    assert_status(&resp, StatusCode::OK);
    let fetched = body_json(resp).await;
    assert_eq!(
        fetched["config"]["auto_create_emitters"], true,
        "body: {fetched}"
    );
}

/// An unrecognized wifi mode is still rejected with 400, both at
/// create-time and update-time -- confirms widening `monitor` -> `{monitor,
/// scan}` didn't accidentally open validation up to arbitrary strings.
#[tokio::test]
async fn create_wifi_with_bogus_mode_is_rejected_with_400() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let cookie = login(&app).await;

    let resp = post_json_with_cookie(
        &app,
        "/api/data-sources",
        r#"{"kind":"wifi","mode":"bogus","interface":"wlan0","config":{}}"#,
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

/// After a restart, a data source left `status = 'running'` in the DB (whose
/// in-memory capturer did *not* survive the restart) is genuinely resumed by
/// `CaptureSupervisor::resume_running` — not merely shown as running, but
/// actually capturing again: the mock capturer's observation flows through
/// `ingest` into a queryable emission. Regression guard for the field bug
/// where a `docker compose down && up` left sources phantom-"running" but
/// dead.
#[tokio::test]
async fn resume_running_restarts_a_wifi_source_after_restart() {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let bssid = "aa:bb:cc:dd:ee:ff";
    let pool = fresh_pool_shared().await;

    // Model a wifi source that was capturing when the previous process died:
    // present in the DB and marked running, but with no live supervisor.
    let created = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .expect("insert data source");
    DataSourceRepo::set_status(&pool, created.id, "running", None)
        .await
        .expect("mark running");

    // A fresh supervisor (empty in-memory set) on the same DB == a restart.
    let factory = Arc::new(MockCapturerFactory::with_wifi_observations(vec![wifi_obs(
        bssid, base,
    )]));
    let state = state_with_factory(pool.clone(), factory);
    state.capture.resume_running().await;

    // Genuinely resumed: the mock's observation flows through ingest.
    let found = wait_until(Duration::from_secs(5), || {
        let pool = pool.clone();
        async move {
            let (rows, total) = EmissionRepo::query(
                &pool,
                EmissionFilter {
                    data_source_id: Some(created.id),
                    ..Default::default()
                },
            )
            .await
            .expect("query should succeed");
            total >= 1 && rows[0].payload["bssid"] == bssid
        }
    })
    .await;
    assert!(
        found,
        "resume_running should restart capture, not just flip status"
    );

    let row = DataSourceRepo::get(&pool, created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "running");
}

/// `resume_running` resumes GPS before wifi so the shared survey session is
/// opened gps-backed. With both a gps and a wifi source left "running", the
/// gps source must end up `running`, not `error` — the deterministic proof of
/// ordering: `ensure_gps_session` only succeeds (→ `running`) when the gps
/// source opens the session itself, which requires it to run *before* any
/// wifi source opens an `InertGps` session. Resume the other way round and the
/// gps start is rejected, leaving it `error`. `resume_running().await` commits
/// both statuses before returning, so no polling (and no unbounded mock-gps
/// loop) is needed.
#[tokio::test]
async fn resume_running_brings_up_gps_before_wifi() {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let pool = fresh_pool_shared().await;

    let gps_src = NewDataSource {
        config: json!({"host": "127.0.0.1", "port": 2947}),
        ..NewDataSource::gps_gpsd()
    };
    let gps = DataSourceRepo::insert(&pool, gps_src).await.unwrap();
    DataSourceRepo::set_status(&pool, gps.id, "running", None)
        .await
        .unwrap();

    let wifi = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .unwrap();
    DataSourceRepo::set_status(&pool, wifi.id, "running", None)
        .await
        .unwrap();

    let factory = Arc::new(MockCapturerFactory::with_gps_fixes(vec![GpsFix {
        at: base,
        lon: -122.4,
        lat: 37.7,
        altitude: None,
        speed: None,
        heading: None,
        quality: 1,
    }]));
    let state = state_with_factory(pool.clone(), factory);

    state.capture.resume_running().await;

    assert_eq!(
        DataSourceRepo::get(&pool, gps.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "running",
        "gps must be resumed before wifi so it opens the gps-backed session \
         (a wifi-first resume would reject the gps start, leaving it 'error')"
    );
    assert_eq!(
        DataSourceRepo::get(&pool, wifi.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
}

/// A Stop after a restart must take effect even though this fresh supervisor
/// has no in-memory handle for the source: the DB's phantom `running` row is
/// reconciled to `stopped`. This is the exact stuck-"running" symptom from the
/// field (clicking Stop did nothing after a container restart).
#[tokio::test]
async fn stop_reconciles_a_phantom_running_row_after_restart() {
    let pool = fresh_pool_shared().await;
    let created = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .unwrap();
    DataSourceRepo::set_status(&pool, created.id, "running", None)
        .await
        .unwrap();

    // Fresh supervisor: never started this source, so no in-memory handle.
    let state = state_with_factory(pool.clone(), Arc::new(MockCapturerFactory::new()));
    state
        .capture
        .stop(created.id)
        .await
        .expect("stop should succeed");

    let row = DataSourceRepo::get(&pool, created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.status, "stopped",
        "stop must reconcile a phantom-running row after restart"
    );
}

/// The phantom-running reconciliation in `stop` is narrow: a Stop on a source
/// that isn't `running` (e.g. one left in `error`) leaves its status *and*
/// `last_error` untouched, rather than clobbering them to `stopped`/`None`.
#[tokio::test]
async fn stop_leaves_a_non_running_source_untouched() {
    let pool = fresh_pool_shared().await;
    let created = DataSourceRepo::insert(&pool, NewDataSource::wifi_monitor("wlan0"))
        .await
        .unwrap();
    DataSourceRepo::set_status(&pool, created.id, "error", Some("boom"))
        .await
        .unwrap();

    let state = state_with_factory(pool.clone(), Arc::new(MockCapturerFactory::new()));
    state
        .capture
        .stop(created.id)
        .await
        .expect("stop should succeed");

    let row = DataSourceRepo::get(&pool, created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "error");
    assert_eq!(row.last_error.as_deref(), Some("boom"));
}
