//! The map: everything in stage micrometres, drawn in one transform. Pan with the mouse (drag), zoom with the
//! wheel, move the stage with the arrow keys or a double click; the tools add selections and crosses.

use crate::geom::{self, Rect, P2};
use crate::picture::View;
use crate::keys::Act;
use crate::{cross, export, scan, thumb_picture, App, Tool};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Sense, Shape, Stroke};
use serde_json::json;
use std::sync::Arc;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let rect = resp.rect;
    // the map takes clicks, not the keyboard: keys stay shortcuts (arrows would otherwise move focus around)
    if resp.has_focus() {
        resp.surrender_focus();
    }
    if !ui.ctx().text_edit_focused() && app.capturing.is_none() && app.keys.pressed(ui, Act::Fit) {
        app.fit = true;
    }
    let turn = app.view_turn();
    if app.fit {
        app.fit = false;
        if let Some(e) = app.proj.sample().extent() {
            // the sample's box as it stands on the (maybe turned) screen, at 1 px per um
            let unit = View { center: e.center(), scale: 1.0, rect, flip_x: app.flip_x, flip_y: app.flip_y, swap: app.swap, turn };
            let c = e.corners().map(|q| unit.dir_to_screen(geom::sub(q, e.center())));
            let zw = c.iter().map(|q| q[0]).fold(f64::MIN, f64::max) - c.iter().map(|q| q[0]).fold(f64::MAX, f64::min);
            let zh = c.iter().map(|q| q[1]).fold(f64::MIN, f64::max) - c.iter().map(|q| q[1]).fold(f64::MAX, f64::min);
            app.scale = ((rect.width() as f64 / zw.max(1.0)).min(rect.height() as f64 / zh.max(1.0)) * 0.8).clamp(0.002, 50.0);
            app.center = e.center();
            app.follow = false;
        }
    }
    let mut view = View { center: app.center, scale: app.scale, rect, flip_x: app.flip_x, flip_y: app.flip_y, swap: app.swap, turn };
    let ctx = ui.ctx().clone();
    let stage = app.link.stage_pos();

    // ---- input: pan, zoom, keys
    if resp.dragged_by(egui::PointerButton::Primary) || resp.dragged_by(egui::PointerButton::Middle) || resp.dragged_by(egui::PointerButton::Secondary) {
        let d = resp.drag_delta();
        let a = view.to_stage(rect.center());
        let b = view.to_stage(rect.center() - d);
        app.center = geom::add(app.center, geom::sub(b, a));
        app.follow = false;
    }
    if let Some(hover) = resp.hover_pos() {
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let k = zoom as f64 * (scroll as f64 / 300.0).exp();
        if (k - 1.0).abs() > 1e-4 {
            let before = view.to_stage(hover);
            app.scale = (app.scale * k).clamp(0.002, 50.0);
            view.scale = app.scale;
            let after = view.to_stage(hover);
            app.center = geom::add(app.center, geom::sub(before, after));
            app.follow = false;
        }
    }
    view.center = app.center;
    if !ui.ctx().text_edit_focused() && app.capturing.is_none() {
        keys(app, ui, stage, &view);
    }

    // ---- drawing, back to front
    painter.rect_filled(rect, 0.0, Color32::from_gray(24));
    grid(&painter, &view);
    let mut budget = 12usize;
    let mut missing = false;
    for t in app.thumbs.iter_mut() {
        missing |= t.draw(&ctx, &painter, &view, app.frame_no, &mut budget);
    }
    // stitched maps: bottom of the list first, so the top of the list lies on top
    let sample = app.proj.sample().clone();
    for s in sample.selections.iter().rev().filter(|s| s.visible) {
        if let Some(m) = s.best_scan().and_then(|sc| sc.map.clone()) {
            if let (Some(p), Some(raw)) = (app.maps.get_mut(&m), app.map_raw.get(&m)) {
                // where the map stands: as stitched, then Align's correction
                p.to_stage = s.fix.map(|f| raw.then(&f)).unwrap_or(*raw);
                let a = (s.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                p.tint = Color32::from_white_alpha(a);
                missing |= p.draw(&ctx, &painter, &view, app.frame_no, &mut budget);
            }
        }
    }
    for s in sample.selections.iter().rev().filter(|s| s.visible) {
        selection(&painter, &view, s, app.focus == Some(s.id));
    }
    live(app, &ctx, &painter, &view, &mut budget);
    // a corner of the substrate found: its two edges, red and blue, for the operator to say which runs left-right
    if let Some((_, c)) = &app.corner {
        let o = app.proj.optics();
        let at = view.to_screen(c.stage(o));
        let f = o.field_um();
        let len = 0.35 * f[0].min(f[1]);
        for (e, col) in c.edges().iter().zip([Color32::from_rgb(240, 90, 70), Color32::from_rgb(90, 160, 255)]) {
            let to = view.to_screen(geom::add(c.stage(o), geom::mul(*e, len)));
            painter.arrow(at, to - at, Stroke::new(4.0, col));
        }
        painter.circle_stroke(at, 6.0, Stroke::new(2.0, Color32::from_rgb(250, 200, 80)));
    }
    // the lattice and the crosses over everything, the live picture included
    if let Some(l) = &sample.lattice {
        lattice(&painter, &view, l, &sample);
    }
    for c in &sample.crosses {
        let p = view.to_screen(c.at);
        painter.circle_stroke(p, 7.0, Stroke::new(2.0, Color32::from_rgb(255, 60, 60)));
        let (t, col) = match c.label {
            Some(l) => (format!("{},{}", l[0], l[1]), Color32::from_rgb(255, 90, 90)),
            None => ("?".to_string(), Color32::from_gray(200)),
        };
        painter.text(p + egui::vec2(9.0, -9.0), Align2::LEFT_BOTTOM, t, FontId::monospace(12.0), col);
    }
    if let Some(p) = stage {
        let s = view.to_screen([p[0], p[1]]);
        let c = Color32::from_rgba_unmultiplied(255, 255, 255, 140);
        painter.line_segment([s - egui::vec2(10.0, 0.0), s + egui::vec2(10.0, 0.0)], Stroke::new(1.0, c));
        painter.line_segment([s - egui::vec2(0.0, 10.0), s + egui::vec2(0.0, 10.0)], Stroke::new(1.0, c));
    }

    // ---- tools
    let hover = resp.hover_pos().map(|h| view.to_stage(h));
    match app.tool {
        Tool::Select => {
            // frames this rectangle takes with the objective on the microscope: over the quick limit it is refused
            let count = |r: &Rect| {
                let o = app.proj.optics();
                (scan::frames(r, o, app.proj.scan.overlap_quick, app.proj.step_um), scan::frames(r, o, app.proj.scan.overlap_quality, app.proj.step_um))
            };
            if let (Some(a), Some(h)) = (app.anchor, hover) {
                let r = Rect::from_corners(a, h, app.proj.step_um);
                let (nq, nh) = count(&r);
                let col = if nq > scan::QUICK_MAX { Color32::from_rgb(255, 80, 60) } else { Color32::YELLOW };
                outline(&painter, &view, &r, Stroke::new(1.5, col));
                let sz = r.size();
                let quality = if nh > scan::QUALITY_MAX { format!("quality: too big (>{})", scan::QUALITY_MAX) } else { format!("quality {nh}") };
                let text = format!("{:.0} x {:.0} um, {}\nframes: quick {nq} of {}, {quality}", sz[0], sz[1], app.proj.optics().objective, scan::QUICK_MAX);
                painter.text(resp.hover_pos().unwrap() + egui::vec2(14.0, 14.0), Align2::LEFT_TOP, text, FontId::monospace(12.0), col);
            }
            if resp.clicked() {
                if let Some(h) = hover {
                    match app.anchor.take() {
                        None => app.anchor = Some(h),
                        Some(a) => {
                            let r = Rect::from_corners(a, h, app.proj.step_um);
                            let (nq, _) = count(&r);
                            if nq > scan::QUICK_MAX {
                                app.message = format!("too big: {nq} frames with {}, at most {}; draw a smaller one or put a weaker objective on", app.proj.optics().objective, scan::QUICK_MAX);
                                app.anchor = Some(a);
                            } else if r.size()[0] > 0.0 && r.size()[1] > 0.0 {
                                let id = app.proj.add_selection(r);
                                app.focus = Some(id);
                                app.changed();
                            }
                        }
                    }
                }
            }
            if app.keys.pressed(ui, Act::Cancel) {
                app.anchor = None;
            }
        }
        Tool::Lattice => {
            if resp.clicked() {
                if let Some(h) = hover {
                    add_cross(app, h);
                }
            }
        }
        Tool::White => {
            if resp.clicked() {
                if let Some(h) = hover {
                    pick_white(app, h);
                }
            }
        }
        Tool::Explore => {}
    }
    if resp.double_clicked() && app.tool == Tool::Explore {
        if let Some(h) = hover {
            app.link.send(json!({"cmd": "goto", "x": h[0], "y": h[1]}));
        }
    }
    resp.context_menu(|ui| {
        if let Some(h) = hover {
            if ui.button(format!("Move the stage here ({:.0}, {:.0})", h[0], h[1])).clicked() {
                app.link.send(json!({"cmd": "goto", "x": h[0], "y": h[1]}));
                ui.close();
            }
        }
    });
    if let Some(h) = hover {
        painter.text(rect.left_bottom() + egui::vec2(8.0, -8.0), Align2::LEFT_BOTTOM, format!("{:9.1} {:9.1} um", h[0], h[1]), FontId::monospace(12.0), Color32::from_gray(200));
    }
    scale_bar(&painter, &view);

    // follow the stage while it moves, unless the operator panned away
    if app.follow {
        if let Some(p) = stage {
            app.center = [p[0], p[1]];
        }
    }
    for t in app.thumbs.iter_mut() {
        t.evict(app.frame_no, 300);
    }
    for p in app.maps.values_mut() {
        p.evict(app.frame_no, 300);
    }
    if missing {
        ctx.request_repaint();
    }
}

fn keys(app: &mut App, ui: &egui::Ui, stage: Option<[f64; 3]>, view: &View) {
    let f = app.proj.optics().field_um();
    let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
    let frac = if shift { 0.1 } else if ctrl { 1.0 } else { app.arrow_frac };
    // an arrow moves the picture that way on the screen, however the view is turned or mirrored
    // Move map: the arrows move that selection's map, one map pixel (Shift: ten), not the stage
    if let Some(id) = app.nudging {
        if app.keys.pressed(ui, Act::Done) || app.keys.pressed(ui, Act::Cancel) {
            app.nudging = None;
            app.message = "map moved; Align maps to the lattice refines it".into();
            return;
        }
        let px = app.proj.sample().selections.iter().find(|s| s.id == id).and_then(|s| app.map_of(s)).map(|(_, a)| a.scale()).unwrap_or(1.0);
        let step = px * if shift { 10.0 } else { 1.0 };
        for (k, dx, dy) in [(Act::Left, -1.0, 0.0), (Act::Right, 1.0, 0.0), (Act::Up, 0.0, -1.0), (Act::Down, 0.0, 1.0)] {
            if app.keys.pressed(ui, k) {
                let s = view.dir_to_stage([dx, dy]);
                let n = geom::len(s);
                app.nudge(id, geom::mul(s, step / n));
            }
        }
        return;
    }
    let mut d = [0.0, 0.0];
    for (k, dx, dy) in [(Act::Left, -1.0, 0.0), (Act::Right, 1.0, 0.0), (Act::Up, 0.0, -1.0), (Act::Down, 0.0, 1.0)] {
        // the arrows as set in Settings: exchanged, reversed
        // the arrows as set in Controls: exchanged, reversed (the picture stays as it is)
        // swap: up moves as right did, down as left, right as up, left as down (screen y grows down)
        let (dx, dy) = if app.arrow_swap { (-dy, -dx) } else { (dx, dy) };
        let dx = if app.arrow_rev_x { -dx } else { dx };
        let dy = if app.arrow_rev_y { -dy } else { dy };
        if let Some([r, u]) = app.arrow_dirs {
            // taught arrows: right / up as the operator sees the stage, whatever the screen shows
            if app.keys.pressed(ui, k) {
                let s = [dx * r[0] - dy * u[0], dx * r[1] - dy * u[1]];
                let step = f[0].min(f[1]) * frac;
                d = geom::add(d, geom::mul(s, step / geom::len(s).max(1e-9)));
            }
            continue;
        }
        if app.keys.pressed(ui, k) {
            let s = view.dir_to_stage([dx, dy]);
            let n = geom::len(s);
            let step = if dx != 0.0 { f[0] } else { f[1] } * frac;
            d = geom::add(d, geom::mul(s, step / n));
        }
    }
    // the queue on PC 1 runs them in order; while it is busy, pressing more only piles up moves
    let queued = app.link.state.lock().unwrap().telemetry["queue"].as_u64().unwrap_or(0);
    if (d[0] != 0.0 || d[1] != 0.0) && queued < 2 && stage.is_some() {
        app.link.send(json!({"cmd": "move", "dx": d[0], "dy": d[1]}));
        app.follow = true;
    }
    // focus by hand: the . / , keys (with > / < on them) or PageUp / PageDown move z by the step in Settings;
    // with Shift held (> and < themselves) five steps
    for (k, sign) in [(Act::ZUp2, 1.0), (Act::ZDown2, -1.0), (Act::ZUp, 1.0), (Act::ZDown, -1.0)] {
        if app.keys.pressed(ui, k) && stage.is_some() {
            let step = app.proj.scan.z_step_um * if shift { 5.0 } else { 1.0 };
            app.link.send(json!({"cmd": "move", "dz": sign * step}));
        }
    }
    if app.keys.pressed(ui, Act::Stop) {
        app.stop_all();
    }
    if app.keys.pressed(ui, Act::Light) {
        app.toggle_light();
    }
    if app.keys.pressed(ui, Act::LaserOn) {
        app.laser(true);
    }
    if app.keys.pressed(ui, Act::LaserOff) {
        app.laser(false);
    }
    if app.keys.pressed(ui, Act::Follow) {
        app.follow = true;
    }
    if app.keys.pressed(ui, Act::Focus) {
        app.focus_here();
    }
    if app.keys.pressed(ui, Act::Snap) {
        app.link.send(json!({"cmd": "snap"}));
    }
}

/// Grid lines every 10^k um, chosen for the zoom.
fn grid(painter: &egui::Painter, view: &View) {
    let target = 80.0 / view.scale; // about 80 px apart
    let step = 10f64.powf(target.log10().ceil());
    let (a, b) = view.stage_box();
    let c = Color32::from_gray(45);
    let mut x = (a[0] / step).floor() * step;
    while x <= b[0] {
        painter.line_segment([view.to_screen([x, a[1]]), view.to_screen([x, b[1]])], Stroke::new(1.0, c));
        x += step;
    }
    let mut y = (a[1] / step).floor() * step;
    while y <= b[1] {
        painter.line_segment([view.to_screen([a[0], y]), view.to_screen([b[0], y])], Stroke::new(1.0, c));
        y += step;
    }
}

fn scale_bar(painter: &egui::Painter, view: &View) {
    let target = 120.0 / view.scale;
    let step = [1.0, 2.0, 5.0].iter().map(|k| k * 10f64.powf(target.log10().floor())).filter(|s| *s <= target).last().unwrap_or(1.0);
    let len = (step * view.scale) as f32;
    let p = view.rect.right_bottom() + egui::vec2(-20.0 - len, -16.0);
    painter.line_segment([p, p + egui::vec2(len, 0.0)], Stroke::new(3.0, Color32::WHITE));
    let label = if step >= 1000.0 { format!("{} mm", step / 1000.0) } else { format!("{step} um") };
    painter.text(p + egui::vec2(len / 2.0, -4.0), Align2::CENTER_BOTTOM, label, FontId::monospace(12.0), Color32::WHITE);
}

pub fn outline(painter: &egui::Painter, view: &View, r: &Rect, stroke: Stroke) {
    let pts: Vec<Pos2> = r.corners().iter().map(|p| view.to_screen(*p)).collect();
    painter.add(Shape::closed_line(pts, stroke));
}

fn selection(painter: &egui::Painter, view: &View, s: &crate::project::Selection, focused: bool) {
    let [r, g, b] = s.color;
    let col = Color32::from_rgb(r, g, b);
    let w = if focused { 3.0 } else { 1.5 };
    outline(painter, view, &s.rect, Stroke::new(w, col));
    let pts: Vec<Pos2> = s.rect.corners().iter().map(|p| view.to_screen(*p)).collect();
    if s.best_scan().is_none() {
        painter.add(Shape::convex_polygon(pts.clone(), Color32::from_rgba_unmultiplied(r, g, b, 25), Stroke::NONE));
    }
    // the name at the corner that is top left on the screen
    let tl = pts.iter().min_by(|a, b| (a.x + a.y).partial_cmp(&(b.x + b.y)).unwrap()).copied().unwrap();
    painter.text(tl + egui::vec2(4.0, 2.0), Align2::LEFT_TOP, &s.name, FontId::proportional(13.0), col);
    // centre of mass
    let c = view.to_screen(s.rect.center());
    painter.circle_stroke(c, 4.0, Stroke::new(1.5, col));
}

/// The red lattice: lines through the crosses over the sample's selections, numbers in the squares.
fn lattice(painter: &egui::Painter, view: &View, l: &geom::Lattice, sample: &crate::project::Sample) {
    let Some(ext) = sample.extent() else { return };
    let cells = export::cells_in(l, &ext);
    if cells.is_empty() || cells.len() > 20_000 {
        return;
    }
    let (i0, i1) = (cells.iter().map(|c| c[0]).min().unwrap(), cells.iter().map(|c| c[0]).max().unwrap() + 1);
    let (j0, j1) = (cells.iter().map(|c| c[1]).min().unwrap(), cells.iter().map(|c| c[1]).max().unwrap() + 1);
    let red = Stroke::new(1.2, Color32::from_rgb(230, 40, 40));
    for i in i0..=i1 {
        painter.line_segment([view.to_screen(l.at(i as f64, j0 as f64)), view.to_screen(l.at(i as f64, j1 as f64))], red);
    }
    for j in j0..=j1 {
        painter.line_segment([view.to_screen(l.at(i0 as f64, j as f64)), view.to_screen(l.at(i1 as f64, j as f64))], red);
    }
    // numbers in the top left corner of each square (as the screen shows it), once 2 crosses are marked
    let cell_px = geom::len(l.a) * view.scale;
    if cell_px > 40.0 && sample.crosses.iter().filter(|c| c.label.is_some()).count() >= 2 {
        let size = (cell_px / 9.0).clamp(10.0, 20.0) as f32;
        let orient = export::Orient::of(l, view.flip_x, view.flip_y, view.swap, view.turn);
        for c in &cells {
            let (i, j) = (c[0] as f64, c[1] as f64);
            let corners = [l.at(i, j), l.at(i + 1.0, j), l.at(i + 1.0, j + 1.0), l.at(i, j + 1.0)].map(|p| view.to_screen(p));
            let tl = corners.iter().min_by(|a, b| (a.x + a.y).partial_cmp(&(b.x + b.y)).unwrap()).copied().unwrap();
            if view.rect.contains(tl) {
                // the number of the cross at this corner
                let n = orient.top_left(*c);
                painter.text(tl + egui::vec2(4.0, 3.0), Align2::LEFT_TOP, format!("{},{}", n[0], n[1]), FontId::monospace(size), Color32::from_rgba_unmultiplied(255, 70, 70, 220));
            }
        }
    }
}

/// The live picture where the camera is now, with a frame around it and a crosshair in its centre.
fn live(app: &mut App, ctx: &egui::Context, painter: &egui::Painter, view: &View, budget: &mut usize) {
    let live = {
        let s = app.link.state.lock().unwrap();
        s.live.as_ref().map(|l| (l.seq, l.rgb.clone(), l.div, l.xyz, l.moving))
    };
    let Some((seq, rgb, div, xyz, moving)) = live else { return };
    let p = [xyz[0], xyz[1]];
    if app.live_pic.as_ref().map(|x| x.0) != Some(seq) {
        app.live_pic = Some((seq, thumb_picture(&app.proj.optics(), p, rgb.clone(), div, 1.0, 0)));
        // the preview map keeps a small copy wherever the camera stood still
        let f = app.proj.optics().field_um();
        if !moving && geom::len(geom::sub(p, app.last_thumb.0)) > f[0].min(f[1]) / 3.0 && app.last_thumb.1.elapsed().as_millis() > 300 {
            let small = Arc::new(rgb.half());
            let n = app.thumbs.len() + 1;
            app.thumbs.push(thumb_picture(&app.proj.optics(), p, small, div * 2.0, 1.0, n));
            if app.thumbs.len() > 5000 {
                app.thumbs.remove(0);
            }
            app.last_thumb = (p, std::time::Instant::now());
        }
    }
    if let Some((_, pic)) = app.live_pic.as_mut() {
        *budget += 1; // the live picture always gets its upload
        pic.draw(ctx, painter, view, app.frame_no, budget);
        pic.evict(app.frame_no, 2);
    }
    let corners: Vec<Pos2> = app.proj.optics().frame_corners(p).iter().map(|c| view.to_screen(*c)).collect();
    let col = if moving { Color32::from_rgb(255, 170, 0) } else { Color32::from_rgb(80, 200, 255) };
    painter.add(Shape::closed_line(corners, Stroke::new(1.5, col)));
    let c = view.to_screen(p);
    let ch = Color32::from_rgba_unmultiplied(255, 255, 255, 110);
    painter.line_segment([c - egui::vec2(14.0, 0.0), c + egui::vec2(14.0, 0.0)], Stroke::new(1.0, ch));
    painter.line_segment([c - egui::vec2(0.0, 14.0), c + egui::vec2(0.0, 14.0)], Stroke::new(1.0, ch));
}

/// A cross where the operator clicked: refined on the stitched map under the click, numbered after the last one.
fn add_cross(app: &mut App, at: P2) {
    let sample = app.proj.sample().clone();
    let mut p = at;
    let mut refined = false;
    for s in sample.selections.iter().filter(|s| s.visible && s.rect.contains(at)) {
        let Some(m) = s.best_scan().and_then(|sc| sc.map.clone()) else { continue };
        let Some(pic) = app.maps.get(&m) else { continue };
        let inv = pic.to_stage.inverse();
        let uv = inv.apply(at);
        let radius = (25.0 / pic.to_stage.scale()).clamp(12.0, 120.0) as usize; // about 25 um around the click
        if let Some((u, v)) = cross::refine(pic.pyr.base(), uv[0], uv[1], radius) {
            p = pic.to_stage.apply([u, v]);
            refined = true;
        }
        break;
    }
    // no numbers of our own: the operator signs the cross (X, Y) in the Lattice panel
    let s = app.proj.sample_mut();
    s.crosses.push(crate::project::Cross { at: p, label: None });
    let _ = s.refit();
    app.message = if refined { format!("cross at ({:.2}, {:.2}) um, centre found on the map: sign its X, Y on the right", p[0], p[1]) } else { "cross where clicked (no clear cross on a map there): sign its X, Y on the right".into() };
    app.changed();
}

/// White balance from a spot that should be grey (a bare substrate, a white pad): the pictures under the click,
/// live first, then the maps from the top of the list. Pictures are shown balanced already, so the new gains
/// are the old ones times what still is off.
fn pick_white(app: &mut App, at: P2) {
    let mut seen: Option<[f64; 3]> = None;
    let probe = |pic: &crate::picture::Picture| -> Option<[f64; 3]> {
        let b = pic.pyr.base();
        let uv = pic.to_stage.inverse().apply(at);
        let r = (3.0 / pic.to_stage.scale()).max(2.0); // about 6 um across
        if uv[0] < r || uv[1] < r || uv[0] + r >= b.w as f64 || uv[1] + r >= b.h as f64 {
            return None;
        }
        b.mean((uv[0] - r) as usize, (uv[1] - r) as usize, (uv[0] + r) as usize, (uv[1] + r) as usize)
    };
    if let Some((_, p)) = &app.live_pic {
        seen = probe(p);
    }
    if seen.is_none() {
        for s in app.proj.sample().selections.iter().filter(|s| s.visible) {
            if let Some(p) = s.best_scan().and_then(|sc| sc.map.as_ref()).and_then(|m| app.maps.get(m)) {
                if let Some(c) = probe(p) {
                    seen = Some(c);
                    break;
                }
            }
        }
    }
    match seen {
        Some(c) if c.iter().all(|v| *v > 8.0) => {
            let grey = (c[0] + c[1] + c[2]) / 3.0;
            let wb = app.proj.wb;
            let mut g = [wb[0] * grey / c[0], wb[1] * grey / c[1], wb[2] * grey / c[2]];
            let k = g[1];
            g = g.map(|v| v / k); // green stays 1
            app.set_white(g);
            app.message = format!("white balance R {:.3}  G 1  B {:.3}", g[0], g[2]);
            app.tool = Tool::Explore;
        }
        _ => app.message = "no picture (or too dark) under the click to balance on".into(),
    }
}
