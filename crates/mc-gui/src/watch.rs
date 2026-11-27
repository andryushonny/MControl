//! Did someone turn the objective on the microscope and not in the program? While the stage stands still the
//! live picture should stay the same (light and exposure aside: the comparison is a normalised correlation).
//! If it changes a lot and stays changed, the old picture is enlarged by each other objective's scale ratio
//! about the centre and compared again: a good match names the objective that is on now.

use crate::geom::Optics;
use crate::pixels::Rgb;

const W: usize = 160;
const H: usize = 120;

/// Grey, W x H, from a live picture.
pub fn small(rgb: &Rgb) -> Vec<f32> {
    let mut g = vec![0f32; W * H];
    for y in 0..H {
        for x in 0..W {
            let p = rgb.get(x * rgb.w / W, y * rgb.h / H);
            g[y * W + x] = (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0;
        }
    }
    g
}

fn ncc(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().map(|v| *v as f64).sum::<f64>() / n, b.iter().map(|v| *v as f64).sum::<f64>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (*x as f64 - ma, *y as f64 - mb);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa < 1e-6 || bb < 1e-6 { 0.0 } else { ab / (aa * bb).sqrt() }
}

/// The old picture as it would look with `m` times the magnification (the middle 1/m of it, enlarged).
fn zoom(a: &[f32], m: f64) -> Vec<f32> {
    let (cx, cy) = (W as f64 / 2.0, H as f64 / 2.0);
    let mut o = vec![0f32; W * H];
    for y in 0..H {
        for x in 0..W {
            let sx = (cx + (x as f64 + 0.5 - cx) / m).clamp(0.0, W as f64 - 1.0) as usize;
            let sy = (cy + (y as f64 + 0.5 - cy) / m).clamp(0.0, H as f64 - 1.0) as usize;
            o[y * W + x] = a[sy * W + sx];
        }
    }
    o
}

pub struct Watch {
    /// the picture while the stage stood here, and where
    still: Option<(Vec<f32>, [f64; 3])>,
    changed: usize,
    last_seq: u64,
}

pub enum Hint {
    /// this objective seems to be on
    Objective(String),
    /// the picture changed, which objective is not clear
    Changed,
}

impl Watch {
    pub fn new() -> Watch {
        Watch { still: None, changed: 0, last_seq: 0 }
    }

    /// A live frame. `busy`: a scan or calibration runs (the picture changes on purpose).
    pub fn frame(&mut self, seq: u64, rgb: &Rgb, xyz: [f64; 3], moving: bool, busy: bool, now: &Optics, all: &[Optics]) -> Option<Hint> {
        if seq == self.last_seq {
            return None;
        }
        self.last_seq = seq;
        let g = small(rgb);
        let here = |p: [f64; 3]| (0..3).all(|k| (p[k] - xyz[k]).abs() < 0.5);
        match &self.still {
            Some((_, p)) if !moving && !busy && here(*p) => {}
            _ => {
                // moved (or busy): start again from this picture
                self.still = if moving || busy { None } else { Some((g, xyz)) };
                self.changed = 0;
                return None;
            }
        }
        let (old, _) = self.still.as_ref().unwrap();
        if ncc(old, &g) > 0.6 {
            self.changed = 0;
            return None;
        }
        self.changed += 1;
        if self.changed < 4 {
            return None; // a moment (a shadow, a hand): wait until it stays
        }
        let mut best: Option<(String, f64)> = None;
        for o in all.iter().filter(|o| o.objective != now.objective) {
            let m = now.um_per_px() / o.um_per_px();
            let c = ncc(&zoom(old, m), &g);
            if c > 0.5 && best.as_ref().is_none_or(|b| c > b.1) {
                best = Some((o.objective.clone(), c));
            }
        }
        self.still = Some((g, xyz));
        self.changed = 0;
        Some(match best {
            Some((o, _)) => Hint::Objective(o),
            None => Hint::Changed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_twice_stronger_objective_is_named() {
        // a textured picture; then its middle half enlarged twice: 10x -> 20x
        let mut a = Rgb::new(640, 480);
        for y in 0..480 {
            for x in 0..640 {
                let v = (128.0 + 60.0 * ((x as f64 / 23.0).sin() * (y as f64 / 17.0).cos()) + 40.0 * ((x as f64 / 7.0 + y as f64 / 11.0).sin())) as u8;
                let i = (y * 640 + x) * 3;
                a.px[i..i + 3].copy_from_slice(&[v; 3]);
            }
        }
        let mut b = Rgb::new(640, 480);
        for y in 0..480 {
            for x in 0..640 {
                let p = a.get(160 + x / 2, 120 + y / 2);
                let i = (y * 640 + x) * 3;
                b.px[i..i + 3].copy_from_slice(&p);
            }
        }
        let o10 = Optics::guess("10x", 0.5, [2592, 1944]);
        let all = vec![o10.clone(), Optics::guess("20x", 0.25, [2592, 1944]), Optics::guess("50x", 0.1, [2592, 1944])];
        let mut w = Watch::new();
        let p = [1.0, 2.0, 3.0];
        assert!(w.frame(1, &a, p, false, false, &o10, &all).is_none());
        assert!(w.frame(2, &a, p, false, false, &o10, &all).is_none());
        let mut hint = None;
        for s in 3..10 {
            if let Some(h) = w.frame(s, &b, p, false, false, &o10, &all) {
                hint = Some(h);
                break;
            }
        }
        assert!(matches!(hint, Some(Hint::Objective(ref o)) if o == "20x"));
    }
}
