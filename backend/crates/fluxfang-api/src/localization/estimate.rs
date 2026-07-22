//! The estimator: RSSI-weighted centroid + a guarded trilateration refine +
//! an uncertainty radius. Pure; unit-tested with synthetic geometry below.

use std::f64::consts::{LN_10, TAU};

use super::binning::{bin, Bin};
use super::{
    Estimate, Obs, BIN_GRID_M, MIN_BINS, PATHLOSS_N, REFINE_MAX_MOVE_M, REFINE_MAX_RESIDUAL_DB,
    REFINE_MIN_BINS,
};

/// Localize an emitter from its located+signal observations.
///
/// Returns `None` when there are fewer than [`MIN_BINS`] distinct observation
/// locations — you can't localize from a single spot.
pub fn localize(obs: &[Obs]) -> Option<Estimate> {
    let (proj, bins) = bin(obs);
    if bins.len() < MIN_BINS {
        return None;
    }

    // Linear-power weights: strong signal pulls harder.
    let ws: Vec<f64> = bins.iter().map(|b| 10f64.powf(b.avg_rssi / 10.0)).collect();
    let wsum: f64 = ws.iter().sum();
    if !(wsum > 0.0) {
        return None;
    }

    // Baseline: RSSI-weighted centroid.
    let cx = bins.iter().zip(&ws).map(|(b, w)| b.x * w).sum::<f64>() / wsum;
    let cy = bins.iter().zip(&ws).map(|(b, w)| b.y * w).sum::<f64>() / wsum;

    // Guarded refine; falls back to the centroid.
    let (ex, ey) = refine(&bins, cx, cy).unwrap_or((cx, cy));

    let uncertainty_m = uncertainty(&bins, &ws, wsum, ex, ey);
    let (lon, lat) = proj.inv(ex, ey);
    Some(Estimate {
        lon,
        lat,
        uncertainty_m,
        bin_count: bins.len(),
    })
}

/// Guarded trilateration refine. Gauss-Newton on `(px, py, A)` with the
/// path-loss exponent fixed, initialised at the centroid; the reference level
/// `A` is fit jointly so no per-device TX-power calibration is needed. Returns
/// the refined point only if geometry is good, the fit converged, its RMS
/// residual is under [`REFINE_MAX_RESIDUAL_DB`], and it stayed within
/// [`REFINE_MAX_MOVE_M`] of the centroid — otherwise `None` (keep the centroid).
fn refine(bins: &[Bin], cx: f64, cy: f64) -> Option<(f64, f64)> {
    if bins.len() < REFINE_MIN_BINS || !well_spread(bins) {
        return None;
    }
    let n = PATHLOSS_N;
    let mut px = cx;
    let mut py = cy;
    // Init A at the strongest bin's RSSI (≈ the near-field reference level).
    let mut a = bins.iter().map(|b| b.avg_rssi).fold(f64::MIN, f64::max);

    for _ in 0..12 {
        // Normal equations for r_i = rssi_i - (A - 10 n log10 d_i), minimising Σr².
        let mut jtj = [[0.0f64; 3]; 3];
        let mut jtr = [0.0f64; 3];
        for b in bins {
            let dx = px - b.x;
            let dy = py - b.y;
            let d2 = (dx * dx + dy * dy).max(0.25); // clamp at (0.5 m)^2
            let d = d2.sqrt();
            let model = a - 10.0 * n * d.log10();
            let r = b.avg_rssi - model;
            // ∂r/∂px = 10n/ln10 · dx/d² ; ∂r/∂py likewise ; ∂r/∂A = -1
            let g = 10.0 * n / LN_10;
            let j = [g * dx / d2, g * dy / d2, -1.0];
            for i in 0..3 {
                for k in 0..3 {
                    jtj[i][k] += j[i] * j[k];
                }
                jtr[i] += j[i] * r;
            }
        }
        // Light Levenberg damping for stability.
        for i in 0..3 {
            jtj[i][i] += 1e-6 * jtj[i][i].abs().max(1e-9);
        }
        let delta = solve3(jtj, jtr)?;
        // GN step is β -= (JᵀJ)⁻¹ Jᵀr.
        px -= delta[0];
        py -= delta[1];
        a -= delta[2];
        if !(px.is_finite() && py.is_finite() && a.is_finite()) {
            return None;
        }
        if delta[0].hypot(delta[1]) < 1e-3 {
            break;
        }
    }

    let rms = rms_residual(bins, px, py, a, n);
    if !rms.is_finite() || rms > REFINE_MAX_RESIDUAL_DB {
        return None;
    }
    if (px - cx).hypot(py - cy) > REFINE_MAX_MOVE_M {
        return None;
    }
    Some((px, py))
}

/// Bins span a real area and aren't near-collinear (so trilateration is
/// well-posed).
fn well_spread(bins: &[Bin]) -> bool {
    let (mut minx, mut maxx, mut miny, mut maxy) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for b in bins {
        minx = minx.min(b.x);
        maxx = maxx.max(b.x);
        miny = miny.min(b.y);
        maxy = maxy.max(b.y);
    }
    let diag = (maxx - minx).hypot(maxy - miny);
    if diag < 2.0 * BIN_GRID_M {
        return false;
    }
    // PCA: reject if the minor/major variance ratio is tiny (near-collinear).
    let nf = bins.len() as f64;
    let mx = bins.iter().map(|b| b.x).sum::<f64>() / nf;
    let my = bins.iter().map(|b| b.y).sum::<f64>() / nf;
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for b in bins {
        let dx = b.x - mx;
        let dy = b.y - my;
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let tr = sxx + syy;
    let det = sxx * syy - sxy * sxy;
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let l1 = tr / 2.0 + disc;
    let l2 = tr / 2.0 - disc;
    l1 > 0.0 && (l2 / l1) > 0.05
}

fn rms_residual(bins: &[Bin], px: f64, py: f64, a: f64, n: f64) -> f64 {
    let mut s = 0.0;
    for b in bins {
        let d = ((px - b.x).hypot(py - b.y)).max(0.5);
        let r = b.avg_rssi - (a - 10.0 * n * d.log10());
        s += r * r;
    }
    (s / bins.len() as f64).sqrt()
}

/// Uncertainty radius: the RSSI-weighted RMS distance of the bins from the
/// estimate (how tightly the strong-signal observations cluster), inflated when
/// coverage is one-sided (a big angular gap around the estimate). Floored at the
/// bin size.
fn uncertainty(bins: &[Bin], ws: &[f64], wsum: f64, ex: f64, ey: f64) -> f64 {
    let mut sd = 0.0;
    for (b, w) in bins.iter().zip(ws) {
        sd += w * ((ex - b.x).powi(2) + (ey - b.y).powi(2));
    }
    let spread = (sd / wsum).sqrt();

    // One-sidedness = the largest angular gap between bins (as seen from the
    // estimate) as a fraction of the full circle. A full ring -> ~0; all bins
    // to one side -> a gap approaching the whole circle.
    let mut angles: Vec<f64> = bins.iter().map(|b| (b.y - ey).atan2(b.x - ex)).collect();
    angles.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut max_gap = 0.0f64;
    for i in 0..angles.len() {
        let next = if i + 1 < angles.len() {
            angles[i + 1]
        } else {
            angles[0] + TAU
        };
        max_gap = max_gap.max(next - angles[i]);
    }
    let one_sided = (max_gap / TAU).clamp(0.0, 1.0);

    (spread * (1.0 + 2.0 * one_sided)).max(BIN_GRID_M)
}

/// Solve a 3x3 system `A x = b` by Gaussian elimination with partial pivoting.
/// `None` if singular.
fn solve3(mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    for col in 0..3 {
        // pivot
        let mut piv = col;
        for r in (col + 1)..3 {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        // eliminate
        for r in (col + 1)..3 {
            let f = a[r][col] / a[col][col];
            for c in col..3 {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    // back-substitute
    let mut x = [0.0; 3];
    for i in (0..3).rev() {
        let mut s = b[i];
        for c in (i + 1)..3 {
            s -= a[i][c] * x[c];
        }
        x[i] = s / a[i][i];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Small helper: build observations at a lon/lat with a given rssi. Time is
    // monotonically increasing so binning's "recent K" is deterministic.
    fn obs_at(lon: f64, lat: f64, rssi: i32, t: i64) -> Obs {
        Obs {
            lon,
            lat,
            rssi,
            observed_at_ms: t,
        }
    }

    // Metres-offset -> lon/lat around a base point (inverse of the projection's
    // scale), so tests can place bins at known metric offsets.
    fn at_m(lon0: f64, lat0: f64, east_m: f64, north_m: f64, rssi: i32, t: i64) -> Obs {
        let cos = lat0.to_radians().cos();
        obs_at(
            lon0 + east_m / (cos * 111_320.0),
            lat0 + north_m / 111_320.0,
            rssi,
            t,
        )
    }

    const LON0: f64 = -71.06;
    const LAT0: f64 = 42.36;

    // Distance in metres between two lon/lat points (small-span planar approx).
    fn dist_m(lon0: f64, lat0: f64, lon: f64, lat: f64) -> f64 {
        let cos = lat0.to_radians().cos();
        ((lon - lon0) * cos * 111_320.0).hypot((lat - lat0) * 111_320.0)
    }

    #[test]
    fn four_bins_ringing_a_point_estimate_near_center() {
        // Emitter at the origin; heard from N/E/S/W at ~30 m, all similar RSSI.
        let obs = vec![
            at_m(LON0, LAT0, 0.0, 30.0, -60, 1),
            at_m(LON0, LAT0, 30.0, 0.0, -60, 2),
            at_m(LON0, LAT0, 0.0, -30.0, -60, 3),
            at_m(LON0, LAT0, -30.0, 0.0, -60, 4),
        ];
        let est = localize(&obs).expect("should localize");
        let off = dist_m(LON0, LAT0, est.lon, est.lat);
        assert!(off < 8.0, "estimate {off:.1} m from center");
        assert_eq!(est.bin_count, 4);
        assert!(est.uncertainty_m > 0.0);
    }

    #[test]
    fn stronger_side_pulls_the_estimate_toward_it() {
        // Same ring, but the east observation is much stronger -> pull east.
        let obs = vec![
            at_m(LON0, LAT0, 0.0, 30.0, -75, 1),
            at_m(LON0, LAT0, 30.0, 0.0, -45, 2), // strong (close) on the east
            at_m(LON0, LAT0, 0.0, -30.0, -75, 3),
            at_m(LON0, LAT0, -30.0, 0.0, -75, 4),
        ];
        let est = localize(&obs).unwrap();
        let (ex, _ey) = super::super::binning::Proj::new(LON0, LAT0).fwd(est.lon, est.lat);
        assert!(ex > 3.0, "estimate should sit east of center, got x={ex:.1}");
    }

    #[test]
    fn one_sided_coverage_yields_large_uncertainty() {
        // All observations to the north -> biased + a big uncertainty circle.
        let ring = vec![
            at_m(LON0, LAT0, -20.0, 40.0, -60, 1),
            at_m(LON0, LAT0, 0.0, 45.0, -58, 2),
            at_m(LON0, LAT0, 20.0, 40.0, -60, 3),
        ];
        let est = localize(&ring).unwrap();
        // A tight all-around ring of the same spread would be much smaller; a
        // one-sided cluster inflates well past the bin size.
        assert!(est.uncertainty_m > 20.0, "unc {}", est.uncertainty_m);
    }

    #[test]
    fn sitting_still_does_not_drag_the_estimate() {
        // A ring around the origin, plus one far weak spot flooded with reads
        // (someone sitting there). The flooded spot must count as ONE bin.
        let mut ring = vec![
            at_m(LON0, LAT0, 0.0, 30.0, -60, 1),
            at_m(LON0, LAT0, 30.0, 0.0, -60, 2),
            at_m(LON0, LAT0, 0.0, -30.0, -60, 3),
            at_m(LON0, LAT0, -30.0, 0.0, -60, 4),
        ];
        let baseline = localize(&ring).unwrap();

        // Flood a distant, weak location with 5000 readings.
        for t in 0..5000 {
            ring.push(at_m(LON0, LAT0, 200.0, 200.0, -90, 1000 + t));
        }
        let flooded = localize(&ring).unwrap();
        let moved = dist_m(baseline.lon, baseline.lat, flooded.lon, flooded.lat);
        assert!(
            moved < 12.0,
            "flooding one spot moved the estimate {moved:.1} m (should be small)"
        );
    }

    #[test]
    fn fewer_than_min_bins_returns_none() {
        // Two distinct locations only.
        let obs = vec![
            at_m(LON0, LAT0, 0.0, 0.0, -50, 1),
            at_m(LON0, LAT0, 40.0, 0.0, -60, 2),
        ];
        assert!(localize(&obs).is_none());
    }

    #[test]
    fn all_readings_from_one_spot_is_one_bin_and_none() {
        let obs: Vec<Obs> = (0..100)
            .map(|t| at_m(LON0, LAT0, 0.0, 0.0, -50, t))
            .collect();
        assert!(localize(&obs).is_none(), "one location cannot localize");
    }

    #[test]
    fn multi_node_fixed_sensors_localize_inside_the_triangle() {
        // Three fixed sensors at distinct points; strongest hears it best.
        let obs = vec![
            at_m(LON0, LAT0, -40.0, -30.0, -70, 1),
            at_m(LON0, LAT0, 40.0, -30.0, -50, 2), // strongest -> nearest
            at_m(LON0, LAT0, 0.0, 40.0, -70, 3),
        ];
        let est = localize(&obs).unwrap();
        let (ex, ey) = super::super::binning::Proj::new(LON0, LAT0).fwd(est.lon, est.lat);
        // Inside the triangle's bounding box, pulled toward the strong east-south sensor.
        assert!(ex > -40.0 && ex < 40.0 && ey > -30.0 && ey < 40.0);
        assert!(ex > 0.0, "pulled toward the strongest (east) sensor, x={ex:.1}");
    }

    #[test]
    fn refine_declines_on_near_collinear_bins() {
        // Bins nearly on a line -> refine must decline; estimate == centroid.
        let obs = vec![
            at_m(LON0, LAT0, -30.0, 0.0, -60, 1),
            at_m(LON0, LAT0, -10.0, 0.3, -55, 2),
            at_m(LON0, LAT0, 10.0, -0.3, -55, 3),
            at_m(LON0, LAT0, 30.0, 0.0, -60, 4),
        ];
        // Should still localize (centroid), just not crash / diverge.
        let est = localize(&obs).unwrap();
        assert!(est.lon.is_finite() && est.lat.is_finite());
    }
}
