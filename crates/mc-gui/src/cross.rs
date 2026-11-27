//! Finding the exact centre of a lithographic cross near where the operator clicked, on a stitched map.
//! The arms of the cross make one row and one column of the window stand out from the rest (darker or
//! brighter); their sub-pixel positions are the centre. A click within a third of the window is enough.

use crate::pixels::Rgb;

/// `(u, v)`: the click on the map, in map pixels. Returns the refined centre, or None if no clear cross is there.
/// Coarse: the arms' row and column in a big window (tolerant of where the click was). Fine: the same in a
/// window smaller than the arms, centred on the coarse result. There the arm across is the same in every
/// row (column) of the profile, a constant the baseline removes, so the result does not depend on where the
/// window stands; the arm along gives a narrow peak whose centroid, partly covered edge pixels included,
/// is its sub-pixel centre.
pub fn refine(map: &Rgb, u: f64, v: f64, radius: usize) -> Option<(f64, f64)> {
    let (mut cu, mut cv) = profiles(map, u, v, radius, 0.1)?;
    let r2 = (radius * 3 / 10).max(4);
    for _ in 0..3 {
        let (nu, nv) = profiles(map, cu, cv, r2, 0.25)?;
        let moved = (nu - cu).abs().max((nv - cv).abs());
        (cu, cv) = (nu, nv);
        if moved < 0.01 {
            break;
        }
    }
    Some((cu, cv))
}

/// The cross's column and row (edge based, sub-pixel) from the column and row profiles of a window.
fn profiles(map: &Rgb, u: f64, v: f64, r: usize, frac: f64) -> Option<(f64, f64)> {
    let (g, n, x0, y0) = window(map, u, v, r)?;
    let rows: Vec<f64> = (0..n).map(|y| (0..n).map(|x| g[y * n + x]).sum::<f64>() / n as f64).collect();
    let cols: Vec<f64> = (0..n).map(|x| (0..n).map(|y| g[y * n + x]).sum::<f64>() / n as f64).collect();
    let y = peak(&rows, frac)?;
    let x = peak(&cols, frac)?;
    Some((x0 as f64 + x + 0.5, y0 as f64 + y + 0.5))
}

/// Grey values of a (2r+1)^2 window around (u, v), and its top left pixel; None at the map's edge.
fn window(map: &Rgb, u: f64, v: f64, r: usize) -> Option<(Vec<f64>, usize, i64, i64)> {
    let (cx, cy) = (u.floor() as i64, v.floor() as i64);
    let r = r as i64;
    if cx - r < 0 || cy - r < 0 || cx + r >= map.w as i64 || cy + r >= map.h as i64 {
        return None;
    }
    let n = (2 * r + 1) as usize;
    let mut g = vec![0f64; n * n];
    for y in 0..n {
        for x in 0..n {
            let p = map.get((cx - r) as usize + x, (cy - r) as usize + y);
            g[y * n + x] = (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0;
        }
    }
    Some((g, n, cx - r, cy - r))
}

/// The position of the one line that stands out from the profile's median, sub-pixel; None if nothing does.
/// `frac`: the band is what deviates more than this part of the peak.
fn peak(p: &[f64], frac: f64) -> Option<f64> {
    let mut s = p.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = s[s.len() / 2];
    let mad = {
        let mut d: Vec<f64> = p.iter().map(|v| (v - med).abs()).collect();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        d[d.len() / 2].max(0.5)
    };
    let dev: Vec<f64> = p.iter().map(|v| (v - med).abs()).collect();
    let (k, &best) = dev.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap())?;
    if best < 6.0 * mad || k == 0 || k + 1 == p.len() {
        return None;
    }
    // the arm is a few pixels wide: centroid of the whole band (its partly covered edge pixels included,
    // they weigh by how much of them the arm covers, which is what makes it sub-pixel)
    let half = best * frac;
    let (mut a, mut b) = (k, k);
    while a > 0 && dev[a - 1] > half {
        a -= 1;
    }
    while b + 1 < p.len() && dev[b + 1] > half {
        b += 1;
    }
    let (sw, sx) = (a..=b).fold((0.0, 0.0), |(w, x), i| (w + dev[i], x + dev[i] * i as f64));
    Some(sx / sw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_cross_to_a_tenth_of_a_pixel() {
        let (w, h) = (200usize, 200usize);
        let (tx, ty) = (97.3f64, 104.6f64);
        let mut m = Rgb::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // dark arms 4 px wide, antialiased: coverage of the pixel by the band
                let cov = |c: f64, t: f64| ((t + 2.0).min(c + 1.0) - (t - 2.0).max(c)).clamp(0.0, 1.0);
                let ax = if (y as f64 - ty).abs() < 40.0 { cov(x as f64, tx) } else { 0.0 };
                let ay = if (x as f64 - tx).abs() < 40.0 { cov(y as f64, ty) } else { 0.0 };
                let v = 200.0 - 140.0 * ax.max(ay);
                let i = (y * w + x) * 3;
                m.px[i..i + 3].copy_from_slice(&[v.round() as u8; 3]);
            }
        }
        let (u, v) = refine(&m, 104.0, 98.0, 30).unwrap();
        assert!((u - tx).abs() < 0.1 && (v - ty).abs() < 0.1, "{u} {v}");
    }
}
