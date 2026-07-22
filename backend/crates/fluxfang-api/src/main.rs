use std::net::SocketAddr;
use std::sync::Arc;

use fluxfang_api::capture::RealCapturerFactory;
use fluxfang_api::AppState;
use fluxfang_core::secrets::key_from_base64;

#[tokio::main]
async fn main() {
    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set (see .env.example)");

    let pool = fluxfang_db::connect(&database_url)
        .await
        .expect("connect to Postgres");
    fluxfang_db::run_migrations(&pool)
        .await
        .expect("run database migrations");

    // Task 6.2: the CaptureSupervisor's `IngestCtx` needs this to decrypt
    // `alert_method.config_encrypted` when dispatching a fired alert (Task
    // 5.3's `alerts::fire_rule`). Loaded once here, not lazily, so a
    // misconfigured key fails fast at startup rather than surfacing as a
    // mysterious decrypt failure the first time an alert fires.
    let secret_key_raw = std::env::var("FLUXFANG_SECRET_KEY")
        .expect("FLUXFANG_SECRET_KEY must be set (see .env.example)");
    let secret_key = key_from_base64(&secret_key_raw)
        .expect("FLUXFANG_SECRET_KEY must be valid base64-encoded 32 bytes");

    // This node's role configuration (from app_config.settings) — role,
    // node_sensor_id (every locally-captured emission gets tagged with it,
    // see IngestCtx::node_sensor_id), and, for Sensor nodes, the Standalone
    // connection + cache settings.
    let node = fluxfang_db::AppConfigRepo::node_config(&pool)
        .await
        .ok()
        .flatten();
    let node_sensor_id = node
        .as_ref()
        .map(|n| n.node_sensor_id.clone())
        .unwrap_or_else(|| "local".to_string());
    let is_sensor = matches!(
        node.as_ref().map(|n| n.role),
        Some(fluxfang_db::NodeRole::Sensor)
    );

    let state = AppState::with_capture(
        pool.clone(),
        secret_key,
        Arc::new(RealCapturerFactory),
        node_sensor_id.clone(),
        is_sensor,
    );

    // Start the supervisor's background tasks (the device-failure drain) before
    // resuming, so a source that dies during/after resume is reconciled.
    state.capture.spawn_background();

    // Data sources that were capturing when this process last stopped still
    // carry `status = 'running'` in Postgres, but the supervisor's in-memory
    // running set and capture session don't survive a restart. Resume them so
    // capture actually comes back (rather than a phantom "running" that
    // captures nothing). Spawned, not awaited, so a slow or unavailable device
    // can't delay the HTTP listener coming up.
    let startup = state.clone();
    tokio::spawn(async move {
        startup.capture.resume_running().await;
    });

    if is_sensor {
        // Sensor node: forward cached captures to the Standalone + prune by
        // TTL. Sensor nodes don't run their own sensor listeners or TPMS
        // correlation — that's the Standalone's job once it receives the
        // forwarded emissions.
        //
        // Both tasks re-read the node config from the DB each cycle, so they
        // run unconditionally for a Sensor node and self-heal live: the pruner
        // (falling back to a 7-day TTL when unset) keeps `cached_emission`
        // bounded even with no valid forwarder config, and the forwarder pauses
        // until a valid key/host/port is saved in Settings — then picks it up
        // within one cycle, no restart. (This is why an invalid key at boot no
        // longer disables forwarding permanently.)
        fluxfang_api::forwarder::spawn_pruner(pool.clone());
        fluxfang_api::forwarder::spawn_forwarder(fluxfang_api::forwarder::SensorForwarder::new(
            pool.clone(),
        ));
    } else {
        // Standalone node: rebind any sensor listeners the user left running
        // (mirrors the capture supervisor's resume_running for capture
        // datasources).
        let startup_listeners = state.clone();
        tokio::spawn(async move {
            startup_listeners.sensor_listeners.resume_running().await;
        });

        // Periodic TPMS correlation pass (Spec B). Runs every minute; a pass
        // is a no-op unless some tpms_sensor emitter belongs to an
        // auto-correlate data source. Errors are logged, never fatal.
        let corr_pool = state.pool.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                ticker.tick().await;
                match fluxfang_api::correlate::run_correlation_pass(&corr_pool, chrono::Utc::now())
                    .await
                {
                    Ok(n) if n > 0 => eprintln!("TPMS correlation: added {n} association(s)"),
                    Ok(_) => {}
                    Err(err) => eprintln!("TPMS correlation pass failed: {err:#}"),
                }
            }
        });
    }

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    axum::serve(
        listener,
        fluxfang_api::app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}
