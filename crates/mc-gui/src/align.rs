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
#[allow(dead_code)]
pub fn align(l: &Lattice, rect: &Rect, map: &Rgb, to_stage: &Affine) -> Result<Aligned, String> {
    align_to(l, rect, map, to_stage, None, false)
}

/// Onto another map (the base: the selection the lattice was marked on), cross to cross: every lattice cross in
/// both selections is found on both maps, and this map is moved so its crosses lie on the base map's. Without a
/// base (or no cross in common), onto the lattice's nodes. `rigid`: shift and turn only (the same objective: its
/// scale is right), else also scale.
pub fn align_to(l: &Lattice, rect: &Rect, map: &Rgb, to_stage: &Affine, base: Option<(&Rect, &Rgb, &Affine)>, rigid: bool) -> Result<Aligned, String> {
    let umpp = to_stage.scale();
    let p = l.pitch();
    let radius_um = (0.3 * p[0].min(p[1])).min(25.0);
    let r_px = |a: &Affine| (radius_um / a.scale()).round().max(6.0) as usize;
    let _ = umpp;
    let find = |m: &Rgb, a: &Affine, near: P2| -> Option<P2> {
        let uv = a.inverse().apply(near);
        let (u, v) = cross::refine(m, uv[0], uv[1], r_px(a))?;
        let found = a.apply([u, v]);
        (geom::len(geom::sub(found, near)) < radius_um).then_some(found)
    };
    let nodes = |r: &Rect| -> Vec<P2> {
        let c = r.corners().map(|q| l.coords(q));
        let (i1, j1) = (c.iter().map(|q| q[0]).fold(f64::MIN, f64::max).floor() as i32, c.iter().map(|q| q[1]).fold(f64::MIN, f64::max).floor() as i32);
        let (i0, j0) = (c.iter().map(|q| q[0]).fold(f64::MAX, f64::min).ceil() as i32, c.iter().map(|q| q[1]).fold(f64::MAX, f64::min).ceil() as i32);
        let mut v = vec![];
        for j in j0..=j1 {
            for i in i0..=i1 {
                let n = l.at(i as f64, j as f64);
                if r.contains(n) {
                    v.push(n);
                }
            }
        }
        v
    };
    let mut pairs: Vec<(P2, P2)> = vec![];
    // cross to cross with the base map, where both have the cross
    if let Some((brect, bmap, bplace)) = base {
        for n in nodes(rect).into_iter().filter(|n| brect.contains(*n)) {
            if let (Some(a), Some(b)) = (find(map, to_stage, n), find(bmap, bplace, n)) {
                pairs.push((a, b));
            }
        }
    }
    // else onto the lattice's nodes
    if pairs.is_empty() {
        for n in nodes(rect) {
            if let Some(a) = find(map, to_stage, n) {
                pairs.push((a, n));
            }
        }
    }
    if pairs.is_empty() {
        return Err("no lattice crosses found on this map".into());
    }
    let fit = |ps: &[(P2, P2)]| if rigid { rigid_fit(ps) } else { similarity(ps) };
    // fit, drop what does not agree (a speck taken for a cross), fit again
    let mut fix = fit(&pairs);
    let res = |f: &Affine, ps: &[(P2, P2)]| ps.iter().map(|(a, b)| geom::len(geom::sub(f.apply(*a), *b))).collect::<Vec<f64>>();
    if pairs.len() >= 3 {
        let r = res(&fix, &pairs);
        let mut s = r.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let lim = (3.0 * s[s.len() / 2]).max(0.5);
        let kept: Vec<(P2, P2)> = pairs.iter().zip(&r).filter(|(_, e)| **e <= lim).map(|(p, _)| *p).collect();
        if kept.len() >= 2 && kept.len() < pairs.len() {
            pairs = kept;
            fix = fit(&pairs);
        }
    }
    let r = res(&fix, &pairs);
    let rms = (r.iter().map(|e| e * e).sum::<f64>() / r.len() as f64).sqrt();
    let c = rect.center();
    Ok(Aligned { fix, crosses: pairs.len(), rms_um: rms, moved_um: geom::len(geom::sub(fix.apply(c), c)) })
}

/// Shift and turn (no scale) taking the first points onto the second, least squares.
fn rigid_fit(ps: &[(P2, P2)]) -> Affine {
    let s = similarity(ps);
    let [p, mq, _, q, _, _] = s.0;
    let _ = mq;
    let k = (p * p + q * q).sqrt();
    let (p, q) = if k > 1e-12 { (p / k, q / k) } else { (1.0, 0.0) };
    let n = ps.len() as f64;
    let ca = ps.iter().fold([0.0, 0.0], |s, (a, _)| [s[0] + a[0] / n, s[1] + a[1] / n]);
    let cb = ps.iter().fold([0.0, 0.0], |s, (_, b)| [s[0] + b[0] / n, s[1] + b[1] / n]);
    Affine([p, -q, cb[0] - (p * ca[0] - q * ca[1]), q, p, cb[1] - (q * ca[0] + p * ca[1])])
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

    /// A crosses map: pixel (u, v) shows the stage point truth(u, v); crosses every 100 um.
    fn crosses_map(truth: &Affine, w: usize, h: usize) -> Rgb {
        let mut map = Rgb::new(w, h);
        for v in 0..h {
            for u in 0..w {
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
                let k = (v * w + u) * 3;
                map.px[k..k + 3].copy_from_slice(&[(200.0 - 140.0 * dark).round() as u8; 3]);
            }
        }
        map
    }

    /// Two maps of the same crosses: the base placed 2 um off, the other 5 um and 0.2 degree off. Aligned onto
    /// the base (not onto the lattice), its crosses land on the base map's, to well under a pixel; the lattice
    /// marked by hand is off itself (1.5 um), which must not matter.
    #[test]
    fn brings_a_map_onto_the_base_map() {
        let umpp = 0.5;
        let lattice = Lattice { origin: [1.5, -1.0], a: [100.0, 0.0], b: [0.0, 100.0], rms_um: 0.0 };
        let base_truth = Affine([umpp, 0.0, 0.0, 0.0, umpp, 0.0]);
        let base_map = crosses_map(&base_truth, 800, 700);
        let base_place = Affine([umpp, 0.0, 2.0, 0.0, umpp, 0.0]);
        let t = 0.2f64.to_radians();
        let truth = Affine([umpp * t.cos(), -umpp * t.sin(), 60.0, umpp * t.sin(), umpp * t.cos(), 40.0]);
        let map = crosses_map(&truth, 500, 450);
        let placed = Affine([umpp, 0.0, 65.0, 0.0, umpp, 40.0]);
        let base_rect = Rect { x0: 0.0, y0: 0.0, x1: 400.0, y1: 350.0 };
        let rect = Rect { x0: 60.0, y0: 40.0, x1: 300.0, y1: 260.0 };
        let a = align_to(&lattice, &rect, &map, &placed, Some((&base_rect, &base_map, &base_place)), true).unwrap();
        assert!(a.crosses >= 4, "{a:?}");
        // where the base map shows the point this map's pixel sees
        let on_base = |u: f64, v: f64| base_place.apply(base_truth.inverse().apply(truth.apply([u, v])));
        for (u, v) in [(30.0, 30.0), (470.0, 40.0), (250.0, 420.0)] {
            let e = geom::len(geom::sub(a.fix.apply(placed.apply([u, v])), on_base(u, v)));
            assert!(e < 0.15, "{e} um at {u},{v}: {a:?}");
        }
    }
}
