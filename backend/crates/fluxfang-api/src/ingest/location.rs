//! `LocationProvider`: the single in-memory "where am I?" value, decoupled
//! from *how* the fix was obtained (GPS today; a hardcoded source later).
//!
//! Every emission reads its location through [`LocationProvider::classify`],
//! which is the one place the freshness + quality gate lives (previously
//! duplicated in `gps_status`). A fix is only surfaced as coordinates while
//! it is fresh (age <= [`FRESH_FIX_MAX_AGE_SECONDS`]) and usable (quality
//! >= [`MIN_USABLE_QUALITY`]); otherwise `classify` returns `None`
//! coordinates plus a [`LocationQuality`] explaining why (a fix exists but is
//! stale, vs no fix at all). The provider is fed by a `LocationPump` and
//! cleared on a user-initiated stop.

use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use fluxfang_capture::GpsFix;

/// A fix older than this many seconds is no longer trusted for tagging.
pub const FRESH_FIX_MAX_AGE_SECONDS: f64 = 15.0;

/// Minimum NMEA/gpsd-style fix quality treated as a real, usable fix (`0`
/// conventionally means "no fix"/invalid in both protocols).
pub const MIN_USABLE_QUALITY: i32 = 1;

/// Why an emission's location is what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationQuality {
    /// A fresh, usable fix was available — coordinates were attached.
    Fresh,
    /// A fix exists but is too old / low-quality to trust — location is NULL.
    Stale,
    /// No fix has ever been recorded (or the provider was cleared) — NULL.
    None,
}

impl LocationQuality {
    pub fn as_str(&self) -> &'static str {
        match self {
            LocationQuality::Fresh => "fresh",
            LocationQuality::Stale => "stale",
            LocationQuality::None => "none",
        }
    }
}

/// Holds the most-recent location fix, shared across the pump (writer) and
/// every ingest call + the gps status endpoint (readers). Uses
/// `std::sync::RwLock` (not tokio's): every critical section is a single
/// synchronous read/assign with no `.await` held across the guard — same
/// idiom `SessionManager` documents for its own locks.
pub struct LocationProvider {
    latest: Arc<RwLock<Option<GpsFix>>>,
}

impl LocationProvider {
    pub fn new() -> Self {
        Self {
            latest: Arc::new(RwLock::new(None)),
        }
    }

    /// Record a newly-received fix (called by the pump on every fix).
    pub fn update(&self, fix: GpsFix) {
        *self.latest.write().expect("location provider lock poisoned") = Some(fix);
    }

    /// Drop the current fix so subsequent reads classify as `None`. Called on
    /// a *user-initiated* stop of a location source (not on failure).
    pub fn clear(&self) {
        *self.latest.write().expect("location provider lock poisoned") = None;
    }

    /// The raw last fix, if any — for the gps status endpoint's display of
    /// lat/lon/quality/age. Prefer [`classify`] for tagging decisions.
    pub fn latest_raw(&self) -> Option<GpsFix> {
        self.latest
            .read()
            .expect("location provider lock poisoned")
            .clone()
    }

    /// The tagging decision: coordinates iff a fresh, usable fix exists as of
    /// `reference`; otherwise `None` coordinates + the reason.
    pub fn classify(&self, reference: DateTime<Utc>) -> (Option<(f64, f64)>, LocationQuality) {
        let guard = self.latest.read().expect("location provider lock poisoned");
        match guard.as_ref() {
            None => (None, LocationQuality::None),
            Some(fix) => {
                let age = (reference - fix.at).num_milliseconds() as f64 / 1000.0;
                let fresh = age <= FRESH_FIX_MAX_AGE_SECONDS;
                let usable = fix.quality >= MIN_USABLE_QUALITY;
                if fresh && usable {
                    (Some((fix.lon, fix.lat)), LocationQuality::Fresh)
                } else {
                    (None, LocationQuality::Stale)
                }
            }
        }
    }
}

impl Default for LocationProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fix_at(base: DateTime<Utc>, offset_secs: i64, quality: i32) -> GpsFix {
        GpsFix {
            at: base + chrono::Duration::seconds(offset_secs),
            lon: -122.0,
            lat: 37.0,
            altitude: None,
            speed: None,
            heading: None,
            quality,
        }
    }

    fn base() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    #[test]
    fn no_fix_classifies_as_none() {
        let p = LocationProvider::new();
        let (loc, q) = p.classify(base());
        assert_eq!(loc, None);
        assert_eq!(q, LocationQuality::None);
    }

    #[test]
    fn fresh_usable_fix_returns_coords() {
        let p = LocationProvider::new();
        p.update(fix_at(base(), 0, 1));
        // reference 14.9s later: still fresh.
        let reference = base() + chrono::Duration::milliseconds(14_900);
        let (loc, q) = p.classify(reference);
        assert_eq!(loc, Some((-122.0, 37.0)));
        assert_eq!(q, LocationQuality::Fresh);
    }

    #[test]
    fn fix_older_than_threshold_classifies_as_stale_null() {
        let p = LocationProvider::new();
        p.update(fix_at(base(), 0, 1));
        // reference 15.1s later: stale.
        let reference = base() + chrono::Duration::milliseconds(15_100);
        let (loc, q) = p.classify(reference);
        assert_eq!(loc, None);
        assert_eq!(q, LocationQuality::Stale);
    }

    #[test]
    fn low_quality_fix_classifies_as_stale_null() {
        let p = LocationProvider::new();
        p.update(fix_at(base(), 0, 0)); // quality 0 == no fix
        let (loc, q) = p.classify(base());
        assert_eq!(loc, None);
        assert_eq!(q, LocationQuality::Stale);
    }

    #[test]
    fn clear_reverts_to_none() {
        let p = LocationProvider::new();
        p.update(fix_at(base(), 0, 1));
        p.clear();
        let (loc, q) = p.classify(base());
        assert_eq!(loc, None);
        assert_eq!(q, LocationQuality::None);
    }
}
