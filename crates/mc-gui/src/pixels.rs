//! Frames from mc-server: unpacking (raw / lossless / jpeg), Bayer to colour, scaling, files.

use crate::geom::Optics;
use serde_json::Value;

/// A colour picture, 8-bit RGB, rows top to bottom.
#[derive(Clone, Default)]
pub struct Rgb {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Rgb {
    pub fn new(w: usize, h: usize) -> Rgb {
        Rgb { w, h, px: vec![0; w * h * 3] }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * self.w + x) * 3;
        [self.px[i], self.px[i + 1], self.px[i + 2]]
    }

    /// Half size, 2x2 box average.
    pub fn half(&self) -> Rgb {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut o = Rgb::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // black is "no picture": averaged only over the pixels that have one (else a picture's edge
                // turns into a dark line in the zoomed-out levels)
                let (mut s, mut n) = ([0u32; 3], 0u32);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (xx, yy) = ((2 * x + dx).min(self.w - 1), (2 * y + dy).min(self.h - 1));
                    let i = (yy * self.w + xx) * 3;
                    let p = &self.px[i..i + 3];
                    if p != [0, 0, 0] {
                        for c in 0..3 {
                            s[c] += p[c] as u32;
                        }
                        n += 1;
                    }
                }
                if n > 0 {
                    let v = s.map(|q| ((q + n / 2) / n) as u8);
                    o.px[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(if v == [0, 0, 0] { &[1, 1, 1] } else { &v });
                }
            }
        }
        o
    }

    /// The picture grown by `n` pixels into the "no picture" (black) around it: each such pixel next to the
    /// picture takes its neighbours' average. Covers the fraction of a pixel by which two pictures placed side by
    /// side may miss each other (a map ends where its frames' pixel centres end, up to a pixel short).
    pub fn bleed(&mut self, n: usize) {
        let (w, h) = (self.w, self.h);
        for _ in 0..n {
            let src = self.px.clone();
            let has = |x: usize, y: usize| src[(y * w + x) * 3..(y * w + x) * 3 + 3] != [0, 0, 0];
            for y in 0..h {
                for x in 0..w {
                    if has(x, y) {
                        continue;
                    }
                    let (mut s, mut k) = ([0u32; 3], 0u32);
                    for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
                        let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                        if xx < 0 || yy < 0 || xx >= w as i64 || yy >= h as i64 || !has(xx as usize, yy as usize) {
                            continue;
                        }
                        let i = (yy as usize * w + xx as usize) * 3;
                        for c in 0..3 {
                            s[c] += src[i + c] as u32;
                        }
                        k += 1;
                    }
                    if k > 0 {
                        let v = s.map(|q| ((q + k / 2) / k).max(1) as u8);
                        self.px[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&v);
                    }
                }
            }
        }
    }

    /// Bilinear sample at (u, v) in pixel units (pixel centres at +0.5). None outside, or on "no data" (black).
    pub fn sample(&self, u: f64, v: f64) -> Option<[u8; 3]> {
        let (x, y) = (u - 0.5, v - 0.5);
        if x < -0.5 || y < -0.5 || x > self.w as f64 - 0.5 || y > self.h as f64 - 0.5 {
            return None;
        }
        let (x0, y0) = (x.floor().max(0.0) as usize, y.floor().max(0.0) as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = ((x - x0 as f64).clamp(0.0, 1.0), (y - y0 as f64).clamp(0.0, 1.0));
        let p = [self.get(x0, y0), self.get(x1, y0), self.get(x0, y1), self.get(x1, y1)];
        if p.iter().all(|q| *q == [0, 0, 0]) {
            return None;
        }
        // black neighbours are "no picture": left out, not averaged in (no dark fringe at a picture's edge)
        let wgt = [(1.0 - fx) * (1.0 - fy), fx * (1.0 - fy), (1.0 - fx) * fy, fx * fy];
        let (mut acc, mut sw) = ([0f64; 3], 0f64);
        for k in 0..4 {
            if p[k] != [0, 0, 0] {
                for c in 0..3 {
                    acc[c] += p[k][c] as f64 * wgt[k];
                }
                sw += wgt[k];
            }
        }
        if sw < 1e-9 {
            return None;
        }
        let o = acc.map(|v| (v / sw).round().clamp(0.0, 255.0) as u8);
        Some(if o == [0, 0, 0] { [1, 1, 1] } else { o })
    }
}

impl Rgb {
    /// White balance: multiply the channels (black stays black: it means "no picture").
    pub fn balance(&mut self, g: [f64; 3]) {
        if g == [1.0, 1.0, 1.0] {
            return;
        }
        let lut: [[u8; 256]; 3] = std::array::from_fn(|c| std::array::from_fn(|v| (v as f64 * g[c]).round().clamp(0.0, 255.0) as u8));
        for p in self.px.chunks_exact_mut(3) {
            if p != [0, 0, 0] {
                let q = [lut[0][p[0] as usize], lut[1][p[1] as usize], lut[2][p[2] as usize]];
                p.copy_from_slice(if q == [0, 0, 0] { &[1, 1, 1] } else { &q });
            }
        }
    }

    /// Mean colour of the pixels with picture (inside the box x0..x1, y0..y1), None if there are none.
    pub fn mean(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Option<[f64; 3]> {
        let (mut s, mut n) = ([0f64; 3], 0usize);
        for y in y0..y1.min(self.h) {
            for x in x0..x1.min(self.w) {
                let p = self.get(x, y);
                if p != [0, 0, 0] {
                    for c in 0..3 {
                        s[c] += p[c] as f64;
                    }
                    n += 1;
                }
            }
        }
        (n > 0).then(|| s.map(|v| v / n as f64))
    }

    /// The lens's distortion taken out: a picture of a frame where one pixel is `div` full-resolution pixels.
    /// What the lens never showed (corners after barrel correction) is black, i.e. no picture.
    pub fn undistorted(&self, o: &Optics, div: f64) -> Rgb {
        if !o.distorted() {
            return self.clone();
        }
        let mut out = Rgb::new(self.w, self.h);
        for v in 0..self.h {
            for u in 0..self.w {
                let p = o.distort([(u as f64 + 0.5) * div, (v as f64 + 0.5) * div]);
                if let Some(c) = self.sample(p[0] / div, p[1] / div) {
                    let i = (v * self.w + u) * 3;
                    out.px[i..i + 3].copy_from_slice(&c);
                }
            }
        }
        out
    }
}

/// Sensor pixels of a full frame: (width, height, bits, values as u16).
pub struct Sensor {
    pub w: usize,
    pub h: usize,
    pub bits: u32,
    pub fmt: String,
    pub v: Vec<u16>,
}

/// Unpack a frame's payload the way the server packed it (header "codec").
pub fn unpack(meta: &Value, data: &[u8]) -> Result<Sensor, String> {
    let w = meta["w"].as_u64().unwrap_or(0) as usize;
    let h = meta["h"].as_u64().unwrap_or(0) as usize;
    let fmt = meta["fmt"].as_str().unwrap_or("").to_string();
    let bits = fmt.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(8);
    let bpp = if bits > 8 { 2 } else { 1 };
    let codec = meta["codec"].as_str().unwrap_or("raw");
    let v = match codec {
        "raw" => to_u16(data, bpp),
        "lossless" => {
            let d = zstd::bulk::decompress(data, w * h * bpp).map_err(|e| format!("zstd: {e}"))?;
            let d = to_u16(&d, bpp);
            let mask: u32 = if bpp == 1 { 0xff } else { 0xffff };
            let mut o = vec![0u16; w * h];
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let p = if x >= 2 { o[i - 2] } else if y >= 2 { o[i - 2 * w] } else { 0 };
                    o[i] = ((d[i] as u32 + p as u32) & mask) as u16;
                }
            }
            o
        }
        "jpeg" => {
            let mut o = vec![0u16; w * h];
            let mut pos = 0;
            for (ox, oy) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)] {
                let n = u32::from_le_bytes(data.get(pos..pos + 4).ok_or("short jpeg payload")?.try_into().unwrap()) as usize;
                let jpg = data.get(pos + 4..pos + 4 + n).ok_or("short jpeg payload")?;
                let mut dec = zune_jpeg::JpegDecoder::new(jpg);
                let plane = dec.decode().map_err(|e| format!("jpeg: {e:?}"))?;
                let (pw, _) = dec.dimensions().unwrap_or((w / 2, h / 2));
                for (k, val) in plane.iter().enumerate() {
                    let (x, y) = (k % pw, k / pw);
                    if 2 * y + oy < h && 2 * x + ox < w {
                        o[(2 * y + oy) * w + 2 * x + ox] = *val as u16;
                    }
                }
                pos += 4 + n;
            }
            o
        }
        c => return Err(format!("unknown codec {c}")),
    };
    if v.len() < w * h {
        return Err(format!("frame too short: {} of {} pixels", v.len(), w * h));
    }
    Ok(Sensor { w, h, bits, fmt, v })
}

fn to_u16(d: &[u8], bpp: usize) -> Vec<u16> {
    if bpp == 1 {
        d.iter().map(|&b| b as u16).collect()
    } else {
        d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
    }
}

impl Sensor {
    /// The lens's distortion taken out of the raw frame, still raw: each Bayer plane (the pixels of one
    /// colour, every second one in x and y) is resampled on its own, bilinearly, so the frame stays the
    /// same kind of data for MStitch, only without distortion. Zoomed in by `Optics::zoom` (same centre),
    /// so it has m / zoom pixels per um.
    pub fn undistorted(&self, o: &Optics) -> Sensor {
        self.undistorted_scaled(o, 1.0)
    }

    /// As `undistorted`, the picture also brought from magnification `mag` (relative: 1.003 = 0.3 % larger, as
    /// at another focus) back to 1.
    pub fn undistorted_scaled(&self, o: &Optics, mag: f64) -> Sensor {
        if !o.distorted() && mag == 1.0 {
            return Sensor { w: self.w, h: self.h, bits: self.bits, fmt: self.fmt.clone(), v: self.v.clone() };
        }
        let step = if self.fmt.starts_with("bayer") { 2 } else { 1 };
        // zoomed in a little where the lens pushed the edges out, so the frame holds no pixel it did not see;
        // and by the magnification (a larger picture is sampled farther out)
        let z = o.zoom() * mag;
        let c = [o.frame[0] as f64 / 2.0, o.frame[1] as f64 / 2.0];
        // the optics are for full frames of this size: scale if the frame is binned or cropped differently
        let (sx, sy) = (o.frame[0] as f64 / self.w as f64, o.frame[1] as f64 / self.h as f64);
        let mut out = vec![0u16; self.w * self.h];
        let (pw, ph) = (self.w / step, self.h / step);
        for dy in 0..step {
            for dx in 0..step {
                let at = |i: usize, j: usize| self.v[(j * step + dy) * self.w + i * step + dx] as f64;
                for j in 0..ph {
                    for i in 0..pw {
                        let (x, y) = (i * step + dx, j * step + dy);
                        // this pixel's centre, ideal -> on the sensor; then in plane coordinates (pixel centres)
                        let q = [(x as f64 + 0.5) * sx, (y as f64 + 0.5) * sy];
                        let p = o.distort([c[0] + z * (q[0] - c[0]), c[1] + z * (q[1] - c[1])]);
                        // within half a pixel of the plane's outer samples: the nearest one (the plane's grid
                        // is offset by a pixel against the frame's edge)
                        let (fx, fy) = ((p[0] / sx - 0.5 - dx as f64) / step as f64, (p[1] / sy - 0.5 - dy as f64) / step as f64);
                        let (fx, fy) = (fx.clamp(0.0, (pw - 1) as f64), fy.clamp(0.0, (ph - 1) as f64));
                        let (i0, j0) = (fx.floor() as usize, fy.floor() as usize);
                        let (i1, j1) = ((i0 + 1).min(pw - 1), (j0 + 1).min(ph - 1));
                        let (ax, ay) = (fx - i0 as f64, fy - j0 as f64);
                        let top = at(i0, j0) * (1.0 - ax) + at(i1, j0) * ax;
                        let bot = at(i0, j1) * (1.0 - ax) + at(i1, j1) * ax;
                        out[y * self.w + x] = (top * (1.0 - ay) + bot * ay).round() as u16;
                    }
                }
            }
        }
        Sensor { w: self.w, h: self.h, bits: self.bits, fmt: self.fmt.clone(), v: out }
    }

    /// Grey at half size (one value per 2x2 cell), for measuring: pixel i covers full-res [2i, 2i+2).
    pub fn grey2(&self) -> (Vec<f32>, usize, usize) {
        let (w, h) = (self.w / 2, self.h / 2);
        let mut g = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let i = 2 * y * self.w + 2 * x;
                g[y * w + x] = (self.v[i] as f32 + self.v[i + 1] as f32 + self.v[i + self.w] as f32 + self.v[i + self.w + 1] as f32) / 4.0;
            }
        }
        (g, w, h)
    }

    /// Colour at half size: each 2x2 Bayer cell is one pixel (exact, no interpolation). `div`: extra box
    /// reduction (1 = half size, 2 = quarter, 4 = eighth).
    pub fn rgb(&self, div: usize) -> Rgb {
        let shift = self.bits.saturating_sub(8);
        let (r_at, b_at) = match self.fmt.get(6..8).unwrap_or("") {
            "rg" => ((0, 0), (1, 1)),
            "gr" => ((1, 0), (0, 1)),
            "gb" => ((0, 1), (1, 0)),
            "bg" => ((1, 1), (0, 0)),
            _ => ((9, 9), (9, 9)),
        };
        let bayer = self.fmt.starts_with("bayer") && r_at.0 < 2;
        let s = 2 * div.max(1);
        let (ow, oh) = (self.w / s, self.h / s);
        let mut o = Rgb::new(ow, oh);
        let at = |x: usize, y: usize| (self.v[y * self.w + x] >> shift) as u32;
        let n = (div * div) as u32;
        for oy in 0..oh {
            for ox in 0..ow {
                let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
                for cy in 0..div {
                    for cx in 0..div {
                        let (x0, y0) = (ox * s + cx * 2, oy * s + cy * 2);
                        if bayer {
                            r += at(x0 + r_at.0, y0 + r_at.1);
                            b += at(x0 + b_at.0, y0 + b_at.1);
                            g += (at(x0 + 1 - r_at.0, y0 + r_at.1) + at(x0 + r_at.0, y0 + 1 - r_at.1)) / 2;
                        } else {
                            let m = (at(x0, y0) + at(x0 + 1, y0) + at(x0, y0 + 1) + at(x0 + 1, y0 + 1)) / 4;
                            r += m;
                            g += m;
                            b += m;
                        }
                    }
                }
                let i = (oy * ow + ox) * 3;
                // never pure black: that means "no picture"; a dark pixel is 1 at least
                o.px[i] = (r / n).clamp(1, 255) as u8;
                o.px[i + 1] = (g / n).clamp(1, 255) as u8;
                o.px[i + 2] = (b / n).clamp(1, 255) as u8;
            }
        }
        o
    }

    /// Save as MControl's Frame.save does: name.npy (sensor values) + name.json (header). MStitch reads these.
    pub fn save(&self, meta: &Value, stem: &std::path::Path) -> std::io::Result<()> {
        let wide = self.bits > 8;
        let descr = if wide { "<u2" } else { "|u1" };
        let mut head = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': ({}, {}), }}", self.h, self.w);
        while (10 + head.len() + 1) % 64 != 0 {
            head.push(' ');
        }
        head.push('\n');
        let mut out = Vec::with_capacity(10 + head.len() + self.v.len() * 2);
        out.extend_from_slice(b"\x93NUMPY\x01\x00");
        out.extend_from_slice(&(head.len() as u16).to_le_bytes());
        out.extend_from_slice(head.as_bytes());
        if wide {
            for v in &self.v {
                out.extend_from_slice(&v.to_le_bytes());
            }
        } else {
            out.extend(self.v.iter().map(|&v| v as u8));
        }
        std::fs::write(stem.with_extension("npy"), out)?;
        let mut m = meta.clone();
        if let Some(o) = m.as_object_mut() {
            o.remove("codec");
            o.remove("quality");
        }
        std::fs::write(stem.with_extension("json"), serde_json::to_vec_pretty(&m).unwrap())
    }
}

/// PNG file -> RGB (8-bit RGB or RGBA; alpha 0 becomes "no data" = black).
pub fn read_png(path: &std::path::Path) -> Result<Rgb, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(f));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let mut o = Rgb::new(w, h);
    let ch = info.color_type.samples();
    for i in 0..w * h {
        let p = &buf[i * ch..i * ch + ch];
        let (r, g, b, a) = match ch {
            1 => (p[0], p[0], p[0], 255),
            2 => (p[0], p[0], p[0], p[1]),
            3 => (p[0], p[1], p[2], 255),
            _ => (p[0], p[1], p[2], p[3]),
        };
        if a > 0 {
            // a black pixel that is there is dark, not missing ("no picture" is pure black here)
            let px = if (r, g, b) == (0, 0, 0) { [1, 1, 1] } else { [r, g, b] };
            o.px[i * 3..i * 3 + 3].copy_from_slice(&px);
        }
    }
    Ok(o)
}

/// RGBA PNG (alpha 0 where there is no picture).
pub fn write_png_rgba(path: &std::path::Path, w: usize, h: usize, rgba: &[u8]) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(rgba).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lossless_and_rgb() {
        let (w, h) = (8usize, 6usize);
        let raw: Vec<u8> = (0..w * h).map(|i| (i * 13 % 251) as u8).collect();
        // pack like the server
        let mut d = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let p = if x >= 2 { raw[i - 2] } else if y >= 2 { raw[i - 2 * w] } else { 0 };
                d[i] = raw[i].wrapping_sub(p);
            }
        }
        let z = zstd::bulk::compress(&d, 1).unwrap();
        let s = unpack(&json!({"w": w, "h": h, "fmt": "bayer_rg8", "codec": "lossless"}), &z).unwrap();
        assert_eq!(s.v.iter().map(|&v| v as u8).collect::<Vec<_>>(), raw);
        let c = s.rgb(1);
        assert_eq!((c.w, c.h), (4, 3));
        assert_eq!(c.get(0, 0)[0], raw[0].max(1)); // R of the first cell (a dark pixel is 1 at least)
    }

    /// A picture 1 % larger (another focus) brought back by undistorted_scaled: as the one at 1.
    #[test]
    fn scale_brought_back() {
        let (w, h) = (400usize, 300usize);
        let pic = |mag: f64| {
            let mut v = vec![0u16; w * h];
            for y in 0..h {
                for x in 0..w {
                    // smooth rings about the centre, magnified by mag
                    let (dx, dy) = ((x as f64 + 0.5 - 200.0) / mag, (y as f64 + 0.5 - 150.0) / mag);
                    v[y * w + x] = (128.0 + 100.0 * ((dx * dx + dy * dy).sqrt() / 9.0).sin()) as u16;
                }
            }
            Sensor { w, h, bits: 8, fmt: "mono8".into(), v }
        };
        let o = Optics::guess("t", 1.0, [w as u32, h as u32]);
        let (a, b) = (pic(1.0), pic(1.01).undistorted_scaled(&o, 1.01));
        let err: f64 = a.v.iter().zip(&b.v).map(|(p, q)| (*p as f64 - *q as f64).abs()).sum::<f64>() / a.v.len() as f64;
        let before: f64 = a.v.iter().zip(&pic(1.01).v).map(|(p, q)| (*p as f64 - *q as f64).abs()).sum::<f64>() / a.v.len() as f64;
        assert!(err < 0.25 * before, "after {err}, before {before}");
    }
}
