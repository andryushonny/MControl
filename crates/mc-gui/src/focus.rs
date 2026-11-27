//! Focus during scans. Every frame gets a sharpness: the variance of the Laplacian of its grey picture,
//! over the squared mean brightness (so light and exposure do not change it). A frame much less sharp
//! than the last sharp ones is looked for in z: +-5 um, then +-10, +-20, +-50 and only then +-100;
//! once one is sharper, the stage climbs that way and the step is halved down to 2.5 um. If none is
//! sharper, the frame where it was stays (a plain spot with nothing to focus on looks the same).
//! Every frame's x, y, z and sharpness are kept (focus.csv by the scan, surface.csv for the project):
//! the surface of the sample. The next frame starts from the plane through the points so far.

use crate::link::{Link, Packed};
use crate::pixels::{self, Sensor};
use serde_json::json;
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

/// Sharpness of a frame (dimensionless, larger is sharper).
pub fn sharpness(s: &Sensor) -> f64 {
    let (g, w, h) = s.grey2();
    if w < 3 || h < 3 {
        return 0.0;
    }
    let mean = g.iter().map(|v| *v as f64).sum::<f64>() / g.len() as f64;
    let (mut s1, mut s2, mut n) = (0f64, 0f64, 0f64);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let l = (4.0 * g[i] - g[i - 1] - g[i + 1] - g[i - w] - g[i + w]) as f64;
            s1 += l;
            s2 += l * l;
            n += 1.0;
        }
    }
    let var = s2 / n - (s1 / n).powi(2);
    var / mean.max(1.0).powi(2) * 1e4
}

#[derive(Clone)]
pub struct Settings {
    pub on: bool,
    /// look for focus when a frame is less sharp than this part of the last sharp ones
    pub drop: f64,
    /// how far from where the frame started it may look, um
    pub max_um: f64,
    pub settle_ms: f64,
    /// z never leaves +- this from where it started (the scan's start, or here for the focus button); 0: no limit
    pub window_um: f64,
}

pub struct Shot {
    pub frame: Arc<Packed>,
    pub sensor: Sensor,
    pub z: f64,
    pub sharpness: f64,
    /// frames taken to find focus (0: none needed)
    pub probes: usize,
}

pub struct Focus {
    pub s: Settings,
    /// sharpness of the last sharp frames
    recent: VecDeque<f64>,
    /// x, y, z, sharpness of every frame of this scan
    pub points: Vec<[f64; 4]>,
    /// frames since a search that found nothing better (a plain area: do not search at every frame of it)
    since_plain: usize,
    csv: Option<std::fs::File>,
    surface: Option<std::fs::File>,
    tag: String,
    /// the z the window is around (set by the caller: where it started)
    pub center: Option<f64>,
}

impl Focus {
    pub fn new(s: Settings, scan_dir: &std::path::Path, surface: Option<PathBuf>, tag: &str) -> Focus {
        let _ = std::fs::create_dir_all(scan_dir);
        let csv = std::fs::File::create(scan_dir.join("focus.csv")).ok().map(|mut f| {
            let _ = writeln!(f, "x_um,y_um,z_um,sharpness,probes");
            f
        });
        let surface = surface.and_then(|p| {
            let new = !p.exists();
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p).ok()?;
            if new {
                let _ = writeln!(f, "x_um,y_um,z_um,sharpness,scan");
            }
            Some(f)
        });
        Focus { s, recent: VecDeque::new(), points: vec![], since_plain: 99, csv, surface, tag: tag.into(), center: None }
    }

    /// Where z should be at (x, y): the plane through the points so far (3 or more, spread out), else the
    /// nearest point, else `z0`.
    /// Within the window around where it started?
    fn ok(&self, z: f64) -> bool {
        match self.center {
            Some(c) if self.s.window_um > 0.0 => (z - c).abs() <= self.s.window_um + 1e-9,
            _ => true,
        }
    }

    /// z brought into the window.
    pub fn clamp(&self, z: f64) -> f64 {
        match self.center {
            Some(c) if self.s.window_um > 0.0 => z.clamp(c - self.s.window_um, c + self.s.window_um),
            _ => z,
        }
    }

    pub fn predict(&self, x: f64, y: f64, z0: f64) -> f64 {
        self.clamp(self.predict_free(x, y, z0))
    }

    fn predict_free(&self, x: f64, y: f64, z0: f64) -> f64 {
        // autofocus off: z stays where it was when the scan began
        if !self.s.on {
            return z0;
        }
        let ps: Vec<&[f64; 4]> = self.points.iter().collect();
        if ps.len() >= 3 {
            // least squares z = a x + b y + c, around the points' centre
            let n = ps.len() as f64;
            let (mx, my, mz) = (ps.iter().map(|p| p[0]).sum::<f64>() / n, ps.iter().map(|p| p[1]).sum::<f64>() / n, ps.iter().map(|p| p[2]).sum::<f64>() / n);
            let (mut sxx, mut sxy, mut syy, mut sxz, mut syz) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for p in &ps {
                let (dx, dy, dz) = (p[0] - mx, p[1] - my, p[2] - mz);
                sxx += dx * dx;
                sxy += dx * dy;
                syy += dy * dy;
                sxz += dx * dz;
                syz += dy * dz;
            }
            let det = sxx * syy - sxy * sxy;
            if det > 1e-6 * (sxx * syy).max(1e-12) && sxx > 1.0 && syy > 1.0 {
                let a = (sxz * syy - syz * sxy) / det;
                let b = (syz * sxx - sxz * sxy) / det;
                return mz + a * (x - mx) + b * (y - my);
            }
        }
        ps.iter().min_by(|p, q| ((p[0] - x).powi(2) + (p[1] - y).powi(2)).partial_cmp(&((q[0] - x).powi(2) + (q[1] - y).powi(2))).unwrap())
            .map(|p| p[2]).unwrap_or(z0)
    }

    fn take(&self, link: &Link, x: f64, y: f64, z: f64) -> Result<(Arc<Packed>, Sensor, f64), String> {
        let f = link.snap(json!({"x": x, "y": y, "z": z, "settle_ms": self.s.settle_ms}))?;
        let s = pixels::unpack(&f.meta, &f.data)?;
        let sh = sharpness(&s);
        Ok((f, s, sh))
    }

    /// A frame at (x, y), in focus if focus can be found. `stop`: checked between probes.
    pub fn shot(&mut self, link: &Link, x: f64, y: f64, z_guess: f64, stop: &dyn Fn() -> bool) -> Result<Shot, String> {
        let z_guess = self.clamp(z_guess);
        let (f0, s0, sh0) = self.take(link, x, y, z_guess)?;
        let reference = {
            let mut r: Vec<f64> = self.recent.iter().copied().collect();
            r.sort_by(|a, b| a.partial_cmp(b).unwrap());
            r.get(r.len() / 2).copied()
        };
        let blurred = match reference {
            None => true, // the first frame: find focus once
            Some(r) => sh0 < self.s.drop * r,
        };
        let mut best = (z_guess, sh0, f0, s0);
        let mut probes = 0;
        // in focus: sharp as the frames before it, or made sharper by the search
        let mut good = !blurred;
        if self.s.on && blurred && self.since_plain >= 3 {
            let mut found = None;
            'grow: for d in [5.0, 10.0, 20.0, 50.0, 100.0] {
                if d > self.s.max_um {
                    break;
                }
                for z in [z_guess + d, z_guess - d] {
                    if stop() {
                        return Err("stopped".into());
                    }
                    if !self.ok(z) {
                        continue;
                    }
                    let (f, s, sh) = self.take(link, x, y, z)?;
                    probes += 1;
                    if sh > best.1 {
                        best = (z, sh, f, s);
                    }
                }
                if best.1 > sh0 * 1.05 {
                    found = Some(d);
                    break 'grow;
                }
            }
            match found {
                Some(d) => {
                    // keep going that way while it gets sharper, then halve the step around the best
                    let dir = (best.0 - z_guess).signum();
                    for _ in 0..8 {
                        let z = best.0 + dir * d;
                        if (z - z_guess).abs() > self.s.max_um || !self.ok(z) || stop() {
                            break;
                        }
                        let (f, s, sh) = self.take(link, x, y, z)?;
                        probes += 1;
                        if sh > best.1 {
                            best = (z, sh, f, s);
                        } else {
                            break;
                        }
                    }
                    let mut step = d / 2.0;
                    while step >= 2.5 && !stop() {
                        let zc = best.0;
                        for z in [zc + step, zc - step] {
                            if !self.ok(z) {
                                continue;
                            }
                            let (f, s, sh) = self.take(link, x, y, z)?;
                            probes += 1;
                            if sh > best.1 {
                                best = (z, sh, f, s);
                            }
                        }
                        step /= 2.0;
                    }
                    self.since_plain = 99;
                    good = true;
                }
                None => {
                    // nothing sharper anywhere: a plain spot; back where it was, and no search for the next few
                    self.since_plain = 0;
                    best = (z_guess, sh0, best.2, best.3);
                    if probes > 0 {
                        let (f, s, sh) = self.take(link, x, y, z_guess)?;
                        best = (z_guess, sh, f, s);
                    }
                }
            }
        } else {
            self.since_plain += 1;
        }
        if good {
            self.recent.push_back(best.1);
            if self.recent.len() > 12 {
                self.recent.pop_front();
            }
        }
        let z = best.2.xyz()[2];
        self.points.push([x, y, z, best.1]);
        if let Some(c) = self.csv.as_mut() {
            let _ = writeln!(c, "{x:.3},{y:.3},{z:.3},{:.4},{probes}", best.1);
        }
        if let Some(c) = self.surface.as_mut() {
            let _ = writeln!(c, "{x:.3},{y:.3},{z:.3},{:.4},{}", best.1, self.tag);
        }
        Ok(Shot { frame: best.2, sensor: best.3, z, sharpness: best.1, probes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharper_scores_higher_and_light_does_not_matter() {
        let (w, h) = (200, 160);
        let mk = |blur: usize, gain: f64| {
            let mut v = vec![0u16; w * h];
            for y in 0..h {
                for x in 0..w {
                    // stripes, box-blurred by `blur`
                    let mut s = 0.0;
                    for k in 0..=blur {
                        s += if ((x + k) / 6) % 2 == 0 { 180.0 } else { 60.0 };
                    }
                    v[y * w + x] = (gain * s / (blur + 1) as f64) as u16;
                }
            }
            Sensor { w, h, bits: 8, fmt: "mono8".into(), v }
        };
        let (a, b, c) = (sharpness(&mk(0, 1.0)), sharpness(&mk(5, 1.0)), sharpness(&mk(0, 0.5)));
        assert!(a > 3.0 * b, "{a} {b}");
        assert!((a - c).abs() < 0.05 * a, "{a} {c}");
    }

    #[test]
    fn surface_plane() {
        let mut f = Focus { s: Settings { on: true, drop: 0.7, max_um: 100.0, settle_ms: 0.0, window_um: 0.0 }, recent: VecDeque::new(), points: vec![], since_plain: 99, csv: None, surface: None, tag: String::new(), center: None };
        for (x, y) in [(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (100.0, 100.0)] {
            f.points.push([x, y, 5.0 + 0.02 * x - 0.01 * y, 1.0]);
        }
        assert!((f.predict(300.0, 200.0, 0.0) - (5.0 + 6.0 - 2.0)).abs() < 1e-9);
    }
}
