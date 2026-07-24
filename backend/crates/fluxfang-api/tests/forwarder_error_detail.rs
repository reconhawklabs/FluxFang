//! A rejected batch must tell the operator *why*, not just the status code.
//!
//! ## The bug this pins
//!
//! `/sensor/ingest` returns three distinct 400s -- `missing X-Sensor-Id`,
//! `sensor_id mismatch`, and `stale batch` -- each with the reason in the
//! response body. The forwarder reported only `format!("ingest status {}",
//! status)` and dropped the body, so the Sensor dashboard showed
//! "Forwarding problem: ingest status 400 Bad Request" for all of them.
//!
//! That difference matters enormously in practice. `stale batch` means the
//! two nodes' clocks differ by more than the replay window, which is the
//! normal state of a just-rebooted Raspberry Pi (no RTC, clock wrong until
//! NTP settles). It presents as a sensor that enrolls fine and shows *online*
//! -- enrollment carries no timestamp, so only ingest rejects -- while
//! delivering exactly zero emissions. Told "stale batch", an operator fixes
//! NTP in a minute. Told "400 Bad Request", they have nothing to go on.
//!
//! The Standalone already knows the answer and puts it in the body. This only
//! required not throwing it away.

mod common;

use std::net::SocketAddr;

use axum::routing::post;
use axum::Router;
use fluxfang_api::forwarder::{ForwardOutcome, ForwarderTarget, SensorForwarder};
use fluxfang_db::models::NewCachedEmission;
use fluxfang_db::CachedEmissionRepo;

/// A stand-in Standalone that rejects every batch exactly the way the real
/// `/sensor/ingest` rejects a clock-skewed one. Using a stub rather than the
/// real listener is deliberate: `forward_once` stamps `sent_at_ms` with
/// `Utc::now()`, so a genuine stale batch cannot be produced from the sensor
/// side without moving the machine's clock.
async fn spawn_rejecting_standalone(status: axum::http::StatusCode, body: &'static str) -> u16 {
    let app = Router::new().route(
        "/sensor/ingest",
        post(move || async move { (status, body) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    port
}

#[tokio::test]
async fn a_rejected_batch_reports_the_standalones_reason_not_just_the_status() {
    let pool = common::fresh_pool_shared().await;

    // One cached emission, so there is something to forward.
    CachedEmissionRepo::insert(
        &pool,
        NewCachedEmission {
            kind: "wifi".into(),
            signal_strength: Some(-40),
            lat: None,
            lon: None,
            observed_at: chrono::Utc::now(),
            payload: serde_json::json!({"bssid":"aa:bb:cc:dd:ee:ff"}),
            data_source_id: None,
        },
    )
    .await
    .unwrap();

    let port = spawn_rejecting_standalone(axum::http::StatusCode::BAD_REQUEST, "stale batch").await;
    let key = fluxfang_sensor_proto::generate_key();
    let fwd = SensorForwarder::new(pool.clone(), Default::default());
    let target = ForwarderTarget {
        sensor_id: "frontgate".into(),
        key,
        base_url: format!("http://127.0.0.1:{port}"),
    };

    let outcome = fwd.forward_once(&target).await;
    let ForwardOutcome::Error(msg) = outcome else {
        panic!("expected a rejected batch to surface as an error, got {outcome:?}");
    };
    // Assert on meaning rather than the server's verbatim string: what the
    // operator needs is the *diagnosis*, and the forwarder is free to phrase
    // it better than the wire does.
    assert!(
        msg.to_lowercase().contains("stale"),
        "the operator must see the Standalone's reason. Got: {msg:?}",
    );
    // The clock is the actionable part and is not something an operator would
    // infer from the word "stale" alone.
    assert!(
        msg.to_lowercase().contains("clock"),
        "a stale batch must point at the clocks, since that is the fix. Got: {msg:?}",
    );
    assert!(
        !msg.contains("ingest status 400 Bad Request") || msg.len() > 60,
        "must not collapse back to the bare status code. Got: {msg:?}",
    );

    // Nothing was accepted, so nothing may be marked delivered -- otherwise
    // the rejected rows would be silently dropped from the queue.
    let stats = CachedEmissionRepo::stats(&pool).await.unwrap();
    assert_eq!(
        stats.undelivered, 1,
        "a rejected batch must stay queued for retry",
    );
}
