//! Alert evaluation on ingest (Task 5.3).
//!
//! [`evaluate_alerts`] is called from `ingest`'s Task 5.3 seam, right after
//! auto-attach finalizes `emission.emitter_id` and *before* `ingest`'s own
//! `Event::Emission` broadcast (see that module's doc comment for the exact
//! call site) — so a hypothetical UI reacting to the live event stream sees
//! "alerts evaluated" (any `Event::Notification`s this produces) ahead of
//! "here's the emission that triggered them" in the same tick.
//!
//! [`fire_rule`] is the shared "a rule just fired" helper: dispatch delivery
//! to every linked `alert_method`, persist a `notification` row per method,
//! and broadcast `Event::Notification` for each one stored. It knows nothing
//! about *why* the rule fired (a `detected` trigger here, a zone-transition
//! trigger in Task 5.4) — callers decide that and hand it an already-built
//! [`NotificationPayload`], which is what makes it reusable across both.
//!
//! ## Scope: `detected` triggers only
//!
//! This task only interprets `alert_rule.trigger.on == "detected"`. Every
//! other trigger kind (`enters_zone`, `leaves_zone`, `host_enters_zone`,
//! `host_leaves_zone`) is Task 5.4's job and is skipped here.

use chrono::Utc;
use uuid::Uuid;

use fluxfang_core::rule::{eval, Rule};
use fluxfang_db::models::{AlertRule, Emission, Emitter, NewNotification};
use fluxfang_db::{AlertRuleRepo, EmitterRepo, EntityRepo, NotificationRepo};
use serde::Deserialize;

use crate::notify::{dispatch, DeliveryStatus, NotificationPayload};

use super::{Event, IngestCtx};

/// `alert_rule.trigger`'s JSON shape: `{"on": "detected" | "enters_zone" |
/// "leaves_zone" | "host_enters_zone" | "host_leaves_zone", "zone_id":
/// <uuid?>, "content_match": <Rule?>}` (see [`AlertRule`]'s own doc
/// comment). This task only reads `on` and `content_match`; `zone_id` is
/// kept here (unread for now) so this struct already mirrors the full
/// trigger shape ahead of Task 5.4 adding zone-transition evaluation.
#[derive(Debug, Clone, Deserialize)]
struct Trigger {
    on: String,
    #[serde(default)]
    #[allow(dead_code)] // Task 5.4: read for enters_zone/leaves_zone triggers.
    zone_id: Option<Uuid>,
    #[serde(default)]
    content_match: Option<Rule>,
}

/// Evaluate every enabled `alert_rule` whose trigger is `on == "detected"`
/// against `emission`, firing (via [`fire_rule`]) each rule whose target and
/// optional `content_match` both match.
///
/// ## Target match
/// - `target_type == "emitter"`: fires iff `emission.emitter_id ==
///   Some(rule.target_id)`.
/// - `target_type == "entity"`: fires iff the emission's emitter's
///   `entity_id == Some(rule.target_id)`.
/// - anything else (including a host rule, `target_type`/`target_id` both
///   `NULL`): a `detected` trigger never fires for it.
///
/// If `emission.emitter_id` is `None` (auto-attach found no match), no
/// `detected` rule can possibly match — checked once up front rather than
/// per rule, and the emitter is loaded at most once here too (not
/// re-queried per rule).
///
/// ## `content_match`
/// If `trigger.content_match` is present, [`fluxfang_core::rule::eval`]
/// must also return `true` against `emission.payload`, in addition to the
/// target match, for the rule to fire. Malformed `trigger` JSON (the whole
/// object fails to parse, or `content_match` isn't a valid [`Rule`]) makes
/// that one rule non-matching (skipped), not a hard failure of the whole
/// evaluation — same "one operator's mistake can't break everyone else"
/// stance `ingest`'s auto-attach already takes for a malformed
/// `emitter.match_criteria`.
///
/// ## Cost
/// Loads every `alert_rule` (`AlertRuleRepo::list`, no pagination) on every
/// single ingested emission. Fine at this slice's scale (documented, not
/// fixed, per this task's YAGNI framing — the same trade-off Task 5.2
/// already made for `EmitterRepo::list` inside auto-attach).
///
/// Never panics and never returns an error: every DB error along the way
/// (loading rules, loading the emitter/entity, `fire_rule`'s own internals)
/// is treated as "this emission triggers no further alerts", not a failure
/// of `ingest` itself — see `ingest`'s own doc comment for why this seam's
/// return type is `()`, not a `Result`.
pub(crate) async fn evaluate_alerts(ctx: &IngestCtx, emission: &Emission) {
    let rules = match AlertRuleRepo::list(&ctx.pool).await {
        Ok(rules) => rules,
        Err(_) => return,
    };
    if rules.is_empty() {
        return;
    }

    let Some(emitter_id) = emission.emitter_id else {
        return;
    };
    let Some(emitter) = EmitterRepo::get(&ctx.pool, emitter_id)
        .await
        .unwrap_or(None)
    else {
        return;
    };
    let entity_name = match emitter.entity_id {
        Some(entity_id) => EntityRepo::get(&ctx.pool, entity_id)
            .await
            .unwrap_or(None)
            .map(|e| e.name),
        None => None,
    };

    for rule in rules {
        if !rule.enabled {
            continue;
        }

        let Ok(trigger) = serde_json::from_value::<Trigger>(rule.trigger.clone()) else {
            continue;
        };
        if trigger.on != "detected" {
            continue;
        }

        let target_matches = match rule.target_type.as_deref() {
            Some("emitter") => rule.target_id == Some(emitter.id),
            Some("entity") => rule.target_id.is_some() && rule.target_id == emitter.entity_id,
            _ => false,
        };
        if !target_matches {
            continue;
        }

        if let Some(content_rule) = &trigger.content_match {
            // `fluxfang_core::rule::eval` is pure, in-memory structural JSON
            // matching -- never JS/Python `eval` (see that function's own
            // doc comment's explicit NOTE).
            if !eval(content_rule, &emission.payload) {
                continue;
            }
        }

        let payload = detected_payload(&rule, &emitter, entity_name.as_deref(), emission);
        fire_rule(ctx, &rule, &payload).await;
    }
}

/// Build the [`NotificationPayload`] describing one `detected`-trigger
/// match: a human-readable title/body plus a structured `context` (rule,
/// emission, emitter, and entity identifiers/names, and the emission's raw
/// payload) for consumers — e.g. a webhook receiver — that want more than
/// the prose.
fn detected_payload(
    rule: &AlertRule,
    emitter: &Emitter,
    entity_name: Option<&str>,
    emission: &Emission,
) -> NotificationPayload {
    let title = format!("Emitter \"{}\" detected", emitter.name);
    let body = match entity_name {
        Some(name) => format!(
            "Emitter \"{}\" (entity \"{name}\") detected at {} -- rule \"{}\"",
            emitter.name, emission.observed_at, rule.name
        ),
        None => format!(
            "Emitter \"{}\" detected at {} -- rule \"{}\"",
            emitter.name, emission.observed_at, rule.name
        ),
    };

    NotificationPayload {
        title,
        body,
        context: serde_json::json!({
            "rule_id": rule.id,
            "rule_name": rule.name,
            "emission_id": emission.id,
            "emitter_id": emitter.id,
            "emitter_name": emitter.name,
            "entity_id": emitter.entity_id,
            "entity_name": entity_name,
            "observed_at": emission.observed_at,
            "payload": emission.payload,
        }),
    }
}

/// Fire one already-matched `rule`'s notification through every linked
/// `alert_method`: dispatch delivery, persist a `notification` row, and
/// broadcast `Event::Notification` for each row stored.
///
/// **Shared/reusable by design**: any caller that has already decided a
/// rule fired -- this module's own [`evaluate_alerts`] for a `detected`
/// trigger, Task 5.4's zone-transition evaluation -- hands it the rule and a
/// rendered [`NotificationPayload`] and gets identical dispatch +
/// persistence + broadcast behavior, with no knowledge of *why* the rule
/// fired baked into this function.
///
/// `notification.payload` is the notification content (`title`/`body`/
/// `context`) as JSON, with a `failure_reason` key added when `dispatch`
/// returns [`DeliveryStatus::Failed`] -- the `delivery_status` column itself
/// has no room for the reason (`CHECK`-constrained to `'pending' | 'sent' |
/// 'failed'`; see [`DeliveryStatus::as_db_str`]'s own doc comment), so it's
/// folded into the JSON payload instead.
///
/// **Disabled methods are skipped entirely**: a linked `alert_method` with
/// `enabled == false` gets no dispatch attempt, no `notification` row, and
/// no broadcast -- as if it weren't linked at all for this firing.
///
/// **Never panics; a failure for one method never aborts the others**: a
/// `dispatch` failure records `DeliveryStatus::Failed` and the loop
/// continues to the next method; a DB error loading the linked methods, or
/// inserting one method's `notification` row, is swallowed (there's no
/// tracing/logging crate wired into this workspace yet -- same gap Task
/// 5.2's report already flagged for its own auto-attach path) rather than
/// propagated up to `ingest`.
pub(crate) async fn fire_rule(ctx: &IngestCtx, rule: &AlertRule, payload: &NotificationPayload) {
    let methods = match AlertRuleRepo::methods_for_rule(&ctx.pool, rule.id).await {
        Ok(methods) => methods,
        Err(_) => return,
    };

    for method in methods {
        // A disabled `alert_method` must be skipped entirely, not just
        // "dispatched but doesn't matter" -- no dispatch attempt, no
        // `notification` row, no broadcast. An operator disabling a channel
        // (e.g. taking a broken webhook offline) must actually stop it from
        // firing, not merely stop it from being *linked* (which would
        // require editing every rule that references it).
        if !method.enabled {
            continue;
        }

        let status = dispatch(&method, &ctx.secret_key, payload).await;
        let stored_payload = notification_payload_json(payload, &status);

        let new = NewNotification {
            alert_rule_id: Some(rule.id),
            alert_method_id: Some(method.id),
            fired_at: Utc::now(),
            payload: stored_payload,
            delivery_status: status.as_db_str().to_string(),
        };

        match NotificationRepo::insert(&ctx.pool, new).await {
            Ok(stored) => {
                // No subscribers is a normal, expected state (see
                // `ingest`'s own `Event::Emission` broadcast for the same
                // reasoning) -- deliberately not treated as a failure.
                let _ = ctx.events.send(Event::Notification(stored));
            }
            // A failed insert for this one method must not stop the rest.
            Err(_) => continue,
        }
    }
}

/// Render `payload`/`status` into the JSON stored in `notification.payload`
/// -- see [`fire_rule`]'s doc comment for why the failure reason lives here
/// rather than in `delivery_status`.
fn notification_payload_json(
    payload: &NotificationPayload,
    status: &DeliveryStatus,
) -> serde_json::Value {
    let mut value = serde_json::to_value(payload).unwrap_or_else(|_| serde_json::json!({}));
    if let DeliveryStatus::Failed(reason) = status {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("failure_reason".to_string(), serde_json::json!(reason));
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc as ChronoUtc};
    use fluxfang_capture::{GpsFix, GpsSource, RawObservation};
    use fluxfang_core::secrets::encrypt;
    use fluxfang_db::models::{NewAlertMethod, NewAlertRule, NewDataSource, NewEmitter, NewEntity};
    use fluxfang_db::{AlertMethodRepo, DataSourceRepo, EntityRepo};
    use sqlx::PgPool;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::broadcast;
    use tokio::sync::mpsc;

    use super::super::session::{no_op_hook, SessionManager, SessionManagerConfig};
    use crate::test_support::fresh_pool;

    /// See `ingest::tests::ChannelGps` (mod.rs) for the full rationale --
    /// duplicated here since that module's test-only helpers are private to
    /// it, same convention `ingest::tests` itself already follows relative
    /// to `session::tests`.
    struct ChannelGps(mpsc::UnboundedReceiver<GpsFix>);

    #[async_trait]
    impl GpsSource for ChannelGps {
        async fn next_fix(&mut self) -> Option<GpsFix> {
            self.0.recv().await
        }
    }

    fn test_key() -> [u8; 32] {
        [0x11u8; 32]
    }

    async fn seed_wifi_source(pool: &PgPool) -> Uuid {
        DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0"))
            .await
            .expect("seed wifi data_source")
            .id
    }

    /// Open a `SessionManager` on a `ChannelGps`, send exactly one `fix`,
    /// and poll (bounded) until `latest_fix()` reflects it -- identical
    /// rationale to `ingest::tests::session_with_fix`.
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

    fn a_fix(at: chrono::DateTime<ChronoUtc>) -> GpsFix {
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

    fn wifi_obs(
        bssid: &str,
        channel: i64,
        observed_at: chrono::DateTime<ChronoUtc>,
    ) -> RawObservation {
        RawObservation {
            kind: "wifi".to_string(),
            observed_at,
            signal_strength: Some(-55),
            payload: serde_json::json!({"bssid": bssid, "channel": channel}),
        }
    }

    async fn full_ctx(
        pool: PgPool,
        manager: SessionManager,
    ) -> (IngestCtx, broadcast::Receiver<Event>) {
        let (events_tx, events_rx) = broadcast::channel(16);
        let ctx = IngestCtx {
            pool,
            sessions: Arc::new(manager),
            events: events_tx,
            secret_key: test_key(),
        };
        (ctx, events_rx)
    }

    #[tokio::test]
    async fn detected_rule_on_matching_entity_with_one_in_app_method_fires_exactly_one_notification(
    ) {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = ChronoUtc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, _tx) = session_with_fix(pool.clone(), a_fix(base)).await;

        let entity = EntityRepo::insert(
            &pool,
            NewEntity {
                name: "Bob's Phone".to_string(),
                notes: None,
            },
        )
        .await
        .unwrap();

        let emitter = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "Bob's AP".to_string(),
                type_: Some("wifi".to_string()),
                entity_id: Some(entity.id),
                match_criteria: serde_json::json!({
                    "match": "all",
                    "conditions": [{"field": "bssid", "op": "eq", "value": "aa:bb:cc:dd:ee:ff"}]
                }),
            },
        )
        .await
        .unwrap();

        let rule = AlertRuleRepo::insert(
            &pool,
            NewAlertRule {
                name: "Bob detected".to_string(),
                enabled: true,
                target_type: Some("entity".to_string()),
                target_id: Some(entity.id),
                trigger: serde_json::json!({"on": "detected"}),
            },
        )
        .await
        .unwrap();

        let method = AlertMethodRepo::insert(
            &pool,
            NewAlertMethod {
                name: "in-app".to_string(),
                type_: "in_app".to_string(),
                enabled: true,
                config_encrypted: vec![],
            },
        )
        .await
        .unwrap();

        AlertRuleRepo::link_method(&pool, rule.id, method.id)
            .await
            .unwrap();

        let (ctx, mut events_rx) = full_ctx(pool.clone(), manager).await;

        let emission = super::super::ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", 6, base))
            .await
            .expect("ingest should succeed");
        assert_eq!(emission.emitter_id, Some(emitter.id));

        let (rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(total, 1, "exactly one notification row should exist");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].alert_rule_id, Some(rule.id));
        assert_eq!(rows[0].alert_method_id, Some(method.id));
        assert_eq!(rows[0].delivery_status, "sent");

        // Ordering: alerts are evaluated (and broadcast) before ingest's own
        // Event::Emission broadcast -- see this file's + `ingest`'s module
        // docs.
        let first = events_rx.try_recv().expect("expected a broadcast event");
        match first {
            Event::Notification(n) => assert_eq!(n.id, rows[0].id),
            Event::Emission(_) => panic!("expected Event::Notification first"),
        }
        let second = events_rx
            .try_recv()
            .expect("expected a second broadcast event");
        match second {
            Event::Emission(e) => assert_eq!(e.id, emission.id),
            Event::Notification(_) => panic!("expected Event::Emission second"),
        }
    }

    #[tokio::test]
    async fn detected_rule_with_content_match_only_fires_when_channel_matches() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = ChronoUtc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, _tx) = session_with_fix(pool.clone(), a_fix(base)).await;

        let entity = EntityRepo::insert(
            &pool,
            NewEntity {
                name: "Bob's Phone".to_string(),
                notes: None,
            },
        )
        .await
        .unwrap();

        EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "Bob's AP".to_string(),
                type_: Some("wifi".to_string()),
                entity_id: Some(entity.id),
                match_criteria: serde_json::json!({
                    "match": "all",
                    "conditions": [{"field": "bssid", "op": "eq", "value": "aa:bb:cc:dd:ee:ff"}]
                }),
            },
        )
        .await
        .unwrap();

        let rule = AlertRuleRepo::insert(
            &pool,
            NewAlertRule {
                name: "channel 6 only".to_string(),
                enabled: true,
                target_type: Some("entity".to_string()),
                target_id: Some(entity.id),
                trigger: serde_json::json!({
                    "on": "detected",
                    "content_match": {
                        "match": "all",
                        "conditions": [{"field": "channel", "op": "eq", "value": 6}]
                    }
                }),
            },
        )
        .await
        .unwrap();

        let method = AlertMethodRepo::insert(
            &pool,
            NewAlertMethod {
                name: "in-app".to_string(),
                type_: "in_app".to_string(),
                enabled: true,
                config_encrypted: vec![],
            },
        )
        .await
        .unwrap();
        AlertRuleRepo::link_method(&pool, rule.id, method.id)
            .await
            .unwrap();

        let (ctx, _events_rx) = full_ctx(pool.clone(), manager).await;

        // channel = 1 must NOT fire.
        super::super::ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", 1, base))
            .await
            .expect("ingest should succeed");
        let (_rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(
            total, 0,
            "channel=1 must not match content_match channel eq 6"
        );

        // channel = 6 must fire.
        super::super::ingest(
            &ctx,
            ds,
            wifi_obs("aa:bb:cc:dd:ee:ff", 6, base + chrono::Duration::seconds(1)),
        )
        .await
        .expect("ingest should succeed");
        let (_rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(total, 1, "channel=6 must match content_match channel eq 6");
    }

    #[tokio::test]
    async fn rule_linked_to_two_methods_produces_two_notification_rows() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = ChronoUtc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, _tx) = session_with_fix(pool.clone(), a_fix(base)).await;

        let emitter = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "AP".to_string(),
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

        let rule = AlertRuleRepo::insert(
            &pool,
            NewAlertRule {
                name: "AP detected".to_string(),
                enabled: true,
                target_type: Some("emitter".to_string()),
                target_id: Some(emitter.id),
                trigger: serde_json::json!({"on": "detected"}),
            },
        )
        .await
        .unwrap();

        let method_a = AlertMethodRepo::insert(
            &pool,
            NewAlertMethod {
                name: "in-app A".to_string(),
                type_: "in_app".to_string(),
                enabled: true,
                config_encrypted: vec![],
            },
        )
        .await
        .unwrap();
        let method_b = AlertMethodRepo::insert(
            &pool,
            NewAlertMethod {
                name: "in-app B".to_string(),
                type_: "in_app".to_string(),
                enabled: true,
                config_encrypted: vec![],
            },
        )
        .await
        .unwrap();
        AlertRuleRepo::link_method(&pool, rule.id, method_a.id)
            .await
            .unwrap();
        AlertRuleRepo::link_method(&pool, rule.id, method_b.id)
            .await
            .unwrap();

        let (ctx, _events_rx) = full_ctx(pool.clone(), manager).await;

        super::super::ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", 6, base))
            .await
            .expect("ingest should succeed");

        let (rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(total, 2, "one notification per linked method");
        let method_ids: std::collections::HashSet<_> =
            rows.iter().map(|r| r.alert_method_id).collect();
        assert_eq!(
            method_ids,
            std::collections::HashSet::from([Some(method_a.id), Some(method_b.id)])
        );
    }

    #[tokio::test]
    async fn a_failing_method_is_recorded_failed_without_aborting_other_rules_or_methods() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = ChronoUtc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, _tx) = session_with_fix(pool.clone(), a_fix(base)).await;

        let emitter = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "AP".to_string(),
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

        // Rule A: linked ONLY to a webhook method pointed at a closed port
        // (connection refused quickly, well inside the dispatcher's 10s
        // timeout) -- its one method must fail.
        let rule_a = AlertRuleRepo::insert(
            &pool,
            NewAlertRule {
                name: "rule A (fails)".to_string(),
                enabled: true,
                target_type: Some("emitter".to_string()),
                target_id: Some(emitter.id),
                trigger: serde_json::json!({"on": "detected"}),
            },
        )
        .await
        .unwrap();
        let webhook_config = serde_json::json!({"url": "http://127.0.0.1:1/hook"});
        let ciphertext = encrypt(&test_key(), webhook_config.to_string().as_bytes());
        let failing_method = AlertMethodRepo::insert(
            &pool,
            NewAlertMethod {
                name: "unreachable webhook".to_string(),
                type_: "webhook".to_string(),
                enabled: true,
                config_encrypted: ciphertext,
            },
        )
        .await
        .unwrap();
        AlertRuleRepo::link_method(&pool, rule_a.id, failing_method.id)
            .await
            .unwrap();

        // Rule B: a completely separate rule/method pair, proving rule A's
        // failure doesn't stop the rest of `evaluate_alerts` from running.
        let rule_b = AlertRuleRepo::insert(
            &pool,
            NewAlertRule {
                name: "rule B (succeeds)".to_string(),
                enabled: true,
                target_type: Some("emitter".to_string()),
                target_id: Some(emitter.id),
                trigger: serde_json::json!({"on": "detected"}),
            },
        )
        .await
        .unwrap();
        let ok_method = AlertMethodRepo::insert(
            &pool,
            NewAlertMethod {
                name: "in-app".to_string(),
                type_: "in_app".to_string(),
                enabled: true,
                config_encrypted: vec![],
            },
        )
        .await
        .unwrap();
        AlertRuleRepo::link_method(&pool, rule_b.id, ok_method.id)
            .await
            .unwrap();

        let (ctx, _events_rx) = full_ctx(pool.clone(), manager).await;

        let result = tokio::time::timeout(
            Duration::from_secs(15),
            super::super::ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", 6, base)),
        )
        .await
        .expect("ingest must not hang");
        result.expect("ingest should succeed even though one alert method fails to dispatch");

        let (rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(total, 2, "both rules should have recorded a notification");

        let failed = rows
            .iter()
            .find(|r| r.alert_rule_id == Some(rule_a.id))
            .expect("rule A's notification should exist");
        assert_eq!(failed.delivery_status, "failed");
        assert!(
            failed.payload.get("failure_reason").is_some(),
            "failed delivery should record a failure_reason in the payload"
        );

        let ok = rows
            .iter()
            .find(|r| r.alert_rule_id == Some(rule_b.id))
            .expect("rule B's notification should exist");
        assert_eq!(ok.delivery_status, "sent");
    }

    /// Carried-over fix (this task's brief): `fire_rule` must skip a
    /// disabled `alert_method` entirely -- one rule linked to both an
    /// enabled and a disabled `in_app` method must produce exactly one
    /// notification (the enabled one's), not two.
    #[tokio::test]
    async fn disabled_method_is_skipped_and_only_the_enabled_method_fires() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = ChronoUtc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, _tx) = session_with_fix(pool.clone(), a_fix(base)).await;

        let emitter = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "AP".to_string(),
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

        let rule = AlertRuleRepo::insert(
            &pool,
            NewAlertRule {
                name: "AP detected".to_string(),
                enabled: true,
                target_type: Some("emitter".to_string()),
                target_id: Some(emitter.id),
                trigger: serde_json::json!({"on": "detected"}),
            },
        )
        .await
        .unwrap();

        let enabled_method = AlertMethodRepo::insert(
            &pool,
            NewAlertMethod {
                name: "enabled in-app".to_string(),
                type_: "in_app".to_string(),
                enabled: true,
                config_encrypted: vec![],
            },
        )
        .await
        .unwrap();
        let disabled_method = AlertMethodRepo::insert(
            &pool,
            NewAlertMethod {
                name: "disabled in-app".to_string(),
                type_: "in_app".to_string(),
                enabled: false,
                config_encrypted: vec![],
            },
        )
        .await
        .unwrap();
        AlertRuleRepo::link_method(&pool, rule.id, enabled_method.id)
            .await
            .unwrap();
        AlertRuleRepo::link_method(&pool, rule.id, disabled_method.id)
            .await
            .unwrap();

        let (ctx, _events_rx) = full_ctx(pool.clone(), manager).await;

        super::super::ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", 6, base))
            .await
            .expect("ingest should succeed");

        let (rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(
            total, 1,
            "exactly one notification: the disabled method must not fire"
        );
        assert_eq!(rows[0].alert_method_id, Some(enabled_method.id));
    }

    /// Within a SINGLE rule linked to two methods -- one that fails to
    /// dispatch (unreachable webhook) and one that succeeds (in_app) --
    /// both `notification` rows must be recorded (failed + sent); the
    /// failing method must not abort or skip the other. This differs from
    /// `a_failing_method_is_recorded_failed_without_aborting_other_rules_or_methods`
    /// above, which spreads the failing/succeeding methods across two
    /// separate rules.
    #[tokio::test]
    async fn one_rule_with_one_failing_and_one_succeeding_method_records_both() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = ChronoUtc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, _tx) = session_with_fix(pool.clone(), a_fix(base)).await;

        let emitter = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "AP".to_string(),
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

        let rule = AlertRuleRepo::insert(
            &pool,
            NewAlertRule {
                name: "AP detected".to_string(),
                enabled: true,
                target_type: Some("emitter".to_string()),
                target_id: Some(emitter.id),
                trigger: serde_json::json!({"on": "detected"}),
            },
        )
        .await
        .unwrap();

        let webhook_config = serde_json::json!({"url": "http://127.0.0.1:1/hook"});
        let ciphertext = encrypt(&test_key(), webhook_config.to_string().as_bytes());
        let failing_method = AlertMethodRepo::insert(
            &pool,
            NewAlertMethod {
                name: "unreachable webhook".to_string(),
                type_: "webhook".to_string(),
                enabled: true,
                config_encrypted: ciphertext,
            },
        )
        .await
        .unwrap();
        let ok_method = AlertMethodRepo::insert(
            &pool,
            NewAlertMethod {
                name: "in-app".to_string(),
                type_: "in_app".to_string(),
                enabled: true,
                config_encrypted: vec![],
            },
        )
        .await
        .unwrap();
        AlertRuleRepo::link_method(&pool, rule.id, failing_method.id)
            .await
            .unwrap();
        AlertRuleRepo::link_method(&pool, rule.id, ok_method.id)
            .await
            .unwrap();

        let (ctx, _events_rx) = full_ctx(pool.clone(), manager).await;

        let result = tokio::time::timeout(
            Duration::from_secs(15),
            super::super::ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", 6, base)),
        )
        .await
        .expect("ingest must not hang");
        result.expect("ingest should succeed even though one method fails to dispatch");

        let (rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(
            total, 2,
            "both methods of the one rule should have recorded a notification"
        );

        let failed = rows
            .iter()
            .find(|r| r.alert_method_id == Some(failing_method.id))
            .expect("failing method's notification should exist");
        assert_eq!(failed.delivery_status, "failed");

        let ok = rows
            .iter()
            .find(|r| r.alert_method_id == Some(ok_method.id))
            .expect("ok method's notification should exist");
        assert_eq!(ok.delivery_status, "sent");
    }

    /// A host rule (`target_type`/`target_id` both `None`) with
    /// `trigger.on == "detected"` must never fire via `evaluate_alerts` --
    /// `detected` triggers only ever match an `emitter`/`entity` target
    /// (see this module's own doc comment); a host rule with that trigger
    /// kind is simply never matched by any emission.
    #[tokio::test]
    async fn host_rule_with_detected_trigger_never_fires() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = ChronoUtc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, _tx) = session_with_fix(pool.clone(), a_fix(base)).await;

        EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "AP".to_string(),
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

        let rule = AlertRuleRepo::insert(
            &pool,
            NewAlertRule {
                name: "host rule with a detected trigger (nonsensical, must never fire)"
                    .to_string(),
                enabled: true,
                target_type: None,
                target_id: None,
                trigger: serde_json::json!({"on": "detected"}),
            },
        )
        .await
        .unwrap();

        let method = AlertMethodRepo::insert(
            &pool,
            NewAlertMethod {
                name: "in-app".to_string(),
                type_: "in_app".to_string(),
                enabled: true,
                config_encrypted: vec![],
            },
        )
        .await
        .unwrap();
        AlertRuleRepo::link_method(&pool, rule.id, method.id)
            .await
            .unwrap();

        let (ctx, _events_rx) = full_ctx(pool.clone(), manager).await;

        super::super::ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", 6, base))
            .await
            .expect("ingest should succeed");

        let (_rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(total, 0, "a host rule's detected trigger must never fire");
    }
}
