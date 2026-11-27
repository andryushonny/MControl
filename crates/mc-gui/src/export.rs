//! Cutting and the drawing. Every selection marked "cut" is cut like paper along the lattice of crosses into
//! squares; each square is resampled straight from its stitched map (the lattice may be slightly turned against
//! the camera) into a picture aligned with the crosses, transparent where the selection or the map ends.
//! The drawing (DXF, um) puts every square at its exact lattice place, one layer per selection in the order of
//! the list (the top of the list is drawn last, on top), plus the lattice itself with its numbers.

use crate::geom::{Affine, Lattice, Rect, P2};
use crate::pixels::{self, Rgb};
use crate::drawing::Dxf;
use std::path::{Path, PathBuf};

/// A selection to cut: its rectangle and its stitched map.
pub struct Source<'a> {
    pub rect: Rect,
    pub map: &'a Rgb,
    /// map pixel -> stage
    pub to_stage: Affine,
}

pub struct Square {
    pub cell: [i32; 2],
    pub file: PathBuf,
    pub px: [usize; 2],
}

/// How the lattice stands on the screen, so the drawing looks like the screen: which lattice axis is the
/// horizontal one (`swap`: the second, b), and whether it runs right (sx = +1) and the other down (sy = +1).
#[derive(Clone, Copy, Debug)]
pub struct Orient {
    pub sx: f64,
    pub sy: f64,
    pub swap: bool,
}

impl Orient {
    /// From the view (mirroring and turn included).
    pub fn of(l: &Lattice, flip_x: bool, flip_y: bool, turn: f64) -> Orient {
        let v = crate::picture::View { center: [0.0, 0.0], scale: 1.0, rect: eframe::egui::Rect::ZERO, flip_x, flip_y, turn };
        let (a, b) = (v.dir_to_screen(l.a), v.dir_to_screen(l.b));
        let swap = b[0].abs() > a[0].abs();
        let (h, vert) = if swap { (b, a) } else { (a, b) };
        Orient { sx: if h[0] >= 0.0 { 1.0 } else { -1.0 }, sy: if vert[1] >= 0.0 { 1.0 } else { -1.0 }, swap }
    }

    /// (horizontal, vertical) pitch, um
    pub fn pitch(&self, l: &Lattice) -> (f64, f64) {
        let p = l.pitch();
        if self.swap { (p[1], p[0]) } else { (p[0], p[1]) }
    }

    /// A point of a cell given as parts of its width (left to right) and height (top to bottom), as lattice
    /// fractions (along a, along b).
    pub fn lat(&self, fx: f64, fy: f64) -> (f64, f64) {
        let u = if self.sx > 0.0 { fx } else { 1.0 - fx };
        let v = if self.sy > 0.0 { fy } else { 1.0 - fy };
        if self.swap { (v, u) } else { (u, v) }
    }

    /// The numbers of a cell: those of the cross at its top left corner (as the screen shows it), so the
    /// number written in the corner is the cross's.
    pub fn top_left(&self, c: [i32; 2]) -> [i32; 2] {
        let (u0, v0) = if self.swap { (c[1], c[0]) } else { (c[0], c[1]) };
        let u = if self.sx > 0.0 { u0 } else { u0 + 1 };
        let v = if self.sy > 0.0 { v0 } else { v0 + 1 };
        if self.swap { [v, u] } else { [u, v] }
    }
}

/// Cells of the lattice that touch a rectangle: (i, j) = the cell between crosses (i, j) and (i+1, j+1).
pub fn cells_in(l: &Lattice, r: &Rect) -> Vec<[i32; 2]> {
    let c = r.corners().map(|p| l.coords(p));
    let (i0, i1) = c.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
    let (j0, j1) = c.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    let mut out = vec![];
    for j in j0.floor() as i32..j1.ceil() as i32 {
        for i in i0.floor() as i32..i1.ceil() as i32 {
            out.push([i, j]);
        }
    }
    out
}

/// Cut the squares of one source into `dir` (one PNG per square). `um_per_px`: picture resolution.
pub fn cut(src: &Source, l: &Lattice, o: Orient, um_per_px: f64, dir: &Path, progress: &mut dyn FnMut(usize, usize)) -> Result<Vec<Square>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (ph, pv) = o.pitch(l);
    let (nx, ny) = ((ph / um_per_px).round().max(1.0) as usize, (pv / um_per_px).round().max(1.0) as usize);
    let inv = src.to_stage.inverse();
    let cells = cells_in(l, &src.rect);
    let mut out = vec![];
    let mut rgba = vec![0u8; nx * ny * 4];
    for (k, cell) in cells.iter().enumerate() {
        progress(k, cells.len());
        let mut filled = 0usize;
        for q in 0..ny {
            for pp in 0..nx {
                // the square in screen order: p to the right, q down
                let fx = (pp as f64 + 0.5) / nx as f64;
                let fy = (q as f64 + 0.5) / ny as f64;
                let (fi, fj) = o.lat(fx, fy);
                let s = l.at(cell[0] as f64 + fi, cell[1] as f64 + fj);
                let px = if src.rect.contains(s) {
                    let c = inv.apply(s);
                    src.map.sample(c[0], c[1])
                } else {
                    None
                };
                let o4 = (q * nx + pp) * 4;
                match px {
                    Some(c) => {
                        rgba[o4..o4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                        filled += 1;
                    }
                    None => rgba[o4..o4 + 4].copy_from_slice(&[0, 0, 0, 0]),
                }
            }
        }
        if filled == 0 {
            continue;
        }
        let name = o.top_left(*cell);
        let file = dir.join(format!("{}_{}.png", name[0], name[1]));
        pixels::write_png_rgba(&file, nx, ny, &rgba)?;
        out.push(Square { cell: *cell, file, px: [nx, ny] });
    }
    Ok(out)
}

/// Lattice point (fractional i, j) -> drawing coordinates (um), screen-like: x right, y up.
fn cad(l: &Lattice, o: Orient, i: f64, j: f64) -> P2 {
    let (ph, pv) = o.pitch(l);
    let (u, v) = if o.swap { (j, i) } else { (i, j) };
    [o.sx * u * ph, -o.sy * v * pv]
}

pub fn layer_name(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// Nearest AutoCAD colour index for an RGB colour (the basic ones are enough for layers).
fn aci(c: [u8; 3]) -> u8 {
    let base: [(u8, [u8; 3]); 7] = [(1, [255, 0, 0]), (2, [255, 255, 0]), (3, [0, 255, 0]), (4, [0, 255, 255]), (5, [0, 0, 255]), (6, [255, 0, 255]), (8, [128, 128, 128])];
    let d = |a: [u8; 3]| (0..3).map(|k| (a[k] as i32 - c[k] as i32).pow(2)).sum::<i32>();
    base.iter().min_by_key(|b| d(b.1)).unwrap().0
}

/// The drawing. `layers`: (name, colour, rectangle, squares), top of the list first.
pub fn write_dxf(path: &Path, l: &Lattice, o: Orient, layers: &[(String, [u8; 3], Rect, Vec<Square>)], extent: &Rect) -> Result<(), String> {
    let mut d = Dxf::new();
    let base = path.parent().unwrap_or(Path::new("."));
    d.layer("LATTICE", 1, None);
    d.layer("LATTICE_NUMBERS", 1, None);
    for (name, color, _, _) in layers {
        d.layer(&layer_name(name), aci(*color), Some(*color));
        d.layer(&format!("{}_OUTLINE", layer_name(name)), aci(*color), Some(*color));
    }
    let (ph, pv) = o.pitch(l);
    // bottom of the list first: later entities lie on top
    for (name, _, rect, squares) in layers.iter().rev() {
        let lay = layer_name(name);
        for sq in squares {
            let rel = sq.file.strip_prefix(base).unwrap_or(&sq.file).to_string_lossy().replace('\\', "/");
            // the square's lower left corner in the drawing; one picture pixel is pitch / pixels
            let (i, j) = (sq.cell[0] as f64, sq.cell[1] as f64);
            let c = [cad(l, o, i, j), cad(l, o, i + 1.0, j), cad(l, o, i, j + 1.0), cad(l, o, i + 1.0, j + 1.0)];
            let x0 = c.iter().map(|q| q[0]).fold(f64::MAX, f64::min);
            let y0 = c.iter().map(|q| q[1]).fold(f64::MAX, f64::min);
            d.image(&lay, &rel, sq.px, [x0, y0], ph / sq.px[0] as f64, pv / sq.px[1] as f64);
        }
        // the selection's outline, in lattice place
        let corners: Vec<P2> = rect.corners().iter().map(|q| {
            let c = l.coords(*q);
            cad(l, o, c[0], c[1])
        }).collect();
        d.polyline(&format!("{lay}_OUTLINE"), &corners, true);
    }
    // the lattice over everything: lines through the crosses, numbers in the squares
    let cells = cells_in(l, extent);
    if let (Some(i0), Some(i1), Some(j0), Some(j1)) = (
        cells.iter().map(|c| c[0]).min(),
        cells.iter().map(|c| c[0]).max(),
        cells.iter().map(|c| c[1]).min(),
        cells.iter().map(|c| c[1]).max(),
    ) {
        for i in i0..=i1 + 1 {
            d.line("LATTICE", cad(l, o, i as f64, j0 as f64), cad(l, o, i as f64, (j1 + 1) as f64));
        }
        for j in j0..=j1 + 1 {
            d.line("LATTICE", cad(l, o, i0 as f64, j as f64), cad(l, o, (i1 + 1) as f64, j as f64));
        }
        let h = ph.min(pv) * 0.06;
        for c in &cells {
            // in the upper left corner of the square, as on the screen
            // in the top left corner of the square, the number of the cross there (as on the screen)
            let (i, j) = (c[0] as f64, c[1] as f64);
            let k = [cad(l, o, i, j), cad(l, o, i + 1.0, j), cad(l, o, i, j + 1.0), cad(l, o, i + 1.0, j + 1.0)];
            let x0 = k.iter().map(|q| q[0]).fold(f64::MAX, f64::min);
            let y1 = k.iter().map(|q| q[1]).fold(f64::MIN, f64::max);
            let n = o.top_left(*c);
            d.text("LATTICE_NUMBERS", [x0 + ph * 0.04, y1 - pv * 0.04 - h], h, &format!("{},{}", n[0], n[1]));
        }
    }
    d.save(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map turned by 3 degrees, a lattice turned by 2: each cut square must be exactly one lattice cell,
    /// which the map paints by the cell's numbers.
    /// Crosses (15,16) and (15,17) side by side, X growing upwards on the screen: the square whose top left
    /// corner is the cross (15,16) is numbered 15,16; the drawing is not turned.
    #[test]
    fn numbers_are_the_corner_crosses() {
        let cross = |x: f64, y: f64, l: [i32; 2]| ([x, y], l);
        // screen y grows down: X growing upwards means the stage point of (16,16) is above (15,16)
        let l = Lattice::fit(&[cross(0.0, 0.0, [15, 16]), cross(100.0, 0.0, [15, 17])]).unwrap();
        let o = Orient::of(&l, false, false, 0.0);
        assert!(o.swap, "{o:?}");
        // the cell between crosses (14..15, 16..17): its top left on the screen is the cross (15,16)
        let c = l.coords([1.0, 1.0]); // a point just right of and below (15,16)
        let cell = [c[0].floor() as i32, c[1].floor() as i32];
        assert_eq!(o.top_left(cell), [15, 16], "cell {cell:?}, {o:?}");
        // the drawing: the horizontal axis runs right, one pixel along it is the pitch along b
        let (ph, pv) = o.pitch(&l);
        assert!((ph - 100.0).abs() < 1e-9 && (pv - 100.0).abs() < 1e-9);
    }

    #[test]
    fn squares_follow_the_lattice_exactly() {
        let (t, u) = (3f64.to_radians(), 2f64.to_radians());
        let umpp = 0.5;
        let to_stage = Affine([umpp * t.cos(), -umpp * t.sin(), 100.0, umpp * t.sin(), umpp * t.cos(), 200.0]);
        let l = Lattice { origin: [113.0, 207.0], a: [50.0 * u.cos(), 50.0 * u.sin()], b: [-50.0 * u.sin(), 50.0 * u.cos()], rms_um: 0.0 };
        let colour = |i: i32, j: i32| [(40 + 30 * i.rem_euclid(5)) as u8, (40 + 30 * j.rem_euclid(5)) as u8, 128];
        let mut map = Rgb::new(500, 500);
        for v in 0..500 {
            for w in 0..500 {
                let s = to_stage.apply([w as f64 + 0.5, v as f64 + 0.5]);
                let c = l.coords(s);
                let k = (v * 500 + w) * 3;
                map.px[k..k + 3].copy_from_slice(&colour(c[0].floor() as i32, c[1].floor() as i32));
            }
        }
        let rect = Rect { x0: 120.0, y0: 215.0, x1: 300.0, y1: 380.0 };
        let dir = std::env::temp_dir().join(format!("mc-cut-{}", std::process::id()));
        let o = Orient::of(&l, false, false, 0.0);
        let sq = cut(&Source { rect, map: &map, to_stage }, &l, o, umpp, &dir, &mut |_, _| {}).unwrap();
        assert!(!sq.is_empty());
        for s in &sq {
            let img = crate::pixels::read_png(&s.file).unwrap();
            assert_eq!([img.w, img.h], [100, 100]);
            let want = colour(s.cell[0], s.cell[1]);
            // every pixel with picture, one pixel away from the cell's edges, has the cell's colour
            for y in 1..99 {
                for x in 1..99 {
                    let p = img.get(x, y);
                    if p != [0, 0, 0] {
                        assert_eq!(p, want, "cell {:?} at {x},{y}", s.cell);
                    }
                }
            }
        }
        let path = dir.join("drawing.dxf");
        write_dxf(&path, &l, o, &[("Flake A".into(), [255, 0, 0], rect, sq)], &rect).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1000);
        println!("{}", path.display());
    }
}
