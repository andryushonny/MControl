//! Calibrating the optics with the stage as the ruler. Nine frames of one textured spot, the stage moved by
//! known amounts between them (about a third of the field), so every feature is seen at several places of the
//! field. Patches of the middle frame are found in the others (normalised cross-correlation, coarse to fine,
//! sub-pixel). For a feature seen at p in the middle frame and at p' in a frame moved by D (um):
//!     U(p') - U(p) = -m D
//! where U takes the lens distortion out. m (how stage um become pixels: scale, turn, mirror) comes out by
//! least squares for given distortion; the distortion (k1, k2) is what makes that fit best.

use crate::geom::{Optics, P2};
use crate::link::Link;
use crate::pixels;
use serde_json::json;
use std::sync::{Arc, Mutex};

pub struct Calib {
    pub phase: String,
    pub done: usize,
    pub total: usize,
    pub stop: bool,
    pub result: Option<Result<Fit, String>>,
}

#[derive(Clone, Debug)]
pub struct Fit {
    pub m: [f64; 4],
    pub k: [f64; 2],
    /// residual, full-resolution pixels: with m only (no distortion), and with m and the distortion
    pub rms_plain_px: f64,
    pub rms_px: f64,
    pub matches: usize,
}

/// One matched feature: where it is in the middle frame and in the other (full-res pixels), and the stage move.
#[derive(Clone, Copy, Debug)]
pub struct Match {
    pub p0: P2,
    pub p1: P2,
    pub d_um: P2,
}

pub fn start(link: Arc<Link>, optics: Optics, settle_ms: f64, back_um: f64) -> Arc<Mutex<Calib>> {
    let st = Arc::new(Mutex::new(Calib { phase: "taking frames".into(), done: 0, total: 9, stop: false, result: None }));
    let s2 = st.clone();
    std::thread::spawn(move || {
        let r = run(&link, &optics, settle_ms, back_um, &s2);
        s2.lock().unwrap().result = Some(r);
    });
    st
}

fn run(link: &Link, o: &Optics, settle_ms: f64, back_um: f64, st: &Mutex<Calib>) -> Result<Fit, String> {
    let here = link.stage_pos().ok_or("no stage position yet")?;
    let f = o.field_um();
    let (sx, sy) = (0.3 * f[0], 0.3 * f[1]);
    let mut spots = vec![[0.0, 0.0]];
    for (i, j) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
        spots.push([i as f64 * sx, j as f64 * sy]);
    }
    let mut frames = vec![];
    link.call(json!({"cmd": "begin"}))?;
    let shot = (|| -> Result<(), String> {
        for d in &spots {
            if st.lock().unwrap().stop {
                return Err("stopped".into());
            }
            let (x, y) = (here[0] + d[0], here[1] + d[1]);
            // always from the same side, so the stage's backlash is the same for every frame
            link.call(json!({"cmd": "goto", "x": x - back_um, "y": y - back_um}))?;
            let fr = link.snap(json!({"x": x, "y": y, "settle_ms": settle_ms}))?;
            let s = pixels::unpack(&fr.meta, &fr.data)?;
            let xyz = fr.xyz();
            frames.push((s.grey2(), [xyz[0], xyz[1]]));
            st.lock().unwrap().done += 1;
        }
        Ok(())
    })();
    let _ = link.call(json!({"cmd": "end"}));
    let _ = link.call(json!({"cmd": "goto", "x": here[0], "y": here[1]}));
    shot?;
    st.lock().unwrap().phase = "matching".into();
    let mut ms = vec![];
    let ((g0, w, h), at0) = &frames[0];
    let (w, h) = (w, h);
    for ((g1, _, _), at1) in &frames[1..] {
        let d = [at1[0] - at0[0], at1[1] - at0[1]];
        ms.extend(match_frames(g0, g1, *w, *h, o, d));
    }
    if ms.len() < 30 {
        return Err(format!("only {} features matched: calibrate on a spot with more texture", ms.len()));
    }
    st.lock().unwrap().phase = "fitting".into();
    #[cfg(test)]
    {
        *DEBUG.lock().unwrap() = ms.clone();
    }
    let first = fit_robust(&ms, o.frame);
    // second pass: with the distortion taken out, the same feature looks the same in both frames (the lens no
    // longer scales the two parts of the field differently), so the matches are sharper; back to sensor
    // pixels, fit again
    st.lock().unwrap().phase = "matching again".into();
    let o1 = Optics { m: first.m, k: first.k, ..o.clone() };
    let flat: Vec<(Vec<f32>, P2)> = frames.iter().map(|((g, w, h), at)| (undistort_grey(g, *w, *h, &o1), *at)).collect();
    let mut ms2 = vec![];
    let o_flat = Optics { k: [0.0, 0.0], ..o1.clone() };
    for (g1, at1) in &flat[1..] {
        let d = [at1[0] - flat[0].1[0], at1[1] - flat[0].1[1]];
        for m in match_frames(&flat[0].0, g1, *w, *h, &o_flat, d) {
            ms2.push(Match { p0: o1.distort(m.p0), p1: o1.distort(m.p1), d_um: m.d_um });
        }
    }
    if ms2.len() < 30 {
        return Ok(first);
    }
    Ok(fit_robust(&ms2, o.frame))
}

/// A half-size grey picture with the distortion taken out (pixel i covers full-res [2i, 2i+2)).
fn undistort_grey(g: &[f32], w: usize, h: usize, o: &Optics) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = o.distort([2.0 * x as f64 + 1.0, 2.0 * y as f64 + 1.0]);
            let (fx, fy) = (p[0] / 2.0 - 0.5, p[1] / 2.0 - 0.5);
            if fx < 0.0 || fy < 0.0 || fx > (w - 1) as f64 || fy > (h - 1) as f64 {
                continue;
            }
            let (x0, y0) = (fx as usize, fy as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (ax, ay) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
            let t = g[y0 * w + x0] * (1.0 - ax) + g[y0 * w + x1] * ax;
            let b = g[y1 * w + x0] * (1.0 - ax) + g[y1 * w + x1] * ax;
            out[y * w + x] = t * (1.0 - ay) + b * ay;
        }
    }
    out
}

#[cfg(test)]
pub static DEBUG: Mutex<Vec<Match>> = Mutex::new(Vec::new());

/// Patches of frame 0 found in frame 1 (half-size grey pictures), frame 1 taken `d_um` away.
pub fn match_frames(g0: &[f32], g1: &[f32], w: usize, h: usize, o: &Optics, d_um: P2) -> Vec<Match> {
    const T: usize = 64; // template, half-size pixels
    const S: i64 = 100; // search, half-size pixels, around where the stage says (distortion + stage error)
    let q0 = quarter(g0, w, h);
    let q1 = quarter(g1, w, h);
    let (qw, qh) = (w / 4, h / 4);
    // where content moves: a stage move d shifts the picture by -m d (full-res px)
    let [a, b, c, dd] = o.m;
    let shift = [-(a * d_um[0] + b * d_um[1]) / 2.0, -(c * d_um[0] + dd * d_um[1]) / 2.0];
    let mut out = vec![];
    let (nx, ny) = (11, 8);
    for gy in 0..ny {
        for gx in 0..nx {
            // template centre in frame 0 (half-size pixel corner coordinates of its top-left)
            let tx = (T + (w - 3 * T) * gx / (nx - 1)) as i64;
            let ty = (T + (h - 3 * T) * gy / (ny - 1)) as i64;
            let ex = tx as f64 + shift[0];
            let ey = ty as f64 + shift[1];
            if ex < -(S as f64) || ey < -(S as f64) || ex > (w + S as usize) as f64 || ey > (h + S as usize) as f64 {
                continue;
            }
            // coarse on quarter pictures
            let (ctx, cty) = (tx / 4, ty / 4);
            if !textured(&q0, qw, ctx as usize, cty as usize, T / 4) {
                continue;
            }
            let Some((cx, cy, cn)) = best(&q0, &q1, qw, qh, ctx, cty, T / 4, (ex / 4.0).round() as i64, (ey / 4.0).round() as i64, S / 4 + 2) else { continue };
            if cn < 0.5 {
                continue;
            }
            // fine on half-size pictures
            let Some((fx, fy, fnc)) = best(g0, g1, w, h, tx, ty, T, cx * 4, cy * 4, 6) else { continue };
            if fnc < 0.8 {
                continue;
            }
            let (sx, sy) = subpixel(g0, g1, w, h, tx, ty, T, fx, fy);
            // template centres, half-size -> full-res (edge based): x_full = 2 x_half
            let c0 = [2.0 * (tx as f64 + T as f64 / 2.0), 2.0 * (ty as f64 + T as f64 / 2.0)];
            let c1 = [2.0 * (fx as f64 + sx + T as f64 / 2.0), 2.0 * (fy as f64 + sy + T as f64 / 2.0)];
            out.push(Match { p0: c0, p1: c1, d_um });
        }
    }
    out
}

fn quarter(g: &[f32], w: usize, h: usize) -> Vec<f32> {
    let (qw, qh) = (w / 4, h / 4);
    let mut q = vec![0f32; qw * qh];
    for y in 0..qh {
        for x in 0..qw {
            let mut s = 0.0;
            for yy in 0..4 {
                for xx in 0..4 {
                    s += g[(4 * y + yy) * w + 4 * x + xx];
                }
            }
            q[y * qw + x] = s / 16.0;
        }
    }
    q
}

fn textured(g: &[f32], w: usize, x: usize, y: usize, t: usize) -> bool {
    let (mut s, mut s2) = (0.0f64, 0.0f64);
    for yy in y..y + t {
        for xx in x..x + t {
            let v = g[yy * w + xx] as f64;
            s += v;
            s2 += v * v;
        }
    }
    let n = (t * t) as f64;
    (s2 / n - (s / n).powi(2)).sqrt() > 2.0
}

/// NCC of the template at (tx, ty) of g0 against g1 at (x, y); None if outside.
#[allow(clippy::too_many_arguments)]
fn ncc(g0: &[f32], g1: &[f32], w: usize, h: usize, tx: i64, ty: i64, t: usize, x: i64, y: i64) -> Option<f64> {
    if x < 0 || y < 0 || x as usize + t > w || y as usize + t > h || tx < 0 || ty < 0 || tx as usize + t > w || ty as usize + t > h {
        return None;
    }
    let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for j in 0..t {
        let ra = &g0[(ty as usize + j) * w + tx as usize..][..t];
        let rb = &g1[(y as usize + j) * w + x as usize..][..t];
        for (a, b) in ra.iter().zip(rb) {
            let (a, b) = (*a as f64, *b as f64);
            sa += a;
            sb += b;
            saa += a * a;
            sbb += b * b;
            sab += a * b;
        }
    }
    let n = (t * t) as f64;
    let va = saa - sa * sa / n;
    let vb = sbb - sb * sb / n;
    (va > 1e-6 && vb > 1e-6).then(|| (sab - sa * sb / n) / (va * vb).sqrt())
}

/// The best place for the template around (ex, ey) within r, with its NCC; None if nothing, or if another
/// place (3 or more pixels away) fits almost as well: a line or a repeated pattern, not a feature.
#[allow(clippy::too_many_arguments)]
fn best(g0: &[f32], g1: &[f32], w: usize, h: usize, tx: i64, ty: i64, t: usize, ex: i64, ey: i64, r: i64) -> Option<(i64, i64, f64)> {
    let mut all = vec![];
    for y in ey - r..=ey + r {
        for x in ex - r..=ex + r {
            if let Some(v) = ncc(g0, g1, w, h, tx, ty, t, x, y) {
                all.push((x, y, v));
            }
        }
    }
    let top = *all.iter().max_by(|a, b| a.2.partial_cmp(&b.2).unwrap())?;
    let second = all.iter().filter(|p| (p.0 - top.0).abs().max((p.1 - top.1).abs()) >= 3).map(|p| p.2).fold(-1.0, f64::max);
    (top.2 - second > 0.1).then_some(top)
}

/// Parabola through the NCC at the best place and its neighbours, in x and in y: the sub-pixel offset.
#[allow(clippy::too_many_arguments)]
fn subpixel(g0: &[f32], g1: &[f32], w: usize, h: usize, tx: i64, ty: i64, t: usize, x: i64, y: i64) -> (f64, f64) {
    let v = |dx: i64, dy: i64| ncc(g0, g1, w, h, tx, ty, t, x + dx, y + dy);
    let par = |a: Option<f64>, b: f64, c: Option<f64>| match (a, c) {
        (Some(a), Some(c)) if a - 2.0 * b + c < 0.0 => (0.5 * (a - c) / (a - 2.0 * b + c)).clamp(-0.5, 0.5),
        _ => 0.0,
    };
    let c = v(0, 0).unwrap_or(0.0);
    (par(v(-1, 0), c, v(1, 0)), par(v(0, -1), c, v(0, 1)))
}

/// m for given distortion (least squares) and the residual: sum of squares (px^2) and count.
fn solve_m(ms: &[Match], o: &Optics) -> ([f64; 4], f64) {
    // U(p1) - U(p0) = -m d : for each row of m, normal equations in 2 unknowns
    let mut ata = [[0f64; 2]; 2];
    let (mut bx, mut by) = ([0f64; 2], [0f64; 2]);
    let du: Vec<P2> = ms.iter().map(|m| {
        let (a, b) = (o.undistort(m.p0), o.undistort(m.p1));
        [b[0] - a[0], b[1] - a[1]]
    }).collect();
    for (m, u) in ms.iter().zip(&du) {
        let d = m.d_um;
        for r in 0..2 {
            for c in 0..2 {
                ata[r][c] += d[r] * d[c];
            }
            bx[r] += -u[0] * d[r];
            by[r] += -u[1] * d[r];
        }
    }
    let det = ata[0][0] * ata[1][1] - ata[0][1] * ata[1][0];
    let sol = |b: [f64; 2]| [(b[0] * ata[1][1] - b[1] * ata[0][1]) / det, (ata[0][0] * b[1] - ata[1][0] * b[0]) / det];
    let (r0, r1) = (sol(bx), sol(by));
    let m = [r0[0], r0[1], r1[0], r1[1]];
    let sse = ms.iter().zip(&du).map(|(mm, u)| {
        let d = mm.d_um;
        (u[0] + m[0] * d[0] + m[1] * d[1]).powi(2) + (u[1] + m[2] * d[0] + m[3] * d[1]).powi(2)
    }).sum();
    (m, sse)
}

/// The fit with what does not agree left out: first the matches far from their frame's typical shift
/// (a false match is off by tens of pixels or more, the lens moves things by less), then, a few times over,
/// those whose residual is well above the rest.
pub fn fit_robust(ms: &[Match], frame: [u32; 2]) -> Fit {
    let r0 = ((frame[0] as f64).powi(2) + (frame[1] as f64).powi(2)).sqrt() / 2.0;
    let med = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    let mut keep: Vec<Match> = vec![];
    let mut moves: Vec<P2> = vec![];
    for m in ms {
        if !moves.iter().any(|d| d[0] == m.d_um[0] && d[1] == m.d_um[1]) {
            moves.push(m.d_um);
        }
    }
    for d in &moves {
        let of: Vec<&Match> = ms.iter().filter(|m| m.d_um == *d).collect();
        let sx = med(of.iter().map(|m| m.p1[0] - m.p0[0]).collect());
        let sy = med(of.iter().map(|m| m.p1[1] - m.p0[1]).collect());
        keep.extend(of.into_iter().filter(|m| ((m.p1[0] - m.p0[0] - sx).powi(2) + (m.p1[1] - m.p0[1] - sy).powi(2)).sqrt() < (0.12 * r0).max(30.0)));
    }
    let mut f = fit(&keep, frame);
    for _ in 0..4 {
        let o = Optics { objective: String::new(), m: f.m, frame, profile_path: None, k: f.k };
        let res: Vec<f64> = keep.iter().map(|m| residual(m, &o)).collect();
        let lim = (3.0 * med(res.clone())).max(1.0);
        let next: Vec<Match> = keep.iter().zip(&res).filter(|(_, r)| **r <= lim).map(|(m, _)| *m).collect();
        if next.len() == keep.len() || next.len() < 20 {
            break;
        }
        keep = next;
        f = fit(&keep, frame);
    }
    f
}

fn residual(mm: &Match, o: &Optics) -> f64 {
    let (a, b) = (o.undistort(mm.p0), o.undistort(mm.p1));
    let d = mm.d_um;
    let m = o.m;
    ((b[0] - a[0] + m[0] * d[0] + m[1] * d[1]).powi(2) + (b[1] - a[1] + m[2] * d[0] + m[3] * d[1]).powi(2)).sqrt()
}

pub fn fit(ms: &[Match], frame: [u32; 2]) -> Fit {
    let optics = |k: [f64; 2]| Optics { objective: String::new(), m: [1.0, 0.0, 0.0, 1.0], frame, profile_path: None, k };
    let cost = |k: [f64; 2]| solve_m(ms, &optics(k)).1;
    let (_, plain) = solve_m(ms, &optics([0.0, 0.0]));
    // Nelder-Mead on (k1, k2)
    let mut s = [[0.0, 0.0], [0.02, 0.0], [0.0, 0.02]];
    let mut f = s.map(cost);
    for _ in 0..300 {
        let mut idx = [0, 1, 2];
        idx.sort_by(|&i, &j| f[i].partial_cmp(&f[j]).unwrap());
        let (b, g, wst) = (idx[0], idx[1], idx[2]);
        if (f[wst] - f[b]).abs() < 1e-12 * (1.0 + f[b]) {
            break;
        }
        let c = [(s[b][0] + s[g][0]) / 2.0, (s[b][1] + s[g][1]) / 2.0];
        let at = |t: f64| [c[0] + t * (s[wst][0] - c[0]), c[1] + t * (s[wst][1] - c[1])];
        let r = at(-1.0);
        let fr = cost(r);
        if fr < f[b] {
            let e = at(-2.0);
            let fe = cost(e);
            if fe < fr {
                (s[wst], f[wst]) = (e, fe);
            } else {
                (s[wst], f[wst]) = (r, fr);
            }
        } else if fr < f[g] {
            (s[wst], f[wst]) = (r, fr);
        } else {
            let k = at(0.5);
            let fk = cost(k);
            if fk < f[wst] {
                (s[wst], f[wst]) = (k, fk);
            } else {
                for i in [g, wst] {
                    s[i] = [(s[i][0] + s[b][0]) / 2.0, (s[i][1] + s[b][1]) / 2.0];
                    f[i] = cost(s[i]);
                }
            }
        }
    }
    let bi = (0..3).min_by(|&i, &j| f[i].partial_cmp(&f[j]).unwrap()).unwrap();
    let k = s[bi];
    let (m, sse) = solve_m(ms, &optics(k));
    let n = ms.len() as f64;
    Fit { m, k, rms_plain_px: (plain / n).sqrt(), rms_px: (sse / n).sqrt(), matches: ms.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Features seen through a distorting lens at several stage positions: the fit finds the lens and m back.
    #[test]
    fn finds_distortion_and_m() {
        let truth = Optics { objective: String::new(), m: [4.5, 0.08, -0.08, 4.5], frame: [2592, 1944], profile_path: None, k: [0.06, -0.01] };
        let [a, b, c, d] = truth.m;
        let mut ms = vec![];
        for (i, dm) in [[-250.0, -180.0], [250.0, 0.0], [0.0, 180.0], [250.0, 180.0], [-250.0, 0.0]].iter().enumerate() {
            for gy in 0..8 {
                for gx in 0..11 {
                    // an ideal pixel in the middle frame; in the moved frame it is at q - m d
                    let q = [150.0 + 2290.0 * gx as f64 / 10.0 + i as f64, 150.0 + 1640.0 * gy as f64 / 7.0];
                    let q1 = [q[0] - (a * dm[0] + b * dm[1]), q[1] - (c * dm[0] + d * dm[1])];
                    if q1[0] < 0.0 || q1[1] < 0.0 || q1[0] > 2592.0 || q1[1] > 1944.0 {
                        continue;
                    }
                    ms.push(Match { p0: truth.distort(q), p1: truth.distort(q1), d_um: *dm });
                }
            }
        }
        // a fifth of the matches false
        let n = ms.len();
        for k in (0..n).step_by(5) {
            ms[k].p1[0] += 150.0 + k as f64;
            ms[k].p1[1] -= 90.0;
        }
        let f = fit_robust(&ms, truth.frame);
        assert!((f.k[0] - 0.06).abs() < 1e-3 && (f.k[1] + 0.01).abs() < 2e-3, "{:?}", f);
        assert!(f.m.iter().zip(truth.m).all(|(x, y)| (x - y).abs() < 1e-3), "{:?}", f);
        assert!(f.rms_px < 0.01 && f.rms_plain_px > 1.0, "{:?}", f);
    }
}
