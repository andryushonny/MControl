//! How well each stitch went: numbers measured after every scan and stitch, kept next to the map
//! (map/summary.json), with targets and their history (mean and sigma of earlier stitches with the same
//! objective and mode), so a stitch out of its usual range shows.
//!
//! From MStitch (map/metrics.json): the pairs' consistency after the solve, overlap similarity, the stage's
//! landing, objects kept whole, the brightness step over overlaps, the background left after the flat field.
//! From the scan (focus.csv): blurred frames, the spread of z, the surface's tilt, curvature and roughness.
//! From the map and the lattice: the crosses' scatter around the lattice after the best similarity, scale and
//! skew against it. From Align: what is left between this map and the base.

use crate::geom::{self, Affine, Lattice, Rect, P2};
use crate::pixels::Rgb;
use serde_json::{json, Value};
use std::path::Path;

/// One number to watch: where it sits in the json, its name, unit, the target (None: watched for its history
/// only), and whether smaller is better (else larger; `abs`: its size counts).
pub struct Metric {
    pub key: &'static str,
    pub name: &'static str,
    pub unit: &'static str,
    pub target: Option<f64>,
    pub smaller: bool,
    pub abs: bool,
    pub help: &'static str,
}

pub const METRICS: &[Metric] = &[
    Metric { key: "crosses.rms_um", name: "Crosses around the lattice, rms", unit: "um", target: Some(0.2), smaller: true, abs: false, help: "the crosses found on the map against the lattice, after the best shift, turn and scale: the map's own shape error (the main number for lithography)" },
    Metric { key: "crosses.max_um", name: "Crosses around the lattice, max", unit: "um", target: Some(0.5), smaller: true, abs: false, help: "the worst cross" },
    Metric { key: "crosses.scale_ppm", name: "Scale against the drawing", unit: "ppm", target: Some(50.0), smaller: true, abs: true, help: "how much the map's cross pitch is larger (+) or smaller than the drawing's (without a drawing chosen: than the lattice marked by hand)" },
    Metric { key: "crosses.skew_mrad", name: "Skew against the lattice", unit: "mrad", target: Some(0.1), smaller: true, abs: true, help: "the lattice's axes on the map, how far from the lattice's angle" },
    Metric { key: "align.rms_um", name: "Left after Align", unit: "um", target: Some(0.1), smaller: true, abs: false, help: "between this map's crosses and the base map's (or the lattice) after Align" },
    Metric { key: "mstitch.frame_scale_ppm", name: "Camera scale against the drawing", unit: "ppm", target: Some(100.0), smaller: true, abs: true, help: "two crosses in one frame: their distance in pixels against the drawing's; Correct um/px applies it" },
    Metric { key: "mstitch.anchor_rms_px", name: "Crosses after pinning, rms", unit: "px", target: Some(0.3), smaller: true, abs: false, help: "the crosses pinned in the stitch: how far each is left from its place" },
    Metric { key: "mstitch.anchor_drift_before_px", name: "Crosses' drift before pinning, rms", unit: "px", target: None, smaller: true, abs: false, help: "how far the crosses would have been without pinning (what the pins took out)" },
    Metric { key: "mstitch.anchors_used", name: "Crosses pinned", unit: "", target: None, smaller: false, abs: false, help: "crosses found in the frames and pinned (of those in anchors.csv)" },
    Metric { key: "mstitch.pair_rms_px", name: "Pairs after the solve, rms", unit: "px", target: Some(0.2), smaller: true, abs: false, help: "how far overlapping frames stay from their measured offset once all are placed (full-resolution px)" },
    Metric { key: "mstitch.pair_max_px", name: "Pairs after the solve, max", unit: "px", target: Some(0.5), smaller: true, abs: false, help: "the worst pair" },
    Metric { key: "mstitch.ncc_median", name: "Overlap likeness (NCC), median", unit: "", target: Some(0.5), smaller: false, abs: false, help: "how alike overlapping frames are: texture and focus" },
    Metric { key: "mstitch.reshoot", name: "Frames to take again", unit: "", target: Some(0.0), smaller: true, abs: false, help: "frames that did not fit with their neighbours" },
    Metric { key: "mstitch.objects_cut", name: "Objects cut by seams", unit: "", target: Some(0.0), smaller: true, abs: false, help: "objects larger than any one frame holds (the others are whole from one frame)" },
    Metric { key: "mstitch.objects", name: "Objects on the map", unit: "", target: None, smaller: true, abs: false, help: "crystals, crosses, dust found and kept whole" },
    Metric { key: "mstitch.seam_step_pct", name: "Brightness step over overlaps", unit: "%", target: Some(1.0), smaller: true, abs: false, help: "two frames' brightness over their common part: flat field, exposure" },
    Metric { key: "mstitch.gain_max_pct", name: "Largest brightness correction", unit: "%", target: Some(10.0), smaller: true, abs: false, help: "frames evened out against each other before blending: the largest correction (more: exposure or light changed during the scan)" },
    Metric { key: "mstitch.background_sigma_pct", name: "Background unevenness", unit: "%", target: Some(1.0), smaller: true, abs: false, help: "each frame's background after the flat field: vignetting left" },
    Metric { key: "mstitch.stage_dev_rms_um", name: "Stage landing, rms", unit: "um", target: None, smaller: true, abs: false, help: "how far frames landed from where the stage said, the common offset taken out" },
    Metric { key: "focus.blurred_pct", name: "Blurred frames", unit: "%", target: Some(2.0), smaller: true, abs: false, help: "frames less sharp than half the scan's median" },
    Metric { key: "focus.z_std_um", name: "z spread", unit: "um", target: None, smaller: true, abs: false, help: "standard deviation of the frames' z" },
    Metric { key: "surface.tilt_mrad", name: "Surface tilt", unit: "mrad", target: None, smaller: true, abs: false, help: "the plane through the frames' z" },
    Metric { key: "surface.curvature_per_m", name: "Surface curvature", unit: "1/m", target: None, smaller: true, abs: true, help: "mean curvature of the best quadratic surface (1 / radius)" },
    Metric { key: "surface.rough_um", name: "Surface roughness", unit: "um", target: None, smaller: true, abs: false, help: "z around the quadratic surface, rms" },
    Metric { key: "optics.calib_rms_px", name: "Optics calibration left", unit: "px", target: Some(0.1), smaller: true, abs: false, help: "residual of the distortion calibration used" },
];

/// A value by its dotted key.
pub fn get(v: &Value, key: &str) -> Option<f64> {
    key.split('.').try_fold(v, |v, k| v.get(k))?.as_f64()
}

/// Mean and sigma of earlier values.
pub fn history(vals: &[f64]) -> Option<(f64, f64)> {
    if vals.len() < 3 {
        return None;
    }
    let n = vals.len() as f64;
    let m = vals.iter().sum::<f64>() / n;
    let s = (vals.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();
    Some((m, s))
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Verdict {
    Good,
    /// within the target, but out of its usual range (mean +- 3 sigma)
    Unusual,
    Bad,
    Unknown,
}

pub fn verdict(m: &Metric, v: f64, hist: Option<(f64, f64)>) -> Verdict {
    let x = if m.abs { v.abs() } else { v };
    if let Some(t) = m.target {
        let ok = if m.smaller { x <= t + 1e-12 } else { x >= t - 1e-12 };
        if !ok {
            return Verdict::Bad;
        }
    }
    match hist {
        Some((mean, s)) if s > 0.0 && (v - mean).abs() > 3.0 * s => Verdict::Unusual,
        _ if m.target.is_some() => Verdict::Good,
        Some(_) => Verdict::Good,
        None => Verdict::Unknown,
    }
}

/// The scan's own numbers: focus.csv (x, y, z, sharpness, probes per frame), the surface through it.
pub fn scan_part(scan_dir: &Path) -> Value {
    let Ok(text) = std::fs::read_to_string(scan_dir.join("focus.csv")) else { return json!({}) };
    let rows: Vec<[f64; 5]> = text
        .lines()
        .skip(1)
        .filter_map(|l| {
            let v: Vec<f64> = l.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.len() >= 5).then(|| [v[0], v[1], v[2], v[3], v[4]])
        })
        .collect();
    if rows.is_empty() {
        return json!({});
    }
    let n = rows.len() as f64;
    let mut sh: Vec<f64> = rows.iter().map(|r| r[3]).collect();
    sh.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = sh[sh.len() / 2];
    let blurred = rows.iter().filter(|r| r[3] < 0.5 * med).count() as f64 / n * 100.0;
    let zm = rows.iter().map(|r| r[2]).sum::<f64>() / n;
    let zs = (rows.iter().map(|r| (r[2] - zm).powi(2)).sum::<f64>() / n).sqrt();
    let refocused = rows.iter().filter(|r| r[4] > 0.0).count() as f64 / n * 100.0;
    let mut out = json!({"focus": {"blurred_pct": blurred, "z_std_um": zs, "refocused_pct": refocused, "frames": rows.len()}});
    // the surface: z = a + b x + c y (+ d x^2 + e x y + f y^2), x and y in mm around their mean
    if rows.len() >= 6 {
        let (xm, ym) = (rows.iter().map(|r| r[0]).sum::<f64>() / n, rows.iter().map(|r| r[1]).sum::<f64>() / n);
        let pts: Vec<(f64, f64, f64)> = rows.iter().map(|r| ((r[0] - xm) / 1000.0, (r[1] - ym) / 1000.0, r[2])).collect();
        let plane = lsq(&pts, 3);
        let quad = lsq(&pts, 6);
        if let (Some(p), Some(q)) = (plane, quad) {
            // um per mm = mrad
            let tilt = (p[1] * p[1] + p[2] * p[2]).sqrt();
            // mean curvature of z(x, y): (z_xx + z_yy) / 2
            let curv = q[3] + q[5]; // z in um, x, y in mm: um / mm^2 = 1 / m
            let rough = (pts.iter().map(|(x, y, z)| (z - (q[0] + q[1] * x + q[2] * y + q[3] * x * x + q[4] * x * y + q[5] * y * y)).powi(2)).sum::<f64>() / n).sqrt();
            out["surface"] = json!({"tilt_mrad": tilt, "tilt_x_mrad": p[1], "tilt_y_mrad": p[2], "curvature_per_m": curv, "rough_um": rough});
        }
    }
    out
}

/// Least squares z on 1, x, y (k = 3) or also x^2, xy, y^2 (k = 6).
fn lsq(pts: &[(f64, f64, f64)], k: usize) -> Option<Vec<f64>> {
    let t = |x: f64, y: f64| [1.0, x, y, x * x, x * y, y * y];
    let mut a = vec![vec![0.0; k + 1]; k];
    for (x, y, z) in pts {
        let v = t(*x, *y);
        for r in 0..k {
            for c in 0..k {
                a[r][c] += v[r] * v[c];
            }
            a[r][k] += v[r] * z;
        }
    }
    for col in 0..k {
        let piv = (col..k).max_by(|&i, &j| a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap())?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        for r in 0..k {
            if r != col {
                let f = a[r][col] / a[col][col];
                for c in col..=k {
                    a[r][c] -= f * a[col][c];
                }
            }
        }
    }
    Some((0..k).map(|i| a[i][k] / a[i][i]).collect())
}

/// The crosses on a map against the lattice: every node inside the selection looked for on the map (where it is
/// drawn now), the best similarity taken out; what is left, and the similarity's scale and the affine's skew.
pub fn crosses(l: &Lattice, rect: &Rect, map: &Rgb, place: &Affine, design_pitch: Option<P2>) -> Option<Value> {
    let p = l.pitch();
    let radius_um = (0.3 * p[0].min(p[1])).min(25.0);
    let r_px = (radius_um / place.scale()).round().max(6.0) as usize;
    let inv = place.inverse();
    let c = rect.corners().map(|q| l.coords(q));
    let (i0, i1) = (c.iter().map(|q| q[0]).fold(f64::MAX, f64::min).ceil() as i32, c.iter().map(|q| q[0]).fold(f64::MIN, f64::max).floor() as i32);
    let (j0, j1) = (c.iter().map(|q| q[1]).fold(f64::MAX, f64::min).ceil() as i32, c.iter().map(|q| q[1]).fold(f64::MIN, f64::max).floor() as i32);
    // found -> node, and the node's lattice coordinates
    let mut pairs: Vec<(P2, P2, P2)> = vec![];
    for j in j0..=j1 {
        for i in i0..=i1 {
            let node = l.at(i as f64, j as f64);
            if !rect.contains(node) {
                continue;
            }
            let uv = inv.apply(node);
            if let Some((u, v)) = crate::cross::refine(map, uv[0], uv[1], r_px) {
                let f = place.apply([u, v]);
                if geom::len(geom::sub(f, node)) < radius_um {
                    pairs.push((f, node, [i as f64, j as f64]));
                }
            }
        }
    }
    if pairs.len() < 3 {
        return None;
    }
    // the found crosses in lattice coordinates, by least squares: found = O + A (i, j); A against the lattice's
    // own (a, b) gives scale and skew
    let n = pairs.len() as f64;
    let (mi, mj) = (pairs.iter().map(|p| p.2[0]).sum::<f64>() / n, pairs.iter().map(|p| p.2[1]).sum::<f64>() / n);
    let (mx, my) = (pairs.iter().map(|p| p.0[0]).sum::<f64>() / n, pairs.iter().map(|p| p.0[1]).sum::<f64>() / n);
    let (mut sii, mut sij, mut sjj, mut sxi, mut sxj, mut syi, mut syj) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for (f, _, ij) in &pairs {
        let (di, dj, dx, dy) = (ij[0] - mi, ij[1] - mj, f[0] - mx, f[1] - my);
        sii += di * di;
        sij += di * dj;
        sjj += dj * dj;
        sxi += dx * di;
        sxj += dx * dj;
        syi += dy * di;
        syj += dy * dj;
    }
    let det = sii * sjj - sij * sij;
    if det.abs() < 1e-9 {
        return None;
    }
    // columns: the map's a and b
    let a_map = [(sxi * sjj - sxj * sij) / det, (syi * sjj - syj * sij) / det];
    let b_map = [(sxj * sii - sxi * sij) / det, (syj * sii - syi * sij) / det];
    let ang = |v: P2| v[1].atan2(v[0]);
    let skew = ((ang(b_map) - ang(a_map)) - (ang(l.b) - ang(l.a))).sin() * 1000.0;
    // against the drawing's pitch when the sample has one, else against the lattice marked by hand
    let (pa, pb) = match design_pitch {
        Some(p) => (p[0], p[1]),
        None => (geom::len(l.a), geom::len(l.b)),
    };
    let scale = ((geom::len(a_map) / pa + geom::len(b_map) / pb) / 2.0 - 1.0) * 1e6;
    // scatter after the best similarity (the map may sit anywhere, turned a little)
    let sim = crate::align::similarity_pub(&pairs.iter().map(|p| (p.0, p.1)).collect::<Vec<_>>());
    let r: Vec<f64> = pairs.iter().map(|p| geom::len(geom::sub(sim.apply(p.0), p.1))).collect();
    let rms = (r.iter().map(|e| e * e).sum::<f64>() / n).sqrt();
    let max = r.iter().cloned().fold(0.0, f64::max);
    Some(json!({"n": pairs.len(), "rms_um": rms, "max_um": max, "scale_ppm": scale, "skew_mrad": skew}))
}

/// Read a stitch's summary (empty if none).
pub fn read(map_dir: &Path) -> Value {
    std::fs::read(map_dir.join("summary.json")).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_else(|| json!({}))
}

pub fn write(map_dir: &Path, v: &Value) {
    let _ = std::fs::write(map_dir.join("summary.json"), serde_json::to_vec_pretty(v).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts() {
        let m = &METRICS[0];
        assert_eq!(verdict(m, 0.1, None), Verdict::Good);
        assert_eq!(verdict(m, 0.3, None), Verdict::Bad);
        assert_eq!(verdict(m, 0.19, Some((0.05, 0.01))), Verdict::Unusual);
        assert_eq!(history(&[1.0, 2.0, 3.0]), Some((2.0, 1.0)));
    }

    #[test]
    fn surface_of_a_tilted_plane() {
        let d = std::env::temp_dir().join("mc_summary_test");
        let _ = std::fs::create_dir_all(&d);
        let mut s = String::from("x_um,y_um,z_um,sharpness,probes\n");
        for j in 0..5 {
            for i in 0..5 {
                let (x, y) = (i as f64 * 500.0, j as f64 * 500.0);
                // 2 um per mm along x
                s += &format!("{x},{y},{},{},0\n", 0.002 * x, 10.0);
            }
        }
        std::fs::write(d.join("focus.csv"), s).unwrap();
        let v = scan_part(&d);
        assert!((get(&v, "surface.tilt_mrad").unwrap() - 2.0).abs() < 1e-6, "{v}");
        assert!(get(&v, "surface.rough_um").unwrap() < 1e-6);
        assert_eq!(get(&v, "focus.blurred_pct"), Some(0.0));
    }
}
