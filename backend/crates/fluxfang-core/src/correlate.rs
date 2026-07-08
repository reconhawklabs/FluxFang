//! Pure TPMS correlation logic (Spec B). No I/O, no clock reads — the caller
//! supplies each emitter's recent `Reading`s (timestamp + optional location).
//! Decides whether two same-model sensors are "on the same vehicle" from
//! co-observation over space (they travel together across ≥1 mile) or, when
//! GPS is unavailable, repeated co-observation over time.
//!
//! Distance is haversine-in-Rust (not PostGIS), so this is fully unit-testable
//! without a database and needs no per-pair distance query.

use std::time::Duration;

use chrono::{DateTime, Utc};

/// One emission reduced to what correlation needs.
#[derive(Debug, Clone)]
pub struct Reading {
    pub at: DateTime<Utc>,
    pub lon: Option<f64>,
    pub lat: Option<f64>,
}

/// A moment where two sensors each emitted within the co-occurrence window,
/// with a representative time and (if either reading had one) location.
#[derive(Debug, Clone)]
pub struct CoEvent {
    pub at: DateTime<Utc>,
    pub lon: Option<f64>,
    pub lat: Option<f64>,
}

/// Tunable thresholds. Defaults per Spec B.
#[derive(Debug, Clone)]
pub struct CorrelationConfig {
    pub cooccur_window: Duration,
    pub mile_meters: f64,
    pub fallback_min_events: usize,
    pub fallback_min_spread: Duration,
}

impl Default for CorrelationConfig {
    fn default() -> Self {
        Self {
            cooccur_window: Duration::from_secs(60),
            mile_meters: 1609.34,
            fallback_min_events: 3,
            fallback_min_spread: Duration::from_secs(600),
        }
    }
}

/// Great-circle distance in meters between two lon/lat points.
pub fn haversine_meters(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    const R: f64 = 6_371_000.0; // mean Earth radius, meters
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R * a.sqrt().asin()
}

/// Pair up readings from two sensors whose timestamps fall within `window`
/// into distinct co-occurrence events. Both slices are sorted by `at` first;
/// a greedy two-pointer walk pairs the closest unused readings so no reading
/// is reused. Each event takes the earlier reading's time and the first
/// available location (prefer `a`'s, else `b`'s).
pub fn cooccurrences(a: &[Reading], b: &[Reading], window: Duration) -> Vec<CoEvent> {
    let mut a: Vec<&Reading> = a.iter().collect();
    let mut b: Vec<&Reading> = b.iter().collect();
    a.sort_by_key(|r| r.at);
    b.sort_by_key(|r| r.at);

    let mut events = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (ra, rb) = (a[i], b[j]);
        let delta = (ra.at - rb.at).abs().to_std().unwrap_or(Duration::MAX);
        if delta <= window {
            let at = ra.at.min(rb.at);
            let (lon, lat) = if ra.lon.is_some() && ra.lat.is_some() {
                (ra.lon, ra.lat)
            } else {
                (rb.lon, rb.lat)
            };
            events.push(CoEvent { at, lon, lat });
            i += 1;
            j += 1;
        } else if ra.at < rb.at {
            i += 1;
        } else {
            j += 1;
        }
    }
    events
}

/// Decide whether the two sensors should be auto-associated. `models_match`
/// must be true (the two emitters carry the same `attributes.model`). Returns
/// `Some(confidence)` on a decision, else `None`.
///
/// - Geographic (primary): two located events ≥ `mile_meters` apart → 0.9.
/// - Time fallback: ≥ `fallback_min_events` events spread over ≥
///   `fallback_min_spread` → 0.5.
pub fn should_associate(
    events: &[CoEvent],
    models_match: bool,
    cfg: &CorrelationConfig,
) -> Option<f64> {
    if !models_match {
        return None;
    }
    // Geographic: any two located events far enough apart.
    let located: Vec<&CoEvent> = events
        .iter()
        .filter(|e| e.lon.is_some() && e.lat.is_some())
        .collect();
    for (idx, e1) in located.iter().enumerate() {
        for e2 in &located[idx + 1..] {
            let d = haversine_meters(
                e1.lon.unwrap(),
                e1.lat.unwrap(),
                e2.lon.unwrap(),
                e2.lat.unwrap(),
            );
            if d >= cfg.mile_meters {
                return Some(0.9);
            }
        }
    }
    // Time fallback.
    if events.len() >= cfg.fallback_min_events {
        if let (Some(min), Some(max)) = (
            events.iter().map(|e| e.at).min(),
            events.iter().map(|e| e.at).max(),
        ) {
            let spread = (max - min).to_std().unwrap_or(Duration::ZERO);
            if spread >= cfg.fallback_min_spread {
                return Some(0.5);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn t(sec: i64) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 7, 12, 0, 0).unwrap() + chrono::Duration::seconds(sec)
    }
    fn r(sec: i64, loc: Option<(f64, f64)>) -> Reading {
        Reading {
            at: t(sec),
            lon: loc.map(|l| l.0),
            lat: loc.map(|l| l.1),
        }
    }

    #[test]
    fn haversine_one_mile_is_about_1609m() {
        // ~1 mile north at ~ (−122, 37): 1609 m ≈ 0.01449° latitude.
        let d = haversine_meters(-122.0, 37.0, -122.0, 37.0 + 0.01449);
        assert!((d - 1609.0).abs() < 30.0, "got {d}");
    }

    #[test]
    fn cooccurrences_pairs_readings_within_window() {
        let a = vec![r(0, None), r(120, None)];
        let b = vec![r(30, None), r(200, None)];
        // 0&30 within 60s -> one event; 120&200 are 80s apart -> no event.
        let events = cooccurrences(&a, &b, Duration::from_secs(60));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].at, t(0)); // uses the earlier reading's time
    }

    #[test]
    fn geographic_rule_fires_when_two_events_over_a_mile_apart() {
        let far = (-122.0, 37.0 + 0.02); // ~2200 m north of the other
        let events = vec![
            CoEvent {
                at: t(0),
                lon: Some(-122.0),
                lat: Some(37.0),
            },
            CoEvent {
                at: t(300),
                lon: Some(far.0),
                lat: Some(far.1),
            },
        ];
        assert!(should_associate(&events, true, &CorrelationConfig::default()).is_some());
        // models must match
        assert!(should_associate(&events, false, &CorrelationConfig::default()).is_none());
    }

    #[test]
    fn geographic_rule_does_not_fire_within_a_mile() {
        let events = vec![
            CoEvent {
                at: t(0),
                lon: Some(-122.0),
                lat: Some(37.0),
            },
            CoEvent {
                at: t(300),
                lon: Some(-122.0),
                lat: Some(37.001),
            }, // ~111 m
        ];
        // Too close for geographic; only 2 events so fallback also fails.
        assert!(should_associate(&events, true, &CorrelationConfig::default()).is_none());
    }

    #[test]
    fn fallback_fires_on_enough_spread_out_events_without_location() {
        let events = vec![
            CoEvent {
                at: t(0),
                lon: None,
                lat: None,
            },
            CoEvent {
                at: t(400),
                lon: None,
                lat: None,
            },
            CoEvent {
                at: t(700),
                lon: None,
                lat: None,
            }, // >= 3 events, spread 700s >= 600s
        ];
        assert!(should_associate(&events, true, &CorrelationConfig::default()).is_some());
    }

    #[test]
    fn fallback_does_not_fire_below_thresholds() {
        let events = vec![
            CoEvent {
                at: t(0),
                lon: None,
                lat: None,
            },
            CoEvent {
                at: t(60),
                lon: None,
                lat: None,
            }, // only 2 events, small spread
        ];
        assert!(should_associate(&events, true, &CorrelationConfig::default()).is_none());
    }
}
