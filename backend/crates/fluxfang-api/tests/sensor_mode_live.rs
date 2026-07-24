//! A node that becomes a Sensor *after* startup must cache what it captures,
//! without a restart.
//!
//! ## The bug this pins
//!
//! This is the second half of the startup-ordering bug fixed in
//! `forwarder_startup.rs`. That one made the forwarder spawn regardless of the
//! role at boot; this one is the capture side, which was left stale.
//!
//! `main.rs` computes `is_sensor` once, before the HTTP listener is up, and
//! threads it into `CaptureSupervisor` as `sensor_mode`. The capture reader
//! branches on it per observation: a Sensor caches for forwarding, a
//! Standalone runs the full local pipeline. On a fresh install the role at
//! boot is "none", so `sensor_mode` is false; running setup and choosing
//! Sensor does not change the already-running process.
//!
//! The result is a node that looks configured and healthy -- it enrolls, it
//! is approved, its data sources start -- but every observation it captures
//! goes down the Standalone path instead of into `cached_emission`. Nothing
//! appears on the Sensor dashboard and nothing is ever forwarded, for any
//! data source, until the backend is restarted.
//!
//! The fix makes the flag live rather than a boot-time snapshot, so this test
//! flips the role after the supervisor exists and requires the very next
//! observation to be cached.

mod common;

use std::sync::Arc;
use std::time::Duration;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_capture::RawObservation;
use fluxfang_db::{
    AppConfigRepo, CachedEmissionRepo, DataSourceRepo, NewDataSource, NodeConfig, NodeRole,
    SensorConfig,
};
use sqlx::PgPool;

/// Poll until `f` holds, so the test never depends on the capture reader's
/// scheduling. Fails with `label` rather than hanging.
async fn eventually(label: &str, mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {label}");
}

/// Write the node config first-run setup would write for a Sensor.
async fn provision_as_sensor(pool: &PgPool) {
    AppConfigRepo::set_password_hash(pool, "x").await.unwrap();
    AppConfigRepo::set_node_config(
        pool,
        &NodeConfig {
            role: NodeRole::Sensor,
            node_sensor_id: "frontgate".to_string(),
            sensor: Some(SensorConfig {
                // Nothing needs to be reachable: this test is about where
                // captured observations are written, not about forwarding.
                host: "127.0.0.1".to_string(),
                port: 1,
                key: fluxfang_sensor_proto::encode_key(&fluxfang_sensor_proto::generate_key()),
                cache_ttl_secs: 604_800,
            }),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_data_source_started_after_the_node_becomes_a_sensor_caches_its_captures() {
    let pool = common::fresh_pool_shared().await;

    // Boot as an unconfigured node, exactly like a fresh install: the process
    // decides `sensor_mode = false` because there is no role yet.
    // The mock replays whatever it is seeded with; an empty factory captures
    // nothing, which would make this test pass or fail for the wrong reason.
    let factory = MockCapturerFactory::with_wifi_observations(vec![RawObservation {
        kind: "bluetooth".into(),
        observed_at: chrono::Utc::now(),
        signal_strength: Some(-55),
        payload: serde_json::json!({"address":"aa:bb:cc:dd:ee:ff","name":"probe"}),
    }]);
    let state = common::state_with_factory(pool.clone(), Arc::new(factory));

    // The operator now runs setup and picks Sensor. No restart.
    provision_as_sensor(&pool).await;

    // Then adds a data source, which is when capture actually begins.
    let ds = DataSourceRepo::insert(
        &pool,
        NewDataSource {
            kind: "bluetooth".into(),
            mode: "scan".into(),
            interface: Some("hci0".into()),
            config: serde_json::json!({}),
        },
    )
    .await
    .unwrap();
    state.capture.start(ds.id).await.expect("start capture");

    eventually("the sensor to cache what it captured", async || {
        CachedEmissionRepo::stats(&pool)
            .await
            .map(|s| s.total > 0)
            .unwrap_or(false)
    })
    .await;

    let stats = CachedEmissionRepo::stats(&pool).await.unwrap();
    assert!(
        stats.undelivered > 0,
        "cached rows must start undelivered so the forwarder ships them",
    );

    let _ = state.capture.stop(ds.id).await;
}
