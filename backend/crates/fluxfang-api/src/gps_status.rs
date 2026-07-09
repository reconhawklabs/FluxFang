//! `GET /api/gps/status` (Phase 5): the Dashboard **GPS Status** block's and
//! the map page's auto-centering data source. PROTECTED — mounted in
//! `lib.rs::app`'s protected router group, behind `require_auth`, same as
//! every other non-setup/login route.
//!
//! ## `source_running`
//!
//! Simplest reliable signal available: is there a `kind='gps'` `data_source`
//! row with `status='running'`? Read via a plain `DataSourceRepo::list`
//! scan rather than reaching into `CaptureSupervisor`'s private running-set
//! map — that map is keyed by `data_source_id` with no `kind` of its own
//! recorded against it (a caller would have to re-fetch each running id's
//! row anyway to learn its kind), and `DataSourceRepo::list` is already the
//! established, tested source of truth for a source's persisted `status`
//! (`CaptureSupervisor::start`/`stop` write through it on every outcome —
//! see that module's docs). One extra small `SELECT` on a request-path
//! endpoint at this app's homelab scale is a non-issue.
//!
//! ## `latest_fix`
//!
//! Via `CaptureSupervisor::latest_gps_fix()`, itself a thin pass-through to
//! the shared `LocationProvider`'s `latest_raw()` (fed by the running
//! location source's `LocationPump`). `fix_age_seconds` is computed against
//! `Utc::now()` taken *here*, in the handler — this is a request path, not
//! the deterministic ingest/session code, so wall-clock "now" is exactly
//! right (unlike e.g. `NewLocationFix`, which stamps `observed_at` from the
//! fix itself).
//!
//! ## `status` derivation
//!
//! - no running gps source -> `"disabled"` (regardless of any stale fix
//!   left over in memory from a since-stopped session).
//! - a running gps source but no fix yet -> `"acquiring"` (freshly started,
//!   waiting on the first NMEA/gpsd sentence).
//! - a fix exists, is no older than [`FRESH_FIX_MAX_AGE_SECONDS`], and its
//!   `quality` is at least [`MIN_USABLE_QUALITY`] -> `"active"`.
//! - a fix exists but is stale or low-quality -> `"degraded"` (hardware is
//!   still attached and "running", but not delivering something usable
//!   right now — lost satellite lock, a wedged serial reader, etc).
//!
//! Both thresholds are deliberately generous relative to GPS's typical ~1Hz
//! NMEA/gpsd cadence, giving headroom for scheduling jitter without masking
//! a genuinely stuck feed for long.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;

use fluxfang_db::DataSourceRepo;

use crate::dto::GpsStatusDto;
use crate::state::AppState;

/// A fix older than this many seconds is treated as stale (`"degraded"`)
/// even if the gps data source is still `running` — see module docs.
const FRESH_FIX_MAX_AGE_SECONDS: f64 = 15.0;

/// Minimum NMEA/gpsd-style fix quality treated as "a real, usable fix"
/// (`0` conventionally means "no fix" / invalid in both protocols). Below
/// this, a fix is reported as `"degraded"` even if it just arrived.
const MIN_USABLE_QUALITY: i32 = 1;

pub fn protected_routes() -> Router<AppState> {
    Router::new().route("/api/gps/status", get(gps_status))
}

async fn gps_status(State(state): State<AppState>) -> Result<Json<GpsStatusDto>, ApiError> {
    let sources = DataSourceRepo::list(&state.pool).await?;
    let source_running = sources
        .iter()
        .any(|s| s.kind == "gps" && s.status == "running");

    let fix = state.capture.latest_gps_fix().await;

    let has_fix = fix.is_some();
    let lat = fix.as_ref().map(|f| f.lat);
    let lon = fix.as_ref().map(|f| f.lon);
    let quality = fix.as_ref().map(|f| f.quality);
    let fix_age_seconds = fix
        .as_ref()
        .map(|f| (Utc::now() - f.at).num_milliseconds() as f64 / 1000.0);

    let status = if !source_running {
        "disabled"
    } else if !has_fix {
        "acquiring"
    } else {
        let fresh = fix_age_seconds.is_some_and(|age| age <= FRESH_FIX_MAX_AGE_SECONDS);
        let usable = quality.is_some_and(|q| q >= MIN_USABLE_QUALITY);
        if fresh && usable {
            "active"
        } else {
            "degraded"
        }
    };

    Ok(Json(GpsStatusDto {
        source_running,
        has_fix,
        lat,
        lon,
        quality,
        fix_age_seconds,
        status,
    }))
}

/// Small internal error type, same convention as `data_sources::ApiError`:
/// this handler's only fallible step is the `DataSourceRepo::list` query,
/// so the only variant needed is a DB-error-mapped `500`.
struct ApiError;

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        eprintln!("fluxfang-api: db error in gps_status route: {err}");
        ApiError
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}
