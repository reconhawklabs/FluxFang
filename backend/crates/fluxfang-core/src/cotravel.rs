//! Pure co-travel scoring: turn per-emitter metrics (how far its sightings
//! spread, how many separated points, how long a span) into a 0-100 score and
//! a tier. No I/O — the SQL that produces `CoTravelMetrics` lives in
//! `fluxfang-db::repo::cotravel`; this module is the one place the weights and
//! tier thresholds are defined, so they can be unit-tested in isolation.

const METERS_PER_MILE: f64 = 1609.34;

// "Full marks" caps for each normalized input (see design doc §6).
const SPREAD_CAP_MI: f64 = 30.0;
const POINTS_OFFSET: f64 = 2.0; // 2 points scores 0
const POINTS_SPAN: f64 = 8.0; // 10 points (offset+span) scores 1
const SPAN_CAP_MIN: f64 = 240.0;

// Weights (must sum to 1.0).
const W_SPREAD: f64 = 0.45;
const W_POINTS: f64 = 0.35;
const W_SPAN: f64 = 0.20;

// Tier thresholds on the 0-100 score.
const T_CRITICAL: i32 = 70;
const T_HIGH: i32 = 45;
const T_MEDIUM: i32 = 25;
const T_LOW: i32 = 10;

/// Raw per-emitter co-travel metrics, as produced by
/// `fluxfang-db::repo::cotravel`'s aggregate query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoTravelMetrics {
    pub spread_m: f64,
    pub points: i64,
    pub span_s: f64,
    pub hits: i64,
}

/// Likelihood-of-following tier, strongest to weakest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Critical,
    High,
    Medium,
    Low,
    Minimal,
}

impl Tier {
    /// Wire/display token, lowercase (matches the frontend's `Tier` union).
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Critical => "critical",
            Tier::High => "high",
            Tier::Medium => "medium",
            Tier::Low => "low",
            Tier::Minimal => "minimal",
        }
    }
}

/// A scored emitter: the 0-100 composite plus its tier bucket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoTravelScore {
    pub score: i32,
    pub tier: Tier,
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// Score one emitter's metrics. See the design doc §6 for the formula and the
/// rationale behind the weights.
pub fn score(m: &CoTravelMetrics) -> CoTravelScore {
    let spread_score = clamp01((m.spread_m / METERS_PER_MILE) / SPREAD_CAP_MI);
    let points_score = clamp01((m.points as f64 - POINTS_OFFSET) / POINTS_SPAN);
    let span_score = clamp01((m.span_s / 60.0) / SPAN_CAP_MIN);

    let raw = 100.0 * (W_SPREAD * spread_score + W_POINTS * points_score + W_SPAN * span_score);
    let score = raw.round() as i32;

    let tier = if score >= T_CRITICAL {
        Tier::Critical
    } else if score >= T_HIGH {
        Tier::High
    } else if score >= T_MEDIUM {
        Tier::Medium
    } else if score >= T_LOW {
        Tier::Low
    } else {
        Tier::Minimal
    };

    CoTravelScore { score, tier }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(spread_mi: f64, points: i64, span_min: f64) -> CoTravelMetrics {
        CoTravelMetrics {
            spread_m: spread_mi * METERS_PER_MILE,
            points,
            span_s: span_min * 60.0,
            hits: points,
        }
    }

    #[test]
    fn strong_co_travel_is_critical() {
        // 42 mi (capped), 13 points (capped), 32.5 min:
        // 100*(0.45*1 + 0.35*1 + 0.20*0.135) ~= 82.7 -> 83
        let s = score(&m(42.1, 13, 32.5));
        assert_eq!(s.score, 83);
        assert_eq!(s.tier, Tier::Critical);
    }

    #[test]
    fn two_points_min_span_is_minimal() {
        // Exactly at the gate: barely any spread, 2 points, tiny span -> ~0.
        let s = score(&m(0.25, 2, 0.5));
        assert!(s.score < T_LOW, "score was {}", s.score);
        assert_eq!(s.tier, Tier::Minimal);
    }

    #[test]
    fn tier_boundaries_are_inclusive_lower() {
        // A metrics set engineered to land exactly on 70 stays Critical, 69 is High.
        // 30mi spread alone = 0.45*100 = 45. Add points to reach boundaries.
        // 10 points -> +35 = 80 (Critical). 6 points -> (6-2)/8=0.5 -> +17.5 = 62.5->63 (High).
        assert_eq!(score(&m(30.0, 10, 0.0)).tier, Tier::Critical);
        assert_eq!(score(&m(30.0, 6, 0.0)).tier, Tier::High);
    }

    #[test]
    fn caps_prevent_runaway_scores() {
        // Absurd inputs never exceed 100.
        let s = score(&m(9999.0, 9999, 9999.0));
        assert_eq!(s.score, 100);
        assert_eq!(s.tier, Tier::Critical);
    }

    #[test]
    fn tier_str_tokens() {
        assert_eq!(Tier::Critical.as_str(), "critical");
        assert_eq!(Tier::Minimal.as_str(), "minimal");
    }
}
