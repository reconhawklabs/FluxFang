mod common;
use fluxfang_api::forwarder::{EnrollResult, ForwardOutcome, ForwarderTarget, SensorForwarder};
use fluxfang_db::models::NewCachedEmission;
use fluxfang_db::{CachedEmissionRepo, DataSourceRepo, EmissionRepo, NewDataSource, SensorRepo};

async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

/// Build the forwarding target the loop would derive from config.
fn target(sensor_id: &str, key: &fluxfang_sensor_proto::Key, port: u16) -> ForwarderTarget {
    ForwarderTarget {
        sensor_id: sensor_id.into(),
        key: *key,
        base_url: format!("http://127.0.0.1:{port}"),
    }
}

#[tokio::test]
async fn forward_once_delivers_cached_emissions_to_an_approved_listener() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;

    // Stand up a Standalone sensor listener + an approved sensor with a known key.
    let ds = DataSourceRepo::insert(
        &pool,
        NewDataSource {
            kind: "sensor".into(),
            mode: "listener".into(),
            interface: None,
            config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port,"enrollment_window_secs":900}),
        },
    )
    .await
    .unwrap();
    let key = fluxfang_sensor_proto::generate_key();
    let key_b64 = fluxfang_sensor_proto::encode_key(&key);
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key);
    let s = SensorRepo::insert_pending(&pool, ds.id, "frontgate", &fp, None)
        .await
        .unwrap();
    SensorRepo::set_key(&pool, s.id, &key_b64, &fp).await.unwrap();
    SensorRepo::set_status(&pool, s.id, "approved", true)
        .await
        .unwrap();
    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;

    // Cache two emissions locally.
    for _ in 0..2 {
        CachedEmissionRepo::insert(
            &pool,
            NewCachedEmission {
                kind: "wifi".into(),
                signal_strength: Some(-40),
                lat: Some(1.5),
                lon: Some(2.5),
                observed_at: chrono::Utc::now(),
                payload: serde_json::json!({"bssid":"aa:bb:cc:dd:ee:ff"}),
                data_source_id: None,
            },
        )
        .await
        .unwrap();
    }

    // Forward.
    let fwd = SensorForwarder::new(pool.clone());
    let outcome = fwd.forward_once(&target("frontgate", &key, port)).await;
    assert!(
        matches!(outcome, ForwardOutcome::Delivered(2)),
        "got {outcome:?}"
    );

    // Cached rows now delivered; emissions landed on the Standalone side.
    assert_eq!(
        CachedEmissionRepo::list_undelivered(&pool, 100)
            .await
            .unwrap()
            .len(),
        0
    );

    mgr.stop(ds.id).await;
}

#[tokio::test]
async fn forward_once_returns_not_approved_for_a_pending_sensor() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;

    // Stand up a Standalone sensor listener + a sensor left PENDING (never approved).
    let ds = DataSourceRepo::insert(
        &pool,
        NewDataSource {
            kind: "sensor".into(),
            mode: "listener".into(),
            interface: None,
            config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port,"enrollment_window_secs":900}),
        },
    )
    .await
    .unwrap();
    let key = fluxfang_sensor_proto::generate_key();
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key);
    SensorRepo::insert_pending(&pool, ds.id, "frontgate", &fp, None)
        .await
        .unwrap();
    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;

    // Cache an emission locally.
    let cached = CachedEmissionRepo::insert(
        &pool,
        NewCachedEmission {
            kind: "wifi".into(),
            signal_strength: Some(-40),
            lat: Some(1.5),
            lon: Some(2.5),
            observed_at: chrono::Utc::now(),
            payload: serde_json::json!({"bssid":"aa:bb:cc:dd:ee:ff"}),
            data_source_id: None,
        },
    )
    .await
    .unwrap();

    let fwd = SensorForwarder::new(pool.clone());
    let outcome = fwd.forward_once(&target("frontgate", &key, port)).await;
    assert!(
        matches!(outcome, ForwardOutcome::NotApproved),
        "got {outcome:?}"
    );

    // Cached rows stay undelivered.
    let undelivered = CachedEmissionRepo::list_undelivered(&pool, 100)
        .await
        .unwrap();
    assert_eq!(undelivered.len(), 1);
    assert_eq!(undelivered[0].id, cached.id);

    // Nothing was ingested on the Standalone side either.
    assert!(EmissionRepo::get(&pool, cached.id).await.unwrap().is_none());

    mgr.stop(ds.id).await;
}

#[tokio::test]
async fn enroll_registers_pending_then_reports_approved_after_operator_approves() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;

    // Stand up a Standalone sensor listener with its enrollment window open.
    let ds = DataSourceRepo::insert(
        &pool,
        NewDataSource {
            kind: "sensor".into(),
            mode: "listener".into(),
            interface: None,
            config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port,"enrollment_window_secs":900}),
        },
    )
    .await
    .unwrap();
    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;
    mgr.open_enrollment_window(ds.id).await;

    // Fresh key on the Sensor side; NO cached emissions.
    let key = fluxfang_sensor_proto::generate_key();
    let key_b64 = fluxfang_sensor_proto::encode_key(&key);
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key);
    let fwd = SensorForwarder::new(pool.clone());
    let tgt = target("frontgate", &key, port);

    // Proactive enrollment: the sensor transmits only {sensor_id, fingerprint}.
    let result = fwd.enroll(&tgt).await;
    assert_eq!(result, EnrollResult::Pending, "first enroll is still pending");

    // A pending row now exists on the Standalone, with NO key (key arrives only
    // at approval) and the fingerprint the sensor claimed.
    let s = SensorRepo::get_by_sensor_id(&pool, ds.id, "frontgate")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.status, "pending");
    assert_eq!(s.key, "", "the key must never be transmitted at enrollment");
    assert_eq!(s.fingerprint, fp, "stored fingerprint must match the sensor's");

    // Operator approves server-side (types the key in → set_key, then approves).
    SensorRepo::set_key(&pool, s.id, &key_b64, &fp).await.unwrap();
    SensorRepo::set_status(&pool, s.id, "approved", true)
        .await
        .unwrap();

    // Re-enroll now reports Approved.
    let result = fwd.enroll(&tgt).await;
    assert_eq!(
        result,
        EnrollResult::Approved,
        "after approval the sensor learns it is approved"
    );

    mgr.stop(ds.id).await;
}
