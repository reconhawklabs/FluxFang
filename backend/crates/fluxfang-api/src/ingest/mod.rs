//! Ingest: turns raw capture-layer data into stored, session-bounded rows.
//!
//! Task 5.1 added [`session::SessionManager`] (session bounding + the
//! host's own GPS trajectory log). Task 5.2 (this module's top level) adds
//! the emission ingest pipeline: [`ingest`] turns one
//! `fluxfang_capture::RawObservation` into a persisted, auto-attached,
//! broadcast [`fluxfang_db::models::Emission`]. Later tasks add alert
//! evaluation (5.3) and zone-membership tracking (5.4) as calls inside
//! [`ingest`] itself — see the seam comments in its body.
//!
//! ## `Event` and `IngestCtx`
//!
//! [`Event`] is the one type published on [`IngestCtx::events`]
//! (a `tokio::sync::broadcast` channel) for every live pipeline occurrence
//! this backend produces — an `Emission` from [`ingest`] itself, or (from
//! Task 5.3, not implemented here) a `Notification` when an alert fires.
//! It lives in this module (not a separate `events`/`ws` module) because
//! nothing outside `fluxfang-api` needs it yet: Task 7.1's WebSocket
//! handler is itself part of this crate and can reach it as
//! `crate::ingest::Event`.
//!
//! [`IngestCtx`] bundles [`ingest`]'s dependencies (the pool, the active
//! [`session::SessionManager`], and the broadcast sender). Kept minimal per
//! this task's YAGNI scope — Task 5.3 will need to add a secret key here
//! (to decrypt `alert_method.config_encrypted` when dispatching a fired
//! alert's notification), not built now since nothing in this task needs
//! it.
//!
//! ## No active session
//!
//! Capture is only ever meant to run while a `survey_session` is open —
//! [`session::SessionManager::open`] is what starts a capture run in the
//! first place (Task 5.1; Task 6.2 wires the actual start-capture
//! endpoint). So "no active session" inside [`ingest`] means the caller
//! invoked it outside that lifecycle, which is a caller bug, not a
//! transient condition to paper over silently. [`ingest`] returns `Err`
//! rather than inserting with a fabricated/absent session — see its own
//! doc comment for the full rationale.
//!
//! ## Auto-attach: first match wins
//!
//! [`ingest`] loads every `emitter` via `EmitterRepo::list` (which orders
//! `ORDER BY created_at ASC`) and evaluates each one's `match_criteria`
//! (parsed as a [`fluxfang_core::rule::Rule`]) against the new emission's
//! `payload` via [`fluxfang_core::rule::eval`], in that order, stopping at
//! the **first** emitter whose rule matches — later emitters, even ones
//! that would also match, are never evaluated. This is a deliberate,
//! simple tie-break (oldest-created-emitter-wins) rather than any notion
//! of rule "specificity"; see [`ingest`]'s doc comment for the full
//! rationale and the regression test that pins this exact ordering.

pub mod alerts;
pub mod session;
pub mod zones;

use std::sync::Arc;

use fluxfang_capture::RawObservation;
use fluxfang_core::rule::{eval, Rule};
use fluxfang_db::models::{Emission, NewEmission, Notification};
use fluxfang_db::{EmissionRepo, EmitterRepo};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

use session::SessionManager;

/// A live pipeline event, broadcast on [`IngestCtx::events`] for Task 7.1's
/// WebSocket handler to fan out to connected clients.
///
/// - `Emission` fires once per [`ingest`] call, carrying the emission in
///   its final state (i.e. *after* auto-attach has possibly set
///   `emitter_id`).
/// - `Notification` is published by Task 5.3's [`alerts::fire_rule`] once
///   per `notification` row it persists, whenever an alert fires -- see
///   that module for the full evaluation/dispatch/broadcast pipeline.
///
/// Serde-tagged (`{"type": "emission", "data": {...}}`) so a WS client can
/// dispatch on `type` without inspecting `data`'s shape — the same tagged-
/// enum convention `fluxfang_api::notify::DeliveryStatus` already uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    Emission(Emission),
    Notification(Notification),
}

/// Shared dependencies for [`ingest`].
pub struct IngestCtx {
    pub pool: PgPool,
    pub sessions: Arc<SessionManager>,
    pub events: broadcast::Sender<Event>,
    /// AES-256-GCM key used to decrypt `alert_method.config_encrypted` when
    /// [`alerts::fire_rule`] dispatches a fired alert's notification (Task
    /// 5.3). Production wiring (loading `FLUXFANG_SECRET_KEY` via
    /// `fluxfang_core::secrets::key_from_base64` into `AppState`/this
    /// struct) is Task 6.2's job; this struct just needs the parsed 32
    /// bytes, however the caller obtained them.
    pub secret_key: [u8; 32],
}

/// Turn one capture-layer [`RawObservation`] into a persisted, auto-attached,
/// broadcast [`Emission`].
///
/// `data_source_id` identifies which configured `data_source` produced
/// `obs` — a `RawObservation` is hardware-agnostic and carries no such id
/// itself (see `fluxfang_capture`'s module docs), so the caller (Task 6.2's
/// start-capture, which owns the mapping from a running capturer to its
/// `data_source` row) supplies it explicitly.
///
/// ## Steps
///
/// 1. Build a `NewEmission`: `session_id` from
///    `ctx.sessions.current_session_id()` — required; see the module docs'
///    "No active session" section for why a missing session is an `Err`,
///    not a skip. `location` from `ctx.sessions.latest_fix()` (`None` if no
///    fix has arrived yet this session). `observed_at`/`signal_strength`/
///    `kind`/`payload` copied from `obs`. `emitter_id` starts `None`.
/// 2. Insert it (`EmissionRepo::insert`).
/// 3. **Auto-attach** (see module docs for the full first-match-wins
///    rationale): load every emitter, evaluate each one's `match_criteria`
///    rule against the emission's `payload` in `EmitterRepo::list`'s
///    `created_at ASC` order, and on the first match, `EmissionRepo::set_emitter`
///    stamps the emission's `emitter_id` and `EmitterRepo::touch_seen`
///    widens that emitter's `first_seen_at`/`last_seen_at` to include this
///    emission's `observed_at`. An emitter whose `match_criteria` fails to
///    parse as a `Rule` is treated as non-matching (skipped) rather than
///    failing the whole ingest — one operator's malformed rule must not
///    break auto-attach for every other emitter/emission. No match leaves
///    the emission unassigned.
/// 4. `alerts::evaluate_alerts(ctx, &emission).await` (Task 5.3) — checks
///    every enabled `alert_rule` with a `detected` trigger against this
///    now-finalized emission's emitter/entity (+ optional `content_match`),
///    dispatching+persisting+broadcasting any resulting `Notification`s
///    itself (see that module for the full pipeline). Returns `()` and
///    self-contains every error — a problem evaluating alerts must never
///    prevent the emission itself from finishing its own insert/broadcast.
/// 5. `zones::update_target_zones(ctx, &emission).await` (Task 5.4) —
///    recomputes zone membership for the emission's emitter (and, if
///    grouped, its entity) against every zone, firing any
///    `enters_zone`/`leaves_zone` `alert_rule` whose target and zone match
///    a just-happened transition (state-diffed against `zone_membership`,
///    so a still-inside/still-outside emission fires nothing). Same
///    self-containment guarantee as `evaluate_alerts` — see that module's
///    own doc comment for the full pipeline.
/// 6. Broadcast `Event::Emission(emission)` on `ctx.events`. `send` returns
///    `Err` when there are currently no subscribers (nobody has a WS
///    connection open) — an expected, benign state, not a failure of
///    `ingest` itself, so the result is deliberately discarded.
/// 7. Return the stored (and possibly auto-attached) emission.
pub async fn ingest(
    ctx: &IngestCtx,
    data_source_id: Uuid,
    obs: RawObservation,
) -> anyhow::Result<Emission> {
    let session_id = ctx.sessions.current_session_id().ok_or_else(|| {
        anyhow::anyhow!(
            "ingest called with no active survey_session -- capture must only run while a \
             SessionManager-bounded session is open (see this module's docs)"
        )
    })?;
    let location = ctx.sessions.latest_fix().map(|fix| (fix.lon, fix.lat));

    let new = NewEmission {
        data_source_id: Some(data_source_id),
        emitter_id: None,
        session_id,
        observed_at: obs.observed_at,
        signal_strength: obs.signal_strength,
        location,
        kind: obs.kind,
        payload: obs.payload,
    };

    let mut emission = EmissionRepo::insert(&ctx.pool, new).await?;

    // Auto-attach: first match wins, in EmitterRepo::list's created_at ASC
    // order (see module docs).
    let emitters = EmitterRepo::list(&ctx.pool).await?;
    for emitter in emitters {
        let rule: Rule = match serde_json::from_value(emitter.match_criteria.clone()) {
            Ok(rule) => rule,
            // Malformed match_criteria: treat this emitter as non-matching
            // rather than failing the whole ingest over one bad rule.
            Err(_) => continue,
        };
        // `fluxfang_core::rule::eval` is pure, in-memory structural
        // matching of a JSON payload against a `Rule`'s conditions -- it
        // never parses or executes code (see that function's own doc
        // comment's explicit NOTE). Not JS/Python `eval`.
        if eval(&rule, &emission.payload) {
            emission = EmissionRepo::set_emitter(&ctx.pool, emission.id, emitter.id).await?;
            EmitterRepo::touch_seen(&ctx.pool, emitter.id, emission.observed_at).await?;
            break;
        }
    }

    alerts::evaluate_alerts(ctx, &emission).await;
    zones::update_target_zones(ctx, &emission).await;

    // No subscribers is a normal, expected state (see doc comment above) --
    // deliberately not propagated as an ingest failure.
    let _ = ctx.events.send(Event::Emission(emission.clone()));

    Ok(emission)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fresh_pool;
    use async_trait::async_trait;
    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use fluxfang_capture::mock::MockGps;
    use fluxfang_capture::{GpsFix, GpsSource};
    use fluxfang_db::models::{NewDataSource, NewEmitter};
    use fluxfang_db::DataSourceRepo;
    use session::{no_op_hook, SessionManagerConfig};
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// A `GpsSource` backed by a channel, letting tests control exactly
    /// when a fix arrives without racing the source's own exhaustion (a
    /// finite `MockGps` track would close the session as soon as it drains,
    /// which every test here needs to stay open through). Duplicated from
    /// `session::tests::ChannelGps` rather than reused: that type is
    /// private to `session`'s own test module.
    struct ChannelGps(mpsc::UnboundedReceiver<GpsFix>);

    #[async_trait]
    impl GpsSource for ChannelGps {
        async fn next_fix(&mut self) -> Option<GpsFix> {
            self.0.recv().await
        }
    }

    async fn seed_wifi_source(pool: &PgPool) -> Uuid {
        DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0"))
            .await
            .expect("seed wifi data_source")
            .id
    }

    /// Open a `SessionManager` on a `ChannelGps`, send exactly one `fix`,
    /// and poll (bounded) until `latest_fix()` reflects it. The sender is
    /// returned (rather than dropped) so the channel -- and therefore the
    /// session -- stays open for the rest of the test; only the gap timer
    /// (5 minutes, never reached in a test) could otherwise close it.
    async fn session_with_fix(
        pool: PgPool,
        fix: GpsFix,
    ) -> (SessionManager, mpsc::UnboundedSender<GpsFix>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let gps = ChannelGps(rx);
        let manager = SessionManager::open(
            pool,
            gps,
            SessionManagerConfig {
                inactivity_gap: Duration::from_secs(5 * 60),
                write_interval: Duration::ZERO,
            },
            no_op_hook(),
        )
        .await
        .expect("open SessionManager");

        tx.send(fix.clone()).expect("send fix over ChannelGps");
        tokio::time::timeout(Duration::from_secs(5), async {
            while manager.latest_fix().as_ref() != Some(&fix) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("latest_fix should reflect the sent fix");

        (manager, tx)
    }

    fn wifi_obs(bssid: &str, observed_at: chrono::DateTime<Utc>) -> RawObservation {
        RawObservation {
            kind: "wifi".to_string(),
            observed_at,
            signal_strength: Some(-55),
            payload: serde_json::json!({"bssid": bssid, "channel": 6}),
        }
    }

    fn a_fix(at: chrono::DateTime<Utc>) -> GpsFix {
        GpsFix {
            at,
            lon: -122.4,
            lat: 37.7,
            altitude: None,
            speed: None,
            heading: None,
            quality: 1,
        }
    }

    #[tokio::test]
    async fn ingest_with_no_matching_emitter_persists_unassigned_stamps_session_location_and_broadcasts(
    ) {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let fix = a_fix(base);
        let (manager, _tx) = session_with_fix(pool.clone(), fix.clone()).await;
        let session_id = manager.current_session_id().unwrap();

        let (events_tx, mut events_rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Arc::new(manager),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let obs = wifi_obs("aa:bb:cc:dd:ee:ff", base);
        let emission = ingest(&ctx, ds, obs).await.expect("ingest should succeed");

        assert_eq!(
            emission.emitter_id, None,
            "no emitters exist, so unassigned"
        );
        assert_eq!(emission.session_id, Some(session_id));
        assert_eq!(emission.lon, Some(fix.lon));
        assert_eq!(emission.lat, Some(fix.lat));

        // Persisted, not just returned in-memory.
        let got = EmissionRepo::get(&pool, emission.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.id, emission.id);
        assert_eq!(got.emitter_id, None);

        let event = events_rx
            .try_recv()
            .expect("expected a broadcast Event::Emission");
        match event {
            Event::Emission(e) => assert_eq!(e.id, emission.id),
            Event::Notification(_) => panic!("expected an Emission event"),
        }
    }

    #[tokio::test]
    async fn ingest_returns_err_when_no_session_is_active() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;

        // An empty, non-looping MockGps track exhausts (returns `None`)
        // essentially immediately, closing the session -- `join` waits for
        // exactly that deterministic point.
        let mut manager = SessionManager::open(
            pool.clone(),
            MockGps::new(vec![]),
            SessionManagerConfig::default(),
            no_op_hook(),
        )
        .await
        .unwrap();
        manager.join().await;
        assert!(manager.current_session_id().is_none());

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool,
            sessions: Arc::new(manager),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let err = ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", base))
            .await
            .expect_err("no active session must be an error, not a silent skip");
        assert!(err.to_string().contains("no active survey_session"));
    }

    #[tokio::test]
    async fn ingest_auto_attaches_to_matching_emitter_and_advances_last_seen_at() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, _tx) = session_with_fix(pool.clone(), a_fix(base)).await;

        let emitter = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "Bob's AP".to_string(),
                type_: Some("wifi".to_string()),
                entity_id: None,
                match_criteria: serde_json::json!({
                    "match": "all",
                    "conditions": [{"field": "bssid", "op": "eq", "value": "aa:bb:cc:dd:ee:ff"}]
                }),
            },
        )
        .await
        .unwrap();

        // Seed an existing, earlier last_seen_at so the ingest below has to
        // actually *widen* the window, not merely populate it from NULL.
        let earlier = base - ChronoDuration::days(1);
        EmitterRepo::touch_seen(&pool, emitter.id, earlier)
            .await
            .unwrap();

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Arc::new(manager),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let emission = ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", base))
            .await
            .expect("ingest should succeed");
        assert_eq!(emission.emitter_id, Some(emitter.id));

        let updated = EmitterRepo::get(&pool, emitter.id).await.unwrap().unwrap();
        assert_eq!(
            updated.first_seen_at,
            Some(earlier),
            "first_seen_at should stay at the earlier timestamp"
        );
        assert_eq!(
            updated.last_seen_at,
            Some(base),
            "last_seen_at should have advanced to this emission's observed_at"
        );
    }

    #[tokio::test]
    async fn ingest_first_match_wins_by_emitter_creation_order() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, _tx) = session_with_fix(pool.clone(), a_fix(base)).await;

        // Both emitters would match this observation (channel=6, and the
        // exact bssid) -- `first` is created (and therefore listed) before
        // `second`, and must win regardless of which rule looks more
        // "specific".
        let first = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "first".to_string(),
                type_: None,
                entity_id: None,
                match_criteria: serde_json::json!({
                    "match": "all",
                    "conditions": [{"field": "channel", "op": "eq", "value": 6}]
                }),
            },
        )
        .await
        .unwrap();
        // A small real delay guarantees a distinct (later) `created_at` --
        // `now()` inside a single fast statement could otherwise tie at
        // microsecond resolution.
        tokio::time::sleep(Duration::from_millis(5)).await;
        let second = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "second".to_string(),
                type_: None,
                entity_id: None,
                match_criteria: serde_json::json!({
                    "match": "all",
                    "conditions": [{"field": "bssid", "op": "eq", "value": "aa:bb:cc:dd:ee:ff"}]
                }),
            },
        )
        .await
        .unwrap();

        // Pin the ordering assumption this test relies on, so a clock-tie
        // regression fails here with a clear message instead of silently
        // asserting the wrong thing below.
        let listed = EmitterRepo::list(&pool).await.unwrap();
        assert_eq!(listed[0].id, first.id);
        assert_eq!(listed[1].id, second.id);

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Arc::new(manager),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let emission = ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", base))
            .await
            .expect("ingest should succeed");
        assert_eq!(
            emission.emitter_id,
            Some(first.id),
            "the earlier-created emitter must win even though the later one's rule is also a match"
        );
    }

    /// Carried over from Task 5.2 (its report flagged this as "code correct,
    /// untested"): a malformed `match_criteria` emitter sits alongside a
    /// valid one -- `ingest` must still succeed overall and attach to the
    /// valid emitter, not abort auto-attach (or the whole ingest) just
    /// because one operator's rule fails to parse as a `Rule`.
    #[tokio::test]
    async fn ingest_skips_emitter_with_malformed_match_criteria_and_still_attaches_to_valid_one() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, _tx) = session_with_fix(pool.clone(), a_fix(base)).await;

        // Created first (and therefore listed/evaluated first by auto-
        // attach's created_at ASC order) but its match_criteria isn't a
        // valid `Rule` at all -- must be treated as non-matching, not a
        // fatal error.
        EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "broken emitter".to_string(),
                type_: Some("wifi".to_string()),
                entity_id: None,
                match_criteria: serde_json::json!({"this": "is not a Rule"}),
            },
        )
        .await
        .unwrap();

        tokio::time::sleep(Duration::from_millis(5)).await;
        let valid = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "valid emitter".to_string(),
                type_: Some("wifi".to_string()),
                entity_id: None,
                match_criteria: serde_json::json!({
                    "match": "all",
                    "conditions": [{"field": "bssid", "op": "eq", "value": "aa:bb:cc:dd:ee:ff"}]
                }),
            },
        )
        .await
        .unwrap();

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Arc::new(manager),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let emission = ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", base))
            .await
            .expect("ingest should succeed despite the malformed emitter rule");
        assert_eq!(
            emission.emitter_id,
            Some(valid.id),
            "auto-attach should skip the malformed rule and still attach to the valid emitter"
        );
    }
}
