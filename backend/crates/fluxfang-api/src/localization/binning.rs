//! Spatial binning: project observations to a local metric plane, snap to a
//! grid, and average each cell's RSSI over its most-recent readings.

use std::collections::HashMap;

use super::{Obs, BIN_GRID_M, K_PER_BIN};

/// Metres per degree of latitude (and of longitude at the equator).
const M_PER_DEG: f64 = 111_320.0;

/// Equirectangular projection around a reference lon/lat — good enough over the
/// small spans this operates on (a building / a short walk).
#[derive(Debug, Clone, Copy)]
pub struct Proj {
    pub lon0: f64,
    pub lat0: f64,
    cos_lat0: f64,
}

impl Proj {
    pub fn new(lon0: f64, lat0: f64) -> Self {
        Self {
            lon0,
            lat0,
            cos_lat0: lat0.to_radians().cos(),
        }
    }

    /// lon/lat -> local (x east, y north) metres.
    pub fn fwd(&self, lon: f64, lat: f64) -> (f64, f64) {
        (
            (lon - self.lon0) * self.cos_lat0 * M_PER_DEG,
            (lat - self.lat0) * M_PER_DEG,
        )
    }

    /// local (x, y) metres -> lon/lat.
    pub fn inv(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.lon0 + x / (self.cos_lat0 * M_PER_DEG),
            self.lat0 + y / M_PER_DEG,
        )
    }
}

/// One location bin: its mean position (metres, in the projection) and the RSSI
/// averaged over its most-recent readings.
#[derive(Debug, Clone, Copy)]
pub struct Bin {
    pub x: f64,
    pub y: f64,
    pub avg_rssi: f64,
    pub n: usize,
}

/// Project `obs` around their mean lon/lat, snap to a `BIN_GRID_M` grid, and
/// build one [`Bin`] per occupied cell — averaging RSSI over the most recent
/// `K_PER_BIN` readings in that cell (so a stationary vantage that floods one
/// cell counts once, not thousands of times). Returns the projection (to invert
/// the final estimate) and the bins. Empty input -> a trivial projection + no
/// bins.
pub fn bin(obs: &[Obs]) -> (Proj, Vec<Bin>) {
    if obs.is_empty() {
        return (Proj::new(0.0, 0.0), Vec::new());
    }
    let lon0 = obs.iter().map(|o| o.lon).sum::<f64>() / obs.len() as f64;
    let lat0 = obs.iter().map(|o| o.lat).sum::<f64>() / obs.len() as f64;
    let proj = Proj::new(lon0, lat0);

    // cell -> (x, y, rssi, ts)
    let mut cells: HashMap<(i64, i64), Vec<(f64, f64, i32, i64)>> = HashMap::new();
    for o in obs {
        let (x, y) = proj.fwd(o.lon, o.lat);
        let cell = (
            (x / BIN_GRID_M).round() as i64,
            (y / BIN_GRID_M).round() as i64,
        );
        cells.entry(cell).or_default().push((x, y, o.rssi, o.observed_at_ms));
    }

    let mut bins = Vec::with_capacity(cells.len());
    for (_cell, mut pts) in cells {
        // newest first, keep only the most-recent K
        pts.sort_by(|a, b| b.3.cmp(&a.3));
        pts.truncate(K_PER_BIN);
        let n = pts.len();
        let inv_n = 1.0 / n as f64;
        let avg_rssi = pts.iter().map(|p| p.2 as f64).sum::<f64>() * inv_n;
        let bx = pts.iter().map(|p| p.0).sum::<f64>() * inv_n;
        let by = pts.iter().map(|p| p.1).sum::<f64>() * inv_n;
        bins.push(Bin {
            x: bx,
            y: by,
            avg_rssi,
            n,
        });
    }
    (proj, bins)
}
