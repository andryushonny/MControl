//! Flat field: the frame is darker towards its edges (vignetting, and the lamp lights the field unevenly).
//! 25 frames around the current spot (5 x 5, the stage moved by about half a field between them), and two with the light off for the sensor's dark level, so a feature of the
//! sample is at a different place of the field in each; per Bayer plane (each colour on its own) every frame
//! is reduced to a coarse grid and divided by its mean, the median of the nine at each grid point is the
//! illumination there (features fall out of the median), smoothed, mean 1. A frame is then corrected as
//!     I'(x, y) = I(x, y) / F(x, y)
//! on the raw sensor values, before the lens's distortion is taken out (the flat belongs to the sensor).

use crate::geom::Optics;
use crate::link::Link;
use crate::pixels::{self, Sensor};
use serde_json::json;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Grid cell, in pixels of one Bayer plane.
const CELL: usize = 4;

/// The illumination of the field, per Bayer plane on a coarse grid, mean 1 per plane.
pub struct Flat {
    /// the sensor frame it was taken for
    pub w: usize,
    pub h: usize,
    /// 2 for Bayer (4 planes), 1 for mono (1 plane)
    pub step: usize,
    /// grid size per plane
    pub gw: usize,
    pub gh: usize,
    /// planes (dy, dx) in order, each gh x gw
    pub g: Vec<f32>,
    /// darkest point against the mean (for the message)
    pub min: f32,
    /// what the sensor gives with no light, per plane (subtracted before dividing; 0 when not measured)
    pub dark: Vec<f32>,
}

impl std::fmt::Debug for Flat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Flat({}x{}, grid {}x{})", self.w, self.h, self.gw, self.gh)
    }
}

impl Flat {
    /// The grid of one frame: mean of each cell per plane.
    fn grid(s: &Sensor, step: usize) -> (Vec<f32>, usize, usize) {
        let (pw, ph) = (s.w / step, s.h / step);
        let (gw, gh) = (pw / CELL, ph / CELL);
        let mut g = vec![0f32; step * step * gw * gh];
        for dy in 0..step {
            for dx in 0..step {
                let p = dy * step + dx;
                for gj in 0..gh {
                    for gi in 0..gw {
                        let mut sum = 0u32;
                        for j in 0..CELL {
                            for i in 0..CELL {
                                let (x, y) = ((gi * CELL + i) * step + dx, (gj * CELL + j) * step + dy);
                                sum += s.v[y * s.w + x] as u32;
                            }
                        }
                        g[(p * gh + gj) * gw + gi] = sum as f32 / (CELL * CELL) as f32;
                    }
                }
            }
        }
        (g, gw, gh)
    }

    /// From frames of one objective, the stage moved between them.
    /// `dark`: frames with the light off at the same exposure (may be none).
    pub fn from_frames(frames: &[Sensor], dark: &[Sensor]) -> Result<Flat, String> {
        let s0 = frames.first().ok_or("no frames")?;
        let step = if s0.fmt.starts_with("bayer") { 2 } else { 1 };
        let full = ((1u32 << s0.bits) - 1) as f32;
        let mut grids = vec![];
        let (mut gw, mut gh) = (0, 0);
        for s in frames {
            if s.w != s0.w || s.h != s0.h {
                return Err("the frames differ in size".into());
            }
            let (g, w, h) = Flat::grid(s, step);
            (gw, gh) = (w, h);
            grids.push(g);
        }
        let n = gw * gh;
        let np = step * step;
        // the dark level per plane: subtracted from every frame first (else the edges stay a little dark)
        let mut dk = vec![0f32; np];
        if !dark.is_empty() {
            for d in dark.iter().filter(|d| d.w == s0.w && d.h == s0.h) {
                let (g, _, _) = Flat::grid(d, step);
                for p in 0..np {
                    dk[p] += g[p * n..(p + 1) * n].iter().sum::<f32>() / n as f32 / dark.len() as f32;
                }
            }
        }
        for g in grids.iter_mut() {
            for p in 0..np {
                g[p * n..(p + 1) * n].iter_mut().for_each(|v| *v = (*v - dk[p]).max(0.5));
            }
        }
        // each frame per plane divided by its mean: a brighter or darker part of the sample does not count
        for g in grids.iter_mut() {
            for p in 0..np {
                let plane = &mut g[p * n..(p + 1) * n];
                let mean = plane.iter().sum::<f32>() / n as f32;
                let top = plane.iter().cloned().fold(0f32, f32::max);
                if mean < 0.03 * full {
                    return Err("too dark: more light or a longer exposure".into());
                }
                if top > 0.97 * full {
                    return Err("too bright (saturated): less light or a shorter exposure".into());
                }
                plane.iter_mut().for_each(|v| *v /= mean);
            }
        }
        // the median of the frames at every grid point
        let mut med = vec![0f32; np * n];
        let mut v = vec![0f32; grids.len()];
        for i in 0..np * n {
            for (k, g) in grids.iter().enumerate() {
                v[k] = g[i];
            }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            med[i] = if v.len() % 2 == 1 { v[v.len() / 2] } else { 0.5 * (v[v.len() / 2 - 1] + v[v.len() / 2]) };
        }
        // smooth: the illumination changes slowly across the field, what is left of the sample does not; a
        // polynomial of degree 4 in x, y fitted to the grid, twice (points far off the first fit left out)
        for p in 0..np {
            let plane = &mut med[p * n..(p + 1) * n];
            let fit = smooth(plane, gw, gh);
            plane.copy_from_slice(&fit);
            let mean = plane.iter().sum::<f32>() / n as f32;
            plane.iter_mut().for_each(|v| *v /= mean);
        }
        let min = med.iter().cloned().fold(f32::MAX, f32::min);
        if min < 0.2 {
            return Err(format!("the field is lit very unevenly (darkest {:.0} % of the mean): check the lamp and the aperture", min * 100.0));
        }
        Ok(Flat { w: s0.w, h: s0.h, step, gw, gh, g: med, min, dark: dk })
    }

    /// The gain at a plane pixel (bilinear between grid points; grid point i is the centre of cell i).
    fn row(&self, p: usize, fy: f32, out: &mut [f32]) {
        let n = self.gw * self.gh;
        // past the outer grid points (the frame's last pixel or two): on along the edge's slope
        let j0 = (fy.floor().max(0.0) as usize).min(self.gh.saturating_sub(2));
        let j1 = (j0 + 1).min(self.gh - 1);
        let ay = fy - j0 as f32;
        for i in 0..self.gw {
            let a = self.g[p * n + j0 * self.gw + i];
            let b = self.g[p * n + j1 * self.gw + i];
            out[i] = a + (b - a) * ay;
        }
    }

    /// Correct a raw frame in place: each value divided by the illumination there. Frames of another size
    /// (binned, cropped) are left as they are.
    /// `strength`: 1 as measured; more corrects harder (the gain raised to this power).
    pub fn apply(&self, s: &mut Sensor, strength: f32) -> bool {
        if s.w != self.w || s.h != self.h {
            return false;
        }
        let step = self.step;
        let (pw, ph) = (s.w / step, s.h / step);
        let full = ((1u32 << s.bits) - 1) as f32;
        let c = CELL as f32;
        let off = (CELL as f32 - 1.0) / 2.0;
        // where each plane column falls between grid points
        let xs: Vec<(usize, usize, f32)> = (0..pw)
            .map(|i| {
                let fx = (i as f32 - off) / c;
                let i0 = (fx.floor().max(0.0) as usize).min(self.gw.saturating_sub(2));
                (i0, (i0 + 1).min(self.gw - 1), fx - i0 as f32)
            })
            .collect();
        let mut row = vec![0f32; self.gw];
        for dy in 0..step {
            for dx in 0..step {
                let p = dy * step + dx;
                for j in 0..ph {
                    self.row(p, (j as f32 - off) / c, &mut row);
                    let y = j * step + dy;
                    for (i, &(i0, i1, ax)) in xs.iter().enumerate() {
                        let mut g = (row[i0] + (row[i1] - row[i0]) * ax).max(0.05);
                        if strength != 1.0 {
                            g = g.powf(strength);
                        }
                        let k = y * s.w + i * step + dx;
                        let dk = self.dark.get(p).copied().unwrap_or(0.0);
                        s.v[k] = ((s.v[k] as f32 - dk).max(0.0) / g + dk).round().clamp(0.0, full) as u16;
                    }
                }
            }
        }
        true
    }

    /// File: a small header line (json) and the grid as little-endian f32.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let head = json!({"w": self.w, "h": self.h, "step": self.step, "gw": self.gw, "gh": self.gh, "cell": CELL, "min": self.min, "dark": self.dark});
        let mut out = head.to_string().into_bytes();
        out.push(b'\n');
        for v in &self.g {
            out.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Flat, String> {
        let d = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let nl = d.iter().position(|&b| b == b'\n').ok_or("flat: no header")?;
        let h: serde_json::Value = serde_json::from_slice(&d[..nl]).map_err(|e| e.to_string())?;
        let u = |k: &str| h[k].as_u64().unwrap_or(0) as usize;
        let (w, hh, step, gw, gh) = (u("w"), u("h"), u("step"), u("gw"), u("gh"));
        if u("cell") != CELL || step == 0 || gw == 0 || gh == 0 {
            return Err("flat: unknown format".into());
        }
        let g: Vec<f32> = d[nl + 1..].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        if g.len() != step * step * gw * gh {
            return Err("flat: file too short".into());
        }
        let dark = h["dark"].as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect()).unwrap_or_default();
        Ok(Flat { w, h: hh, step, gw, gh, g, min: h["min"].as_f64().unwrap_or(0.0) as f32, dark })
    }

    /// For MStitch: no flat of its own (its frames are corrected already): ones, at quarter size (level 2).
    pub fn write_ones(path: &Path, w: usize, h: usize) -> Result<(), String> {
        let (w, h) = (w / 4, h / 4);
        let mut head = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({h}, {w}, 3), }}");
        while (10 + head.len() + 1) % 64 != 0 {
            head.push(' ');
        }
        head.push('\n');
        let mut out = Vec::with_capacity(10 + head.len() + w * h * 12);
        out.extend_from_slice(b"\x93NUMPY\x01\x00");
        out.extend_from_slice(&(head.len() as u16).to_le_bytes());
        out.extend_from_slice(head.as_bytes());
        for _ in 0..w * h * 3 {
            out.extend_from_slice(&1f32.to_le_bytes());
        }
        std::fs::write(path, out).map_err(|e| e.to_string())
    }
}

const DEG: usize = 4;
const NT: usize = (DEG + 1) * (DEG + 2) / 2;

/// The terms x^i y^j, i + j <= DEG, at grid point (gi, gj) (x, y from -1 to 1).
fn terms(gi: usize, gj: usize, gw: usize, gh: usize) -> [f64; NT] {
    let x = 2.0 * gi as f64 / (gw - 1).max(1) as f64 - 1.0;
    let y = 2.0 * gj as f64 / (gh - 1).max(1) as f64 - 1.0;
    let mut t = [0.0; NT];
    let mut k = 0;
    for d in 0..=DEG {
        for j in 0..=d {
            t[k] = x.powi((d - j) as i32) * y.powi(j as i32);
            k += 1;
        }
    }
    t
}

/// The grid replaced by a least-squares polynomial of degree 4; a second fit without the points more than
/// 3 sigma off the first.
fn smooth(v: &[f32], gw: usize, gh: usize) -> Vec<f32> {
    let mut keep = vec![true; v.len()];
    let mut out = v.to_vec();
    for _ in 0..2 {
        let mut a = [[0.0f64; NT + 1]; NT];
        for gj in 0..gh {
            for gi in 0..gw {
                let k = gj * gw + gi;
                if !keep[k] {
                    continue;
                }
                let t = terms(gi, gj, gw, gh);
                for r in 0..NT {
                    for c in 0..NT {
                        a[r][c] += t[r] * t[c];
                    }
                    a[r][NT] += t[r] * v[k] as f64;
                }
            }
        }
        let Some(c) = solve(a) else { return out };
        let mut sse = 0.0;
        let mut n = 0;
        for gj in 0..gh {
            for gi in 0..gw {
                let k = gj * gw + gi;
                let t = terms(gi, gj, gw, gh);
                out[k] = t.iter().zip(&c).map(|(a, b)| a * b).sum::<f64>() as f32;
                if keep[k] {
                    sse += ((v[k] - out[k]) as f64).powi(2);
                    n += 1;
                }
            }
        }
        let sigma = (sse / n.max(1) as f64).sqrt() as f32;
        for k in 0..v.len() {
            keep[k] = (v[k] - out[k]).abs() <= 3.0 * sigma;
        }
    }
    out
}

/// Gaussian elimination with partial pivoting on an augmented NT x (NT + 1) system.
fn solve(mut a: [[f64; NT + 1]; NT]) -> Option<[f64; NT]> {
    for col in 0..NT {
        let piv = (col..NT).max_by(|&i, &j| a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap())?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        for r in 0..NT {
            if r != col {
                let f = a[r][col] / a[col][col];
                for c in col..NT + 1 {
                    a[r][c] -= f * a[col][c];
                }
            }
        }
    }
    let mut x = [0.0; NT];
    for i in 0..NT {
        x[i] = a[i][NT] / a[i][i];
    }
    Some(x)
}

pub struct Job {
    pub done: usize,
    pub total: usize,
    pub stop: bool,
    pub result: Option<Result<Flat, String>>,
}

/// Nine frames on a 3 x 3 grid around here, half a field apart; back where it started afterwards.
pub fn start(link: Arc<Link>, optics: Optics, settle_ms: f64, back_um: f64) -> Arc<Mutex<Job>> {
    let st = Arc::new(Mutex::new(Job { done: 0, total: 27, stop: false, result: None }));
    let s2 = st.clone();
    std::thread::spawn(move || {
        let r = run(&link, &optics, settle_ms, back_um, &s2);
        s2.lock().unwrap().result = Some(r);
    });
    st
}

fn run(link: &Link, o: &Optics, settle_ms: f64, back_um: f64, st: &Mutex<Job>) -> Result<Flat, String> {
    let here = link.stage_pos().ok_or("no stage position yet")?;
    let f = o.field_um();
    let mut frames = vec![];
    let mut exposure = None;
    link.call(json!({"cmd": "begin"}))?;
    let shot = (|| -> Result<(), String> {
        for j in -2..=2 {
            for i in -2..=2 {
                if st.lock().unwrap().stop {
                    return Err("stopped".into());
                }
                let (x, y) = (here[0] + 0.45 * f[0] * i as f64, here[1] + 0.45 * f[1] * j as f64);
                link.call(json!({"cmd": "goto", "x": x - back_um, "y": y - back_um}))?;
                let fr = link.snap(json!({"x": x, "y": y, "settle_ms": settle_ms}))?;
                exposure = fr.meta["exp_us"].as_f64();
                frames.push(pixels::unpack(&fr.meta, &fr.data)?);
                st.lock().unwrap().done += 1;
            }
        }
        Ok(())
    })();
    let _ = link.call(json!({"cmd": "end"}));
    let _ = link.call(json!({"cmd": "goto", "x": here[0], "y": here[1]}));
    shot?;
    // the dark level: the light off for a moment, two frames at the same exposure, the light back as it was.
    // Not used if the exposure changed (automatic exposure) or it is not dark then (light from the room).
    let mut dark = vec![];
    let level = link.lamps().0;
    if level > 0 && link.call(json!({"cmd": "light", "level": 0})).is_ok() {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        for _ in 0..2 {
            if let Ok(fr) = link.snap(json!({"x": here[0], "y": here[1], "settle_ms": settle_ms})) {
                if fr.meta["exp_us"].as_f64() == exposure {
                    if let Ok(sn) = pixels::unpack(&fr.meta, &fr.data) {
                        dark.push(sn);
                    }
                }
            }
            st.lock().unwrap().done += 1;
        }
        let _ = link.call(json!({"cmd": "light", "level": level}));
        std::thread::sleep(std::time::Duration::from_millis(800));
        let lit = frames[0].v.iter().map(|&v| v as f64).sum::<f64>() / frames[0].v.len() as f64;
        let dk = dark.iter().map(|d| d.v.iter().map(|&v| v as f64).sum::<f64>() / d.v.len() as f64).fold(0.0, f64::max);
        if dk > 0.25 * lit {
            dark.clear();
        }
    }
    Flat::from_frames(&frames, &dark)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames of a speckled sample under a field that falls off to the edges: the flat found is the fall-off,
    /// and a corrected frame of an even sample is even.
    #[test]
    fn finds_the_vignetting() {
        let (w, h) = (320usize, 240usize);
        let light = |x: usize, y: usize| {
            let (u, v) = ((x as f64 - 160.0) / 200.0, (y as f64 - 120.0) / 200.0);
            1.0 - 0.6 * (u * u + v * v)
        };
        let mut rng = 1u64;
        let mut rnd = || {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (rng >> 33) as f64 / (1u64 << 31) as f64
        };
        let mut frames = vec![];
        for _ in 0..9 {
            let mut v = vec![0u16; w * h];
            for y in 0..h {
                for x in 0..w {
                    // the sample: mostly even, dark specks here and there (different in every frame)
                    let s = if rnd() < 0.03 { 0.3 } else { 1.0 };
                    // and the sensor's dark level of 20 on top
                    v[y * w + x] = (20.0 + 180.0 * light(x, y) * s) as u16;
                }
            }
            frames.push(Sensor { w, h, bits: 8, fmt: "bayer_rg8".into(), v });
        }
        let dark = Sensor { w, h, bits: 8, fmt: "bayer_rg8".into(), v: vec![20; w * h] };
        // a darker part of the sample (half as bright): without the dark level its edges stay darker; with it, even
        let plain = Flat::from_frames(&frames, &[]).unwrap();
        let mut e1 = Sensor { w, h, bits: 8, fmt: "bayer_rg8".into(), v: (0..w * h).map(|k| (20.0 + 90.0 * light(k % w, k / w)) as u16).collect() };
        plain.apply(&mut e1, 1.0);
        let (lo1, hi1) = e1.v.iter().fold((u16::MAX, 0u16), |(a, b), &v| (a.min(v), b.max(v)));
        assert!((hi1 - lo1) as f64 / (hi1 as f64) > 0.05, "without the dark level: {lo1}..{hi1}");
        let f = Flat::from_frames(&frames, &[dark]).unwrap();
        let mut even = Sensor { w, h, bits: 8, fmt: "bayer_rg8".into(), v: (0..w * h).map(|k| (20.0 + 90.0 * light(k % w, k / w)) as u16).collect() };
        assert!(f.apply(&mut even, 1.0));
        let (lo, hi) = even.v.iter().fold((u16::MAX, 0u16), |(a, b), &v| (a.min(v), b.max(v)));

        // before: 180 * (1 - 0.6 * 0.25 * ...) at the corners, about 108..180; after: within 5 %
        assert!((hi - lo) as f64 / (hi as f64) < 0.05, "after the flat: {lo}..{hi}");
        let path = std::env::temp_dir().join("mc_flat_test.flat");
        f.save(&path).unwrap();
        let g = Flat::load(&path).unwrap();
        assert_eq!(g.g, f.g);
        assert_eq!(g.dark, f.dark);
    }
}
