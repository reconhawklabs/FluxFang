//! Task 7.1: `GET /ws` bridges `crate::ingest::Event` broadcasts out to
//! connected WebSocket clients.
//!
//! Uses a real `tokio-tungstenite` client against the app spawned on a real
//! TCP port (via `common::spawn_server`) — a WebSocket handshake needs an
//! actual byte stream to upgrade, which `tower::ServiceExt::oneshot` (every
//! other test in this crate) can't provide. The in-process `Router` handed
//! to `spawn_server` and the one driven directly via `common::post_*`
//! helpers below are the *same* `Router` value (`Router::clone` is cheap —
//! it shares the one `Arc`-backed `AppState`/`CaptureSupervisor`/broadcast
//! channel), so triggering `POST /api/data-sources/:id/start` in-process
//! and observing its effect over the real WS connection exercises the exact
//! same `IngestCtx::events` sender the production wiring uses.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use chrono::{TimeZone, Utc};
use futures_util::StreamExt;
use serde_json::json;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::COOKIE;

use fluxfang_api::capture::MockCapturerFactory;
use fluxfang_capture::RawObservation;

mod common;
use common::{
    assert_status, body_json, post_json, post_json_with_cookie, post_with_cookie, session_cookie,
    spawn_server, test_app_with_factory,
};

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
        signal_strength: Some(-50),
        payload: json!({"bssid": bssid, "channel": 6}),
    }
}

/// RED/GREEN core scenario: connect an authenticated WS client to `/ws`,
/// then trigger a real ingest (starting a wifi data source whose
/// `MockCapturerFactory` replays one observation) and assert the client
/// receives it as `{"type":"emission","data":{...}}`.
///
/// The data source is created but deliberately *not* started until after
/// the WS client's handshake completes: `ws_handler` subscribes to the
/// broadcast channel before upgrading (see `ws.rs` doc comment), and a
/// `broadcast::Receiver` only ever sees events sent after it subscribes —
/// starting the source (and so triggering the mock capturer's emission)
/// any earlier would race the subscription and could make this test flaky.
#[tokio::test]
async fn ws_client_receives_broadcast_emission_as_tagged_json() {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let bssid = "aa:bb:cc:dd:ee:ff";
    let factory = Arc::new(MockCapturerFactory::with_wifi_observations(vec![wifi_obs(
        bssid, base,
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
    assert_status(&resp, StatusCode::CREATED);
    let created = body_json(resp).await;
    let id = created["id"].as_str().unwrap().to_string();

    let addr = spawn_server(app.clone()).await;

    let mut request = format!("ws://{addr}/ws")
        .into_client_request()
        .expect("build ws client request");
    request
        .headers_mut()
        .insert(COOKIE, cookie.parse().expect("cookie header value"));
    let (ws_stream, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("authenticated ws handshake should succeed");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    let (_write, mut read) = ws_stream.split();

    // Now trigger the actual ingest: starting the source runs the
    // MockCapturer's replay, which flows through `ingest` and broadcasts
    // `Event::Emission` on the same `IngestCtx::events` sender `/ws`
    // subscribed to above.
    let resp = post_with_cookie(&app, &format!("/api/data-sources/{id}/start"), &cookie).await;
    assert_status(&resp, StatusCode::OK);

    let msg = tokio::time::timeout(Duration::from_secs(5), read.next())
        .await
        .expect("should receive a ws message before timing out")
        .expect("ws stream should not end")
        .expect("ws message should not be a protocol error");
    let text = msg.into_text().expect("expected a text frame");
    let value: serde_json::Value = serde_json::from_str(&text).expect("frame body should be JSON");

    assert_eq!(value["type"], "emission");
    assert_eq!(value["data"]["payload"]["bssid"], bssid);
}

/// An unauthenticated connect attempt (no session cookie) must be rejected
/// at the handshake — `/ws` is mounted in the protected router group behind
/// `require_auth`, same as every other route.
#[tokio::test]
async fn ws_connect_without_a_session_is_rejected() {
    let (app, _pool) = test_app_with_factory(Arc::new(MockCapturerFactory::new())).await;
    let addr = spawn_server(app).await;

    let result = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await;

    assert!(
        result.is_err(),
        "an unauthenticated ws connect should fail the handshake, not upgrade"
    );
}
