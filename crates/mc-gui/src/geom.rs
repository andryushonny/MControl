//! Geometry: one stage coordinate system (um) for everything; the screen, camera frames, stitched maps and the
//! lattice of crosses are all mapped into it, so whatever is drawn lines up with everything else.

use serde::{Deserialize, Serialize};

pub type P2 = [f64; 2];

pub fn add(a: P2, b: P2) -> P2 {
    [a[0] + b[0], a[1] + b[1]]
}
pub fn sub(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}
pub fn mul(a: P2, k: f64) -> P2 {
    [a[0] * k, a[1] * k]
}
pub fn len(a: P2) -> f64 {
    a[0].hypot(a[1])
}

/// x' = a0 x + a1 y + a2,  y' = a3 x + a4 y + a5
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine(pub [f64; 6]);

impl Affine {
    pub const ID: Affine = Affine([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    pub fn apply(&self, p: P2) -> P2 {
        let a = &self.0;
        [a[0] * p[0] + a[1] * p[1] + a[2], a[3] * p[0] + a[4] * p[1] + a[5]]
    }
    pub fn linear(&self, p: P2) -> P2 {
        let a = &self.0;
        [a[0] * p[0] + a[1] * p[1], a[3] * p[0] + a[4] * p[1]]
    }
    /// First self, then `next`.
    pub fn then(&self, next: &Affine) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let [p, q, r, s, t, u] = next.0;
        Affine([p * a + q * d, p * b + q * e, p * c + q * f + r, s * a + t * d, s * b + t * e, s * c + t * f + u])
    }

    pub fn det(&self) -> f64 {
        self.0[0] * self.0[4] - self.0[1] * self.0[3]
    }
    pub fn inverse(&self) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let k = 1.0 / (a * e - b * d);
        let (ia, ib, id, ie) = (e * k, -b * k, -d * k, a * k);
        Affine([ia, ib, -(ia * c + ib * f), id, ie, -(id * c + ie * f)])
    }
    /// Scale of the linear part (length per unit), geometric mean.
    pub fn scale(&self) -> f64 {
        self.det().abs().sqrt()
    }
}

/// How the camera sees the stage, from an MStitch profile: a stage move (um) shifts the picture by
/// m @ move full-resolution pixels. A pixel `d` from the frame centre shows the stage point p + m^-1 d
/// (MStitch's convention, so live frames, the preview map and stitched maps coincide).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Optics {
    pub objective: String,
    pub m: [f64; 4],
    pub frame: [u32; 2],
    #[serde(default)]
    pub profile_path: Option<String>,
    /// lens distortion, radial: an ideal pixel q shows up at c + (q - c)(1 + k1 r^2 + k2 r^4), where r is
    /// |q - c| over the half diagonal and c the frame centre. Zero: none. Measured by Calibrate optics.
    #[serde(default)]
    pub k: [f64; 2],
}

impl Optics {
    pub fn guess(objective: &str, um_per_px: f64, frame: [u32; 2]) -> Optics {
        Optics { objective: objective.into(), m: [1.0 / um_per_px, 0.0, 0.0, 1.0 / um_per_px], frame, profile_path: None, k: [0.0, 0.0] }
    }

    /// From an MStitch profile json ({"objective", "m", "frame", ...}).
    pub fn load(path: &std::path::Path) -> Result<Optics, String> {
        let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let v: serde_json::Value = serde_json::from_str(&s).map_err(|e| e.to_string())?;
        let m: Vec<f64> = v["m"].as_array().ok_or("profile has no m")?.iter().filter_map(|x| x.as_f64()).collect();
        let fr: Vec<u32> = v["frame"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).map(|x| x as u32).collect()).unwrap_or_default();
        if m.len() != 4 {
            return Err("profile: m must have 4 numbers".into());
        }
        Ok(Optics {
            objective: v["objective"].as_str().unwrap_or("?").into(),
            m: [m[0], m[1], m[2], m[3]],
            frame: if fr.len() == 2 { [fr[0], fr[1]] } else { [2592, 1944] },
            profile_path: Some(path.display().to_string()),
            k: v["k"].as_array().map(|a| [a.first().and_then(|x| x.as_f64()).unwrap_or(0.0), a.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0)]).unwrap_or([0.0, 0.0]),
        })
    }

    pub fn distorted(&self) -> bool {
        self.k != [0.0, 0.0]
    }

    fn radial(&self) -> (P2, f64) {
        let (w, h) = (self.frame[0] as f64, self.frame[1] as f64);
        ([w / 2.0, h / 2.0], (w * w + h * h).sqrt() / 2.0)
    }

    /// Ideal full-resolution pixel (edge based) -> where the lens puts it on the sensor.
    pub fn distort(&self, q: P2) -> P2 {
        let (c, r0) = self.radial();
        let d = sub(q, c);
        let r2 = (d[0] * d[0] + d[1] * d[1]) / (r0 * r0);
        add(c, mul(d, 1.0 + self.k[0] * r2 + self.k[1] * r2 * r2))
    }

    /// How much a frame with the distortion taken out is zoomed in so every pixel of it is one the lens put
    /// on the sensor (1 = not at all; below 1 when the lens pushes the edges outwards). Such a frame has the
    /// same centre and m / zoom pixels per um.
    pub fn zoom(&self) -> f64 {
        if !self.distorted() {
            return 1.0;
        }
        let (w, h) = (self.frame[0] as f64, self.frame[1] as f64);
        let c = [w / 2.0, h / 2.0];
        let border: Vec<P2> = (0..=32).flat_map(|k| {
            let t = k as f64 / 32.0;
            [[t * w, 0.0], [t * w, h], [0.0, t * h], [w, t * h]]
        }).collect();
        let inside = |s: f64| border.iter().all(|b| {
            let p = self.distort(add(c, mul(sub(*b, c), s)));
            p[0] >= -1e-9 && p[1] >= -1e-9 && p[0] <= w + 1e-9 && p[1] <= h + 1e-9
        });
        if inside(1.0) {
            return 1.0;
        }
        let (mut lo, mut hi) = (0.3, 1.0);
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            if inside(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Sensor pixel -> ideal pixel (the inverse of `distort`, by iteration; exact to 1e-6 px for real lenses).
    pub fn undistort(&self, p: P2) -> P2 {
        let (c, r0) = self.radial();
        let d = sub(p, c);
        let mut q = d;
        for _ in 0..20 {
            let r2 = (q[0] * q[0] + q[1] * q[1]) / (r0 * r0);
            let n = mul(d, 1.0 / (1.0 + self.k[0] * r2 + self.k[1] * r2 * r2));
            let done = len(sub(n, q)) < 1e-7;
            q = n;
            if done {
                break;
            }
        }
        add(c, q)
    }

    /// The profile json MStitch reads (written next to a scan when no measured profile is set).
    pub fn profile_json(&self) -> serde_json::Value {
        serde_json::json!({"objective": self.objective, "m": self.m, "frame": self.frame, "repeatability_um": 5.0, "k": self.k})
    }

    pub fn um_per_px(&self) -> f64 {
        let [a, b, c, d] = self.m;
        1.0 / (a * d - b * c).abs().sqrt()
    }

    /// Full-resolution frame pixel (u, v) of a frame taken at stage p -> stage point.
    pub fn pixel_to_stage(&self, p: P2, u: f64, v: f64) -> P2 {
        let [a, b, c, d] = self.m;
        let k = 1.0 / (a * d - b * c);
        let (du, dv) = (u - self.frame[0] as f64 / 2.0, v - self.frame[1] as f64 / 2.0);
        [p[0] + k * (d * du - b * dv), p[1] + k * (-c * du + a * dv)]
    }

    /// The four corners of a frame taken at p (stage um): top-left, top-right, bottom-right, bottom-left.
    pub fn frame_corners(&self, p: P2) -> [P2; 4] {
        let (w, h) = (self.frame[0] as f64, self.frame[1] as f64);
        [self.pixel_to_stage(p, 0.0, 0.0), self.pixel_to_stage(p, w, 0.0), self.pixel_to_stage(p, w, h), self.pixel_to_stage(p, 0.0, h)]
    }

    /// Width and height of the field along the stage axes (axis-aligned box around the frame), um.
    pub fn field_um(&self) -> P2 {
        let c = self.frame_corners([0.0, 0.0]);
        let xs = c.iter().map(|p| p[0]);
        let ys = c.iter().map(|p| p[1]);
        let (x0, x1) = xs.fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(x), b.max(x)));
        let (y0, y1) = ys.fold((f64::MAX, f64::MIN), |(a, b), y| (a.min(y), b.max(y)));
        [x1 - x0, y1 - y0]
    }

    /// Inner box: the largest axis-aligned rectangle fully inside a (possibly rotated) frame, um; with the
    /// lens's distortion taken out, the part of the frame that is left (see zoom).
    pub fn inner_field_um(&self) -> P2 {
        let z = self.zoom();
        let f = mul(self.field_um(), z);
        let (w, h) = (self.frame[0] as f64 * self.um_per_px() * z, self.frame[1] as f64 * self.um_per_px() * z);
        // for a small rotation the inner box shrinks by the overhang on each side
        let ox = (f[0] - w).max(0.0);
        let oy = (f[1] - h).max(0.0);
        [(w - oy).max(w * 0.5), (h - ox).max(h * 0.5)]
    }
}

/// Snap to the stage's step grid (the motors move in whole steps).
pub fn snap_step(v: f64, step: f64) -> f64 {
    (v / step).round() * step
}

/// A rectangle on the stage, um (x0 <= x1, y0 <= y1).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub fn from_corners(a: P2, b: P2, step: f64) -> Rect {
        let s = |v: f64| snap_step(v, step);
        Rect { x0: s(a[0].min(b[0])), y0: s(a[1].min(b[1])), x1: s(a[0].max(b[0])), y1: s(a[1].max(b[1])) }
    }
    pub fn center(&self) -> P2 {
        [(self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0]
    }
    pub fn size(&self) -> P2 {
        [self.x1 - self.x0, self.y1 - self.y0]
    }
    pub fn contains(&self, p: P2) -> bool {
        p[0] >= self.x0 && p[0] <= self.x1 && p[1] >= self.y0 && p[1] <= self.y1
    }
    pub fn union(&self, o: &Rect) -> Rect {
        Rect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    pub fn corners(&self) -> [P2; 4] {
        [[self.x0, self.y0], [self.x1, self.y0], [self.x1, self.y1], [self.x0, self.y1]]
    }
}

/// The lattice of lithographic crosses: cross (i, j) sits at origin + i a + j b (stage um).
/// Numbering: cross (i, j) is labelled (i + label0[0], j + label0[1]).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lattice {
    pub origin: P2,
    pub a: P2,
    pub b: P2,
    /// how well the marked crosses fit, rms um
    pub rms_um: f64,
}

impl Lattice {
    pub fn at(&self, i: f64, j: f64) -> P2 {
        add(self.origin, add(mul(self.a, i), mul(self.b, j)))
    }
    /// Stage point -> fractional lattice coordinates (i, j).
    pub fn coords(&self, p: P2) -> P2 {
        let d = sub(p, self.origin);
        let det = self.a[0] * self.b[1] - self.a[1] * self.b[0];
        [(d[0] * self.b[1] - d[1] * self.b[0]) / det, (self.a[0] * d[1] - self.a[1] * d[0]) / det]
    }
    pub fn pitch(&self) -> P2 {
        [len(self.a), len(self.b)]
    }

    /// Fit from crosses with known indices: 1 cross is not enough; 2 crosses: a square lattice (b is a turned by
    /// +90 degrees on the screen); 3 or more: a general one, least squares.
    pub fn fit(marks: &[(P2, [i32; 2])]) -> Result<Lattice, String> {
        match marks.len() {
            0 | 1 => Err("mark at least 2 crosses".into()),
            2 => {
                let (p0, k0) = marks[0];
                let (p1, k1) = marks[1];
                let (di, dj) = ((k1[0] - k0[0]) as f64, (k1[1] - k0[1]) as f64);
                if di == 0.0 && dj == 0.0 {
                    return Err("the two crosses have the same number".into());
                }
                // v = di a + dj b with b = R90 a = (-a_y, a_x): solve the 2x2 system for a
                let v = sub(p1, p0);
                let det = di * di + dj * dj;
                let a = [(di * v[0] + dj * v[1]) / det, (di * v[1] - dj * v[0]) / det];
                let b = [-a[1], a[0]];
                let origin = sub(p0, add(mul(a, k0[0] as f64), mul(b, k0[1] as f64)));
                Ok(Lattice { origin, a, b, rms_um: 0.0 })
            }
            _ => {
                // p = o + i a + j b: 6 unknowns, least squares (normal equations, 3x3 per coordinate)
                let mut ata = [[0.0f64; 3]; 3];
                let mut atx = [0.0f64; 3];
                let mut aty = [0.0f64; 3];
                for (p, k) in marks {
                    let r = [1.0, k[0] as f64, k[1] as f64];
                    for u in 0..3 {
                        for v in 0..3 {
                            ata[u][v] += r[u] * r[v];
                        }
                        atx[u] += r[u] * p[0];
                        aty[u] += r[u] * p[1];
                    }
                }
                let sx = solve3(ata, atx).ok_or("the crosses must not lie on one line")?;
                let sy = solve3(ata, aty).ok_or("the crosses must not lie on one line")?;
                let l = Lattice { origin: [sx[0], sy[0]], a: [sx[1], sy[1]], b: [sx[2], sy[2]], rms_um: 0.0 };
                let rms = (marks.iter().map(|(p, k)| len(sub(*p, l.at(k[0] as f64, k[1] as f64))).powi(2)).sum::<f64>()
                    / marks.len() as f64)
                    .sqrt();
                Ok(Lattice { rms_um: rms, ..l })
            }
        }
    }
}

fn solve3(m: [[f64; 3]; 3], r: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(&m);
    if d.abs() < 1e-9 {
        return None;
    }
    let mut out = [0.0; 3];
    for c in 0..3 {
        let mut mc = m;
        for row in 0..3 {
            mc[row][c] = r[row];
        }
        out[c] = det(&mc) / d;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lattice_fits() {
        let truth = Lattice { origin: [10.0, -5.0], a: [200.0, 3.0], b: [-3.0, 200.0], rms_um: 0.0 };
        let marks: Vec<(P2, [i32; 2])> = [[0, 0], [3, 0], [0, 2], [2, 2]].iter().map(|k| (truth.at(k[0] as f64, k[1] as f64), *k)).collect();
        let l = Lattice::fit(&marks).unwrap();
        assert!(len(sub(l.a, truth.a)) < 1e-6 && len(sub(l.b, truth.b)) < 1e-6 && l.rms_um < 1e-6);
        let l2 = Lattice::fit(&marks[..2]).unwrap();
        assert!(len(sub(l2.a, truth.a)) < 1e-6 && len(sub(l2.b, truth.b)) < 1e-6);
        let c = l.coords(truth.at(1.25, -0.5));
        assert!((c[0] - 1.25).abs() < 1e-9 && (c[1] + 0.5).abs() < 1e-9);
    }

    #[test]
    fn optics_round_trip() {
        let o = Optics { objective: "x".into(), m: [0.5, 0.1, -0.1, 0.5], frame: [100, 80], profile_path: None, k: [0.0, 0.0] };
        let p = o.pixel_to_stage([3.0, 4.0], 50.0, 40.0);
        assert!((p[0] - 3.0).abs() < 1e-12 && (p[1] - 4.0).abs() < 1e-12);
        let af = Affine([2.0, 1.0, 5.0, -1.0, 3.0, 7.0]);
        let q = af.inverse().apply(af.apply([1.5, -2.0]));
        assert!((q[0] - 1.5).abs() < 1e-12 && (q[1] + 2.0).abs() < 1e-12);
    }
}
