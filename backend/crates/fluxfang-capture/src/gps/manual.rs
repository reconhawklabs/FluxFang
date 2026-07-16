//! Manual (operator-set) static GPS source: serves a fixed lat/lon as the
//! host location. Unlike [`super::gpsd`]/[`super::serial`] it touches no
//! hardware — the operator types the coordinates when adding the data
//! source. It re-emits the same point on a slow cadence with a fresh
//! timestamp so the shared `LocationProvider` (in `fluxfang-api`) stays fresh
//! (reads as `active`, not `stale`) and the `LocationPump` never sees it
//! exhaust — so a manual source is never reported as a failed device. This is
//! the same "keep the pump fed" reasoning behind [`crate::mock::MockGps`]'s
//! `looping` flag.

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::time::sleep;

use crate::{GpsFix, LocationSource};

/// How long [`ManualGpsSource::next_fix`] waits between re-emitting the fixed
/// point. Well under `fluxfang-api`'s `FRESH_FIX_MAX_AGE_SECONDS` (15s) so the
/// fix never ages into `stale`, while throttling `location_fix` writes to
/// ~0.5 Hz (the pump writes every fix the source yields).
const REEMIT_INTERVAL: Duration = Duration::from_secs(2);

/// A static, operator-set location source. Yields the same `lat`/`lon`
/// forever (never `None`), each stamped with a fresh `Utc::now()`.
pub struct ManualGpsSource {
    lat: f64,
    lon: f64,
    /// The first `next_fix` yields immediately (instant acquisition); every
    /// subsequent call sleeps [`REEMIT_INTERVAL`] first.
    first: bool,
}

impl ManualGpsSource {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self {
            lat,
            lon,
            first: true,
        }
    }
}

#[async_trait]
impl LocationSource for ManualGpsSource {
    async fn next_fix(&mut self) -> Option<GpsFix> {
        if self.first {
            self.first = false;
        } else {
            sleep(REEMIT_INTERVAL).await;
        }
        Some(GpsFix {
            at: Utc::now(),
            lon: self.lon,
            lat: self.lat,
            altitude: None,
            speed: None,
            heading: None,
            quality: 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `start_paused` makes the runtime auto-advance virtual time when the only
    // pending work is the REEMIT_INTERVAL timer, so the second `next_fix`
    // resolves without a real 2s wait.
    #[tokio::test(start_paused = true)]
    async fn yields_configured_point_and_never_exhausts() {
        let mut src = ManualGpsSource::new(37.7, -122.4);

        let a = src.next_fix().await.expect("first fix is Some");
        assert_eq!(a.lat, 37.7);
        assert_eq!(a.lon, -122.4);
        assert!(a.quality >= 1, "quality {} must be usable (>= 1)", a.quality);
        assert!(a.altitude.is_none() && a.speed.is_none() && a.heading.is_none());

        // Never returns None: a second call still yields the same point.
        let b = src.next_fix().await.expect("second fix is Some");
        assert_eq!((b.lat, b.lon), (37.7, -122.4));
    }
}
