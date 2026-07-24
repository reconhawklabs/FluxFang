//! Regressions for the Sensor↔Standalone sync failures: a sensor that
//! forwards continuously but is displayed as offline, an enrollment window
//! that shuts on sensors still queued behind the one just approved, and a
//! stale emitter rule set.

mod common;

use std::sync::Arc;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_db::models::NewEmitter;
use fluxfang_db::{DataSourceRepo, EmitterRepo, NewDataSource, SensorRepo};
use sqlx::PgPool;
use uuid::Uuid;

async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

async fn listener_datasource(pool: &PgPool, port: u16) -> Uuid {
    DataSourceRepo::insert(
        pool,
        NewDataSource {
            kind: "sensor".into(),
            mode: "listener".into(),
            interface: None,
            config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port}),
        },
    )
    .await
    .unwrap()
    .id
}

/// Enroll `sensor_id` as `pending`, with `source_ip` deliberately unset so a
/// later refresh is unambiguous.
async fn pending_sensor(
    pool: &PgPool,
    ds_id: Uuid,
    sensor_id: &str,
) -> (Uuid, fluxfang_sensor_proto::Key) {
    let key = fluxfang_sensor_proto::generate_key();
    let fp = fluxfang_sensor_proto::fingerprint(sensor_id, &key);
    let s = SensorRepo::insert_pending(pool, ds_id, sensor_id, &fp, None)
        .await
        .unwrap();
    (s.id, key)
}

async fn approve(pool: &PgPool, id: Uuid, sensor_id: &str, key: &fluxfang_sensor_proto::Key) {
    let fp = fluxfang_sensor_proto::fingerprint(sensor_id, key);
    SensorRepo::set_key(pool, id, &fluxfang_sensor_proto::encode_key(key), &fp)
        .await
        .unwrap();
    SensorRepo::set_status(pool, id, "approved", true)
        .await
        .unwrap();
}

fn sealed(key: &fluxfang_sensor_proto::Key, sensor_id: &str, bssid: &str) -> Vec<u8> {
    let batch = fluxfang_sensor_proto::SensorBatch {
        sensor_id: sensor_id.into(),
        sent_at_ms: chrono::Utc::now().timestamp_millis(),
        emissions: vec![fluxfang_sensor_proto::WireEmission {
            id: Uuid::new_v4(),
            kind: "wifi".into(),
            signal_strength: Some(-40),
            lat: None,
            lon: None,
            observed_at: chrono::Utc::now(),
            payload: serde_json::json!({"bssid": bssid, "frame_type": "beacon"}),
        }],
    };
    fluxfang_sensor_proto::seal_batch(key, &batch).unwrap()
}

/// How many emissions are currently attached to `emitter_id`.
async fn attached_count(pool: &PgPool, emitter_id: Uuid) -> i64 {
    let row: (i64,) = sqlx::query_as("SELECT count(*) FROM emission WHERE emitter_id = $1")
        .bind(emitter_id)
        .fetch_one(pool)
        .await
        .unwrap();
    row.0
}

async fn login(app: &axum::Router) -> String {
    common::post_json(
        app,
        "/api/setup",
        r#"{"password":"pw123456","role":"standalone","node_sensor_id":"local"}"#,
    )
    .await;
    let resp = common::post_json(app, "/api/login", r#"{"password":"pw123456"}"#).await;
    common::session_cookie(&resp)
}

/// Approve through the real HTTP endpoint — the window-closing decision lives
/// in that handler, so driving the repo directly would test nothing.
async fn approve_via_api(
    app: &axum::Router,
    cookie: &str,
    id: Uuid,
    sensor_id: &str,
    key: &fluxfang_sensor_proto::Key,
) {
    let body = serde_json::json!({
        "auto_group_emitters": false,
        "key": fluxfang_sensor_proto::encode_key(key),
    })
    .to_string();
    let resp =
        common::post_json_with_cookie(app, &format!("/api/sensors/{id}/approve"), &body, cookie)
            .await;
    common::assert_status(&resp, axum::http::StatusCode::OK);
    let _ = sensor_id;
}

async fn post_batch(port: u16, sensor_id: &str, body: Vec<u8>) -> reqwest::StatusCode {
    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/sensor/ingest"))
        .header("X-Sensor-Id", sensor_id)
        .body(body)
        .send()
        .await
        .unwrap()
        .status()
}

/// A forwarding sensor must be recorded as alive, and at its *current*
/// address.
///
/// Both halves were broken and both showed up as "the Standalone says the
/// sensor is down while the sensor says it is connected". `last_seen_at` was
/// stamped after the whole per-emission loop, so a batch slow enough to hit
/// the sensor's HTTP timeout had its handler cancelled before reaching the
/// stamp -- a sensor that never stopped forwarding aged past the 60s online
/// threshold. And `source_ip` was only ever written at enrollment, so the
/// address on the Sensors page was frozen at whatever it was on approval day.
#[tokio::test]
async fn ingesting_a_batch_records_liveness_and_the_sensors_current_address() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
    let ds_id = listener_datasource(&pool, port).await;
    let (id, key) = pending_sensor(&pool, ds_id, "frontgate").await;
    approve(&pool, id, "frontgate", &key).await;

    let before = SensorRepo::get(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        before.source_ip, None,
        "precondition: no address on file yet"
    );

    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds_id).await;
    assert_eq!(
        post_batch(
            port,
            "frontgate",
            sealed(&key, "frontgate", "aa:bb:cc:dd:ee:01")
        )
        .await,
        reqwest::StatusCode::OK,
    );

    let after = SensorRepo::get(&pool, id).await.unwrap().unwrap();
    assert!(after.last_seen_at.is_some(), "batch must record liveness");
    assert_eq!(
        after.source_ip.as_deref(),
        Some("127.0.0.1"),
        "ingest must refresh the sensor's address, not leave the enrollment-time value",
    );

    mgr.stop(ds_id).await;
}

/// A revoked sensor must not be able to stamp itself alive.
///
/// The liveness write moved ahead of the ingest loop, so it is worth pinning
/// that it is still gated on approval: `touch_seen_from` carries the same
/// `status = 'approved'` guard the old call had. Compares against the value
/// from enrollment rather than `None`, because `insert_pending` already
/// stamps `last_seen_at`.
#[tokio::test]
async fn a_revoked_sensor_cannot_stamp_itself_alive() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
    let ds_id = listener_datasource(&pool, port).await;
    let (id, key) = pending_sensor(&pool, ds_id, "frontgate").await;
    approve(&pool, id, "frontgate", &key).await;
    SensorRepo::set_status(&pool, id, "revoked", false)
        .await
        .unwrap();
    let before = SensorRepo::get(&pool, id).await.unwrap().unwrap();

    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds_id).await;
    assert_eq!(
        post_batch(
            port,
            "frontgate",
            sealed(&key, "frontgate", "aa:bb:cc:dd:ee:02")
        )
        .await,
        reqwest::StatusCode::FORBIDDEN,
    );

    let after = SensorRepo::get(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        after.last_seen_at, before.last_seen_at,
        "a revoked sensor must not refresh its liveness",
    );
    assert_eq!(after.source_ip, before.source_ip);
    mgr.stop(ds_id).await;
}

/// Bringing up several sensors at once must not require re-opening the
/// enrollment window between each approval.
///
/// Approving used to close the listener's window unconditionally. The window
/// is per-listener, so the first approval locked out every sibling still
/// waiting, and each of those then got 403 "enrollment window is closed" on
/// every retry until an operator noticed.
#[tokio::test]
async fn approving_one_sensor_leaves_the_window_open_for_others_still_pending() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
    let ds_id = listener_datasource(&pool, port).await;

    let (first_id, first_key) = pending_sensor(&pool, ds_id, "frontgate").await;
    let (second_id, second_key) = pending_sensor(&pool, ds_id, "backgate").await;

    // Build the router from a state we keep a handle on, so the test can read
    // the listener's window state that the approve endpoint manipulates.
    let state = common::state_with_factory(pool.clone(), Arc::new(MockCapturerFactory::new()));
    let listeners = state.sensor_listeners.clone();
    let app = fluxfang_api::app(state);
    let cookie = login(&app).await;
    // Bind the listener before opening a window: `open_enrollment_window`
    // refuses when nothing is actually accepting connections, since a window
    // over a stopped listener is a countdown that can never be used.
    listeners.start(ds_id).await;
    assert!(
        listeners.open_enrollment_window(ds_id).await.is_some(),
        "precondition: the window must open for a running listener",
    );

    approve_via_api(&app, &cookie, first_id, "frontgate", &first_key).await;
    assert!(
        listeners.enrollment_window_remaining(ds_id).await > 0,
        "the window must stay open while another sensor is still pending",
    );

    approve_via_api(&app, &cookie, second_id, "backgate", &second_key).await;
    assert_eq!(
        listeners.enrollment_window_remaining(ds_id).await,
        0,
        "the last approval must still close the window",
    );

    listeners.stop(ds_id).await;
}

/// Editing an emitter's rule must take effect on the very next emission.
///
/// The parsed rule set is now cached in memory across emissions, which is the
/// whole throughput fix. The risk that introduces is serving a stale rule set,
/// so this pins the invalidation: the same in-process index must answer
/// differently before and after an unrelated caller rewrites a rule.
#[tokio::test]
async fn editing_an_emitter_rule_takes_effect_on_the_next_emission() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
    let ds_id = listener_datasource(&pool, port).await;
    let (id, key) = pending_sensor(&pool, ds_id, "frontgate").await;
    approve(&pool, id, "frontgate", &key).await;
    // Auto-grouping ON: with it off, remote emissions are strays and skip
    // matching entirely, so the test would prove nothing. On, an emission
    // that fails to match falls through to auto-create and lands on a
    // *different*, freshly-made emitter -- which is exactly the signal this
    // test reads, since it only ever counts what attached to `emitter`.
    SensorRepo::set_auto_group(&pool, id, true).await.unwrap();

    let emitter = EmitterRepo::insert(
        &pool,
        NewEmitter {
            name: "watched".into(),
            type_: Some("WiFi AP".into()),
            entity_id: None,
            match_criteria: serde_json::json!({
                "match": "all",
                "conditions": [{"field": "bssid", "op": "eq", "value": "aa:bb:cc:dd:ee:10"}]
            }),
            emitter_type: Some("wifi_ap".into()),
            attributes: serde_json::json!({}),
            match_enabled: true,
            identity_key: None,
            source: "manual".into(),
        },
    )
    .await
    .unwrap();

    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds_id).await;

    // Warm the index, and confirm the rule matches as written. Auto-grouping
    // is off so this only succeeds via an explicit rule match.
    post_batch(
        port,
        "frontgate",
        sealed(&key, "frontgate", "aa:bb:cc:dd:ee:10"),
    )
    .await;
    assert_eq!(
        attached_count(&pool, emitter.id).await,
        1,
        "precondition: the rule as written must match",
    );

    // Repoint the rule at a different address. Nothing tells the index
    // directly — it has to notice on its own.
    EmitterRepo::update_rule(
        &pool,
        emitter.id,
        &serde_json::json!({
            "match": "all",
            "conditions": [{"field": "bssid", "op": "eq", "value": "99:99:99:99:99:99"}]
        }),
    )
    .await
    .unwrap();

    post_batch(
        port,
        "frontgate",
        sealed(&key, "frontgate", "aa:bb:cc:dd:ee:10"),
    )
    .await;
    assert_eq!(
        attached_count(&pool, emitter.id).await,
        1,
        "the edited rule no longer matches, so the second emission must not attach — \
         a cached rule set that missed the edit would make this 2",
    );

    mgr.stop(ds_id).await;
}
