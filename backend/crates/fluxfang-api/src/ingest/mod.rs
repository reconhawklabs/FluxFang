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
//! ## Auto-attach: first match wins, enabled emitters only
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
//!
//! Phase A4 adds one filter ahead of that evaluation: an emitter with
//! `match_enabled == false` is skipped outright (its rule isn't even
//! parsed/evaluated) — this is the general "disable an emitter's rule to
//! stop future auto-association" capability the design doc calls for,
//! applying equally to a user-made emitter and an auto-created one.
//!
//! ## Auto-create (Phase A4)
//!
//! If auto-attach finishes with the emission still unassigned, and the
//! emission's `data_source.config.auto_create_emitters` is `true`,
//! [`auto_create_emitter`] classifies the payload
//! ([`fluxfang_core::classify`]) and get-or-creates a matching emitter
//! (`EmitterRepo::get_or_create_by_identity`, keyed on the classification's
//! `identity_key`), then attaches the emission to it — **unless** an
//! existing emitter for that identity has since had its rule disabled, in
//! which case the emission is deliberately left unassigned rather than
//! re-creating or re-attaching (see that function's doc comment). A `None`
//! classification (unrecognized kind/payload shape, or missing identity
//! field) also leaves the emission unassigned; nothing here can fail
//! `ingest` itself (see [`auto_create_emitter`]'s self-containment note).

pub mod alerts;
pub mod location;
pub mod pump;
pub mod session;
pub mod zones;

use std::sync::Arc;

use fluxfang_capture::RawObservation;
use fluxfang_core::rule::{eval, Condition, MatchMode, Op, Rule};
use fluxfang_core::{classify, emitter_type_label};
use fluxfang_db::models::{Emission, NewEmission, NewEmitter, Notification};
use fluxfang_db::{DataSourceRepo, EmissionRepo, EmitterRepo};
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
///
/// `#[derive(Clone)]` (Task 6.2): every field is itself cheaply `Clone`
/// (`PgPool` is an internally-`Arc`'d pool handle, `Option<Arc<SessionManager>>`
/// and `broadcast::Sender` are reference-counted, `[u8; 32]` is `Copy`), so
/// deriving `Clone` on the whole struct costs nothing beyond what callers
/// already pay when they clone individual fields — and it's exactly what
/// lets `CaptureSupervisor` (`crate::capture`) hand every spawned
/// wifi-ingest task, and the host-zone hook, its own owned copy instead of
/// juggling a `&IngestCtx` with a lifetime tied to the supervisor.
#[derive(Clone)]
pub struct IngestCtx {
    pub pool: PgPool,
    /// The `SessionManager` currently bounding capture, if any.
    ///
    /// `Option`, not a bare `Arc<SessionManager>` (Task 6.2): a
    /// `SessionManager` doesn't exist until `SessionManager::open` returns,
    /// but building the `HostZoneHook` passed *into* `open` needs an
    /// `IngestCtx` up front (see `crate::ingest::zones::update_host_zones`'s
    /// signature) — a chicken-and-egg problem `CaptureSupervisor` resolves
    /// by building the hook's `IngestCtx` with `sessions: None` (that call
    /// path never reads this field; `update_host_zones`/`update_subject_zones`
    /// only touch `pool`/`events`/`secret_key`) and only setting
    /// `Some(session_manager)` on the "full" `IngestCtx` used for actual
    /// emission ingest, built after the `SessionManager` exists. [`ingest`]
    /// itself still requires a session (`None` here behaves exactly like a
    /// closed/absent one — see "No active session" above).
    pub sessions: Option<Arc<SessionManager>>,
    /// The shared "where am I?" value, fed by a `LocationPump` (Task 4). Every
    /// emission reads its location from here, decoupled from whether/how a GPS
    /// source is running.
    pub location: std::sync::Arc<crate::ingest::location::LocationProvider>,
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
///    not a skip. `location`/`location_quality` from
///    `ctx.location.classify(obs.observed_at)` (the shared `LocationProvider`
///    fed by the `LocationPump`, gated by that fix's freshness/quality as of
///    this observation's own timestamp -- not `Utc::now()` -- so replaying
///    older observations classifies deterministically); coordinates are
///    `None` unless the fix is fresh and usable (see
///    `location::LocationProvider::classify`'s doc comment for the exact
///    gate). `observed_at`/`signal_strength`/
///    `kind`/`payload` copied from `obs`. `emitter_id` starts `None`.
/// 2. Insert it (`EmissionRepo::insert`).
/// 3. **Auto-attach** (see module docs for the full first-match-wins
///    rationale): load every emitter, skip any with `match_enabled == false`
///    (Phase A4), evaluate each remaining one's `match_criteria` rule
///    against the emission's `payload` in `EmitterRepo::list`'s
///    `created_at ASC` order, and on the first match, `EmissionRepo::set_emitter`
///    stamps the emission's `emitter_id` and `EmitterRepo::touch_seen`
///    widens that emitter's `first_seen_at`/`last_seen_at` to include this
///    emission's `observed_at`. An emitter whose `match_criteria` fails to
///    parse as a `Rule` is treated as non-matching (skipped) rather than
///    failing the whole ingest — one operator's malformed rule must not
///    break auto-attach for every other emitter/emission. No match leaves
///    the emission unassigned.
/// 4. **Auto-create** (Phase A4, [`auto_create_emitter`]): only runs if the
///    emission is still unassigned after step 3. See module docs and that
///    function's own doc comment for the full classify → get-or-create →
///    attach-if-enabled flow.
/// 5. `alerts::evaluate_alerts(ctx, &emission).await` (Task 5.3) — checks
///    every enabled `alert_rule` with a `detected` trigger against this
///    now-finalized emission's emitter/entity (+ optional `content_match`),
///    dispatching+persisting+broadcasting any resulting `Notification`s
///    itself (see that module for the full pipeline). Returns `()` and
///    self-contains every error — a problem evaluating alerts must never
///    prevent the emission itself from finishing its own insert/broadcast.
/// 6. `zones::update_target_zones(ctx, &emission).await` (Task 5.4) —
///    recomputes zone membership for the emission's emitter (and, if
///    grouped, its entity) against every zone, firing any
///    `enters_zone`/`leaves_zone` `alert_rule` whose target and zone match
///    a just-happened transition (state-diffed against `zone_membership`,
///    so a still-inside/still-outside emission fires nothing). Same
///    self-containment guarantee as `evaluate_alerts` — see that module's
///    own doc comment for the full pipeline.
/// 7. Broadcast `Event::Emission(emission)` on `ctx.events`. `send` returns
///    `Err` when there are currently no subscribers (nobody has a WS
///    connection open) — an expected, benign state, not a failure of
///    `ingest` itself, so the result is deliberately discarded.
/// 8. Return the stored (and possibly auto-attached) emission.
pub async fn ingest(
    ctx: &IngestCtx,
    data_source_id: Uuid,
    obs: RawObservation,
) -> anyhow::Result<Emission> {
    let session_id = ctx
        .sessions
        .as_ref()
        .and_then(|sessions| sessions.current_session_id())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "ingest called with no active survey_session -- capture must only run while a \
                 SessionManager-bounded session is open (see this module's docs)"
            )
        })?;
    let (location, location_quality) = ctx.location.classify(obs.observed_at);

    let new = NewEmission {
        data_source_id: Some(data_source_id),
        emitter_id: None,
        session_id,
        observed_at: obs.observed_at,
        signal_strength: obs.signal_strength,
        location,
        location_quality: location_quality.as_str().to_string(),
        kind: obs.kind,
        payload: obs.payload,
    };

    let mut emission = EmissionRepo::insert(&ctx.pool, new).await?;

    // Auto-attach: first match wins, in EmitterRepo::list's created_at ASC
    // order (see module docs). match_enabled == false emitters are skipped
    // outright (Phase A4) -- not even parsed/evaluated.
    let emitters = EmitterRepo::list(&ctx.pool).await?;
    for emitter in emitters {
        if !emitter.match_enabled {
            continue;
        }
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

    // Auto-create (Phase A4): only if auto-attach above left the emission
    // unassigned. Self-contained (see auto_create_emitter's doc comment) --
    // never fails ingest itself.
    if emission.emitter_id.is_none() {
        auto_create_emitter(ctx, data_source_id, &mut emission).await;
    }

    // Wifi-association enrichment: if this is a client association frame that
    // attached to a wifi_client emitter, record the AP it connected to on that
    // emitter (latest-wins). Self-contained, like alerts/zones below.
    maybe_enrich_connected_ap(ctx, &emission).await;

    alerts::evaluate_alerts(ctx, &emission).await;
    zones::update_target_zones(ctx, &emission).await;

    // No subscribers is a normal, expected state (see doc comment above) --
    // deliberately not propagated as an ingest failure.
    let _ = ctx.events.send(Event::Emission(emission.clone()));

    Ok(emission)
}

/// Phase A4's auto-create: classify `emission`'s payload and, if the
/// emission's data source opts in, get-or-create a matching emitter and
/// attach it to `emission` (updating both the DB row and `*emission` in
/// place). Called from [`ingest`] only when auto-attach left the emission
/// unassigned.
///
/// ## Steps
///
/// 1. Load the emission's `data_source` (`DataSourceRepo::get`) and read
///    `config["auto_create_emitters"]` as a bool, defaulting to `false` if
///    absent/not a bool. Loaded fresh per emission -- documented, not
///    optimized, the same cost trade-off `alerts::evaluate_alerts` already
///    takes for its own per-emission `AlertRuleRepo::list` call. `false`
///    (the common case for sources that haven't opted in) returns
///    immediately, leaving the emission unassigned.
/// 2. [`fluxfang_core::classify`] the emission's `kind`/`payload`. `None`
///    (unrecognized kind, unrecognized payload shape, or missing identity
///    field -- e.g. a beacon with no `bssid`) leaves the emission
///    unassigned; classification is advisory, not a hard requirement.
/// 3. Build a `NewEmitter` from the `Classification`: `name`, `emitter_type`,
///    `attributes` copied straight across; `type_` is the human
///    `emitter_type_label` (matching how a user-made emitter's `type_` is
///    normally a display string); `identity_key` is
///    `Some(classification.identity_key())`; `match_enabled` starts `true`;
///    `match_criteria` is a **visible** `Rule` -- `{match: all, conditions:
///    [{field: identity_field, op: eq, value: identity_value}]}` -- the
///    exact same shape a user builds by hand, so this rule shows up and is
///    toggleable on the emitter's detail page like any other (see the
///    design doc's "Rules are visible + toggleable"). `identity_value` is a
///    JSON string (a MAC/BSSID), matching how `payload[identity_field]` is
///    always stored as a string by the WiFi capturer.
/// 4. `EmitterRepo::get_or_create_by_identity` -- atomic, race-safe
///    get-or-create keyed on `identity_key` (see that function's own doc
///    comment).
/// 5. **Attach only if the resulting emitter's `match_enabled` is `true`.**
///    A freshly-created emitter is always enabled, so this is the common
///    case. But if an emitter for this identity already existed *and its
///    rule had been disabled*, `get_or_create_by_identity` returns that
///    existing (disabled) row unchanged -- this step then deliberately
///    leaves the emission unassigned rather than attaching it or creating a
///    duplicate. This is the "disabling an emitter's rule stops future
///    auto-association" guarantee the design doc calls for. (An existing
///    *enabled* emitter for this identity reaching this function at all
///    would be unexpected -- its rule would already have matched during
///    auto-attach above -- but attaching to it here is harmless.)
/// 6. Attach: `EmissionRepo::set_emitter` + write the returned row back into
///    `*emission` (so the caller's copy, and therefore the eventual
///    broadcast/alerts/zones steps, see the final `emitter_id`), then
///    `EmitterRepo::touch_seen` to seed/widen the emitter's
///    `first_seen_at`/`last_seen_at` with this emission's `observed_at`.
///
/// ## Self-containment
///
/// Every fallible step here (`DataSourceRepo::get`, `get_or_create_by_identity`,
/// `EmissionRepo::set_emitter`, `EmitterRepo::touch_seen`) is handled with an
/// early `return` on `Err`, same as `alerts::evaluate_alerts`/
/// `zones::update_target_zones` -- a DB hiccup here must leave the emission
/// exactly as auto-attach left it (persisted, unassigned), never fail
/// `ingest` itself or drop the emission.
async fn auto_create_emitter(ctx: &IngestCtx, data_source_id: Uuid, emission: &mut Emission) {
    let Ok(Some(data_source)) = DataSourceRepo::get(&ctx.pool, data_source_id).await else {
        return;
    };
    let auto_create_enabled = data_source
        .config
        .get("auto_create_emitters")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !auto_create_enabled {
        return;
    }

    let Some(classification) = classify(&emission.kind, &emission.payload) else {
        return;
    };

    let match_criteria = match &classification.match_criteria {
        Some(rule) => serde_json::to_value(rule).expect("Rule always serializes to JSON"),
        None => serde_json::to_value(Rule {
            match_mode: MatchMode::All,
            conditions: vec![Condition {
                field: classification.identity_field.clone(),
                op: Op::Eq,
                value: serde_json::Value::String(classification.identity_value.clone()),
            }],
        })
        .expect("Rule always serializes to JSON"),
    };

    let new_emitter = NewEmitter {
        name: classification.name.clone(),
        type_: Some(emitter_type_label(&classification.emitter_type).to_string()),
        entity_id: None,
        match_criteria,
        emitter_type: Some(classification.emitter_type.clone()),
        attributes: classification.attributes.clone(),
        match_enabled: true,
        identity_key: Some(classification.identity_key()),
    };

    let Ok((emitter, _created)) =
        EmitterRepo::get_or_create_by_identity(&ctx.pool, new_emitter).await
    else {
        return;
    };

    // A pre-existing, disabled emitter for this identity: leave the
    // emission unassigned (see step 5 above) rather than attaching or
    // re-creating.
    if !emitter.match_enabled {
        return;
    }

    let Ok(updated) = EmissionRepo::set_emitter(&ctx.pool, emission.id, emitter.id).await else {
        return;
    };
    *emission = updated;
    let _ = EmitterRepo::touch_seen(&ctx.pool, emitter.id, emission.observed_at).await;
}

/// If `emission` is a wifi association/reassociation frame carrying a
/// `target_bssid` and it attached to an emitter, merge that AP
/// (`connected_bssid`/`connected_ssid`) onto the emitter's attributes.
/// [`EmitterRepo::merge_client_attributes`] is itself type-guarded to
/// `wifi_client`, so this is a no-op for any other emitter type. Self-
/// contained: a DB error here (like alerts/zones) is swallowed, never failing
/// `ingest`.
async fn maybe_enrich_connected_ap(ctx: &IngestCtx, emission: &Emission) {
    let Some(emitter_id) = emission.emitter_id else {
        return;
    };
    let frame_type = emission
        .payload
        .get("frame_type")
        .and_then(serde_json::Value::as_str);
    if !matches!(
        frame_type,
        Some("association_request") | Some("reassociation_request")
    ) {
        return;
    }
    let Some(target_bssid) = emission
        .payload
        .get("target_bssid")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    let target_ssid = emission
        .payload
        .get("target_ssid")
        .and_then(serde_json::Value::as_str);
    let patch = serde_json::json!({
        "connected_bssid": target_bssid,
        "connected_ssid": target_ssid,
    });
    let _ = EmitterRepo::merge_client_attributes(&ctx.pool, emitter_id, &patch).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::location::LocationProvider;
    use crate::test_support::fresh_pool;
    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use fluxfang_capture::GpsFix;
    use fluxfang_db::models::{NewDataSource, NewEmitter};
    use fluxfang_db::DataSourceRepo;
    use std::time::Duration;

    async fn seed_wifi_source(pool: &PgPool) -> Uuid {
        DataSourceRepo::insert(pool, NewDataSource::wifi_monitor("wlan0"))
            .await
            .expect("seed wifi data_source")
            .id
    }

    /// Same as [`seed_wifi_source`] but with an explicit `config` (Phase A4
    /// tests need `{"auto_create_emitters": true}` / `false` / omitted).
    async fn seed_wifi_source_with_config(pool: &PgPool, config: serde_json::Value) -> Uuid {
        DataSourceRepo::insert(
            pool,
            NewDataSource {
                kind: "wifi".to_string(),
                mode: "monitor".to_string(),
                interface: Some("wlan0".to_string()),
                config,
            },
        )
        .await
        .expect("seed wifi data_source with config")
        .id
    }

    /// A bluetooth `scan`-mode data source with an explicit `config`
    /// (Phase A4 bluetooth auto-create tests need
    /// `{"auto_create_emitters": true}`) — mirrors
    /// [`seed_wifi_source_with_config`].
    async fn seed_bluetooth_source_with_config(pool: &PgPool, config: serde_json::Value) -> Uuid {
        DataSourceRepo::insert(
            pool,
            NewDataSource {
                kind: "bluetooth".to_string(),
                mode: "scan".to_string(),
                interface: Some("hci0".to_string()),
                config,
            },
        )
        .await
        .expect("seed bluetooth data_source with config")
        .id
    }

    /// Open a fresh `survey_session` and a `LocationProvider` pre-loaded with
    /// `fix`, mirroring what the `LocationPump` would feed at runtime. The
    /// session stays open until explicitly closed (it is no longer tied to a
    /// GPS source's lifetime), and the provider gives `ingest` a location to
    /// stamp onto emissions.
    async fn session_with_fix(
        pool: PgPool,
        fix: GpsFix,
    ) -> (SessionManager, Arc<LocationProvider>) {
        let manager = SessionManager::open(pool)
            .await
            .expect("open SessionManager");
        let provider = Arc::new(LocationProvider::new());
        provider.update(fix);
        (manager, provider)
    }

    fn wifi_obs(bssid: &str, observed_at: chrono::DateTime<Utc>) -> RawObservation {
        RawObservation {
            kind: "wifi".to_string(),
            observed_at,
            signal_strength: Some(-55),
            payload: serde_json::json!({"bssid": bssid, "channel": 6}),
        }
    }

    /// An AP beacon observation -- `frame_type: "beacon"` so
    /// `fluxfang_core::classify` recognizes it (Phase A4 auto-create tests).
    fn beacon_obs(bssid: &str, ssid: &str, observed_at: chrono::DateTime<Utc>) -> RawObservation {
        RawObservation {
            kind: "wifi".to_string(),
            observed_at,
            signal_strength: Some(-55),
            payload: serde_json::json!({
                "bssid": bssid,
                "ssid": ssid,
                "frame_type": "beacon",
                "channel": 6,
            }),
        }
    }

    /// A client probe-request observation -- `frame_type: "probe_request"`
    /// with a `src_mac` (Phase A4 auto-create tests). `src_mac` here is a
    /// known locally-administered/randomized MAC (bit 0x02 of the first
    /// octet set), matching `classify_wifi_probe_request`'s randomized-MAC
    /// detection so the auto-created emitter's `attributes.randomized_mac`
    /// can be asserted `true`.
    fn probe_request_obs(src_mac: &str, observed_at: chrono::DateTime<Utc>) -> RawObservation {
        RawObservation {
            kind: "wifi".to_string(),
            observed_at,
            signal_strength: Some(-60),
            payload: serde_json::json!({
                "src_mac": src_mac,
                "frame_type": "probe_request",
            }),
        }
    }

    /// A client association-request observation -- `src_mac` (the client) plus
    /// the target AP under `target_bssid`/`target_ssid` (never plain `bssid`),
    /// matching the parser's serialization for these frames.
    fn association_obs(
        src_mac: &str,
        target_bssid: &str,
        target_ssid: &str,
        observed_at: chrono::DateTime<Utc>,
    ) -> RawObservation {
        RawObservation {
            kind: "wifi".to_string(),
            observed_at,
            signal_strength: Some(-58),
            payload: serde_json::json!({
                "frame_type": "association_request",
                "src_mac": src_mac,
                "target_bssid": target_bssid,
                "target_ssid": target_ssid,
            }),
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
        let (manager, provider) = session_with_fix(pool.clone(), fix.clone()).await;
        let session_id = manager.current_session_id().unwrap();

        let (events_tx, mut events_rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
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

    /// Task 5: a fix that exists but is older than
    /// [`location::FRESH_FIX_MAX_AGE_SECONDS`] as of the observation's own
    /// `observed_at` must tag the emission `location_quality: "stale"` with
    /// NULL coordinates -- not `"fresh"` (the fix is real, just too old) and
    /// not `"none"` (a fix does exist). This is the real `classify` gate
    /// replacing Task 3's temporary presence-based tagging, which conflated
    /// "no fix" and "stale fix" into the same `"none"` bucket.
    #[tokio::test]
    async fn stale_fix_tags_emission_null_with_stale_quality() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        // Fix recorded at `base`; observation 30s later -- well past the 15s
        // freshness gate, so this must classify as stale, not fresh/none.
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let obs = wifi_obs("aa:bb:cc:dd:ee:ff", base + ChronoDuration::seconds(30));
        let emission = ingest(&ctx, ds, obs).await.expect("ingest should succeed");

        assert_eq!(emission.location_quality, "stale");
        assert!(emission.lon.is_none());
        assert!(emission.lat.is_none());
    }

    #[tokio::test]
    async fn ingest_returns_err_when_no_session_is_active() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;

        // Open then immediately close a session: `current_session_id()` is
        // then `None`, standing in for "capture is not currently running".
        let manager = SessionManager::open(pool.clone()).await.unwrap();
        manager.close().await.unwrap();
        assert!(manager.current_session_id().is_none());

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool,
            sessions: Some(Arc::new(manager)),
            location: Arc::new(LocationProvider::new()),
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
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

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
                ..Default::default()
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
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
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
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

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
                ..Default::default()
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
                ..Default::default()
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
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
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
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

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
                ..Default::default()
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
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
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

    // -- Phase A4: auto-create -----------------------------------------------

    /// (a) A beacon with no matching emitter, on a source with
    /// `auto_create_emitters = true`, auto-creates a `wifi_access_point`
    /// emitter and attaches the emission to it; a second beacon with the
    /// SAME bssid must dedupe to that same emitter, not create a duplicate.
    #[tokio::test]
    async fn ingest_auto_creates_wifi_access_point_emitter_from_beacon_and_dedupes_on_bssid() {
        let pool = fresh_pool().await;
        let ds =
            seed_wifi_source_with_config(&pool, serde_json::json!({"auto_create_emitters": true}))
                .await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let bssid = "aa:bb:cc:dd:ee:ff";
        let emission = ingest(&ctx, ds, beacon_obs(bssid, "HomeNet", base))
            .await
            .expect("ingest should succeed");

        let emitter_id = emission
            .emitter_id
            .expect("beacon should auto-create and attach an emitter");
        let emitter = EmitterRepo::get(&pool, emitter_id).await.unwrap().unwrap();
        assert_eq!(emitter.emitter_type.as_deref(), Some("wifi_access_point"));
        assert!(
            emitter.name.starts_with("WiFi AP"),
            "unexpected emitter name: {}",
            emitter.name
        );
        assert!(emitter.match_enabled);
        assert_eq!(
            emitter.identity_key.as_deref(),
            Some("wifi_access_point:aa:bb:cc:dd:ee:ff")
        );

        // A second beacon, same bssid: must attach to the SAME emitter, no
        // duplicate created (get_or_create_by_identity).
        let second_at = base + ChronoDuration::seconds(10);
        let second = ingest(&ctx, ds, beacon_obs(bssid, "HomeNet", second_at))
            .await
            .expect("second ingest should succeed");
        assert_eq!(
            second.emitter_id,
            Some(emitter_id),
            "same bssid must dedupe to the same emitter"
        );

        let all = EmitterRepo::list(&pool).await.unwrap();
        assert_eq!(
            all.len(),
            1,
            "no duplicate emitter should have been created"
        );
    }

    /// (a) A probe_request with no matching emitter auto-creates a
    /// `wifi_client` emitter with a `randomized_mac` attribute reflecting
    /// the src_mac's locally-administered bit.
    #[tokio::test]
    async fn ingest_auto_creates_wifi_client_emitter_from_probe_request_with_randomized_mac() {
        let pool = fresh_pool().await;
        let ds =
            seed_wifi_source_with_config(&pool, serde_json::json!({"auto_create_emitters": true}))
                .await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        // First octet 0x3a has bit 0x02 set -> locally-administered/randomized.
        let src_mac = "3a:de:ad:be:ef:00";
        let emission = ingest(&ctx, ds, probe_request_obs(src_mac, base))
            .await
            .expect("ingest should succeed");

        let emitter_id = emission
            .emitter_id
            .expect("probe_request should auto-create and attach an emitter");
        let emitter = EmitterRepo::get(&pool, emitter_id).await.unwrap().unwrap();
        assert_eq!(emitter.emitter_type.as_deref(), Some("wifi_client"));
        assert_eq!(
            emitter.attributes.get("randomized_mac"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            emitter.identity_key.as_deref(),
            Some("wifi_client:3a:de:ad:be:ef:00")
        );
    }

    /// (b) With `auto_create_emitters` false or absent from `config`, a
    /// no-match emission stays unassigned and no emitter is created.
    #[tokio::test]
    async fn ingest_does_not_auto_create_when_auto_create_emitters_is_false_or_unset() {
        let pool = fresh_pool().await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

        for config in [
            serde_json::json!({"auto_create_emitters": false}),
            serde_json::json!({}),
        ] {
            let ds = seed_wifi_source_with_config(&pool, config).await;
            let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;
            let (events_tx, _rx) = broadcast::channel(8);
            let ctx = IngestCtx {
                pool: pool.clone(),
                sessions: Some(Arc::new(manager)),
                location: provider.clone(),
                events: events_tx,
                secret_key: [0x11u8; 32],
            };

            let emission = ingest(&ctx, ds, beacon_obs("11:22:33:44:55:66", "SomeNet", base))
                .await
                .expect("ingest should succeed");
            assert_eq!(
                emission.emitter_id, None,
                "no emitter should be auto-created or attached when auto_create_emitters isn't true"
            );
        }

        assert_eq!(
            EmitterRepo::list(&pool).await.unwrap().len(),
            0,
            "no emitters should exist at all"
        );
    }

    /// (c) An existing emitter whose identity rule WOULD match but is
    /// disabled (`match_enabled = false`): auto-attach must skip it (already
    /// covered above by the enabled-only loop), and auto-create must find it
    /// via `get_or_create_by_identity` and refuse to attach to it or create
    /// a duplicate -- the emission stays unassigned.
    #[tokio::test]
    async fn ingest_does_not_attach_or_duplicate_when_matching_emitter_is_disabled() {
        let pool = fresh_pool().await;
        let ds =
            seed_wifi_source_with_config(&pool, serde_json::json!({"auto_create_emitters": true}))
                .await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

        let bssid = "aa:bb:cc:dd:ee:ff";
        let disabled = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "WiFi AP \"HomeNet\" (aa:bb:cc:dd:ee:ff)".to_string(),
                type_: Some("WiFi Access Point".to_string()),
                entity_id: None,
                match_criteria: serde_json::json!({
                    "match": "all",
                    "conditions": [{"field": "bssid", "op": "eq", "value": bssid}]
                }),
                emitter_type: Some("wifi_access_point".to_string()),
                attributes: serde_json::json!({"bssid": bssid, "ssid": "HomeNet"}),
                match_enabled: false,
                identity_key: Some(format!("wifi_access_point:{bssid}")),
            },
        )
        .await
        .unwrap();

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let emission = ingest(&ctx, ds, beacon_obs(bssid, "HomeNet", base))
            .await
            .expect("ingest should succeed");

        assert_eq!(
            emission.emitter_id, None,
            "a disabled emitter's identity must not be auto-attached to, and auto-create must not \
             re-attach to it either"
        );

        let all = EmitterRepo::list(&pool).await.unwrap();
        assert_eq!(
            all.len(),
            1,
            "no duplicate emitter should have been created for the same identity_key"
        );
        assert_eq!(all[0].id, disabled.id);
        assert!(!all[0].match_enabled);
    }

    /// (d) `classify()` returning `None` (e.g. a wifi payload with no
    /// recognizable `frame_type`) leaves the emission unassigned, creates no
    /// emitter, and does not panic -- even with `auto_create_emitters: true`.
    #[tokio::test]
    async fn ingest_leaves_emission_unassigned_when_classify_returns_none() {
        let pool = fresh_pool().await;
        let ds =
            seed_wifi_source_with_config(&pool, serde_json::json!({"auto_create_emitters": true}))
                .await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        // No `frame_type` at all -- classify_wifi's match falls through to
        // `_ => None`.
        let obs = RawObservation {
            kind: "wifi".to_string(),
            observed_at: base,
            signal_strength: Some(-70),
            payload: serde_json::json!({"channel": 6}),
        };

        let emission = ingest(&ctx, ds, obs)
            .await
            .expect("ingest should succeed, not panic, even when classify() returns None");
        assert_eq!(emission.emitter_id, None);
        assert_eq!(EmitterRepo::list(&pool).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn ingest_association_attaches_to_client_not_ap_and_records_connected_ap() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

        // Pre-existing AP emitter (rule bssid == the target), created FIRST so
        // first-match-wins would attach to it if the association carried a
        // plain `bssid`. It must not.
        let ap = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "AP".to_string(),
                match_criteria: serde_json::json!({
                    "match": "all",
                    "conditions": [{"field": "bssid", "op": "eq", "value": "aa:bb:cc:dd:ee:ff"}]
                }),
                emitter_type: Some("wifi_access_point".to_string()),
                attributes: serde_json::json!({"bssid": "aa:bb:cc:dd:ee:ff"}),
                match_enabled: true,
                identity_key: Some("wifi_access_point:aa:bb:cc:dd:ee:ff".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // The associating client's emitter (rule src_mac == the client).
        let client = EmitterRepo::insert(
            &pool,
            NewEmitter {
                name: "Client".to_string(),
                match_criteria: serde_json::json!({
                    "match": "all",
                    "conditions": [{"field": "src_mac", "op": "eq", "value": "3a:de:ad:be:ef:00"}]
                }),
                emitter_type: Some("wifi_client".to_string()),
                attributes: serde_json::json!({
                    "src_mac": "3a:de:ad:be:ef:00",
                    "randomized_mac": true
                }),
                match_enabled: true,
                identity_key: Some("wifi_client:3a:de:ad:be:ef:00".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let obs = association_obs("3a:de:ad:be:ef:00", "aa:bb:cc:dd:ee:ff", "HomeNet", base);
        let emission = ingest(&ctx, ds, obs).await.expect("ingest ok");

        assert_eq!(
            emission.emitter_id,
            Some(client.id),
            "association must attach to the client, not the AP"
        );

        let got = EmitterRepo::get(&pool, client.id).await.unwrap().unwrap();
        assert_eq!(got.attributes["connected_bssid"], "aa:bb:cc:dd:ee:ff");
        assert_eq!(got.attributes["connected_ssid"], "HomeNet");
        assert_eq!(got.attributes["src_mac"], "3a:de:ad:be:ef:00");

        let got_ap = EmitterRepo::get(&pool, ap.id).await.unwrap().unwrap();
        assert!(got_ap.attributes.get("connected_bssid").is_none());
    }

    #[tokio::test]
    async fn ingest_association_auto_creates_client_with_connected_ap() {
        let pool = fresh_pool().await;
        let ds =
            seed_wifi_source_with_config(&pool, serde_json::json!({"auto_create_emitters": true}))
                .await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let obs = association_obs("3a:de:ad:be:ef:00", "aa:bb:cc:dd:ee:ff", "HomeNet", base);
        let emission = ingest(&ctx, ds, obs).await.expect("ingest ok");

        let emitter_id = emission.emitter_id.expect("auto-created client emitter");
        let created = EmitterRepo::get(&pool, emitter_id).await.unwrap().unwrap();
        assert_eq!(created.emitter_type.as_deref(), Some("wifi_client"));
        assert_eq!(created.attributes["src_mac"], "3a:de:ad:be:ef:00");
        assert_eq!(created.attributes["connected_bssid"], "aa:bb:cc:dd:ee:ff");
        assert_eq!(created.attributes["connected_ssid"], "HomeNet");
    }

    #[tokio::test]
    async fn ingest_association_unassigned_when_no_client_and_auto_create_off() {
        let pool = fresh_pool().await;
        let ds = seed_wifi_source(&pool).await; // no auto_create config => off
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let obs = association_obs("3a:de:ad:be:ef:00", "aa:bb:cc:dd:ee:ff", "HomeNet", base);
        let emission = ingest(&ctx, ds, obs).await.expect("ingest ok");
        assert_eq!(
            emission.emitter_id, None,
            "no client emitter and auto-create off => unassigned, no enrichment, no panic"
        );
    }

    #[tokio::test]
    async fn bluetooth_advertisement_auto_creates_device_emitter() {
        let pool = fresh_pool().await;
        let ds = seed_bluetooth_source_with_config(
            &pool,
            serde_json::json!({"auto_create_emitters": true}),
        )
        .await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let obs = RawObservation {
            kind: "bluetooth".to_string(),
            observed_at: base,
            signal_strength: Some(-55),
            payload: serde_json::json!({
                "frame_type": "advertisement",
                "address": "3c:15:c2:aa:bb:cc",
                "address_type": "public",
                "name": "Study Speaker",
                "company_id": 76
            }),
        };
        let emission = ingest(&ctx, ds, obs).await.unwrap();
        let emitter_id = emission.emitter_id.expect("auto-created + attached");

        let emitter = EmitterRepo::get(&pool, emitter_id).await.unwrap().unwrap();
        assert_eq!(emitter.emitter_type.as_deref(), Some("bluetooth_device"));
        assert_eq!(
            emitter.name,
            "BT Client \"Study Speaker\" (3c:15:c2:aa:bb:cc)"
        );
        assert_eq!(emitter.attributes["vendor"], "Apple, Inc.");
    }

    #[tokio::test]
    async fn bluetooth_rotated_rpa_same_name_attaches_to_same_emitter() {
        let pool = fresh_pool().await;
        let ds = seed_bluetooth_source_with_config(
            &pool,
            serde_json::json!({"auto_create_emitters": true}),
        )
        .await;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (manager, provider) = session_with_fix(pool.clone(), a_fix(base)).await;

        let (events_tx, _rx) = broadcast::channel(8);
        let ctx = IngestCtx {
            pool: pool.clone(),
            sessions: Some(Arc::new(manager)),
            location: provider.clone(),
            events: events_tx,
            secret_key: [0x11u8; 32],
        };

        let first = ingest(
            &ctx,
            ds,
            RawObservation {
                kind: "bluetooth".to_string(),
                observed_at: base,
                signal_strength: Some(-40),
                payload: serde_json::json!({
                    "frame_type": "advertisement",
                    "address": "7a:11:11:11:11:11",
                    "address_type": "random",
                    "name": "Johns iPhone"
                }),
            },
        )
        .await
        .unwrap();

        let second = ingest(
            &ctx,
            ds,
            RawObservation {
                kind: "bluetooth".to_string(),
                observed_at: base + ChronoDuration::seconds(10),
                signal_strength: Some(-42),
                payload: serde_json::json!({
                    "frame_type": "advertisement",
                    "address": "7c:22:22:22:22:22",  // rotated RPA
                    "address_type": "random",
                    "name": "Johns iPhone"           // same advertised name
                }),
            },
        )
        .await
        .unwrap();

        assert_eq!(
            first.emitter_id.unwrap(),
            second.emitter_id.unwrap(),
            "a rotated RPA advertising the same name must attach to the same emitter"
        );
    }
}
