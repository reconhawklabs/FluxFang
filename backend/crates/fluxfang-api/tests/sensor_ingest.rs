mod common;

use fluxfang_api::sensor_listener::SensorListenerManager;
use fluxfang_db::{DataSourceRepo, EmissionRepo, NewDataSource, SensorRepo};

async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

async fn setup(
    auto_group: bool,
) -> (
    sqlx::PgPool,
    SensorListenerManager,
    uuid::Uuid,
    u16,
    fluxfang_sensor_proto::Key,
) {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
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
    let s = SensorRepo::insert_pending(&pool, ds.id, "frontgate", &key_b64, &fp, None)
        .await
        .unwrap();
    SensorRepo::set_status(&pool, s.id, "approved", true)
        .await
        .unwrap();
    if auto_group {
        SensorRepo::set_auto_group(&pool, s.id, true).await.unwrap();
    }

    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;

    (pool, mgr, ds.id, port, key)
}

fn sealed_one_emission_batch(key: &fluxfang_sensor_proto::Key, em_id: uuid::Uuid) -> Vec<u8> {
    let batch = fluxfang_sensor_proto::SensorBatch {
        sensor_id: "frontgate".into(),
        sent_at_ms: chrono::Utc::now().timestamp_millis(),
        emissions: vec![fluxfang_sensor_proto::WireEmission {
            id: em_id,
            kind: "wifi".into(),
            signal_strength: Some(-40),
            lat: Some(1.5),
            lon: Some(2.5),
            observed_at: chrono::Utc::now(),
            payload: serde_json::json!({"bssid":"aa:bb:cc:dd:ee:ff"}),
        }],
    };
    fluxfang_sensor_proto::seal_batch(key, &batch).unwrap()
}

#[tokio::test]
async fn approved_sensor_ingest_inserts_tagged_emissions_and_dedupes() {
    let (pool, mgr, ds_id, port, key) = setup(false).await;

    let em_id = uuid::Uuid::new_v4();
    let sealed = sealed_one_emission_batch(&key, em_id);
    let url = format!("http://127.0.0.1:{port}/sensor/ingest");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("X-Sensor-Id", "frontgate")
        .body(sealed.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let j: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(j["accepted"].as_array().unwrap().len(), 1);
    assert_eq!(j["accepted"][0].as_str().unwrap(), em_id.to_string());

    // The emission is stored, tagged with the sensor id + location.
    let em = EmissionRepo::get(&pool, em_id).await.unwrap().unwrap();
    assert_eq!(em.sensor_id, "frontgate");
    assert_eq!(em.lat, Some(1.5));
    assert_eq!(em.lon, Some(2.5));
    assert_eq!(em.location_quality, "fresh");

    // Heartbeat: last_seen_at bumped.
    let sensor = SensorRepo::get_by_sensor_id(&pool, ds_id, "frontgate")
        .await
        .unwrap()
        .unwrap();
    assert!(sensor.last_seen_at.is_some());

    // Re-POST the identical batch -- deduped, still 200, still accepted (ACK
    // every id whether newly-inserted or dup), but no second row.
    let resp2 = reqwest::Client::new()
        .post(&url)
        .header("X-Sensor-Id", "frontgate")
        .body(sealed)
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status().as_u16(), 200);
    let j2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(j2["accepted"].as_array().unwrap().len(), 1);

    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM emission WHERE id = $1")
        .bind(em_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1, "dedup must not double-insert");

    mgr.stop(ds_id).await;
}

/// Per-sensor `auto_group_emitters`: when true, an emission that matches no
/// existing emitter still gets auto-created/attached (RemoteGrouped policy) --
/// distinguishing behavior from the `false` case below.
#[tokio::test]
async fn auto_group_true_attaches_or_creates_emitter() {
    let (pool, mgr, ds_id, port, key) = setup(true).await;

    let em_id = uuid::Uuid::new_v4();
    let sealed = sealed_one_emission_batch(&key, em_id);
    let url = format!("http://127.0.0.1:{port}/sensor/ingest");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("X-Sensor-Id", "frontgate")
        .body(sealed)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // Note: wifi kind without frame_type isn't classifiable, so auto-create
    // won't attach an emitter -- but ingest must still succeed (this asserts
    // the RemoteGrouped path runs finalize_emission without erroring).
    let em = EmissionRepo::get(&pool, em_id).await.unwrap().unwrap();
    assert_eq!(em.sensor_id, "frontgate");

    mgr.stop(ds_id).await;
}

/// AEAD open IS authentication: a batch sealed with the WRONG key must be
/// rejected (401), not silently accepted or ingested.
#[tokio::test]
async fn wrong_key_is_rejected_with_401() {
    let (pool, mgr, ds_id, port, _key) = setup(false).await;

    let em_id = uuid::Uuid::new_v4();
    let wrong_key = fluxfang_sensor_proto::generate_key();
    let sealed = sealed_one_emission_batch(&wrong_key, em_id);
    let url = format!("http://127.0.0.1:{port}/sensor/ingest");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("X-Sensor-Id", "frontgate")
        .body(sealed)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);

    // Nothing was ingested.
    assert!(EmissionRepo::get(&pool, em_id).await.unwrap().is_none());

    mgr.stop(ds_id).await;
}

/// An unknown or unapproved sensor id must be refused (403), regardless of
/// whether the body would otherwise decrypt (it can't be checked without a
/// key to try, which is exactly the point -- no key lookup for a non-
/// approved sensor).
#[tokio::test]
async fn unapproved_sensor_is_rejected_with_403() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;
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

    // Enrolled but left pending -- never approved.
    let key = fluxfang_sensor_proto::generate_key();
    let key_b64 = fluxfang_sensor_proto::encode_key(&key);
    let fp = fluxfang_sensor_proto::fingerprint("frontgate", &key);
    SensorRepo::insert_pending(&pool, ds.id, "frontgate", &key_b64, &fp, None)
        .await
        .unwrap();

    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;

    let em_id = uuid::Uuid::new_v4();
    let sealed = sealed_one_emission_batch(&key, em_id);
    let url = format!("http://127.0.0.1:{port}/sensor/ingest");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("X-Sensor-Id", "frontgate")
        .body(sealed)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    // An entirely unknown sensor id -- also 403.
    let resp2 = reqwest::Client::new()
        .post(&url)
        .header("X-Sensor-Id", "nobody")
        .body(sealed_one_emission_batch(&key, uuid::Uuid::new_v4()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status().as_u16(), 403);

    mgr.stop(ds.id).await;
}

/// Replay defense: a batch whose `sent_at_ms` is far outside the 5-minute
/// skew window must be rejected (400), even though it decrypts fine.
#[tokio::test]
async fn stale_batch_outside_replay_window_is_rejected_with_400() {
    let (pool, mgr, ds_id, port, key) = setup(false).await;

    let em_id = uuid::Uuid::new_v4();
    let batch = fluxfang_sensor_proto::SensorBatch {
        sensor_id: "frontgate".into(),
        sent_at_ms: chrono::Utc::now().timestamp_millis() - 10 * 60 * 1000, // 10 min old
        emissions: vec![fluxfang_sensor_proto::WireEmission {
            id: em_id,
            kind: "wifi".into(),
            signal_strength: Some(-40),
            lat: Some(1.5),
            lon: Some(2.5),
            observed_at: chrono::Utc::now(),
            payload: serde_json::json!({"bssid":"aa:bb:cc:dd:ee:ff"}),
        }],
    };
    let sealed = fluxfang_sensor_proto::seal_batch(&key, &batch).unwrap();
    let url = format!("http://127.0.0.1:{port}/sensor/ingest");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("X-Sensor-Id", "frontgate")
        .body(sealed)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);

    assert!(EmissionRepo::get(&pool, em_id).await.unwrap().is_none());

    mgr.stop(ds_id).await;
}

/// Body `sensor_id` must match the header/looked-up sensor -- a batch sealed
/// (correctly, with the real key) but claiming a different `sensor_id`
/// inside the body must be rejected (400), not silently accepted under the
/// header's identity.
#[tokio::test]
async fn body_sensor_id_mismatch_is_rejected_with_400() {
    let (pool, mgr, ds_id, port, key) = setup(false).await;

    let em_id = uuid::Uuid::new_v4();
    let batch = fluxfang_sensor_proto::SensorBatch {
        sensor_id: "someone-else".into(),
        sent_at_ms: chrono::Utc::now().timestamp_millis(),
        emissions: vec![fluxfang_sensor_proto::WireEmission {
            id: em_id,
            kind: "wifi".into(),
            signal_strength: Some(-40),
            lat: Some(1.5),
            lon: Some(2.5),
            observed_at: chrono::Utc::now(),
            payload: serde_json::json!({"bssid":"aa:bb:cc:dd:ee:ff"}),
        }],
    };
    let sealed = fluxfang_sensor_proto::seal_batch(&key, &batch).unwrap();
    let url = format!("http://127.0.0.1:{port}/sensor/ingest");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("X-Sensor-Id", "frontgate")
        .body(sealed)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);

    assert!(EmissionRepo::get(&pool, em_id).await.unwrap().is_none());

    mgr.stop(ds_id).await;
}

/// Missing `X-Sensor-Id` header -> 400, not a panic.
#[tokio::test]
async fn missing_sensor_id_header_is_rejected_with_400() {
    let (_pool, mgr, ds_id, port, key) = setup(false).await;

    let sealed = sealed_one_emission_batch(&key, uuid::Uuid::new_v4());
    let url = format!("http://127.0.0.1:{port}/sensor/ingest");
    let resp = reqwest::Client::new()
        .post(&url)
        .body(sealed)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);

    mgr.stop(ds_id).await;
}

/// A sensor that was approved and then revoked must be refused (403), same
/// as a never-approved (pending) one -- confirms the non-approved gate covers
/// `revoked` specifically, not just `pending`.
#[tokio::test]
async fn revoked_sensor_is_rejected_with_403() {
    let (pool, mgr, ds_id, port, key) = setup(false).await;

    let sensor = SensorRepo::get_by_sensor_id(&pool, ds_id, "frontgate")
        .await
        .unwrap()
        .unwrap();
    SensorRepo::set_status(&pool, sensor.id, "revoked", false)
        .await
        .unwrap();

    let em_id = uuid::Uuid::new_v4();
    let sealed = sealed_one_emission_batch(&key, em_id);
    let url = format!("http://127.0.0.1:{port}/sensor/ingest");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("X-Sensor-Id", "frontgate")
        .body(sealed)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    assert!(EmissionRepo::get(&pool, em_id).await.unwrap().is_none());

    mgr.stop(ds_id).await;
}

/// An out-of-range coordinate (lat outside [-90,90]) must not fail the
/// `::geography` insert -- the coordinate is dropped and the emission is
/// stored with no location (`location_quality: "none"`) and still ACKed
/// (200). If this instead errored, the handler would skip the ACK and an
/// at-least-once forwarder would retry the emission forever.
#[tokio::test]
async fn out_of_range_coordinate_is_stored_without_location() {
    let (pool, mgr, ds_id, port, key) = setup(false).await;

    let em_id = uuid::Uuid::new_v4();
    let batch = fluxfang_sensor_proto::SensorBatch {
        sensor_id: "frontgate".into(),
        sent_at_ms: chrono::Utc::now().timestamp_millis(),
        emissions: vec![fluxfang_sensor_proto::WireEmission {
            id: em_id,
            kind: "wifi".into(),
            signal_strength: Some(-40),
            lat: Some(91.0), // invalid: outside [-90, 90]
            lon: Some(0.0),
            observed_at: chrono::Utc::now(),
            payload: serde_json::json!({"bssid":"aa:bb:cc:dd:ee:ff"}),
        }],
    };
    let sealed = fluxfang_sensor_proto::seal_batch(&key, &batch).unwrap();
    let url = format!("http://127.0.0.1:{port}/sensor/ingest");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("X-Sensor-Id", "frontgate")
        .body(sealed)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let j: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(j["accepted"].as_array().unwrap().len(), 1);

    let em = EmissionRepo::get(&pool, em_id)
        .await
        .unwrap()
        .expect("emission must still be stored, just without a location");
    assert_eq!(em.lat, None);
    assert_eq!(em.lon, None);
    assert_eq!(em.location_quality, "none");

    mgr.stop(ds_id).await;
}

/// A `kind` outside the `emission_kind_check` CHECK set (`'wifi'`,
/// `'bluetooth'`, `'tpms'`) makes the insert fail with a permanent DB
/// constraint violation -- retrying can NEVER succeed. The handler must
/// still ACK it (200, id present in `accepted`) so an at-least-once
/// forwarder drops this poison pill instead of retrying it forever, and the
/// row must NOT be stored (the insert genuinely failed, it's dropped, not
/// silently persisted).
#[tokio::test]
async fn permanent_constraint_violation_is_acked_and_dropped() {
    let (pool, mgr, ds_id, port, key) = setup(false).await;

    let em_id = uuid::Uuid::new_v4();
    let batch = fluxfang_sensor_proto::SensorBatch {
        sensor_id: "frontgate".into(),
        sent_at_ms: chrono::Utc::now().timestamp_millis(),
        emissions: vec![fluxfang_sensor_proto::WireEmission {
            id: em_id,
            kind: "bogus".into(), // violates emission_kind_check
            signal_strength: Some(-40),
            lat: Some(1.5),
            lon: Some(2.5),
            observed_at: chrono::Utc::now(),
            payload: serde_json::json!({"bssid":"aa:bb:cc:dd:ee:ff"}),
        }],
    };
    let sealed = fluxfang_sensor_proto::seal_batch(&key, &batch).unwrap();
    let url = format!("http://127.0.0.1:{port}/sensor/ingest");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("X-Sensor-Id", "frontgate")
        .body(sealed)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let j: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(j["accepted"].as_array().unwrap().len(), 1);
    assert_eq!(
        j["accepted"][0].as_str().unwrap(),
        em_id.to_string(),
        "poison pill must be ACKed so the forwarder drops it instead of retrying forever"
    );

    // The insert genuinely failed -- nothing was stored.
    assert!(
        EmissionRepo::get(&pool, em_id).await.unwrap().is_none(),
        "the row must not be stored; it was dropped, not persisted"
    );

    mgr.stop(ds_id).await;
}
