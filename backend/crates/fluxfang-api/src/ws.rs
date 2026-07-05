//! `GET /ws` (Task 7.1): bridges `crate::ingest::Event` (broadcast by
//! `ingest`/`alerts::fire_rule` on `IngestCtx::events`, see that module's
//! docs) out to connected clients as a live WebSocket stream. PROTECTED —
//! mounted in `lib.rs::app`'s protected router group, behind
//! `require_auth`, same as every other non-setup/login route.
//!
//! ## Auth on the upgrade request
//!
//! A WebSocket handshake is just a plain `GET` request (with `Upgrade`/
//! `Connection` headers) before it's anything else, and the browser sends
//! it same-origin with the session cookie attached exactly like any other
//! request — so mounting `/ws` in the protected group and letting
//! `middleware::require_auth` run *before* axum's `WebSocketUpgrade`
//! extractor even sees the request is sufficient. No special-casing needed:
//! an unauthenticated handshake gets rejected with a plain `401` response
//! instead of `101 Switching Protocols`, which is exactly what a client
//! opening the socket sees as a failed handshake.
//!
//! ## Wire shape
//!
//! Every message sent to the client is a single JSON object,
//! `{"type": "...", "data": ...}`:
//!
//! - `{"type":"emission","data":<Emission>}` / `{"type":"notification","data":<Notification>}`
//!   — `crate::ingest::Event` is already `#[serde(tag = "type", content =
//!   "data", rename_all = "snake_case")]`, so `serde_json::to_string(&event)`
//!   produces this shape with no extra wrapping.
//! - `{"type":"lagged","dropped":<n>}` — sent instead of an event when this
//!   connection's `broadcast::Receiver` fell behind (see
//!   [`next_wire_message`]'s doc comment).
//!
//! This is a server-to-client-only stream (YAGNI per the task brief): no
//! client command handling, no per-topic subscriptions. Inbound client
//! frames are read only to detect disconnect (see [`handle_socket`]).

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde_json::json;
use tokio::sync::broadcast;

use crate::ingest::Event;
use crate::state::AppState;

pub fn protected_routes() -> Router<AppState> {
    Router::new().route("/ws", get(ws_handler))
}

/// Subscribe to `state.capture`'s event broadcast *before* upgrading, so
/// nothing sent between the handshake completing and the read loop
/// starting can be missed (subscribing only sees events sent afterward —
/// see `CaptureSupervisor::subscribe_events`'s doc comment).
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let events = state.capture.subscribe_events();
    ws.on_upgrade(move |socket| handle_socket(socket, events))
}

/// What [`next_wire_message`] decided to do with one `events.recv()` result.
enum WireOutcome {
    /// Send this text frame to the client.
    Send(String),
    /// Nothing to send this iteration (e.g. a value that failed to
    /// serialize) — keep the loop running.
    Skip,
    /// The broadcast channel is permanently done (`RecvError::Closed`, i.e.
    /// every `broadcast::Sender` clone was dropped) — end the connection.
    End,
}

/// Translate one `broadcast::Receiver::recv()` outcome into what to do on
/// the WebSocket. Split out from [`handle_socket`] so the
/// `RecvError::Lagged` handling is unit-testable against a plain
/// `tokio::sync::broadcast` pair, without a real WebSocket/HTTP connection
/// (see this module's `tests::lagged_receiver_yields_a_lagged_wire_message`).
///
/// - `Ok(event)`: the normal case — `{"type":..., "data":...}` (see module
///   docs).
/// - `Err(Lagged(n))`: this receiver fell behind the broadcast channel's
///   capacity and `n` messages were dropped before it could read them.
///   Per the task brief, this must **not** end the connection — the client
///   is told via `{"type":"lagged","dropped":n}` and the loop continues.
/// - `Err(Closed)`: every `broadcast::Sender` clone is gone (in practice,
///   `AppState`/`CaptureSupervisor` itself being torn down) — nothing more
///   will ever arrive, so the connection ends.
async fn next_wire_message(events: &mut broadcast::Receiver<Event>) -> WireOutcome {
    match events.recv().await {
        Ok(event) => match serde_json::to_string(&event) {
            Ok(text) => WireOutcome::Send(text),
            Err(err) => {
                // Practically unreachable -- every `Event` variant's payload
                // (`Emission`/`Notification`) is a plain, always-serializable
                // struct -- but a single bad value must not be allowed to
                // kill an otherwise-healthy connection.
                eprintln!("fluxfang-api: failed to serialize ws Event, skipping: {err}");
                WireOutcome::Skip
            }
        },
        Err(broadcast::error::RecvError::Lagged(dropped)) => {
            WireOutcome::Send(json!({"type": "lagged", "dropped": dropped}).to_string())
        }
        Err(broadcast::error::RecvError::Closed) => WireOutcome::End,
    }
}

/// Drive one connected client: fan out broadcast `Event`s to it as JSON
/// text frames until either side ends the conversation.
///
/// `tokio::select!` races the client socket's own `recv()` against the next
/// broadcast event so a client disconnect (`Close` frame, or the connection
/// simply dropping — `None`/`Err`) is noticed promptly instead of only after
/// the next event happens to be sent. Inbound *data* frames (`Text`/
/// `Binary`/`Ping`/`Pong`) are otherwise ignored — this is a server-to-client
/// stream only (see module docs); axum's `WebSocket` already answers `Ping`
/// with `Pong` internally, so nothing extra is needed for keepalive.
async fn handle_socket(mut socket: WebSocket, mut events: broadcast::Receiver<Event>) {
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Err(_)) => return,
                    Some(Ok(_)) => {} // ignore inbound data frames; only used to detect disconnect
                }
            }
            outcome = next_wire_message(&mut events) => {
                match outcome {
                    WireOutcome::Send(text) => {
                        if socket.send(Message::Text(text)).await.is_err() {
                            return;
                        }
                    }
                    WireOutcome::Skip => {}
                    WireOutcome::End => return,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluxfang_db::models::Emission;
    use uuid::Uuid;

    fn dummy_emission() -> Emission {
        Emission {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            data_source_id: None,
            emitter_id: None,
            session_id: None,
            observed_at: chrono::Utc::now(),
            signal_strength: None,
            kind: "wifi".to_string(),
            payload: serde_json::json!({}),
            lon: None,
            lat: None,
        }
    }

    /// A receiver that hasn't fallen behind gets the plain tagged JSON, not
    /// a lagged notice.
    #[tokio::test]
    async fn healthy_receiver_yields_the_tagged_event_json() {
        let (tx, mut rx) = broadcast::channel(8);
        tx.send(Event::Emission(dummy_emission())).unwrap();

        match next_wire_message(&mut rx).await {
            WireOutcome::Send(text) => {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(value["type"], "emission");
                assert!(value["data"].is_object());
            }
            _ => panic!("expected WireOutcome::Send"),
        }
    }

    /// A receiver that falls behind the channel's capacity (more sends than
    /// capacity happen before it ever reads) must get a `{"type":"lagged",
    /// "dropped":n}` notice, and the loop must keep going rather than end —
    /// this is the core of Task 7.1's backpressure requirement, exercised
    /// here directly against `tokio::sync::broadcast` (no real socket
    /// needed) because reliably forcing a real `WebSocket` connection to lag
    /// would require the test to also stall the client's TCP reads, which is
    /// slow and flaky. See this task's report for the full rationale.
    #[tokio::test]
    async fn lagged_receiver_yields_a_lagged_wire_message_and_does_not_end() {
        let (tx, mut rx) = broadcast::channel(2);
        // Send 3 events into a capacity-2 channel without ever reading `rx`
        // -- the oldest is dropped, and `rx`'s next `recv()` reports it fell
        // behind by (at least) that count.
        tx.send(Event::Emission(dummy_emission())).unwrap();
        tx.send(Event::Emission(dummy_emission())).unwrap();
        tx.send(Event::Emission(dummy_emission())).unwrap();

        match next_wire_message(&mut rx).await {
            WireOutcome::Send(text) => {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(value["type"], "lagged");
                assert!(value["dropped"].as_u64().unwrap() >= 1);
            }
            _ => panic!("expected a lagged WireOutcome::Send, not skip/end"),
        }

        // The loop continues afterward: the receiver can still read the
        // events that weren't dropped.
        match next_wire_message(&mut rx).await {
            WireOutcome::Send(text) => {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(value["type"], "emission");
            }
            _ => panic!("expected the connection to keep receiving after a lag notice"),
        }
    }

    /// `RecvError::Closed` (every `Sender` dropped) ends the loop instead of
    /// looping forever.
    #[tokio::test]
    async fn closed_sender_ends_the_loop() {
        let (tx, mut rx) = broadcast::channel::<Event>(2);
        drop(tx);

        match next_wire_message(&mut rx).await {
            WireOutcome::End => {}
            _ => panic!("expected WireOutcome::End once every Sender is dropped"),
        }
    }
}
