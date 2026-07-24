//! A node provisioned as a Sensor *after* the backend started must begin
//! enrolling without a restart.
//!
//! ## The bug this pins
//!
//! `main.rs` read the node role once at startup and only called
//! `spawn_forwarder` inside `if is_sensor`. Every fresh install hits that
//! ordering head-on:
//!
//! 1. Backend starts against an empty `app_config` -> `is_sensor == false`,
//!    so no forwarder task is ever spawned.
//! 2. The operator runs first-run setup and picks the Sensor role, which
//!    writes `role = sensor` to the database.
//! 3. Nothing spawns the forwarder, because that decision was made in step 1.
//!
//! The node therefore never sends a single `/sensor/enroll` request, so it can
//! never appear in the Standalone's pending list -- with no error anywhere,
//! because nothing is running to report one. The same trap applied to
//! switching role in Settings.
//!
//! The fix is to spawn the loop unconditionally and let it gate itself on the
//! *current* config each cycle (`load_target` already returns `None` for a
//! Standalone or an absent config, which pauses it). This test drives that
//! contract: start the loop against a node that is not yet a sensor, then
//! provision it, and require enrollment to happen on its own.

mod common;

use std::time::Duration;

use fluxfang_api::forwarder::{spawn_forwarder, SensorForwarder};
use fluxfang_db::{
    AppConfigRepo, DataSourceRepo, NewDataSource, NodeConfig, NodeRole, SensorConfig, SensorRepo,
};

async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

/// Poll until `f` reports true, or fail after `label`'s deadline. The
/// forwarder is a background loop on its own schedule, so there is no
/// deterministic moment to assert at; a fixed sleep would either be flaky or
/// needlessly slow.
async fn eventually(label: &str, mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("timed out waiting for {label}");
}

#[tokio::test]
async fn a_node_provisioned_as_a_sensor_after_startup_enrolls_without_a_restart() {
    let pool = common::fresh_pool_shared().await;
    let port = free_port().await;

    // A Standalone listener with an open enrollment window, waiting.
    let ds = DataSourceRepo::insert(
        &pool,
        NewDataSource {
            kind: "sensor".into(),
            mode: "listener".into(),
            interface: None,
            config: serde_json::json!({"bind_ip":"127.0.0.1","bind_port":port}),
        },
    )
    .await
    .unwrap();
    let mgr = common::sensor_manager(pool.clone());
    mgr.start(ds.id).await;
    mgr.open_enrollment_window(ds.id).await;

    // The node is NOT a sensor yet -- exactly the state a freshly-installed or
    // freshly-wiped backend boots into. Starting the loop here is the whole
    // point: it models the process having already made its startup decision.
    spawn_forwarder(SensorForwarder::new(pool.clone(), Default::default()));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        SensorRepo::list(&pool).await.unwrap().len(),
        0,
        "a node with no role must not enroll",
    );

    // Now provision it as a Sensor, the way first-run setup does. No restart.
    let key = fluxfang_sensor_proto::generate_key();
    AppConfigRepo::set_password_hash(&pool, "x").await.unwrap();
    AppConfigRepo::set_node_config(
        &pool,
        &NodeConfig {
            role: NodeRole::Sensor,
            node_sensor_id: "frontgate".to_string(),
            sensor: Some(SensorConfig {
                host: "127.0.0.1".to_string(),
                port,
                key: fluxfang_sensor_proto::encode_key(&key),
                cache_ttl_secs: 604_800,
            }),
        },
    )
    .await
    .unwrap();

    eventually(
        "the sensor to appear as pending on the Standalone",
        async || {
            SensorRepo::get_by_sensor_id(&pool, ds.id, "frontgate")
                .await
                .unwrap()
                .is_some()
        },
    )
    .await;

    let enrolled = SensorRepo::get_by_sensor_id(&pool, ds.id, "frontgate")
        .await
        .unwrap()
        .expect("just asserted present");
    assert_eq!(enrolled.status, "pending");
    assert_eq!(
        enrolled.fingerprint,
        fluxfang_sensor_proto::fingerprint("frontgate", &key),
        "the operator verifies this against the sensor's own display, so it must \
         be the fingerprint of the key the sensor actually holds",
    );

    mgr.stop(ds.id).await;
}
