//! Row structs mapping `backend/migrations/0001_init.sql` tables to Rust.
//!
//! Task 1.3a modeled the three tables with no geography column
//! (AppConfigRepo, DataSourceRepo, SessionRepo). Task 1.3b adds `Emission`
//! (the first geography-bearing table). Later sub-tasks add `Emitter`,
//! `Entity`, `Zone`, `ZoneMembership`, `AlertMethod`, `AlertRule`,
//! `Notification`, etc. to this same file — keep that convention (one
//! `models.rs` for every row type in the crate) rather than splitting
//! per-aggregate model files.
//!
//! ## Geography columns
//!
//! `emission`, `zone`, and `location_fix` have `geography(Point,4326)`
//! columns. sqlx cannot decode PostGIS `geography` directly into a Rust
//! type, so the established pattern (see `tests/schema.rs` for a
//! precedent, and `repo::emission` for the first repo to use it) is: never
//! `SELECT location`/`RETURNING *` as-is. Instead project it in SQL via
//! `ST_X(location::geometry) AS lon`, `ST_Y(location::geometry) AS lat`
//! and decode those as plain `Option<f64>` columns on the Rust side (write
//! it back with `ST_SetSRID(ST_MakePoint(lon, lat), 4326)::geography`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `app_config`: singleton row holding the admin password hash and
/// free-form settings. See `repo::app_config` for how the singleton is
/// created/updated (fixed nil UUID + upsert).
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct AppConfig {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub password_hash: Option<String>,
    pub session_secret: Option<String>,
    pub settings: serde_json::Value,
}

/// `data_source`: a configured capture device (wifi monitor-mode interface
/// or a gps receiver).
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct DataSource {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub kind: String,
    pub mode: String,
    pub interface: Option<String>,
    pub status: String,
    pub config: serde_json::Value,
    pub last_error: Option<String>,
}

/// Fields required to create a new `data_source`. `status` is intentionally
/// not part of this struct: `DataSourceRepo::insert` always creates new
/// sources in `status = 'stopped'`; use `DataSourceRepo::set_status` to
/// transition it afterwards.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewDataSource {
    pub kind: String,
    pub mode: String,
    pub interface: Option<String>,
    pub config: serde_json::Value,
}

impl NewDataSource {
    /// Convenience constructor for a wifi monitor-mode source.
    pub fn wifi_monitor(interface: impl Into<String>) -> Self {
        Self {
            kind: "wifi".to_string(),
            mode: "monitor".to_string(),
            interface: Some(interface.into()),
            config: serde_json::json!({}),
        }
    }

    /// Convenience constructor for a gpsd-backed gps source.
    pub fn gps_gpsd() -> Self {
        Self {
            kind: "gps".to_string(),
            mode: "gpsd".to_string(),
            interface: None,
            config: serde_json::json!({}),
        }
    }
}

/// `survey_session`: bounds a continuous capture period. `ended_at = NULL`
/// means the session is the (at most one) currently-active session.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct SurveySession {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub label: Option<String>,
}

/// `emission`: one captured observation (currently only `kind = "wifi"`).
/// High-volume, time-indexed.
///
/// `location` is `geography(Point,4326)` in the DB, which sqlx cannot
/// decode directly (see the module docs above) — every query that
/// produces this type must project it via `ST_X(location::geometry) AS
/// lon, ST_Y(location::geometry) AS lat` rather than `SELECT *`, so the
/// column list is spelled out explicitly in every `repo::emission` query
/// rather than relying on `RETURNING *`/`SELECT *`.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Emission {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub data_source_id: Option<Uuid>,
    pub emitter_id: Option<Uuid>,
    /// Nullable in the DB (`ON DELETE SET NULL` when the referenced
    /// session is deleted), even though [`NewEmission::session_id`] is
    /// required at insert time.
    pub session_id: Option<Uuid>,
    pub observed_at: DateTime<Utc>,
    pub signal_strength: Option<i32>,
    pub kind: String,
    pub payload: serde_json::Value,
    /// Longitude, decoded from `ST_X(location::geometry)`. `None` when
    /// `location` is NULL.
    pub lon: Option<f64>,
    /// Latitude, decoded from `ST_Y(location::geometry)`. `None` when
    /// `location` is NULL.
    pub lat: Option<f64>,
}

/// Fields required to create a new `emission`. `session_id` is required
/// here (unlike the DB column, which is nullable to tolerate the
/// referenced session later being deleted) — capture always happens within
/// a known survey session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEmission {
    pub data_source_id: Option<Uuid>,
    pub emitter_id: Option<Uuid>,
    pub session_id: Uuid,
    pub observed_at: DateTime<Utc>,
    pub signal_strength: Option<i32>,
    /// `(lon, lat)`. `None` when the emission wasn't geo-located.
    pub location: Option<(f64, f64)>,
    pub kind: String,
    pub payload: serde_json::Value,
}

impl NewEmission {
    /// Convenience constructor for a wifi emission captured right now, with
    /// no emitter/signal/location set yet. Callers that need those fields
    /// populated at insert time should build `NewEmission` directly.
    pub fn wifi(data_source_id: Uuid, session_id: Uuid, payload: serde_json::Value) -> Self {
        Self {
            data_source_id: Some(data_source_id),
            emitter_id: None,
            session_id,
            observed_at: Utc::now(),
            signal_strength: None,
            location: None,
            kind: "wifi".to_string(),
            payload,
        }
    }
}

/// `entity`: the tracked real-world thing an operator has grouped one or
/// more `emitter`s under (e.g. "Bob's phone").
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Entity {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub name: String,
    pub notes: Option<String>,
}

/// Fields required to create a new `entity`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEntity {
    pub name: String,
    pub notes: Option<String>,
}

/// `emitter`: a distinct identified source (e.g. a specific access point),
/// optionally grouped under an `entity` and matched against incoming
/// emissions via `match_criteria` (a [`fluxfang_core::Rule`] as JSON).
///
/// `type_` maps to the DB column `type` — `type` is a Rust keyword, so the
/// field is renamed here (see `repo::emitter` for the explicit column list
/// this requires on every query, same reasoning as `Emission::lon`/`lat`).
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Emitter {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub name: String,
    #[sqlx(rename = "type")]
    pub type_: Option<String>,
    pub entity_id: Option<Uuid>,
    pub match_criteria: serde_json::Value,
    pub first_seen_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

/// Fields required to create a new `emitter`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEmitter {
    pub name: String,
    pub type_: Option<String>,
    pub entity_id: Option<Uuid>,
    pub match_criteria: serde_json::Value,
}

/// `zone`: a user-named geofence. `center` is `geography(Point,4326)` in
/// the DB (same undecodable-by-sqlx situation as `Emission::location`, see
/// module docs above) — every query producing this type projects it via
/// `ST_X(center::geometry) AS lon, ST_Y(center::geometry) AS lat` rather
/// than `SELECT *`/`RETURNING *`. Unlike `Emission::location`, `zone.center`
/// is `NOT NULL`, so `lon`/`lat` here are plain `f64`, not `Option<f64>`.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Zone {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub name: String,
    /// Longitude, decoded from `ST_X(center::geometry)`.
    pub lon: f64,
    /// Latitude, decoded from `ST_Y(center::geometry)`.
    pub lat: f64,
    pub radius_m: f64,
    pub notes: Option<String>,
}

/// Fields required to create (or fully replace, via `ZoneRepo::update`) a
/// `zone`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewZone {
    pub name: String,
    /// `(lon, lat)`.
    pub center: (f64, f64),
    pub radius_m: f64,
    pub notes: Option<String>,
}

/// `zone_membership`: ingest-maintained last-known-membership state for one
/// subject (`emitter`, `entity`, or the singular `host`) in one `zone`, used
/// so enter/leave alert triggers fire once per transition rather than once
/// per emission. `subject_id` is `NULL` for `subject_type = "host"` (there
/// is only one host); the `(subject_type, subject_id, zone_id)` unique
/// index uses `NULLS NOT DISTINCT` so exactly one host row exists per zone
/// — see `repo::zone_membership` for how `get`/`upsert` handle that NULL.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct ZoneMembership {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub subject_type: String,
    pub subject_id: Option<Uuid>,
    pub zone_id: Uuid,
    pub inside: bool,
    pub since: DateTime<Utc>,
}

/// `alert_method`: a reusable, user-configured delivery channel (email,
/// in-app, or webhook). `type_` maps to the DB column `type` (Rust keyword
/// rename, same pattern as `Emitter::type_`) — every query in
/// `repo::alert_method`/`repo::alert_rule` spells out an explicit column
/// list rather than relying on `SELECT *`/`RETURNING *`.
///
/// `config` holds non-secret settings (webhook url/headers, smtp host,
/// etc.) as plain JSON. `config_encrypted` is an opaque ciphertext blob for
/// anything secret (smtp password, webhook secret) — Phase 8 wires up the
/// actual encryption/decryption; this crate only stores/returns the bytes
/// unchanged. It's nullable in the DB (no value has been encrypted yet),
/// hence `Option<Vec<u8>>` here even though callers constructing a
/// [`NewAlertMethod`] always supply bytes (possibly empty) up front.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct AlertMethod {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub name: String,
    #[sqlx(rename = "type")]
    pub type_: String,
    pub enabled: bool,
    pub config: serde_json::Value,
    pub config_encrypted: Option<Vec<u8>>,
}

/// Fields required to create a new `alert_method`. `config` isn't included
/// here (it defaults to `{}` in the DB); only `config_encrypted` is settable
/// at this layer, per Task 1.3e's interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewAlertMethod {
    pub name: String,
    pub type_: String,
    pub enabled: bool,
    pub config_encrypted: Vec<u8>,
}

/// `alert_rule`: watches a target (`emitter`/`entity`) or, for host-zone
/// rules, no target at all (`target_type`/`target_id` both `NULL`).
/// `trigger` is a JSON blob whose shape is `trigger.on ∈ 'detected' |
/// 'enters_zone' | 'leaves_zone' | 'host_enters_zone' | 'host_leaves_zone'`,
/// with optional `trigger.zone_id`/`trigger.content_match` — interpreted by
/// the alert-evaluation logic added in a later phase, not by this repo.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub name: String,
    pub enabled: bool,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub trigger: serde_json::Value,
}

/// Fields required to create (or fully replace, via `AlertRuleRepo::update`)
/// an `alert_rule`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewAlertRule {
    pub name: String,
    pub enabled: bool,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub trigger: serde_json::Value,
}

/// `notification`: fired-alert log; also the source for the in-app
/// Notifications page (`read_at: None` = unread). `alert_rule_id`/
/// `alert_method_id` are `ON DELETE SET NULL` in the DB — deleting the rule
/// or method that produced a notification never deletes the notification
/// itself, it just orphans the reference (unlike `alert_rule_method`, whose
/// join rows `ON DELETE CASCADE` when either side is deleted).
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Notification {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub alert_rule_id: Option<Uuid>,
    pub alert_method_id: Option<Uuid>,
    pub fired_at: DateTime<Utc>,
    pub payload: serde_json::Value,
    pub delivery_status: String,
    pub read_at: Option<DateTime<Utc>>,
}

/// Fields required to create a new `notification` (always the result of an
/// alert actually firing, so `fired_at`/`payload`/`delivery_status` are
/// required; `read_at` starts `NULL` and is set later via
/// `NotificationRepo::mark_read`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewNotification {
    pub alert_rule_id: Option<Uuid>,
    pub alert_method_id: Option<Uuid>,
    pub fired_at: DateTime<Utc>,
    pub payload: serde_json::Value,
    pub delivery_status: String,
}
