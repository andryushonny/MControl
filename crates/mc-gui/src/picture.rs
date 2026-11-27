//! Pictures on the map, from a 300-pixel live thumbnail to a 20 000-pixel stitched map: a pyramid of halved
//! levels cut into 512-pixel tiles; only the tiles on screen, at the level the zoom needs, go to the GPU, a few per
//! frame, and the least recently drawn ones are freed. Every picture carries its pixel -> stage mapping.

use crate::geom::{Affine, P2};
use crate::pixels::Rgb;
use eframe::egui::{self, Color32, ColorImage, Mesh, Pos2, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::sync::Arc;

pub const TILE: usize = 512;

/// Level 0 = full picture, each next level half the size.
pub struct Pyramid {
    pub levels: Vec<Rgb>,
}

impl Pyramid {
    pub fn new(base: Rgb) -> Pyramid {
        let mut levels = vec![base];
        while levels.last().unwrap().w.max(levels.last().unwrap().h) > TILE {
            let next = levels.last().unwrap().half();
            levels.push(next);
        }
        Pyramid { levels }
    }
    pub fn base(&self) -> &Rgb {
        &self.levels[0]
    }
}

/// Screen <-> stage. `scale`: screen pixels per um. Screen y grows with stage y unless flipped; `turn`
/// (radians) turns the whole picture on the screen, e.g. to stand a tilted lattice level.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub center: P2,
    pub scale: f64,
    pub rect: egui::Rect,
    pub flip_x: bool,
    pub flip_y: bool,
    pub turn: f64,
}

impl View {
    fn signs(&self) -> (f64, f64) {
        (if self.flip_x { -1.0 } else { 1.0 }, if self.flip_y { -1.0 } else { 1.0 })
    }
    /// A stage direction (um) as a screen direction (pixels).
    pub fn dir_to_screen(&self, d: P2) -> P2 {
        let (sx, sy) = self.signs();
        let (x, y) = (sx * d[0] * self.scale, sy * d[1] * self.scale);
        let (c, s) = (self.turn.cos(), self.turn.sin());
        [c * x - s * y, s * x + c * y]
    }
    /// A screen direction (pixels) as a stage direction (um).
    pub fn dir_to_stage(&self, d: P2) -> P2 {
        let (sx, sy) = self.signs();
        let (c, s) = (self.turn.cos(), self.turn.sin());
        let (x, y) = (c * d[0] + s * d[1], -s * d[0] + c * d[1]);
        [sx * x / self.scale, sy * y / self.scale]
    }
    pub fn to_screen(&self, p: P2) -> Pos2 {
        let d = self.dir_to_screen([p[0] - self.center[0], p[1] - self.center[1]]);
        let c = self.rect.center();
        Pos2::new(c.x + d[0] as f32, c.y + d[1] as f32)
    }
    pub fn to_stage(&self, s: Pos2) -> P2 {
        let c = self.rect.center();
        let d = self.dir_to_stage([(s.x - c.x) as f64, (s.y - c.y) as f64]);
        [self.center[0] + d[0], self.center[1] + d[1]]
    }
    /// The stage box around what is seen on screen.
    pub fn stage_box(&self) -> (P2, P2) {
        let r = self.rect;
        let c = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].map(|p| self.to_stage(p));
        let (x0, x1) = c.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
        let (y0, y1) = c.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
        ([x0, y0], [x1, y1])
    }
}

pub struct Picture {
    pub pyr: Arc<Pyramid>,
    /// level-0 pixel (u, v) -> stage um
    pub to_stage: Affine,
    tiles: HashMap<(usize, usize, usize), (TextureHandle, u64)>,
    pub tint: Color32,
    name: String,
}

impl Picture {
    pub fn new(name: &str, pyr: Arc<Pyramid>, to_stage: Affine) -> Picture {
        Picture { pyr, to_stage, tiles: HashMap::new(), tint: Color32::WHITE, name: name.to_string() }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.pyr.base().w, self.pyr.base().h)
    }

    /// Stage corners of the whole picture.
    pub fn corners(&self) -> [P2; 4] {
        let (w, h) = self.size();
        let (w, h) = (w as f64, h as f64);
        [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]].map(|c| self.to_stage.apply(c))
    }

    /// Draw the visible tiles. `budget`: tiles that may still be uploaded this frame (shared by all pictures).
    /// Returns true if something is still missing (ask for another frame).
    pub fn draw(&mut self, ctx: &egui::Context, painter: &egui::Painter, view: &View, frame_no: u64, budget: &mut usize) -> bool {
        let (w, h) = self.size();
        // screen pixels per level-0 pixel -> the level that gives about one picture pixel per screen pixel
        let s = self.to_stage.scale() * view.scale;
        let lvl = ((1.0 / s).log2().floor().max(0.0) as usize).min(self.pyr.levels.len() - 1);
        let k = (1usize << lvl) as f64;
        // the picture area on screen, in level-0 pixels
        let inv = self.to_stage.inverse();
        let (a, b) = view.stage_box();
        let cs = [[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]].map(|p| inv.apply(p));
        let (u0, u1) = cs.iter().fold((f64::MAX, f64::MIN), |(m, n), p| (m.min(p[0]), n.max(p[0])));
        let (v0, v1) = cs.iter().fold((f64::MAX, f64::MIN), |(m, n), p| (m.min(p[1]), n.max(p[1])));
        if u1 < 0.0 || v1 < 0.0 || u0 > w as f64 || v0 > h as f64 {
            return false;
        }
        let level = &self.pyr.levels[lvl];
        let span = TILE as f64 * k; // level-0 pixels per tile
        let (tx0, tx1) = ((u0.max(0.0) / span) as usize, ((u1.min(w as f64 - 1.0)) / span) as usize);
        let (ty0, ty1) = ((v0.max(0.0) / span) as usize, ((v1.min(h as f64 - 1.0)) / span) as usize);
        let mut missing = false;
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let key = (lvl, tx, ty);
                if !self.tiles.contains_key(&key) {
                    if *budget == 0 {
                        missing = true;
                        // a coarser level stands in until this one arrives
                        self.draw_fallback(painter, view, lvl, tx, ty, k);
                        continue;
                    }
                    *budget -= 1;
                    let tex = upload(ctx, &format!("{}-{lvl}-{tx}-{ty}", self.name), level, tx, ty);
                    self.tiles.insert(key, (tex, frame_no));
                }
                let (tex, used) = self.tiles.get_mut(&key).unwrap();
                *used = frame_no;
                let id = tex.id();
                let [tw, th] = tex.size();
                let u = tx as f64 * span;
                let v = ty as f64 * span;
                let (du, dv) = (tw as f64 * k, th as f64 * k);
                quad(painter, view, &self.to_stage, [u, v], [u + du, v], [u + du, v + dv], [u, v + dv], id, self.tint);
            }
        }
        missing
    }

    fn draw_fallback(&self, painter: &egui::Painter, view: &View, lvl: usize, tx: usize, ty: usize, k: f64) {
        for (&(l, x, y), (tex, _)) in &self.tiles {
            if l > lvl {
                let r = 1usize << (l - lvl);
                if x == tx / r && y == ty / r {
                    let kk = (1usize << l) as f64;
                    let span = TILE as f64 * kk;
                    let [tw, th] = tex.size();
                    let (u, v) = (x as f64 * span, y as f64 * span);
                    let _ = k;
                    quad(painter, view, &self.to_stage, [u, v], [u + tw as f64 * kk, v], [u + tw as f64 * kk, v + th as f64 * kk], [u, v + th as f64 * kk], tex.id(), self.tint);
                    return;
                }
            }
        }
    }

    /// Free tiles not drawn for a while (keeps GPU memory bounded).
    pub fn evict(&mut self, frame_no: u64, keep_frames: u64) {
        self.tiles.retain(|_, (_, used)| frame_no - *used <= keep_frames);
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }
}

fn upload(ctx: &egui::Context, name: &str, level: &Rgb, tx: usize, ty: usize) -> TextureHandle {
    let (x0, y0) = (tx * TILE, ty * TILE);
    let (w, h) = (TILE.min(level.w - x0), TILE.min(level.h - y0));
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &level.px[((y0 + y) * level.w + x0) * 3..((y0 + y) * level.w + x0 + w) * 3];
        for p in row.chunks_exact(3) {
            // pure black is "no picture": transparent, so what lies below shows through
            let a = if p == [0, 0, 0] { 0 } else { 255 };
            rgba.extend_from_slice(&[p[0], p[1], p[2], a]);
        }
    }
    let img = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
    ctx.load_texture(name, img, TextureOptions::LINEAR)
}

/// A textured quad given by four level-0 picture points (mapped to stage, then to screen).
#[allow(clippy::too_many_arguments)]
fn quad(painter: &egui::Painter, view: &View, to_stage: &Affine, a: P2, b: P2, c: P2, d: P2, tex: egui::TextureId, tint: Color32) {
    let mut m = Mesh::with_texture(tex);
    let uv = [Pos2::new(0.0, 0.0), Pos2::new(1.0, 0.0), Pos2::new(1.0, 1.0), Pos2::new(0.0, 1.0)];
    for (p, t) in [a, b, c, d].iter().zip(uv) {
        m.vertices.push(egui::epaint::Vertex { pos: view.to_screen(to_stage.apply(*p)), uv: t, color: tint });
    }
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(m));
}
