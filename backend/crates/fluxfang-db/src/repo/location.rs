//! `LocationRepo`: `location_fix` — continuous log of the host's own GPS
//! trajectory during a `survey_session`.
//!
//! Follows the same geography read/write pattern established by
//! `repo::emission` (see `models.rs` module docs): writes bind `lon`/`lat`
//! as plain `f64` parameters and build the point in SQL via
//! `ST_SetSRID(ST_MakePoint($n, $n+1), 4326)::geography`; reads project
//! `ST_X(location::geometry) AS lon, ST_Y(location::geometry) AS lat`
//! rather than selecting `location` directly. Never `SELECT *`/
//! `RETURNING *` here.

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{LocationFix, NewLocationFix};

pub struct LocationRepo;

/// Column list shared by every query that produces a [`LocationFix`] — see
/// the module docs on why `location` is never selected directly.
const LOCATION_FIX_COLUMNS: &str = "id, created_at, session_id, observed_at, altitude, speed, \
     heading, fix_quality, ST_X(location::geometry) AS lon, ST_Y(location::geometry) AS lat";

impl LocationRepo {
    /// Insert one `location_fix` row (e.g. from a
    /// `fluxfang_capture::GpsFix`, converted by the caller — this crate
    /// doesn't depend on `fluxfang-capture`, see the Task 5.1 report for
    /// why).
    pub async fn insert_fix(
        pool: &PgPool,
        new: NewLocationFix,
    ) -> Result<LocationFix, sqlx::Error> {
        let (lon, lat) = new.location;

        let sql = format!(
            "INSERT INTO location_fix \
                 (session_id, observed_at, location, altitude, speed, heading, fix_quality) \
             VALUES \
                 ($1, $2, ST_SetSRID(ST_MakePoint($3, $4), 4326)::geography, $5, $6, $7, $8) \
             RETURNING {LOCATION_FIX_COLUMNS}"
        );

        sqlx::query_as::<_, LocationFix>(&sql)
            .bind(new.session_id)
            .bind(new.observed_at)
            .bind(lon)
            .bind(lat)
            .bind(new.altitude)
            .bind(new.speed)
            .bind(new.heading)
            .bind(new.fix_quality)
            .fetch_one(pool)
            .await
    }

    /// All fixes logged for a session, oldest first (the natural order for
    /// replaying/plotting a trajectory). Used by `SessionManager`'s tests
    /// to assert on written rows; a future task may add pagination/bbox
    /// filtering here the way `EmissionRepo::query` does, but nothing
    /// needs that yet (YAGNI).
    pub async fn list_for_session(
        pool: &PgPool,
        session_id: Uuid,
    ) -> Result<Vec<LocationFix>, sqlx::Error> {
        let sql = format!(
            "SELECT {LOCATION_FIX_COLUMNS} FROM location_fix \
             WHERE session_id = $1 ORDER BY observed_at ASC"
        );
        sqlx::query_as::<_, LocationFix>(&sql)
            .bind(session_id)
            .fetch_all(pool)
            .await
    }
}
