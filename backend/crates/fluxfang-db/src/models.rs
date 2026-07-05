//! Row structs mapping `backend/migrations/0001_init.sql` tables to Rust.
//!
//! Only the tables needed by Task 1.3a's three repos (AppConfigRepo,
//! DataSourceRepo, SessionRepo) are modeled here. Later sub-tasks add
//! `Emission`, `Emitter`, `Entity`, `Zone`, `ZoneMembership`, `AlertMethod`,
//! `AlertRule`, `Notification`, etc. to this same file — keep that
//! convention (one `models.rs` for every row type in the crate) rather than
//! splitting per-aggregate model files.
//!
//! ## Geography columns
//!
//! Several tables not modeled yet (`emission`, `zone`, `location_fix`) have
//! `geography(Point,4326)` columns. sqlx cannot decode PostGIS `geography`
//! directly into a Rust type, so the established pattern (see
//! `tests/schema.rs` for a precedent) is: never `SELECT location` as-is.
//! Instead project it in SQL via `ST_X(location::geometry) AS lon`,
//! `ST_Y(location::geometry) AS lat` (or `ST_AsText(location)`) and decode
//! those as plain `f64`/`String` columns on the Rust side. None of the
//! three tables in this sub-task have a geography column, so no helper is
//! introduced yet — add one alongside the first repo that needs it.

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
