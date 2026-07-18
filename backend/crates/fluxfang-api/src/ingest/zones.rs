//! Zone-membership tracking + zone-transition alerts (Task 5.4).
//!
//! Zones are user-drawn geofences (`zone`, a point + radius). This module
//! answers "did some subject just cross a zone boundary?" for three kinds
//! of subject:
//!
//! - an **emitter** (a specific detected source, e.g. one access point),
//! - an **entity** (the real-world thing an emitter is grouped under), and
//! - the singular **host** (the machine doing the surveying, tracked via
//!   its own logged `location_fix` trail).
//!
//! [`update_subject_zones`] is the one shared core all three funnel
//! through: given a subject's current point, it recomputes membership
//! against every zone in one query
//! (`ZoneRepo::memberships_for_point`), diffs each zone's result against
//! the subject's last-known state (`zone_membership`, via
//! [`fluxfang_db::ZoneMembershipRepo`]), and on a change, persists the new
//! state and fires any matching `enters_zone`/`leaves_zone` (or
//! `host_enters_zone`/`host_leaves_zone`) `alert_rule` through
//! [`super::alerts::fire_rule`] -- the exact same dispatch/persist/broadcast
//! path Task 5.3's `detected`-trigger evaluation uses, so a zone-transition
//! notification is indistinguishable in shape from a `detected` one, just
//! built from a different [`crate::notify::NotificationPayload`].
//!
//! ## Once per transition, not once per emission
//!
//! Firing depends entirely on `now_inside != was_inside`. A second located
//! emission (or fix) that lands in the same state as the last-persisted
//! `zone_membership` row is a no-op: the diff is `false`, so neither the
//! upsert nor any rule firing happens. This is what makes "one notification
//! per crossing" hold regardless of how many emissions/fixes arrive while
//! a subject stays put on one side of the boundary.
//!
//! ## Entry points
//!
//! - [`update_target_zones`]: called from [`super::ingest`]'s Task 5.4 seam
//!   for every finalized emission. Requires both a location and an
//!   `emitter_id` (an unlocated or unattached emission has no zone
//!   membership to evaluate) -- updates the emitter subject, and, if that
//!   emitter belongs to an entity, the entity subject too (using the same
//!   emission's location as that entity's position).
//! - [`update_host_zones`]: evaluates the `host` subject against one
//!   `GpsFix`. Task 5.1's `SessionManager` `HostZoneHook` is exactly this
//!   function's shape; wiring the *production* hook to call it is Task
//!   6.2's start-capture (see that hook's own doc comment) -- this task
//!   only supplies the function and tests it directly.
//!
//! ## Self-contained errors
//!
//! Like [`super::alerts::evaluate_alerts`], every function here returns
//! `()` and never panics: a DB error at any step (loading memberships,
//! loading/upserting the prior state, loading rules, loading the zone/
//! subject name for the notification body) makes that one step's zone
//! evaluation a no-op rather than propagating up to `ingest` or the
//! `SessionManager`'s ingest loop -- a problem evaluating zone transitions
//! must never prevent the emission (or location fix) itself from finishing
//! its own persistence/broadcast.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use fluxfang_capture::GpsFix;
use fluxfang_db::models::{AlertRule, Emission, Zone};
use fluxfang_db::{AlertRuleRepo, EmitterRepo, EntityRepo, ZoneMembershipRepo, ZoneRepo};

use crate::notify::NotificationPayload;

use super::alerts::fire_rule;
use super::IngestCtx;

/// The subset of `alert_rule.trigger`'s JSON shape this module reads:
/// `{"on": "...", "zone_id": <uuid?>}`. A separate, smaller struct than
/// `alerts::Trigger` (that one also reads `content_match`, which no
/// zone-transition trigger uses) -- both simply deserialize whatever subset
/// of the same `trigger` JSON they care about; unread fields are ignored by
/// serde, not an error.
#[derive(Debug, Clone, Deserialize)]
struct ZoneTrigger {
    on: String,
    #[serde(default)]
    zone_id: Option<Uuid>,
}

/// The `trigger.on` value that fires when `subject_type` transitions to
/// `now_inside`. `"host"` uses the `host_*` variants; `"emitter"`/`"entity"`
/// (or anything else) share the plain `enters_zone`/`leaves_zone` variants,
/// distinguished from each other by `target_type` in [`target_matches`],
/// not by the trigger name itself (see [`AlertRule`]'s own doc comment for
/// the full `trigger.on` enumeration).
fn trigger_on_for(subject_type: &str, now_inside: bool) -> &'static str {
    match (subject_type, now_inside) {
        ("host", true) => "host_enters_zone",
        ("host", false) => "host_leaves_zone",
        (_, true) => "enters_zone",
        (_, false) => "leaves_zone",
    }
}

/// Does `rule` target this exact subject? Host rules have `target_type`/
/// `target_id` both `NULL` (there's only one host, so no id is needed);
/// emitter/entity rules must match both the subject class (`target_type`)
/// and the specific id (`target_id`).
fn target_matches(rule: &AlertRule, subject_type: &str, subject_id: Option<Uuid>) -> bool {
    if subject_type == "host" {
        rule.target_type.is_none() && rule.target_id.is_none()
    } else {
        rule.target_type.as_deref() == Some(subject_type) && rule.target_id == subject_id
    }
}

/// Best-effort human-readable name for a subject, for the notification
/// body -- `None` (falls back to a generic description) rather than
/// aborting zone evaluation if the lookup fails or the subject has no
/// name (the host has no row to look up at all).
async fn subject_display_name(
    ctx: &IngestCtx,
    subject_type: &str,
    subject_id: Option<Uuid>,
) -> Option<String> {
    let id = subject_id?;
    match subject_type {
        "emitter" => EmitterRepo::get(&ctx.pool, id)
            .await
            .ok()
            .flatten()
            .map(|e| e.name),
        "entity" => EntityRepo::get(&ctx.pool, id)
            .await
            .ok()
            .flatten()
            .map(|e| e.name),
        _ => None,
    }
}

/// Render the `NotificationPayload` describing one zone transition.
fn transition_payload(
    rule: &AlertRule,
    zone: &Zone,
    subject_type: &str,
    subject_id: Option<Uuid>,
    subject_name: Option<&str>,
    now_inside: bool,
    at: DateTime<Utc>,
) -> NotificationPayload {
    let verb = if now_inside { "entered" } else { "left" };
    let subject_label = match subject_type {
        "host" => "The host".to_string(),
        other => match subject_name {
            Some(name) => format!("{other} \"{name}\""),
            None => format!("{other} {subject_id:?}"),
        },
    };

    let title = format!("{subject_label} {verb} zone \"{}\"", zone.name);
    let body = format!(
        "{subject_label} {verb} zone \"{}\" at {at} -- rule \"{}\"",
        zone.name, rule.name
    );

    NotificationPayload {
        title,
        body,
        context: serde_json::json!({
            "rule_id": rule.id,
            "rule_name": rule.name,
            "zone_id": zone.id,
            "zone_name": zone.name,
            "subject_type": subject_type,
            "subject_id": subject_id,
            "inside": now_inside,
            "at": at,
        }),
    }
}

/// The shared core: recompute `(subject_type, subject_id)`'s membership
/// against every zone at `point`, diff each zone's result against its
/// last-known `zone_membership` state, and on a change, persist the new
/// state and fire any matching zone-transition `alert_rule`s. See module
/// docs for the full "once per transition" rationale and the
/// self-containment guarantee.
///
/// `subject_id` is `None` for `subject_type == "host"` (there is only one
/// host); `Some(id)` for `"emitter"`/`"entity"`.
pub(crate) async fn update_subject_zones(
    ctx: &IngestCtx,
    subject_type: &str,
    subject_id: Option<Uuid>,
    point: (f64, f64),
    at: DateTime<Utc>,
) {
    let memberships = match ZoneRepo::memberships_for_point(&ctx.pool, point.0, point.1).await {
        Ok(m) => m,
        Err(_) => return,
    };
    if memberships.is_empty() {
        return;
    }

    // Loaded once per call, reused across every zone this subject is
    // diffed against -- same cost trade-off `alerts::evaluate_alerts`
    // already documents for its own `AlertRuleRepo::list` call.
    let rules = AlertRuleRepo::list(&ctx.pool).await.unwrap_or_default();
    let subject_name = subject_display_name(ctx, subject_type, subject_id).await;

    for (zone_id, now_inside) in memberships {
        let was_inside = ZoneMembershipRepo::get(&ctx.pool, subject_type, subject_id, zone_id)
            .await
            .ok()
            .flatten()
            .map(|m| m.inside)
            .unwrap_or(false);

        // No change: not a transition, so no upsert and no firing -- this
        // is exactly what makes a second still-inside emission a no-op.
        if now_inside == was_inside {
            continue;
        }

        if ZoneMembershipRepo::upsert(&ctx.pool, subject_type, subject_id, zone_id, now_inside, at)
            .await
            .is_err()
        {
            continue;
        }

        let Ok(Some(zone)) = ZoneRepo::get(&ctx.pool, zone_id).await else {
            continue;
        };

        let trigger_on = trigger_on_for(subject_type, now_inside);

        for rule in &rules {
            if !rule.enabled {
                continue;
            }
            let Ok(trigger) = serde_json::from_value::<ZoneTrigger>(rule.trigger.clone()) else {
                continue;
            };
            if trigger.on != trigger_on {
                continue;
            }
            if trigger.zone_id != Some(zone_id) {
                continue;
            }
            if !target_matches(rule, subject_type, subject_id) {
                continue;
            }

            let payload = transition_payload(
                rule,
                &zone,
                subject_type,
                subject_id,
                subject_name.as_deref(),
                now_inside,
                at,
            );
            fire_rule(ctx, rule, &payload).await;
        }
    }
}

/// Zone-transition evaluation for one finalized [`Emission`] -- the Task
/// 5.4 seam called from [`super::ingest`], after auto-attach has possibly
/// set `emission.emitter_id`.
///
/// A no-op if the emission has no location, or no `emitter_id` (auto-attach
/// found no match): neither carries enough information to evaluate any
/// zone membership. Otherwise evaluates the **emitter** subject at the
/// emission's location, and, if that emitter belongs to an entity, the
/// **entity** subject too (an entity's position is simply its emitter's
/// emission's location -- there's no separate entity-level location
/// concept in this schema).
pub(crate) async fn update_target_zones(ctx: &IngestCtx, emission: &Emission) {
    let (Some(lon), Some(lat)) = (emission.lon, emission.lat) else {
        return;
    };
    let Some(emitter_id) = emission.emitter_id else {
        return;
    };

    update_subject_zones(
        ctx,
        "emitter",
        Some(emitter_id),
        (lon, lat),
        emission.observed_at,
    )
    .await;

    if let Ok(Some(emitter)) = EmitterRepo::get(&ctx.pool, emitter_id).await {
        if let Some(entity_id) = emitter.entity_id {
            update_subject_zones(
                ctx,
                "entity",
                Some(entity_id),
                (lon, lat),
                emission.observed_at,
            )
            .await;
        }
    }
}

/// Zone-transition evaluation for the `host` subject, given one `GpsFix`
/// (the host's own current position). Task 5.1's `SessionManager`
/// `HostZoneHook` is exactly this shape; Task 6.2's start-capture is what
/// wires the production hook to call this function on every
/// `location_fix` actually written (see this crate's `ingest::session`
/// module docs) -- this task tests it directly instead.
pub async fn update_host_zones(ctx: &IngestCtx, fix: &GpsFix) {
    update_subject_zones(ctx, "host", None, (fix.lon, fix.lat), fix.at).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use fluxfang_capture::RawObservation;
    use fluxfang_db::models::{
        NewAlertMethod, NewAlertRule, NewDataSource, NewEmitter, NewEntity, NewZone,
    };
    use fluxfang_db::{AlertMethodRepo, DataSourceRepo, EntityRepo, NotificationRepo};
    use sqlx::PgPool;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    use crate::ingest::location::LocationProvider;
    use crate::ingest::session::SessionManager;
    use crate::ingest::Event;
    use crate::test_support::fresh_pool;

    fn test_key() -> [u8; 32] {
        [0x11u8; 32]
    }

    async fn seed_wifi_source(pool: &PgPool) -> Uuid {
        DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0"))
            .await
            .expect("seed wifi data_source")
            .id
    }

    /// Zone center: roughly downtown San Francisco.
    const CENTER: (f64, f64) = (-122.4194, 37.7749);
    /// Same point as `CENTER` -- always inside any positive-radius zone.
    const INSIDE: (f64, f64) = CENTER;
    /// Roughly Manhattan -- thousands of km from `CENTER`, always outside.
    const OUTSIDE: (f64, f64) = (-73.9857, 40.7484);
    const RADIUS_M: f64 = 1000.0;

    async fn seed_zone(pool: &PgPool) -> Uuid {
        ZoneRepo::insert(
            pool,
            NewZone {
                name: "Test Zone".to_string(),
                center: CENTER,
                radius_m: RADIUS_M,
                notes: None,
            },
        )
        .await
        .unwrap()
        .id
    }

    /// Open a fresh session and a `LocationProvider` pre-loaded with `fix`
    /// (what the pump would feed at runtime). The returned provider is handed
    /// to `full_ctx`; a test that needs the host's position to *move* just
    /// calls `provider.update(new_fix)` (synchronous — no channel/poll).
    async fn session_with_fix(pool: PgPool, fix: GpsFix) -> (SessionManager, Arc<LocationProvider>) {
        let manager = SessionManager::open(pool)
            .await
            .expect("open SessionManager");
        let provider = Arc::new(LocationProvider::new());
        provider.update(fix);
        (manager, provider)
    }

    fn a_fix(at: chrono::DateTime<Utc>, point: (f64, f64)) -> GpsFix {
        GpsFix {
            at,
            lon: point.0,
            lat: point.1,
            altitude: None,
            speed: None,
            heading: None,
            quality: 1,
        }
    }

    fn wifi_obs(bssid: &str, observed_at: chrono::DateTime<Utc>) -> RawObservation {
        RawObservation {
            kind: "wifi".to_string(),
            observed_at,
            signal_strength: Some(-55),
            payload: serde_json::json!({"bssid": bssid, "channel": 6}),
        }
    }

    async fn full_ctx(
        pool: PgPool,
        manager: SessionManager,
        location: Arc<LocationProvider>,
    ) -> (IngestCtx, broadcast::Receiver<Event>) {
        let (events_tx, events_rx) = broadcast::channel(16);
        let ctx = IngestCtx {
            pool,
            sessions: Some(Arc::new(manager)),
            location,
            events: events_tx,
            secret_key: test_key(),
        };
        (ctx, events_rx)
    }

    async fn seed_emitter_enter_rule(pool: &PgPool, zone_id: Uuid, emitter_id: Uuid) -> Uuid {
        let rule = AlertRuleRepo::insert(
            pool,
            NewAlertRule {
                name: "emitter enters zone".to_string(),
                enabled: true,
                target_type: Some("emitter".to_string()),
                target_id: Some(emitter_id),
                trigger: serde_json::json!({"on": "enters_zone", "zone_id": zone_id}),
            },
        )
        .await
        .unwrap();
        let method = AlertMethodRepo::insert(
            pool,
            NewAlertMethod {
                name: "in-app".to_string(),
                type_: "in_app".to_string(),
                enabled: true,
                config_encrypted: vec![],
            },
        )
        .await
        .unwrap();
        AlertRuleRepo::link_method(pool, rule.id, method.id)
            .await
            .unwrap();
        rule.id
    }

    async fn seed_emitter_leave_rule(pool: &PgPool, zone_id: Uuid, emitter_id: Uuid) -> Uuid {
        let rule = AlertRuleRepo::insert(
            pool,
            NewAlertRule {
                name: "emitter leaves zone".to_string(),
                enabled: true,
                target_type: Some("emitter".to_string()),
                target_id: Some(emitter_id),
                trigger: serde_json::json!({"on": "leaves_zone", "zone_id": zone_id}),
            },
        )
        .await
        .unwrap();
        let method = AlertMethodRepo::insert(
            pool,
            NewAlertMethod {
                name: "in-app".to_string(),
                type_: "in_app".to_string(),
                enabled: true,
                config_encrypted: vec![],
            },
        )
        .await
        .unwrap();
        AlertRuleRepo::link_method(pool, rule.id, method.id)
            .await
            .unwrap();
        rule.id
    }

    #[tokio::test]
    async fn first_located_emission_inside_fires_enters_zone_exactly_once() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base, INSIDE)).await;

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
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let zone_id = seed_zone(&pool).await;
        seed_emitter_enter_rule(&pool, zone_id, emitter.id).await;

        let (ctx, _events_rx) = full_ctx(pool.clone(), manager, provider).await;

        // First located emission, inside the zone -- exactly one
        // notification.
        super::super::ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", base))
            .await
            .expect("ingest should succeed");
        let (_rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(total, 1, "first located emission inside fires once");

        // A second emission, still inside -- state-diffed, so no further
        // notification (this is the "once per transition" property).
        super::super::ingest(
            &ctx,
            ds,
            wifi_obs("aa:bb:cc:dd:ee:ff", base + chrono::Duration::seconds(1)),
        )
        .await
        .expect("ingest should succeed");
        let (_rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(
            total, 1,
            "a second still-inside emission must not fire again"
        );
    }

    #[tokio::test]
    async fn located_emission_moving_outside_fires_leaves_zone_once() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base, INSIDE)).await;

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
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let zone_id = seed_zone(&pool).await;
        seed_emitter_enter_rule(&pool, zone_id, emitter.id).await;
        let leave_rule_id = seed_emitter_leave_rule(&pool, zone_id, emitter.id).await;

        let (ctx, _events_rx) = full_ctx(pool.clone(), manager, provider.clone()).await;

        // First emission: inside -- fires enters_zone (1 notification). An
        // emission's `location` always comes from the shared `LocationProvider`
        // (see `ingest`'s own doc comment), so to observe a transition to
        // OUTSIDE the provider's fix itself must move -- `provider.update`
        // does that synchronously (what the pump would do at runtime).
        super::super::ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", base))
            .await
            .expect("ingest should succeed");
        let (_rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(total, 1, "first located emission inside fires once");

        let outside_fix = a_fix(base + chrono::Duration::seconds(1), OUTSIDE);
        provider.update(outside_fix);

        super::super::ingest(
            &ctx,
            ds,
            wifi_obs("aa:bb:cc:dd:ee:ff", base + chrono::Duration::seconds(2)),
        )
        .await
        .expect("ingest should succeed");

        let (rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(
            total, 2,
            "one enters_zone notification + one leaves_zone notification total"
        );
        assert!(
            rows.iter().any(|r| r.alert_rule_id == Some(leave_rule_id)),
            "the leaves_zone rule should have fired exactly once"
        );
    }

    #[tokio::test]
    async fn entity_target_enters_zone_fires_when_its_emitters_emission_enters() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base, INSIDE)).await;

        let entity = EntityRepo::insert(
            &pool,
            NewEntity {
                name: "Bob's Phone".to_string(),
                notes: None,
                ..Default::default()
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
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let zone_id = seed_zone(&pool).await;
        let rule = AlertRuleRepo::insert(
            &pool,
            NewAlertRule {
                name: "entity enters zone".to_string(),
                enabled: true,
                target_type: Some("entity".to_string()),
                target_id: Some(entity.id),
                trigger: serde_json::json!({"on": "enters_zone", "zone_id": zone_id}),
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

        let (ctx, _events_rx) = full_ctx(pool.clone(), manager, provider).await;

        let emission = super::super::ingest(&ctx, ds, wifi_obs("aa:bb:cc:dd:ee:ff", base))
            .await
            .expect("ingest should succeed");
        assert_eq!(emission.emitter_id, Some(emitter.id));

        let (rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(
            total, 1,
            "the entity-target enters_zone rule should fire once"
        );
        assert_eq!(rows[0].alert_rule_id, Some(rule.id));
    }

    #[tokio::test]
    async fn host_enters_and_leaves_zone_fire_once_each_via_update_host_zones() {
        let pool = fresh_pool().await;
        let (events_tx, _events_rx) = broadcast::channel(16);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(
                SessionManager::open(pool.clone()).await.unwrap(),
            )),
            location: Arc::new(LocationProvider::new()),
            events: events_tx,
            secret_key: test_key(),
        };

        let zone_id = seed_zone(&pool).await;
        let enter_rule = AlertRuleRepo::insert(
            &pool,
            NewAlertRule {
                name: "host enters zone".to_string(),
                enabled: true,
                target_type: None,
                target_id: None,
                trigger: serde_json::json!({"on": "host_enters_zone", "zone_id": zone_id}),
            },
        )
        .await
        .unwrap();
        let leave_rule = AlertRuleRepo::insert(
            &pool,
            NewAlertRule {
                name: "host leaves zone".to_string(),
                enabled: true,
                target_type: None,
                target_id: None,
                trigger: serde_json::json!({"on": "host_leaves_zone", "zone_id": zone_id}),
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
        AlertRuleRepo::link_method(&pool, enter_rule.id, method.id)
            .await
            .unwrap();
        AlertRuleRepo::link_method(&pool, leave_rule.id, method.id)
            .await
            .unwrap();

        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

        // Fix #1: inside -- fires host_enters_zone once.
        update_host_zones(&ctx, &a_fix(base, INSIDE)).await;
        let (_rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(total, 1, "first fix inside fires host_enters_zone once");

        // Fix #2: still inside -- state-diffed, no further firing.
        update_host_zones(&ctx, &a_fix(base + chrono::Duration::seconds(1), INSIDE)).await;
        let (_rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(total, 1, "a second still-inside fix must not fire again");

        // Fix #3: outside -- fires host_leaves_zone once.
        update_host_zones(&ctx, &a_fix(base + chrono::Duration::seconds(2), OUTSIDE)).await;
        let (rows, total) = NotificationRepo::list(&pool, false, 10, 0).await.unwrap();
        assert_eq!(total, 2, "moving outside fires host_leaves_zone once");
        assert!(rows.iter().any(|r| r.alert_rule_id == Some(leave_rule.id)));
        assert!(rows.iter().any(|r| r.alert_rule_id == Some(enter_rule.id)));
    }
}
