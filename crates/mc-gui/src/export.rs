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
    /// 0..1: how opaque its squares are (the alpha of the PNGs)
    pub opacity: f32,
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
    /// in the drawing's coordinates: cross (X, Y) at (ox + (X - 1) px, oy + (Y - 1) py), x right, y up
    pub design: Option<[f64; 4]>,
}

impl Orient {
    /// From the view (mirroring and turn included).
    pub fn of(l: &Lattice, flip_x: bool, flip_y: bool, swap: bool, turn: f64) -> Orient {
        let v = crate::picture::View { center: [0.0, 0.0], scale: 1.0, rect: eframe::egui::Rect::ZERO, flip_x, flip_y, swap, turn };
        let (a, b) = (v.dir_to_screen(l.a), v.dir_to_screen(l.b));
        let swap = b[0].abs() > a[0].abs();
        let (h, vert) = if swap { (b, a) } else { (a, b) };
        Orient { sx: if h[0] >= 0.0 { 1.0 } else { -1.0 }, sy: if vert[1] >= 0.0 { 1.0 } else { -1.0 }, swap, design: None }
    }

    /// As the lithography drawing stands: X to the right, Y up, every cross at its drawing coordinates.
    pub fn drawing(d: &crate::project::Design) -> Orient {
        Orient { sx: 1.0, sy: -1.0, swap: false, design: Some([d.origin[0], d.origin[1], d.pitch[0], d.pitch[1]]) }
    }

    /// (horizontal, vertical) pitch, um
    pub fn pitch(&self, l: &Lattice) -> (f64, f64) {
        if let Some(d) = self.design {
            return (d[2], d[3]);
        }
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
    // every cell between its four crosses as they really are on the map: a cross found near its lattice place
    // is where the square's corner goes, so in the drawing each cross lands exactly on its place (a corner not
    // found: its lattice place, moved as the found ones are)
    let p = l.pitch();
    // the drawing's crosses: a small radius, so the numbers next to each are not taken for it
    let radius_um = if o.design.is_some() { 6.5 } else { (0.3 * p[0].min(p[1])).min(25.0) };
    let r_px = (radius_um / src.to_stage.scale()).round().max(6.0) as usize;
    // the crosses found from where the lattice puts them, growing outwards: one not found where the lattice says
    // is looked for again where its found neighbours say (the lattice may be off far from its crosses by more
    // than the small radius)
    let mut nodes: Vec<[i32; 2]> = vec![];
    for c in &cells {
        for d in [[0, 0], [1, 0], [0, 1], [1, 1]] {
            let n = [c[0] + d[0], c[1] + d[1]];
            if !nodes.contains(&n) {
                nodes.push(n);
            }
        }
    }
    let mut found: std::collections::HashMap<[i32; 2], Option<P2>> = std::collections::HashMap::new();
    let look = |near: P2| -> Option<P2> {
        if !src.rect.contains(near) {
            return None;
        }
        let uv = inv.apply(near);
        let (u, v) = crate::cross::refine(src.map, uv[0], uv[1], r_px)?;
        let f = src.to_stage.apply([u, v]);
        (crate::geom::len(crate::geom::sub(f, near)) < radius_um).then_some(f)
    };
    for pass in 0..8 {
        let mut new = 0;
        for n in &nodes {
            if found.get(n).is_some_and(|f| f.is_some()) {
                continue;
            }
            let node = l.at(n[0] as f64, n[1] as f64);
            let guess = if pass == 0 {
                node
            } else {
                // the found within two pitches: their offset from the lattice, averaged
                let offs: Vec<P2> = found.iter().filter(|(k, f)| f.is_some() && (k[0] - n[0]).abs() <= 2 && (k[1] - n[1]).abs() <= 2).map(|(k, f)| crate::geom::sub(f.unwrap(), l.at(k[0] as f64, k[1] as f64))).collect();
                if offs.is_empty() {
                    continue;
                }
                let s = offs.iter().fold([0.0, 0.0], |a, b| crate::geom::add(a, *b));
                crate::geom::add(node, [s[0] / offs.len() as f64, s[1] / offs.len() as f64])
            };
            let f = look(guess);
            if f.is_some() {
                new += 1;
            }
            found.insert(*n, f);
        }
        if pass > 0 && new == 0 {
            break;
        }
    }
    let corner = |c: [i32; 2]| -> Option<P2> { found.get(&c).copied().flatten() };
    for (k, cell) in cells.iter().enumerate() {
        progress(k, cells.len());
        let ids = [[cell[0], cell[1]], [cell[0] + 1, cell[1]], [cell[0], cell[1] + 1], [cell[0] + 1, cell[1] + 1]];
        let lat: Vec<P2> = ids.iter().map(|c| l.at(c[0] as f64, c[1] as f64)).collect();
        let got: Vec<Option<P2>> = ids.iter().map(|c| corner(*c)).collect();
        let n_got = got.iter().filter(|g| g.is_some()).count();
        let shift = if n_got > 0 {
            let s = got.iter().zip(&lat).filter_map(|(g, q)| g.map(|g| crate::geom::sub(g, *q))).fold([0.0, 0.0], crate::geom::add);
            [s[0] / n_got as f64, s[1] / n_got as f64]
        } else {
            [0.0, 0.0]
        };
        let pin: Vec<P2> = got.iter().zip(&lat).map(|(g, q)| g.unwrap_or(crate::geom::add(*q, shift))).collect();
        // lattice fractions (within the cell) -> stage, bilinear between the four corners
        let at = |fi: f64, fj: f64| -> P2 {
            let lerp = |a: P2, b: P2, t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            lerp(lerp(pin[0], pin[1], fi), lerp(pin[2], pin[3], fi), fj)
        };
        let mut filled = 0usize;
        for q in 0..ny {
            for pp in 0..nx {
                // the square in screen order: p to the right, q down
                let fx = (pp as f64 + 0.5) / nx as f64;
                let fy = (q as f64 + 0.5) / ny as f64;
                let (fi, fj) = o.lat(fx, fy);
                let s = at(fi, fj);
                let px = if src.rect.contains(s) {
                    let c = inv.apply(s);
                    src.map.sample(c[0], c[1])
                } else {
                    None
                };
                let o4 = (q * nx + pp) * 4;
                match px {
                    Some(c) => {
                        rgba[o4..o4 + 4].copy_from_slice(&[c[0], c[1], c[2], (src.opacity.clamp(0.0, 1.0) * 255.0).round().max(1.0) as u8]);
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
    if let Some(d) = o.design {
        return [d[0] + (i - 1.0) * d[2], d[1] + (j - 1.0) * d[3]];
    }
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
        // the drawing's own crosses, to check the overlay against the lithography drawing
        if o.design.is_some() {
            d.layer("DRAWING_CROSSES", 3, None);
            for j in j0..=j1 + 1 {
                for i in i0..=i1 + 1 {
                    let c = cad(l, o, i as f64, j as f64);
                    let r = ph.min(pv) * 0.025;
                    d.line("DRAWING_CROSSES", [c[0] - r, c[1]], [c[0] + r, c[1]]);
                    d.line("DRAWING_CROSSES", [c[0], c[1] - r], [c[0], c[1] + r]);
                }
            }
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
        let o = Orient::of(&l, false, false, false, 0.0);
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
        let o = Orient::of(&l, false, false, false, 0.0);
        let sq = cut(&Source { rect, map: &map, to_stage, opacity: 1.0 }, &l, o, umpp, &dir, &mut |_, _| {}).unwrap();
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

    /// In the drawing's coordinates: the map is turned, scaled 0.3 % and shifted against the stage's lattice,
    /// yet each cut square has a cross exactly in each corner (the squares go between the crosses found on the
    /// map), and the drawing puts cross (X, Y) at the drawing's place.
    #[test]
    fn squares_between_the_real_crosses() {
        let umpp = 0.5;
        let t = 0.4f64.to_radians();
        let k = 1.003;
        // map pixel -> where it truly is on the sample (crosses every 100 um at multiples of 100)
        // and skewed by 0.8 % (the stage's axes not quite square): no square lattice goes through all the crosses
        let truth = Affine([umpp * k * t.cos(), -umpp * k * t.sin() + 0.008 * umpp, 7.0, umpp * k * t.sin(), umpp * k * t.cos(), 4.0]);
        let (w, h) = (1400usize, 1250usize);
        let mut map = Rgb::new(w, h);
        for v in 0..h {
            for u in 0..w {
                let mut dark = 0.0f64;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let s = truth.apply([u as f64 + (sx as f64 + 0.5) / 4.0, v as f64 + (sy as f64 + 0.5) / 4.0]);
                        let (fx, fy) = (s[0].rem_euclid(100.0), s[1].rem_euclid(100.0));
                        let near = |f: f64| f < 1.5 || f > 98.5;
                        let arm = |f: f64| f < 10.0 || f > 90.0;
                        if (near(fx) && arm(fy)) || (near(fy) && arm(fx)) {
                            dark += 1.0 / 16.0;
                        }
                    }
                }
                let o = (v * w + u) * 3;
                map.px[o..o + 3].copy_from_slice(&[(200.0 - 140.0 * dark).round() as u8; 3]);
            }
        }
        // the map as placed (no turn, no scale): the stage's view of it
        let placed = Affine([umpp, 0.0, 7.0, 0.0, umpp, 4.0]);
        // crosses (X, Y) = (2 + i, 3 + j) at (100 i, 100 j), signed by hand where the placed map shows them
        let mark = |i: i32, j: i32| {
            let q = truth.inverse().apply([100.0 * i as f64, 100.0 * j as f64]);
            (placed.apply(q), [2 + i, 3 + j])
        };
        // signed close together (as two neighbouring crosses often are): far from them the lattice misses
        let l = Lattice::fit_square(&[mark(1, 1), mark(2, 1), mark(1, 2)]).unwrap();
        let d = crate::project::Design { name: "t".into(), origin: [-500.0, 300.0], pitch: [100.0, 100.0], n: [30, 30] };
        let o = Orient::drawing(&d);
        let dir = std::env::temp_dir().join("mc_export_pin");
        let _ = std::fs::remove_dir_all(&dir);
        let rect = Rect { x0: 60.0, y0: 60.0, x1: 660.0, y1: 580.0 };
        let sq = cut(&Source { rect, map: &map, to_stage: placed, opacity: 1.0 }, &l, o, umpp, &dir, &mut |_, _| {}).unwrap();
        // a whole square inside: its corner pixels are on the crosses' centres (dark), its middle is not
        let full: Vec<&Square> = sq.iter().filter(|s| {
            let c = l.at(s.cell[0] as f64 + 0.5, s.cell[1] as f64 + 0.5);
            rect.contains(geom_add(c, [-60.0, -60.0])) && rect.contains(geom_add(c, [60.0, 60.0]))
        }).collect();
        assert!(!full.is_empty());
        for s in full {
            let img = pixels::read_png(&s.file).unwrap();
            let px = |x: usize, y: usize| img.get(x, y)[0] as i32;
            let (n0, n1) = (s.px[0] - 1, s.px[1] - 1);
            // each corner on its cross: the corner dark, and both arms run along the square's edges from it
            // (8 px = 4 um in: an arm is 3 um wide, so a corner 1.5 um off its cross already misses one)
            for (x, y, dx, dy) in [(0, 0, 1i64, 1i64), (n0, 0, -1, 1), (0, n1, 1, -1), (n0, n1, -1, -1)] {
                let at = |ax: i64, ay: i64| px((x as i64 + ax) as usize, (y as i64 + ay) as usize);
                assert!(at(0, 0) < 120 && at(8 * dx, 0) < 120 && at(0, 8 * dy) < 120, "corner {x},{y} of {:?}: {} {} {}", s.cell, at(0, 0), at(8 * dx, 0), at(0, 8 * dy));
            }
            assert!(px(n0 / 2, n1 / 2) > 190);
        }
        // the drawing: cross (X, Y) at the drawing's place
        assert_eq!(cad(&l, o, 4.0, 5.0), [-500.0 + 300.0, 300.0 + 400.0]);
    }

    fn geom_add(a: P2, b: P2) -> P2 {
        [a[0] + b[0], a[1] + b[1]]
    }
}
