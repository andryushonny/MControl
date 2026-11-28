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
    /// k3 and the decentering p1, p2 (0 unless they make the fit clearly better)
    pub k3: f64,
    pub p: [f64; 2],
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

/// `orientation_only`: 3 frames, the camera's turn / mirror and scale (m), the distortion left as it is;
/// else 9 frames, m and the distortion.
pub fn start(link: Arc<Link>, optics: Optics, settle_ms: f64, back_um: f64, orientation_only: bool) -> Arc<Mutex<Calib>> {
    let st = Arc::new(Mutex::new(Calib { phase: "taking frames".into(), done: 0, total: if orientation_only { 3 } else { 9 }, stop: false, result: None }));
    let s2 = st.clone();
    std::thread::spawn(move || {
        let r = run(&link, &optics, settle_ms, back_um, orientation_only, &s2);
        s2.lock().unwrap().result = Some(r);
    });
    st
}

fn run(link: &Link, o: &Optics, settle_ms: f64, back_um: f64, quick: bool, st: &Mutex<Calib>) -> Result<Fit, String> {
    let here = link.stage_pos().ok_or("no stage position yet")?;
    let f = o.field_um();
    let (sx, sy) = (0.3 * f[0], 0.3 * f[1]);
    if quick {
        return quick_m(link, o, settle_ms, back_um, st, here);
    }
    let mut spots = vec![[0.0, 0.0]];
    let around: &[(i32, i32)] = if quick { &[(1, 0), (0, 1)] } else { &[(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] };
    for (i, j) in around {
        spots.push([*i as f64 * sx, *j as f64 * sy]);
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
    st.lock().unwrap().phase = "finding the camera's orientation".into();
    let ((g0, w, h), at0) = &frames[0];
    let (w, h) = (w, h);
    // the camera may be turned or mirrored against the stage: which of the 8 ways do the frames agree with?
    let pairs: Vec<(&[f32], P2)> = (if quick { [1usize, 2] } else { [5usize, 7] }).iter().map(|&k| (frames[k].0 .0.as_slice(), [frames[k].1[0] - at0[0], frames[k].1[1] - at0[1]])).collect();
    let m = orientation(g0, &pairs, *w, *h, o);
    let o_in = o;
    let o = &Optics { m, k: [0.0, 0.0], k3: 0.0, p: [0.0, 0.0], ..o.clone() };
    st.lock().unwrap().phase = "matching".into();
    let mut ms = vec![];
    for ((g1, _, _), at1) in &frames[1..] {
        let d = [at1[0] - at0[0], at1[1] - at0[1]];
        ms.extend(match_frames(g0, g1, *w, *h, o, d));
    }
    if ms.len() < if quick { 10 } else { 30 } {
        return Err(format!("only {} features matched: calibrate on a spot with more texture", ms.len()));
    }
    if quick {
        // the turn / mirror found; m (scale and the exact turn) fitted, the lens's distortion kept as it was
        let keep = Optics { k: o_in.k, k3: o_in.k3, p: o_in.p, ..o.clone() };
        let (m, sse) = solve_m(&ms, &keep);
        let rms = (sse / ms.len() as f64).sqrt();
        return Ok(Fit { m, k: o_in.k, k3: o_in.k3, p: o_in.p, rms_plain_px: rms, rms_px: rms, matches: ms.len() });
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
    let o1 = Optics { m: first.m, k: first.k, k3: first.k3, p: first.p, ..o.clone() };
    let flat: Vec<(Vec<f32>, P2)> = frames.iter().map(|((g, w, h), at)| (undistort_grey(g, *w, *h, &o1), *at)).collect();
    let mut ms2 = vec![];
    let o_flat = Optics { k: [0.0, 0.0], k3: 0.0, p: [0.0, 0.0], ..o1.clone() };
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

/// Orientation and scale from whole frames: here, moved by a quarter field along x, along y. Each moved frame's
/// shift against the first (pixels) is found by comparing the whole pictures (coarse to fine), so edges, dust
/// and shading are enough, no texture needed. With shifts s_x, s_y for the moves D_x, D_y:
///     [s_x s_y] = -m [D_x D_y]   ->   m = -[s_x s_y] [D_x D_y]^-1
fn quick_m(link: &Link, o: &Optics, settle_ms: f64, back_um: f64, st: &Mutex<Calib>, here: [f64; 3]) -> Result<Fit, String> {
    let f = o.field_um();
    let d = 0.25 * f[0].min(f[1]);
    let mut frames = vec![];
    link.call(json!({"cmd": "begin"}))?;
    let shot = (|| -> Result<(), String> {
        for (dx, dy) in [(0.0, 0.0), (d, 0.0), (0.0, d)] {
            if st.lock().unwrap().stop {
                return Err("stopped".into());
            }
            let (x, y) = (here[0] + dx, here[1] + dy);
            link.call(json!({"cmd": "goto", "x": x - back_um, "y": y - back_um}))?;
            let fr = link.snap(json!({"x": x, "y": y, "settle_ms": settle_ms}))?;
            let xyz = fr.xyz();
            frames.push((pixels::unpack(&fr.meta, &fr.data)?.grey2(), [xyz[0], xyz[1]]));
            st.lock().unwrap().done += 1;
        }
        Ok(())
    })();
    let _ = link.call(json!({"cmd": "end"}));
    let _ = link.call(json!({"cmd": "goto", "x": here[0], "y": here[1]}));
    shot?;
    st.lock().unwrap().phase = "comparing".into();
    let m = m_from_frames(&frames)?;
    Ok(Fit { m, k: o.k, k3: o.k3, p: o.p, rms_plain_px: 0.0, rms_px: 0.0, matches: 2 })
}

/// How the scale changes with focus: the scale (from whole frames, as Orientation) at z - dz, z, z + dz.
pub struct ZCal {
    pub done: usize,
    pub total: usize,
    /// relative change per um, and the three scales (px per um)
    pub result: Option<Result<(f64, [f64; 3]), String>>,
}

pub fn start_zscale(link: Arc<Link>, o: Optics, settle_ms: f64, back_um: f64, dz: f64) -> Arc<Mutex<ZCal>> {
    let st = Arc::new(Mutex::new(ZCal { done: 0, total: 9, result: None }));
    let s2 = st.clone();
    std::thread::spawn(move || {
        let r = run_zscale(&link, &o, settle_ms, back_um, dz, &s2);
        s2.lock().unwrap().result = Some(r);
    });
    st
}

fn run_zscale(link: &Link, o: &Optics, settle_ms: f64, back_um: f64, dz: f64, st: &Mutex<ZCal>) -> Result<(f64, [f64; 3]), String> {
    let here = link.stage_pos().ok_or("no stage position yet")?;
    let f = o.field_um();
    let d = 0.25 * f[0].min(f[1]);
    let mut k = [0.0; 3];
    link.call(json!({"cmd": "begin"}))?;
    let r = (|| -> Result<(), String> {
        for (n, z) in [here[2] - dz, here[2], here[2] + dz].into_iter().enumerate() {
            let mut frames = vec![];
            for (dx, dy) in [(0.0, 0.0), (d, 0.0), (0.0, d)] {
                let (x, y) = (here[0] + dx, here[1] + dy);
                link.call(json!({"cmd": "goto", "x": x - back_um, "y": y - back_um, "z": z}))?;
                let fr = link.snap(json!({"x": x, "y": y, "z": z, "settle_ms": settle_ms}))?;
                let xyz = fr.xyz();
                frames.push((pixels::unpack(&fr.meta, &fr.data)?.grey2(), [xyz[0], xyz[1]]));
                st.lock().unwrap().done += 1;
            }
            let m = m_from_frames(&frames).map_err(|e| format!("at z {z:.1}: {e} (a smaller step in z keeps the picture sharp enough)"))?;
            k[n] = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
        }
        Ok(())
    })();
    let _ = link.call(json!({"cmd": "end"}));
    let _ = link.call(json!({"cmd": "goto", "x": here[0], "y": here[1], "z": here[2]}));
    r?;
    Ok(((k[2] - k[0]) / (2.0 * dz * k[1]), k))
}

/// m from half-size grey frames: the first, then two taken with the stage moved (not along one line).
pub fn m_from_frames(frames: &[((Vec<f32>, usize, usize), P2)]) -> Result<[f64; 4], String> {
    let ((g0, w, h), at0) = &frames[0];
    let mut s = vec![];
    let mut dm = vec![];
    for ((g1, _, _), at1) in &frames[1..] {
        // half-size pixels -> full-resolution
        let sh = frame_shift(g0, g1, *w, *h).ok_or("the frames could not be matched: too little in the picture (an edge, a corner or dust helps), or the stage did not move")?;
        s.push([2.0 * sh[0], 2.0 * sh[1]]);
        dm.push([at1[0] - at0[0], at1[1] - at0[1]]);
    }
    let det = dm[0][0] * dm[1][1] - dm[1][0] * dm[0][1];
    if det.abs() < 1e-6 {
        return Err("the stage did not move".into());
    }
    // D^-1 for D = [[Dx0, Dy0], [Dx1, Dy1]] (columns are the moves)
    let inv = [dm[1][1] / det, -dm[1][0] / det, -dm[0][1] / det, dm[0][0] / det];
    let (sx, sy) = (s[0], s[1]);
    let m = [-(sx[0] * inv[0] + sy[0] * inv[2]), -(sx[0] * inv[1] + sy[0] * inv[3]), -(sx[1] * inv[0] + sy[1] * inv[2]), -(sx[1] * inv[1] + sy[1] * inv[3])];
    if !m.iter().all(|v| v.is_finite()) || (m[0] * m[3] - m[1] * m[2]).abs() < 1e-9 {
        return Err("the frames could not be matched".into());
    }
    Ok(m)
}

/// A grey picture made `k` times smaller (box mean).
fn shrink(g: &[f32], w: usize, h: usize, k: usize) -> (Vec<f32>, usize, usize) {
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

/// How alike g1(x + s) and g0(x) are over their overlap: the correlation coefficient (1 = the same up to
/// brightness and contrast; plain areas count for nothing). None when the overlap is under a third.
fn zncc(g0: &[f32], g1: &[f32], w: usize, h: usize, sx: i64, sy: i64) -> Option<f64> {
    let (x0, x1) = (0i64.max(-sx), (w as i64).min(w as i64 - sx));
    let (y0, y1) = (0i64.max(-sy), (h as i64).min(h as i64 - sy));
    if (x1 - x0) * (y1 - y0) * 3 < (w * h) as i64 {
        return None;
    }
    let (mut a, mut b, mut aa, mut bb, mut ab) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for y in y0..y1 {
        let (r0, r1) = ((y * w as i64) as usize, ((y + sy) * w as i64 + sx) as usize);
        for x in x0..x1 {
            let (p, q) = (g0[r0 + x as usize] as f64, g1[r1 + x as usize] as f64);
            a += p;
            b += q;
            aa += p * p;
            bb += q * q;
            ab += p * q;
        }
    }
    let n = ((x1 - x0) * (y1 - y0)) as f64;
    let (va, vb) = (aa - a * a / n, bb - b * b / n);
    if va <= 1e-9 || vb <= 1e-9 {
        return Some(0.0);
    }
    Some((ab - a * b / n) / (va * vb).sqrt())
}

/// The shift s (pixels of g0's size) with g1(x + s) = g0(x): every shift at 1/16 size, then refined at 1/4 and
/// full size, sub-pixel by a parabola. None when no shift stands out.
pub fn frame_shift(g0: &[f32], g1: &[f32], w: usize, h: usize) -> Option<P2> {
    // the slow shading out (uneven light, vignetting): it looks alike at any shift
    let r = (w.max(h) / 16).max(2);
    let (g0, g1) = (&highpass(g0, w, h, r)[..], &highpass(g1, w, h, r)[..]);
    let (c0, cw, ch) = shrink(g0, w, h, 16);
    let (c1, _, _) = shrink(g1, w, h, 16);
    let mut all = vec![];
    for sy in -(ch as i64) * 2 / 3..=(ch as i64) * 2 / 3 {
        for sx in -(cw as i64) * 2 / 3..=(cw as i64) * 2 / 3 {
            if let Some(c) = zncc(&c0, &c1, cw, ch, sx, sy) {
                all.push((c, sx, sy));
            }
        }
    }
    all.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    // a clear best: alike, and clearly more than shifts away from it
    let (best, sx0, sy0) = *all.first()?;
    // the best shift well away from it (past the width of its peak)
    let far = (cw.max(ch) / 5).max(3) as i64;
    let rival = all.iter().filter(|c| (c.1 - sx0).abs() > far || (c.2 - sy0).abs() > far).map(|c| c.0).next().unwrap_or(0.0);
    // (on smooth pictures every shift looks fairly alike: how much less unlike the best is, counts)
    if best < 0.5 || (1.0 - best) > 0.7 * (1.0 - rival) {
        return None;
    }
    let (mut sx, mut sy) = (sx0, sy0);
    let mut level = 16;
    for k in [4usize, 1] {
        let (a, aw, ah) = if k == 1 { (g0.to_vec(), w, h) } else { shrink(g0, w, h, k) };
        let (b, _, _) = if k == 1 { (g1.to_vec(), w, h) } else { shrink(g1, w, h, k) };
        let f = (level / k) as i64;
        let (cx, cy) = (sx * f, sy * f);
        let mut best = (f64::MIN, cx, cy);
        for dy in -f - 2..=f + 2 {
            for dx in -f - 2..=f + 2 {
                if let Some(c) = zncc(&a, &b, aw, ah, cx + dx, cy + dy) {
                    if c > best.0 {
                        best = (c, cx + dx, cy + dy);
                    }
                }
            }
        }
        (sx, sy) = (best.1, best.2);
        level = k;
    }
    // sub-pixel: a parabola through the neighbours along x and along y
    let e = |dx: i64, dy: i64| zncc(g0, g1, w, h, sx + dx, sy + dy).map(|c| -c);
    let vertex = |a: Option<f64>, b: Option<f64>, c: Option<f64>| match (a, b, c) {
        (Some(a), Some(b), Some(c)) if a + c - 2.0 * b > 1e-12 => (0.5 * (a - c) / (a + c - 2.0 * b)).clamp(-0.5, 0.5),
        _ => 0.0,
    };
    let fx = vertex(e(-1, 0), e(0, 0), e(1, 0));
    let fy = vertex(e(0, -1), e(0, 0), e(0, 1));
    Some([sx as f64 + fx, sy as f64 + fy])
}

/// g minus its mean over a (2r+1)^2 box (cut at the edges).
fn highpass(g: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut s = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0;
        for x in 0..w {
            row += g[y * w + x] as f64;
            s[(y + 1) * (w + 1) + x + 1] = s[y * (w + 1) + x + 1] + row;
        }
    }
    let mut o = vec![0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let sum = s[y1 * (w + 1) + x1] - s[y0 * (w + 1) + x1] - s[y1 * (w + 1) + x0] + s[y0 * (w + 1) + x0];
            o[y * w + x] = g[y * w + x] - (sum / ((x1 - x0) * (y1 - y0)) as f64) as f32;
        }
    }
    o
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

/// The camera's m among its 8 orientations (turned by 0, 90, 180, 270 degrees, mirrored or not) against the
/// stage: the one under which the most patches of frame 0 are found in the moved frames.
pub fn orientation(g0: &[f32], pairs: &[(&[f32], P2)], w: usize, h: usize, o: &Optics) -> [f64; 4] {
    let [a, b, c, d] = o.m;
    // m P, P a signed permutation (acts on the stage side)
    let ps: [[f64; 4]; 8] = [[1., 0., 0., 1.], [-1., 0., 0., 1.], [1., 0., 0., -1.], [-1., 0., 0., -1.], [0., 1., 1., 0.], [0., -1., 1., 0.], [0., 1., -1., 0.], [0., -1., -1., 0.]];
    let mut best = (o.m, 0usize);
    for p in ps {
        let m = [a * p[0] + b * p[2], a * p[1] + b * p[3], c * p[0] + d * p[2], c * p[1] + d * p[3]];
        let oo = Optics { m, k: [0.0, 0.0], ..o.clone() };
        let n: usize = pairs.iter().map(|(g1, dd)| match_frames(g0, g1, w, h, &oo, *dd).len()).sum();
        if n > best.1 {
            best = (m, n);
        }
    }
    best.0
}

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
        let o = Optics { objective: String::new(), m: f.m, frame, profile_path: None, k: f.k, camera: None, flat_file: None, flat: None, flat_strength: 1.0, calib_rms_px: None, k3: f.k3, p: f.p, scale_per_um: 0.0 };
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
    // the lens's parameters: k1, k2, k3, p1, p2
    let optics = |d: &[f64]| Optics {
        objective: String::new(),
        m: [1.0, 0.0, 0.0, 1.0],
        frame,
        profile_path: None,
        k: [d[0], d[1]],
        camera: None,
        flat_file: None,
        flat: None,
        flat_strength: 1.0,
        calib_rms_px: None,
        k3: d.get(2).copied().unwrap_or(0.0),
        p: [d.get(3).copied().unwrap_or(0.0), d.get(4).copied().unwrap_or(0.0)],
        scale_per_um: 0.0,
    };
    let cost = |d: &[f64]| solve_m(ms, &optics(d)).1;
    let (_, plain) = solve_m(ms, &optics(&[0.0, 0.0]));
    // first k1, k2; then all five from there, kept only if clearly better (else they would fit the noise)
    let two = nelder_mead(&cost, &[0.0, 0.0], 0.02, 300);
    let c2 = cost(&two);
    let five = nelder_mead(&cost, &[two[0], two[1], 0.0, 0.0, 0.0], 0.005, 1500);
    let c5 = cost(&five);
    let d = if c5 < 0.9 * c2 { five } else { vec![two[0], two[1], 0.0, 0.0, 0.0] };
    let o = optics(&d);
    let (m, sse) = solve_m(ms, &o);
    let n = ms.len() as f64;
    Fit { m, k: o.k, k3: o.k3, p: o.p, rms_plain_px: (plain / n).sqrt(), rms_px: (sse / n).sqrt(), matches: ms.len() }
}

/// Nelder-Mead minimum of f near x0 (first steps of `step` along each axis).
fn nelder_mead(f: &dyn Fn(&[f64]) -> f64, x0: &[f64], step: f64, iters: usize) -> Vec<f64> {
    let n = x0.len();
    let mut s: Vec<Vec<f64>> = (0..=n).map(|i| {
        let mut x = x0.to_vec();
        if i > 0 {
            x[i - 1] += step;
        }
        x
    }).collect();
    let mut fs: Vec<f64> = s.iter().map(|x| f(x)).collect();
    for _ in 0..iters {
        let mut idx: Vec<usize> = (0..=n).collect();
        idx.sort_by(|&i, &j| fs[i].partial_cmp(&fs[j]).unwrap());
        let (b, w, sw) = (idx[0], idx[n], idx[n - 1]);
        if (fs[w] - fs[b]).abs() < 1e-12 * (1.0 + fs[b]) {
            break;
        }
        let c: Vec<f64> = (0..n).map(|k| idx[..n].iter().map(|&i| s[i][k]).sum::<f64>() / n as f64).collect();
        let at = |t: f64| -> Vec<f64> { (0..n).map(|k| c[k] + t * (s[w][k] - c[k])).collect() };
        let r = at(-1.0);
        let fr = f(&r);
        if fr < fs[b] {
            let e = at(-2.0);
            let fe = f(&e);
            if fe < fr {
                (s[w], fs[w]) = (e, fe);
            } else {
                (s[w], fs[w]) = (r, fr);
            }
        } else if fr < fs[sw] {
            (s[w], fs[w]) = (r, fr);
        } else {
            let k = at(0.5);
            let fk = f(&k);
            if fk < fs[w] {
                (s[w], fs[w]) = (k, fk);
            } else {
                let best = s[b].clone();
                for i in 0..=n {
                    if i != b {
                        s[i] = (0..n).map(|k| (s[i][k] + best[k]) / 2.0).collect();
                        fs[i] = f(&s[i]);
                    }
                }
            }
        }
    }
    let bi = (0..=n).min_by(|&i, &j| fs[i].partial_cmp(&fs[j]).unwrap()).unwrap();
    s[bi].clone()
}

#[cfg(test)]
mod tests {
    /// Whole-frame shift on a nearly plain picture: a corner, a few specks, shading.
    #[test]
    fn shift_of_a_plain_picture() {
        let (w, h) = (648usize, 486usize);
        let pic = |ox: f64, oy: f64| {
            let mut g = vec![0f32; w * h];
            for y in 0..h {
                for x in 0..w {
                    let (u, v) = (x as f64 - ox, y as f64 - oy);
                    let mut val = if u > 300.0 && v > 200.0 { 180.0 } else { 60.0 };
                    val += 0.02 * u;
                    for (px, py) in [(100.0, 80.0), (420.0, 330.0), (250.0, 400.0)] {
                        if (u - px).powi(2) + (v - py).powi(2) < 25.0 {
                            val -= 40.0;
                        }
                    }
                    g[y * w + x] = val as f32;
                }
            }
            g
        };
        let g0 = pic(0.0, 0.0);
        // the content moved by (-121, 37): g1(x + s) = g0(x) with s = (-121, 37)
        let g1 = pic(-121.0, 37.0);
        let s = super::frame_shift(&g0, &g1, w, h).unwrap();
        assert!((s[0] + 121.0).abs() < 0.6 && (s[1] - 37.0).abs() < 0.6, "{s:?}");
    }

    use super::*;

    /// A camera mirrored in x and turned 90 degrees against the stage: the orientation is found.
    #[test]
    fn finds_a_turned_mirrored_camera() {
        let (w, h) = (648usize, 486usize); // half-size pictures of a 1296 x 972 frame
        let truth = [0.0, -4.5, -4.5, 0.0]; // stage x -> picture -y, stage y -> picture -x (full-res px per um)
        // a sample: smooth blobs, seen at the frame centre p (um); half-size pixel (x, y) shows the stage point
        // p + m^-1 (2 (x + 0.5) - 648, 2 (y + 0.5) - 486)
        let det = truth[0] * truth[3] - truth[1] * truth[2];
        let inv = [truth[3] / det, -truth[1] / det, -truth[2] / det, truth[0] / det];
        // value noise: random levels on a 2 um grid, smoothly between (nothing repeats)
        let rnd = |i: i64, j: i64| (((i.wrapping_mul(73856093) ^ j.wrapping_mul(19349663)).wrapping_mul(2654435761) >> 7) & 255) as f64;
        let sample = |x: f64, y: f64| {
            let (fx, fy) = (x / 2.0, y / 2.0);
            let (i, j) = (fx.floor() as i64, fy.floor() as i64);
            let (tx, ty) = (fx - i as f64, fy - j as f64);
            let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
            let top = rnd(i, j) * (1.0 - sx) + rnd(i + 1, j) * sx;
            let bot = rnd(i, j + 1) * (1.0 - sx) + rnd(i + 1, j + 1) * sx;
            (top * (1.0 - sy) + bot * sy) as f32
        };
        let shot = |p: P2| {
            let mut g = vec![0f32; w * h];
            for y in 0..h {
                for x in 0..w {
                    let (du, dv) = (2.0 * (x as f64 + 0.5) - 648.0, 2.0 * (y as f64 + 0.5) - 486.0);
                    g[y * w + x] = sample(p[0] + inv[0] * du + inv[1] * dv, p[1] + inv[2] * du + inv[3] * dv);
                }
            }
            g
        };
        let g0 = shot([0.0, 0.0]);
        let (gx, gy) = (shot([60.0, 0.0]), shot([0.0, 45.0]));
        let guess = Optics::guess("x", 1.0 / 4.5, [1296, 972]);
        let m = orientation(&g0, &[(&gx, [60.0, 0.0]), (&gy, [0.0, 45.0])], w, h, &guess);
        assert!(m.iter().zip(truth).all(|(a, b)| (a - b).abs() < 1e-9), "{m:?}");
    }

    /// Features seen through a distorting lens at several stage positions: the fit finds the lens and m back.
    #[test]
    fn finds_distortion_and_m() {
        let truth = Optics { objective: String::new(), m: [4.5, 0.08, -0.08, 4.5], frame: [2592, 1944], profile_path: None, k: [0.06, -0.01], camera: None, flat_file: None, flat: None, flat_strength: 1.0, calib_rms_px: None, k3: 0.0, p: [0.0, 0.0], scale_per_um: 0.0 };
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

    /// A lens with k3 and decentering too: the five are found (the residual goes to noise), and the inverse of
    /// the lens is exact.
    #[test]
    fn finds_decentering_and_k3() {
        let frame = [2592u32, 1944];
        let truth = Optics { objective: String::new(), m: [4.5, 0.02, -0.02, 4.5], frame, profile_path: None, k: [0.05, -0.01], camera: None, flat_file: None, flat: None, flat_strength: 1.0, calib_rms_px: None, k3: 0.008, p: [0.0015, -0.001], scale_per_um: 0.0 };
        // round trip
        for q in [[10.0, 10.0], [2500.0, 100.0], [1296.0, 972.0], [300.0, 1900.0]] {
            let b = truth.undistort(truth.distort(q));
            assert!((b[0] - q[0]).abs() < 1e-5 && (b[1] - q[1]).abs() < 1e-5, "{q:?} {b:?}");
        }
        let mut ms = vec![];
        let moves: Vec<P2> = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].iter().map(|(i, j)| [*i as f64 * 170.0, *j as f64 * 130.0]).collect();
        for d in &moves {
            for gy in 0..14 {
                for gx in 0..18 {
                    // an ideal point seen at q in the middle frame; in the moved one at q - m d
                    let q = [80.0 + gx as f64 * 140.0, 70.0 + gy as f64 * 135.0];
                    let m = truth.m;
                    let q1 = [q[0] - (m[0] * d[0] + m[1] * d[1]), q[1] - (m[2] * d[0] + m[3] * d[1])];
                    if q1[0] < 20.0 || q1[1] < 20.0 || q1[0] > 2570.0 || q1[1] > 1920.0 {
                        continue;
                    }
                    ms.push(Match { p0: truth.distort(q), p1: truth.distort(q1), d_um: *d });
                }
            }
        }
        let f = fit(&ms, frame);
        assert!(f.rms_px < 0.05, "{f:?}");
        assert!((f.p[0] - 0.0015).abs() < 3e-4 && (f.p[1] + 0.001).abs() < 3e-4, "{f:?}");
    }
}
