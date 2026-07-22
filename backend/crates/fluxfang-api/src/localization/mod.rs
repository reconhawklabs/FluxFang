//! RSSI-weighted emitter localization (pure math; no DB).
//!
//! Estimates an emitter's real-world position from the GPS location + RSSI of
//! its emissions across space/time. See
//! `docs/superpowers/specs/2026-07-22-emitter-rssi-localization-design.md`.
//!
//! Pipeline: [`binning::bin`] groups observations into location bins and
//! averages each bin's RSSI over its most recent readings; [`estimate::localize`]
//! then computes an RSSI-weighted centroid (with a guarded trilateration
//! refine) plus an uncertainty radius. All functions here are pure and
//! unit-tested with synthetic geometry — no DB.

pub mod binning;
pub mod estimate;
pub mod pass;

/// Spatial bin size in metres — observations snapped to the same cell are one
/// "tug" (see spec). Prevents a stationary vantage from over-voting.
pub const BIN_GRID_M: f64 = 5.0;
/// Per bin, average RSSI over at most this many most-recent readings. Gives
/// volume-adaptive smoothing: a chatty emitter's K samples span seconds, a
/// quiet one's span minutes — same noise suppression.
pub const K_PER_BIN: usize = 20;
/// Minimum distinct bins to produce any estimate (can't localize from one spot).
pub const MIN_BINS: usize = 3;
/// Minimum bins before the trilateration refine is even attempted.
pub const REFINE_MIN_BINS: usize = 4;
/// Log-distance path-loss exponent for the refine (indoor-ish).
pub const PATHLOSS_N: f64 = 3.0;
/// Accept the refined point only if its fit RMS residual is below this (dB).
pub const REFINE_MAX_RESIDUAL_DB: f64 = 6.0;
/// The refine may not move the estimate more than this far from the centroid.
pub const REFINE_MAX_MOVE_M: f64 = 30.0;

/// One located signal observation of an emitter: the capturing node's position
/// (lon/lat), the measured RSSI, and when it was observed (epoch millis, used
/// only to pick the most-recent readings per bin).
#[derive(Debug, Clone, Copy)]
pub struct Obs {
    pub lon: f64,
    pub lat: f64,
    pub rssi: i32,
    pub observed_at_ms: i64,
}

/// The localization result: estimated position + an uncertainty radius (metres)
/// and how many distinct bins fed it.
#[derive(Debug, Clone, Copy)]
pub struct Estimate {
    pub lon: f64,
    pub lat: f64,
    pub uncertainty_m: f64,
    pub bin_count: usize,
}

pub use estimate::localize;
