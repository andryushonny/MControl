//! Frame packing for the data link. The sensor gives Bayer mosaics; PC 1 packs them so a slow link carries more:
//!
//!   raw       as the sensor gives it                         1 : 1
//!   lossless  same-colour neighbour difference + zstd        exact pixels, about 2-3 : 1 on real images
//!   jpeg      each of the 4 Bayer colour planes as a JPEG    about 10 : 1 at quality 92, small error
//!
//! A frame is packed at most once per codec (the first sender that needs it), on the sender's thread,
//! with the 4 JPEG planes in parallel. Decoding is in the Python client (`Frame.raw()`).

use std::str::FromStr;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    Raw,
    Lossless,
    Jpeg,
}

impl FromStr for Codec {
    type Err = String;
    fn from_str(s: &str) -> Result<Codec, String> {
        match s {
            "raw" => Ok(Codec::Raw),
            "lossless" => Ok(Codec::Lossless),
            "jpeg" => Ok(Codec::Jpeg),
            _ => Err(format!("unknown codec {s} (raw, lossless, jpeg)")),
        }
    }
}

impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Codec::Raw => "raw",
            Codec::Lossless => "lossless",
            Codec::Jpeg => "jpeg",
        }
    }
}

/// Difference to the pixel of the same colour two to the left (first two columns: two rows up), wrapping.
/// Smooth images give small numbers, which zstd packs well. `bpp`: 1 or 2 bytes per pixel (little endian).
pub fn lossless(px: &[u8], w: usize, h: usize, bpp: usize) -> Vec<u8> {
    let mut d = vec![0u8; px.len()];
    if bpp == 1 {
        for y in 0..h {
            let row = &px[y * w..(y + 1) * w];
            let out = &mut d[y * w..(y + 1) * w];
            for x in 0..w.min(2) {
                out[x] = if y >= 2 { row[x].wrapping_sub(px[(y - 2) * w + x]) } else { row[x] };
            }
            for x in 2..w {
                out[x] = row[x].wrapping_sub(row[x - 2]);
            }
        }
    } else {
        let at = |i: usize| u16::from_le_bytes([px[2 * i], px[2 * i + 1]]);
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let p = if x >= 2 { at(i - 2) } else if y >= 2 { at(i - 2 * w) } else { 0 };
                let r = at(i).wrapping_sub(p).to_le_bytes();
                d[2 * i] = r[0];
                d[2 * i + 1] = r[1];
            }
        }
    }
    zstd::bulk::compress(&d, 1).unwrap_or(d)
}

/// The 4 Bayer planes (2x2 cell offsets (0,0) (1,0) (0,1) (1,1)) as 8-bit JPEGs, encoded in parallel:
/// payload = 4 x [u32 LE length][JPEG].
pub fn jpeg_planes(px: &[u8], w: usize, h: usize, quality: u8) -> Vec<u8> {
    let (pw, ph) = (w / 2, h / 2);
    let parts: Vec<Vec<u8>> = std::thread::scope(|s| {
        let jobs: Vec<_> = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .iter()
            .map(|&(ox, oy)| {
                s.spawn(move || {
                    let mut plane = vec![0u8; pw * ph];
                    for y in 0..ph {
                        let src = &px[(2 * y + oy) * w..];
                        let dst = &mut plane[y * pw..(y + 1) * pw];
                        for x in 0..pw {
                            dst[x] = src[2 * x + ox];
                        }
                    }
                    let mut out = Vec::with_capacity(pw * ph / 6);
                    let enc = jpeg_encoder::Encoder::new(&mut out, quality);
                    let _ = enc.encode(&plane, pw as u16, ph as u16, jpeg_encoder::ColorType::Luma);
                    out
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap_or_default()).collect()
    });
    let mut out = Vec::with_capacity(parts.iter().map(|p| p.len() + 4).sum());
    for p in parts {
        out.extend_from_slice(&(p.len() as u32).to_le_bytes());
        out.extend_from_slice(&p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lossless_roundtrip_8bit() {
        let (w, h) = (64usize, 48usize);
        let px: Vec<u8> = (0..w * h).map(|i| ((i * 7 + i / w * 3) % 251) as u8).collect();
        let z = lossless(&px, w, h, 1);
        let d = zstd::bulk::decompress(&z, px.len()).unwrap();
        // undo the prediction
        let mut out = vec![0u8; px.len()];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let p = if x >= 2 { out[i - 2] } else if y >= 2 { out[i - 2 * w] } else { 0 };
                out[i] = d[i].wrapping_add(p);
            }
        }
        assert_eq!(out, px);
    }
}
