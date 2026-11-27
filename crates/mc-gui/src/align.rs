//! Align: every stitched map is brought onto the sample's lattice before cutting, so neighbouring selections
//! meet pixel to pixel. Each map is stitched on its own and placed by the stage, which is a few um off;
//! the crosses on it are where the lattice says they are, so: find every lattice cross inside the map,
//! and fit the correction (a shift for one cross; shift, turn and scale for more) that puts them there.

use crate::cross;
use crate::geom::{self, Affine, Lattice, Rect, P2};
use crate::pixels::Rgb;

#[derive(Clone, Debug)]
pub struct Aligned {
    /// stage as the map says -> stage as the lattice says
    pub fix: Affine,
    pub crosses: usize,
    /// what is left after the correction, um
    pub rms_um: f64,
    /// how far the correction moves the selection's centre, um
    pub moved_um: f64,
}

/// `to_stage`: the map's own placement (map pixel -> stage), without any earlier correction.
pub fn align(l: &Lattice, rect: &Rect, map: &Rgb, to_stage: &Affine) -> Result<Aligned, String> {
    let umpp = to_stage.scale();
    let p = l.pitch();
    let radius_um = (0.3 * p[0].min(p[1])).min(25.0);
    let r_px = (radius_um / umpp).round().max(6.0) as usize;
    let inv = to_stage.inverse();
    let mut pairs: Vec<(P2, P2)> = vec![];
    // every lattice node inside the selection
    let c = rect.corners().map(|q| l.coords(q));
    let (i1, j1) = (c.iter().map(|q| q[0]).fold(f64::MIN, f64::max).floor() as i32, c.iter().map(|q| q[1]).fold(f64::MIN, f64::max).floor() as i32);
    let (i0, j0) = (c.iter().map(|q| q[0]).fold(f64::MAX, f64::min).ceil() as i32, c.iter().map(|q| q[1]).fold(f64::MAX, f64::min).ceil() as i32);
    for j in j0..=j1 {
        for i in i0..=i1 {
            let node = l.at(i as f64, j as f64);
            if !rect.contains(node) {
                continue;
            }
            let uv = inv.apply(node);
            if let Some((u, v)) = cross::refine(map, uv[0], uv[1], r_px) {
                let found = to_stage.apply([u, v]);
                if geom::len(geom::sub(found, node)) < radius_um {
                    pairs.push((found, node));
                }
            }
        }
    }
    if pairs.is_empty() {
        return Err("no lattice crosses found on this map".into());
    }
    // fit, drop what does not agree (a speck taken for a cross), fit again
    let mut fix = similarity(&pairs);
    let res = |f: &Affine, ps: &[(P2, P2)]| ps.iter().map(|(a, b)| geom::len(geom::sub(f.apply(*a), *b))).collect::<Vec<f64>>();
    if pairs.len() >= 3 {
        let r = res(&fix, &pairs);
        let mut s = r.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let lim = (3.0 * s[s.len() / 2]).max(0.5);
        let kept: Vec<(P2, P2)> = pairs.iter().zip(&r).filter(|(_, e)| **e <= lim).map(|(p, _)| *p).collect();
        if kept.len() >= 2 && kept.len() < pairs.len() {
            pairs = kept;
            fix = similarity(&pairs);
        }
    }
    let r = res(&fix, &pairs);
    let rms = (r.iter().map(|e| e * e).sum::<f64>() / r.len() as f64).sqrt();
    let c = rect.center();
    Ok(Aligned { fix, crosses: pairs.len(), rms_um: rms, moved_um: geom::len(geom::sub(fix.apply(c), c)) })
}

/// The shift (one pair) or shift, turn and scale (two and more) taking the first points onto the second,
/// least squares.
fn similarity(ps: &[(P2, P2)]) -> Affine {
    let n = ps.len() as f64;
    let ca = ps.iter().fold([0.0, 0.0], |s, (a, _)| [s[0] + a[0] / n, s[1] + a[1] / n]);
    let cb = ps.iter().fold([0.0, 0.0], |s, (_, b)| [s[0] + b[0] / n, s[1] + b[1] / n]);
    let (mut sxx, mut sre, mut sim) = (0.0, 0.0, 0.0);
    for (a, b) in ps {
        let (ax, ay) = (a[0] - ca[0], a[1] - ca[1]);
        let (bx, by) = (b[0] - cb[0], b[1] - cb[1]);
        sxx += ax * ax + ay * ay;
        sre += ax * bx + ay * by;
        sim += ax * by - ay * bx;
    }
    // b - cb = (p + i q)(a - ca): p = s cos t, q = s sin t
    let (p, q) = if ps.len() >= 2 && sxx > 1e-9 { (sre / sxx, sim / sxx) } else { (1.0, 0.0) };
    Affine([p, -q, cb[0] - (p * ca[0] - q * ca[1]), q, p, cb[1] - (q * ca[0] + p * ca[1])])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map of crosses every 100 um placed 3.2 um and 0.1 degree off: Align finds the correction back.
    #[test]
    fn brings_a_map_onto_the_lattice() {
        let umpp = 0.5;
        let l = Lattice { origin: [0.0, 0.0], a: [100.0, 0.0], b: [0.0, 100.0], rms_um: 0.0 };
        let t = 0.1f64.to_radians();
        // where the map's pixels truly are
        let truth = Affine([umpp * t.cos(), -umpp * t.sin(), 3.0, umpp * t.sin(), umpp * t.cos(), -2.0]);
        let mut map = Rgb::new(700, 600);
        for v in 0..600 {
            for u in 0..700 {
                // as a camera sees it: each pixel the average over its area (4 x 4 samples)
                let mut dark = 0.0f64;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let s = truth.apply([u as f64 + (sx as f64 + 0.5) / 4.0, v as f64 + (sy as f64 + 0.5) / 4.0]);
                        let (fx, fy) = (s[0].rem_euclid(100.0), s[1].rem_euclid(100.0));
                        let near = |f: f64| f < 1.5 || f > 98.5;
                        let arm = |f: f64| f < 10.0 || f > 90.0;
                        if (near(fx) && arm(fy)) || (near(fy) && arm(fx)) {
                            dark += 1.0 / 16.0;
                        }
                    }
                }
                let k = (v * 700 + u) * 3;
                map.px[k..k + 3].copy_from_slice(&[(200.0 - 140.0 * dark).round() as u8; 3]);
            }
        }
        // the map as the stage placed it: 3.2 um and the turn off
        let placed = Affine([umpp, 0.0, 6.2, 0.0, umpp, -2.0]);
        let rect = Rect { x0: 20.0, y0: 20.0, x1: 330.0, y1: 280.0 };
        let a = align(&l, &rect, &map, &placed).unwrap();
        assert!(a.crosses >= 6, "{a:?}");
        let fixed = |u: f64, v: f64| a.fix.apply(placed.apply([u, v]));
        for (u, v) in [(50.0, 50.0), (650.0, 80.0), (300.0, 550.0)] {
            let e = geom::len(geom::sub(fixed(u, v), truth.apply([u, v])));
            assert!(e < 0.2, "{e} at {u},{v}: {a:?}");
        }
    }
}
