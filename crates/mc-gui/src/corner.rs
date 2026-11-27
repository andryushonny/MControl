//! Learning how things stand from a corner of the substrate. The operator puts a corner on the crosshair and
//! says which one it is, as they see the substrate (bottom left, ...). Three frames: here, the stage moved a
//! little along x, and along y. From them:
//!  1. the camera's orientation against the stage (which of the 8 straight ways): for each candidate m the
//!     frames moved by D should show the picture shifted by -m D (pixels); the candidate under which the
//!     shifted frames agree best wins. The corner's edges are plenty for this, texture or not;
//!  2. the corner in the picture: the point around which one quadrant differs from the other three (that
//!     one is the substrate; a convex corner covers one quadrant), and which quadrant.
//! The edges run from the corner along the picture's axes into the substrate. One of them is the substrate's
//! side running left-right for the operator: they say which (it is drawn red and blue), and with the corner's
//! name that fixes where right and up are, on the stage and on the screen.

use crate::geom::{Optics, P2};
use crate::link::Link;
use crate::pixels::{self, Sensor};
use serde_json::json;
use std::sync::{Arc, Mutex};

/// Downsampling of the frames used here (full-resolution pixels per pixel).
const DIV: usize = 8;

#[derive(Clone, Debug)]
pub struct Corner {
    /// the camera's m (straight, the scale kept)
    pub m: [f64; 4],
    /// the stage where the middle frame was taken
    pub at: P2,
    /// the corner in that frame (full-resolution pixels)
    pub px: P2,
    /// where the substrate is from the corner, in the picture: +1 / -1 along the picture's x and y
    pub quad: [f64; 2],
}

impl Corner {
    /// The corner on the stage (um).
    pub fn stage(&self, o: &Optics) -> P2 {
        Optics { m: self.m, ..o.clone() }.pixel_to_stage(self.at, self.px[0], self.px[1])
    }
    /// The two edges from the corner as stage directions (unit): along the picture's x, along its y.
    pub fn edges(&self) -> [P2; 2] {
        let [a, b, c, d] = self.m;
        let k = 1.0 / (a * d - b * c);
        let inv = |e: P2| {
            let s = [k * (d * e[0] - b * e[1]), k * (-c * e[0] + a * e[1])];
            let n = (s[0] * s[0] + s[1] * s[1]).sqrt();
            [s[0] / n, s[1] / n]
        };
        [inv([self.quad[0], 0.0]), inv([0.0, self.quad[1]])]
    }
}

pub struct Job {
    pub done: usize,
    pub total: usize,
    pub result: Option<Result<Corner, String>>,
}

pub fn start(link: Arc<Link>, optics: Optics, settle_ms: f64, back_um: f64) -> Arc<Mutex<Job>> {
    let st = Arc::new(Mutex::new(Job { done: 0, total: 3, result: None }));
    let s2 = st.clone();
    std::thread::spawn(move || {
        let r = run(&link, &optics, settle_ms, back_um, &s2);
        s2.lock().unwrap().result = Some(r);
    });
    st
}

fn run(link: &Link, o: &Optics, settle_ms: f64, back_um: f64, st: &Mutex<Job>) -> Result<Corner, String> {
    let here = link.stage_pos().ok_or("no stage position yet")?;
    let f = o.field_um();
    let d = 0.12 * f[0].min(f[1]);
    let mut frames = vec![];
    link.call(json!({"cmd": "begin"}))?;
    let shot = (|| -> Result<(), String> {
        for (dx, dy) in [(0.0, 0.0), (d, 0.0), (0.0, d)] {
            let (x, y) = (here[0] + dx, here[1] + dy);
            link.call(json!({"cmd": "goto", "x": x - back_um, "y": y - back_um}))?;
            let fr = link.snap(json!({"x": x, "y": y, "settle_ms": settle_ms}))?;
            let xyz = fr.xyz();
            frames.push((small(&pixels::unpack(&fr.meta, &fr.data)?), [xyz[0], xyz[1]]));
            st.lock().unwrap().done += 1;
        }
        Ok(())
    })();
    let _ = link.call(json!({"cmd": "end"}));
    let _ = link.call(json!({"cmd": "goto", "x": here[0], "y": here[1]}));
    shot?;
    let ((g0, w, h), at) = &frames[0];
    let moves: Vec<(&[f32], P2)> = frames[1..].iter().map(|((g, _, _), p)| (g.as_slice(), [p[0] - at[0], p[1] - at[1]])).collect();
    let m = orientation(g0, &moves, *w, *h, o.m)?;
    let (c, quad) = find_corner(g0, *w, *h).ok_or("no corner found: put a corner of the substrate (sharp, with the substrate and the background clearly different) on the crosshair")?;
    Ok(Corner { m, at: *at, px: [(c[0] + 0.5) * DIV as f64, (c[1] + 0.5) * DIV as f64], quad })
}

/// Grey, DIV times smaller.
fn small(s: &Sensor) -> (Vec<f32>, usize, usize) {
    let (g, w, h) = s.grey2();
    let k = DIV / 2;
    let (ow, oh) = (w / k, h / k);
    let mut o = vec![0f32; ow * oh];
    for y in 0..oh {
        for x in 0..ow {
            let mut sum = 0.0;
            for j in 0..k {
                for i in 0..k {
                    sum += g[(y * k + j) * w + x * k + i];
                }
            }
            o[y * ow + x] = sum / (k * k) as f32;
        }
    }
    (o, ow, oh)
}

/// The 8 straight m's (turned by 0, 90, 180, 270 degrees, mirrored or not) with this scale.
pub fn straight_ms(k: f64) -> Vec<[f64; 4]> {
    let mut v = vec![];
    for swap in [false, true] {
        for sx in [1.0, -1.0] {
            for sy in [1.0, -1.0] {
                v.push(if swap { [0.0, sx * k, sy * k, 0.0] } else { [sx * k, 0.0, 0.0, sy * k] });
            }
        }
    }
    v
}

/// How well g1 shifted by s (pixels of these small frames) agrees with g0: mean absolute difference over the
/// overlap, each side's mean taken out (a little more or less light does not count). Lower is better.
fn disagreement(g0: &[f32], g1: &[f32], w: usize, h: usize, s: P2) -> f64 {
    let (sx, sy) = (s[0].round() as i64, s[1].round() as i64);
    let (x0, x1) = (0i64.max(-sx), (w as i64).min(w as i64 - sx));
    let (y0, y1) = (0i64.max(-sy), (h as i64).min(h as i64 - sy));
    if x1 - x0 < 8 || y1 - y0 < 8 {
        return f64::MAX;
    }
    let (mut m0, mut m1, mut n) = (0.0, 0.0, 0.0);
    for y in y0..y1 {
        for x in x0..x1 {
            m0 += g0[(y * w as i64 + x) as usize] as f64;
            m1 += g1[((y + sy) * w as i64 + x + sx) as usize] as f64;
            n += 1.0;
        }
    }
    let (m0, m1) = (m0 / n, m1 / n);
    let mut e = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            e += ((g0[(y * w as i64 + x) as usize] as f64 - m0) - (g1[((y + sy) * w as i64 + x + sx) as usize] as f64 - m1)).abs();
        }
    }
    e / n
}

/// The camera's m among the 8 straight ones: the frames moved by D agree with the middle one shifted by -m D.
pub fn orientation(g0: &[f32], moves: &[(&[f32], P2)], w: usize, h: usize, m_now: [f64; 4]) -> Result<[f64; 4], String> {
    let [a, b, c, d] = m_now;
    let k = (a * d - b * c).abs().sqrt();
    let mut scored: Vec<(f64, [f64; 4])> = straight_ms(k)
        .into_iter()
        .map(|m| {
            let e: f64 = moves
                .iter()
                .map(|(g1, dm)| {
                    let s = [-(m[0] * dm[0] + m[1] * dm[1]) / DIV as f64, -(m[2] * dm[0] + m[3] * dm[1]) / DIV as f64];
                    disagreement(g0, g1, w, h, s)
                })
                .sum();
            (e, m)
        })
        .collect();
    scored.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    let (best, second) = (scored[0].0, scored[1].0);
    if !(best < 0.8 * second) {
        return Err("could not tell how the camera stands from these frames: put a corner with clear edges on the crosshair (and is the scale um/px about right?)".into());
    }
    Ok(scored[0].1)
}

/// The corner: the point in the middle part of the picture around which one quadrant differs most from the
/// other three (they being alike). Returns the point (pixels of this picture) and the odd quadrant's
/// direction (+1 / -1 along x and y).
pub fn find_corner(g: &[f32], w: usize, h: usize) -> Option<(P2, [f64; 2])> {
    // integral image: sums of any box in constant time
    let mut s = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0;
        for x in 0..w {
            row += g[y * w + x] as f64;
            s[(y + 1) * (w + 1) + x + 1] = s[y * (w + 1) + x + 1] + row;
        }
    }
    let boxm = |x0: usize, y0: usize, x1: usize, y1: usize| (s[y1 * (w + 1) + x1] - s[y0 * (w + 1) + x1] - s[y1 * (w + 1) + x0] + s[y0 * (w + 1) + x0]) / ((x1 - x0) * (y1 - y0)) as f64;
    let r = (w.min(h) / 4).max(3);
    let top = g.iter().cloned().fold(0f32, f32::max) as f64;
    let mut best: Option<(f64, P2, [f64; 2])> = None;
    for cy in r..=h - r {
        for cx in r..=w - r {
            // only the middle part of the picture: the corner should be near the crosshair
            if (cx as f64) < 0.15 * w as f64 || (cx as f64) > 0.85 * w as f64 || (cy as f64) < 0.15 * h as f64 || (cy as f64) > 0.85 * h as f64 {
                continue;
            }
            let q = [boxm(cx - r, cy - r, cx, cy), boxm(cx, cy - r, cx + r, cy), boxm(cx - r, cy, cx, cy + r), boxm(cx, cy, cx + r, cy + r)];
            for k in 0..4 {
                let others: Vec<f64> = (0..4).filter(|&j| j != k).map(|j| q[j]).collect();
                let mean = others.iter().sum::<f64>() / 3.0;
                let spread = others.iter().map(|v| (v - mean).abs()).fold(0.0, f64::max);
                let score = (q[k] - mean).abs() - 2.0 * spread;
                if best.as_ref().is_none_or(|b| score > b.0) {
                    let dir = [if k % 2 == 1 { 1.0 } else { -1.0 }, if k >= 2 { 1.0 } else { -1.0 }];
                    best = Some((score, [cx as f64, cy as f64], dir));
                }
            }
        }
    }
    let (score, c, dir) = best?;
    // the substrate and the background must differ clearly
    (score > 0.08 * top).then_some((c, dir))
}

/// Right and up for the operator as stage directions (unit), from the corner's name and which of its edges
/// runs left-right for them. `name`: [+1 left / -1 right, +1 bottom / -1 top] (where the substrate is from the
/// corner: to the right of a left corner, above a bottom one); `horizontal`: 0 = the edge along the
/// picture's x (red), 1 = along its y (blue).
pub fn right_up(c: &Corner, name: [f64; 2], horizontal: usize) -> [P2; 2] {
    let e = c.edges();
    let (h, v) = (e[horizontal], e[1 - horizontal]);
    [[name[0] * h[0], name[0] * h[1]], [name[1] * v[0], name[1] * v[1]]]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bright substrate covering the lower-right part, its corner at (cx, cy) (small-picture pixels).
    fn picture(w: usize, h: usize, cx: f64, cy: f64) -> Vec<f32> {
        let mut g = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let inside = x as f64 >= cx && y as f64 >= cy;
                // a little texture on the background, specks on the substrate
                let t = ((x * 7 + y * 13) % 11) as f32;
                g[y * w + x] = if inside { 200.0 - t } else { 40.0 + t };
            }
        }
        g
    }

    #[test]
    fn finds_the_corner_and_the_camera() {
        let (w, h) = (162usize, 121usize);
        let g0 = picture(w, h, 70.0, 55.0);
        let (c, q) = find_corner(&g0, w, h).unwrap();
        assert!((c[0] - 70.0).abs() <= 2.0 && (c[1] - 55.0).abs() <= 2.0, "{c:?}");
        assert_eq!(q, [1.0, 1.0]);
        // the camera turned 90 degrees and mirrored: m = [0, k; k, 0]; stage moves of 40 um at k = 2 px/um
        let truth = [0.0, 2.0, 2.0, 0.0];
        let mv = |d: P2| {
            let s = [-(truth[0] * d[0] + truth[1] * d[1]) / DIV as f64, -(truth[2] * d[0] + truth[3] * d[1]) / DIV as f64];
            picture(w, h, 70.0 + s[0], 55.0 + s[1])
        };
        let (gx, gy) = (mv([40.0, 0.0]), mv([0.0, 40.0]));
        let m = orientation(&g0, &[(&gx, [40.0, 0.0]), (&gy, [0.0, 40.0])], w, h, [2.0, 0.0, 0.0, 2.0]).unwrap();
        assert_eq!(m, truth);
        // the bottom-left corner (substrate to the right and above), the edge along the picture's y runs
        // left-right: right is that edge's direction on the stage, up the other's
        let corner = Corner { m, at: [0.0, 0.0], px: [0.0, 0.0], quad: q };
        let [r, u] = right_up(&corner, [1.0, 1.0], 1);
        let e = corner.edges();
        assert_eq!(r, e[1]);
        assert_eq!(u, e[0]);
    }
}
