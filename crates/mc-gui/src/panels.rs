//! Everything around the map: the toolbar, the selections, the lattice, the log and the settings.

use crate::project::{Mode, Sample};
use crate::{App, Tool};
use eframe::egui::{self, Color32, RichText};
use serde_json::json;

pub fn top(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("top").show(ui, |ui| {
        ui.horizontal(|ui| {

            for (t, name, key) in [(Tool::Explore, "Explore", "E"), (Tool::Select, "Select", "S"), (Tool::Lattice, "Lattice", "L")] {
                if ui.selectable_label(app.tool == t, format!("{name} ({key})")).clicked() {
                    app.tool = t;
                    app.anchor = None;
                }
            }
            if ui.memory(|m| m.focused().is_none()) {
                for (k, t) in [(egui::Key::E, Tool::Explore), (egui::Key::S, Tool::Select), (egui::Key::L, Tool::Lattice)] {
                    if ui.input(|i| i.key_pressed(k)) {
                        app.tool = t;
                        app.anchor = None;
                    }
                }
            }
            ui.separator();
            if ui.button("Snap").on_hover_text("one frame, kept in the preview").clicked() {
                app.link.send(json!({"cmd": "snap"}));
            }
            if ui.button(RichText::new("Stop (Space)").color(Color32::from_rgb(255, 120, 100))).on_hover_text("stops the stage and the scan; the queue pauses").clicked() {
                app.stop_all();
            }
            if ui.button("Follow (F)").clicked() {
                app.follow = true;
            }
            if ui.button("Fit (Home)").on_hover_text("the whole sample").clicked() {
                app.fit = true;
            }
            ui.separator();
            ui.label("On the microscope");
            let names: Vec<String> = app.proj.objectives.iter().map(|o| o.objective.clone()).collect();
            let mut cur = app.proj.optics().objective.clone();
            egui::ComboBox::from_id_salt("objective").width(70.0).selected_text(cur.clone()).show_ui(ui, |ui| {
                for n in &names {
                    ui.selectable_value(&mut cur, n.clone(), n);
                }
            });
            if cur != app.proj.optics().objective {
                app.objective_changed(&cur);
            }
            ui.separator();
            let busy = app.export.as_ref().is_some_and(|e| e.lock().unwrap().result.is_none());
            if ui.add_enabled(!busy, egui::Button::new("Cut and export DXF")).clicked() {
                app.start_export();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    app.settings_open = !app.settings_open;
                }
                if ui.button("Open project").clicked() {
                    if let Some(f) = rfd::FileDialog::new().pick_folder() {
                        match crate::project::Project::open(&f) {
                            Ok(p) => {
                                app.proj = p;
                                app.maps.clear();
                                app.thumbs.clear();
                            }
                            Err(e) => app.message = e,
                        }
                    }
                }
                ui.label(RichText::new(app.proj.folder.display().to_string()).weak());
            });
        });
    });
}

pub fn bottom(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::bottom("bottom").resizable(true).default_size(130.0).show(ui, |ui| {
        if let Some(e) = &app.export {
            let e = e.lock().unwrap();
            ui.horizontal(|ui| match &e.result {
                None => {
                    ui.add(egui::ProgressBar::new(e.done as f32 / e.total.max(1) as f32).desired_width(260.0).text(e.stage.clone()));
                }
                Some(Ok(p)) => {
                    ui.label(RichText::new(format!("Drawing written: {}", p.display())).color(Color32::from_rgb(90, 210, 120)));
                }
                Some(Err(m)) => {
                    ui.label(RichText::new(format!("Export failed: {m}")).color(Color32::from_rgb(230, 90, 70)));
                }
            });
        }
        if !app.message.is_empty() {
            ui.label(RichText::new(&app.message).strong());
        }
        ui.separator();
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false; 2]).show(ui, |ui| {
            let s = app.link.state.lock().unwrap();
            for l in s.log.iter() {
                ui.label(RichText::new(l).monospace().size(11.5));
            }
            for j in &app.jobs {
                for l in j.lock().unwrap().log.iter().rev().take(3).rev() {
                    ui.label(RichText::new(l).monospace().size(11.5).weak());
                }
            }
        });
    });
}

pub fn left(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::left("left").resizable(true).default_size(300.0).show(ui, |ui| {
        // samples
        ui.horizontal(|ui| {
            ui.label("Sample");
            let names: Vec<String> = app.proj.samples.iter().map(|s| s.name.clone()).collect();
            let mut active = app.proj.active;
            egui::ComboBox::from_id_salt("sample").selected_text(names[active].clone()).show_ui(ui, |ui| {
                for (k, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut active, k, n);
                }
            });
            if active != app.proj.active {
                app.proj.active = active;
                app.changed();
            }
            if ui.button("+").on_hover_text("new sample").clicked() {
                let n = app.proj.samples.len() + 1;
                app.proj.samples.push(Sample::new(&format!("Sample {n}")));
                app.proj.active = app.proj.samples.len() - 1;
                app.changed();
            }
        });
        let mut name = app.proj.sample().name.clone();
        if ui.text_edit_singleline(&mut name).changed() {
            app.proj.sample_mut().name = name;
            app.changed();
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.heading("Selections");
            ui.label(RichText::new("top is drawn on top").weak());
        });
        if app.proj.sample().selections.is_empty() {
            ui.label(RichText::new("Select tool (S): click two corners on the map.").weak());
        }
        let mut action: Option<(usize, &str)> = None;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            let n = app.proj.sample().selections.len();
            for k in 0..n {
                let sid = app.proj.sample().selections[k].id;
                let busy = app.jobs.iter().any(|j| j.lock().unwrap().selection == sid) || app.queue.iter().any(|t| t.selection == sid);
                let sel_now = app.proj.sample().selections[k].clone();
                let nq = app.frames_for(&sel_now, Mode::Quick);
                let nh = app.frames_for(&sel_now, Mode::Quality);
                let objectives: Vec<String> = app.proj.objectives.iter().map(|o| o.objective.clone()).collect();
                let focused = app.focus == Some(app.proj.sample().selections[k].id);
                let mut changed = false;
                let frame = egui::Frame::group(ui.style()).fill(if focused { Color32::from_gray(48) } else { Color32::TRANSPARENT });
                frame.show(ui, |ui| {
                    let s = &mut app.proj.sample_mut().selections[k];
                    ui.horizontal(|ui| {
                        let mut c = s.color;
                        if ui.color_edit_button_srgb(&mut c).changed() {
                            s.color = c;
                            changed = true;
                        }
                        changed |= ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(110.0)).changed();
                        changed |= ui.checkbox(&mut s.visible, "").on_hover_text("shown on the map (export takes every selection with a map)").changed();
                    });
                    let c = s.rect.center();
                    let z = s.rect.size();
                    ui.label(RichText::new(format!("centre {:.1}, {:.1} um   {:.0} x {:.0} um", c[0], c[1], z[0], z[1])).monospace().size(11.0));
                    let scans: Vec<String> = s.scans.iter().map(|sc| format!("{} {} ({} frames){}", sc.mode.name(), sc.when, sc.n_frames, if sc.map.is_some() { "" } else { " no map" })).collect();
                    for t in scans.iter().rev().take(3) {
                        ui.label(RichText::new(t).size(11.0).weak());
                    }
                    if let Some(f) = s.fix {
                        let c = s.rect.center();
                        let d = f.apply(c);
                        ui.label(RichText::new(format!("aligned: moved {:+.2}, {:+.2} um", d[0] - c[0], d[1] - c[1])).size(11.0).color(Color32::from_rgb(120, 200, 140)));
                    }
                    if let Some(w) = s.best_scan().map(|sc| sc.note.clone()).filter(|n| !n.is_empty()) {
                        ui.label(RichText::new(w).size(11.0).color(Color32::from_rgb(240, 170, 60)));
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Objective").size(11.0));
                        let mut ob = if objectives.contains(&s.objective) { s.objective.clone() } else { objectives[0].clone() };
                        egui::ComboBox::from_id_salt(("obj", s.id)).width(70.0).selected_text(ob.clone()).show_ui(ui, |ui| {
                            for o in &objectives {
                                ui.selectable_value(&mut ob, o.clone(), o);
                            }
                        });
                        if ob != s.objective {
                            s.objective = ob;
                            changed = true;
                        }
                        let col = |n: usize, max: usize| if n > max { Color32::from_rgb(240, 90, 70) } else { Color32::GRAY };
                        ui.label(RichText::new(format!("{nq}")).size(11.0).color(col(nq, crate::scan::QUICK_MAX)))
                            .on_hover_text(format!("frames of a quick scan (at most {})", crate::scan::QUICK_MAX));
                        ui.label(RichText::new("/").size(11.0).weak());
                        ui.label(RichText::new(format!("{nh} frames")).size(11.0).color(col(nh, crate::scan::QUALITY_MAX)))
                            .on_hover_text(format!("frames of a quality scan (at most {})", crate::scan::QUALITY_MAX));
                    });
                    ui.horizontal(|ui| {
                        let why = |n: usize, max: usize| if busy { "in the queue already".to_string() } else { format!("{n} frames, at most {max}: make the selection smaller or use a weaker objective") };
                        if ui.add_enabled(!busy && nq <= crate::scan::QUICK_MAX, egui::Button::new("Quick scan")).on_disabled_hover_text(why(nq, crate::scan::QUICK_MAX)).clicked() {
                            action = Some((k, "quick"));
                        }
                        if ui.add_enabled(!busy && nh <= crate::scan::QUALITY_MAX, egui::Button::new("Quality scan")).on_disabled_hover_text(why(nh, crate::scan::QUALITY_MAX)).clicked() {
                            action = Some((k, "quality"));
                        }
                        if ui.small_button("Up").on_hover_text("drawn above the next").clicked() {
                            action = Some((k, "up"));
                        }
                        if ui.small_button("Down").clicked() {
                            action = Some((k, "down"));
                        }
                        if ui.small_button("Go").on_hover_text("move the stage to the centre").clicked() {
                            action = Some((k, "go"));
                        }
                        if ui.small_button("Show").clicked() {
                            action = Some((k, "show"));
                        }
                        if ui.small_button("Delete").clicked() {
                            action = Some((k, "delete"));
                        }
                    });
                });
                if changed {
                    app.changed();
                }
            }
        });
        if let Some((k, what)) = action {
            let id = app.proj.sample().selections[k].id;
            let rect = app.proj.sample().selections[k].rect;
            match what {
                "quick" => app.enqueue(id, Mode::Quick),
                "quality" => app.enqueue(id, Mode::Quality),
                "up" if k > 0 => app.proj.sample_mut().selections.swap(k, k - 1),
                "down" if k + 1 < app.proj.sample().selections.len() => app.proj.sample_mut().selections.swap(k, k + 1),
                "go" => {
                    let c = rect.center();
                    app.link.send(json!({"cmd": "goto", "x": c[0], "y": c[1]}));
                }
                "show" => {
                    app.center = rect.center();
                    app.follow = false;
                    app.focus = Some(id);
                }
                "delete" => {
                    app.proj.sample_mut().selections.remove(k);
                    app.queue.retain(|t| t.selection != id);
                }
                _ => {}
            }
            app.changed();
        }
    });
}

/// The lattice: crosses with their numbers, the fit, the pitch.
pub fn right(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::right("right").resizable(true).default_size(260.0).show(ui, |ui| {
        ui.heading("Lattice");
        ui.label(RichText::new("Click crosses on a stitched map. Each cross gets the numbers (X, Y) of its lattice point; edit them to match the sample's numbering. 2 crosses: square lattice; 3 and more: fitted.").weak().size(11.5));
        ui.separator();
        let mut remove = None;
        let mut changed = false;
        let s = app.proj.sample_mut();
        for (k, c) in s.crosses.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{}.", k + 1));
                ui.label("X");
                changed |= ui.add(egui::DragValue::new(&mut c.label[0]).speed(0.1)).changed();
                ui.label("Y");
                changed |= ui.add(egui::DragValue::new(&mut c.label[1]).speed(0.1)).changed();
                if ui.small_button("Delete").clicked() {
                    remove = Some(k);
                }
            });
            ui.label(RichText::new(format!("   at {:.2}, {:.2} um", c.at[0], c.at[1])).monospace().size(11.0));
        }
        if let Some(k) = remove {
            s.crosses.remove(k);
            changed = true;
        }
        let fit = if changed { Some(s.refit()) } else { None };
        ui.separator();
        match &s.lattice {
            Some(l) => {
                let p = l.pitch();
                let ang = l.a[1].atan2(l.a[0]).to_degrees();
                ui.label(RichText::new(format!("pitch {:.3} x {:.3} um\nturn {:.4} deg\nfit rms {:.3} um", p[0], p[1], ang, l.rms_um)).monospace());
            }
            None => {
                ui.label("No lattice yet: mark at least 2 crosses.");
            }
        }
        if ui.button("Clear crosses").clicked() {
            s.crosses.clear();
            s.lattice = None;
            changed = true;
        }
        if let Some(Err(e)) = fit {
            app.message = e;
        }
        if changed {
            app.changed();
        }
        if app.proj.sample().lattice.is_some() {
            ui.separator();
            ui.label(RichText::new("The camera does not turn: selections stay rectangles of the stage, the squares at their edges come out partial.").weak().size(11.5));
            ui.horizontal(|ui| {
                if ui.button("Level grid").on_hover_text("turn the view (all selections together) so the lattice is horizontal and vertical").clicked() {
                    app.level_grid();
                }
                if ui.add_enabled(app.proj.sample().turn_deg != 0.0, egui::Button::new("Stage axes")).on_hover_text("back to the stage's own axes").clicked() {
                    app.proj.sample_mut().turn_deg = 0.0;
                    app.fit = true;
                    app.changed();
                }
            });
            if app.proj.sample().turn_deg != 0.0 {
                ui.label(RichText::new(format!("view turned {:.3} deg", app.proj.sample().turn_deg)).monospace().size(11.0));
            }
            ui.separator();
            ui.label(RichText::new("Align before export: every map is moved onto the lattice by the crosses on it, so neighbouring selections meet pixel to pixel.").weak().size(11.5));
            if ui.button("Align maps to the lattice").clicked() {
                app.align_all();
            }
            let n = app.proj.sample().selections.iter().filter(|s| s.fix.is_some()).count();
            if n > 0 {
                ui.label(RichText::new(format!("{n} of {} maps aligned", app.proj.sample().selections.len())).size(11.0));
                if ui.small_button("Undo align").clicked() {
                    for s in app.proj.sample_mut().selections.iter_mut() {
                        s.fix = None;
                    }
                    app.changed();
                }
            }
        }
    });
}

pub fn settings(app: &mut App, ctx: &egui::Context) {
    let mut open = app.settings_open;
    let mut changed = false;
    egui::Window::new("Settings").open(&mut open).resizable(false).show(ctx, |ui| {
        egui::Grid::new("set").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Server");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut app.address);
                if ui.button("Connect").clicked() {
                    app.link.state.lock().unwrap().address = app.address.clone();
                }
            });
            ui.end_row();
            ui.label("Optics profile");
            ui.horizontal(|ui| {
                ui.label(app.proj.optics().profile_path.clone().unwrap_or_else(|| format!("{} (estimate)", app.proj.optics().objective)));
                if ui.button("Load...").on_hover_text("an MStitch calibration profile (json)").clicked() {
                    if let Some(f) = rfd::FileDialog::new().add_filter("profile", &["json"]).pick_file() {
                        match crate::geom::Optics::load(&f) {
                            Ok(o) => {
                                *app.proj.optics_mut() = o;
                                changed = true;
                            }
                            Err(e) => app.message = e,
                        }
                    }
                }
            });
            ui.end_row();
            ui.label("um per pixel");
            ui.label(format!("{:.4}", app.proj.optics().um_per_px()));
            ui.end_row();
            ui.label("Lens distortion k1, k2");
            ui.horizontal(|ui| {
                changed |= ui.add(egui::DragValue::new(&mut app.proj.optics_mut().k[0]).speed(0.0005).range(-0.5..=0.5).fixed_decimals(4)).changed();
                changed |= ui.add(egui::DragValue::new(&mut app.proj.optics_mut().k[1]).speed(0.0005).range(-0.5..=0.5).fixed_decimals(4)).changed();
            });
            ui.end_row();
            ui.label("Calibrate optics");
            ui.horizontal(|ui| {
                let busy = app.calib.is_some();
                if ui.add_enabled(!busy && app.link.stage_pos().is_some(), egui::Button::new("Measure here"))
                    .on_hover_text("9 frames of this spot, the stage moved by a third of the field between them: scale, turn and lens distortion. Needs texture in the field.")
                    .clicked()
                {
                    app.start_calibration();
                }
                if let Some(c) = &app.calib {
                    let c = c.lock().unwrap();
                    ui.label(format!("{} {}/{}", c.phase, c.done, c.total));
                }
            });
            ui.end_row();
            ui.label("White balance R G B");
            ui.horizontal(|ui| {
                let mut g = app.proj.wb;
                let mut wb_changed = false;
                for v in g.iter_mut() {
                    wb_changed |= ui.add(egui::DragValue::new(v).speed(0.005).range(0.2..=5.0).fixed_decimals(3)).changed();
                }
                if wb_changed {
                    app.set_white(g);
                }
            });
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                if ui.button("Pick grey").on_hover_text("then click on the map on something that should be grey or white").clicked() {
                    app.tool = Tool::White;
                    app.message = "click on something that should be grey or white".into();
                }
                if ui.button("Auto (live)").on_hover_text("grey world: the live picture's average made grey").clicked() {
                    let mean = app.live_pic.as_ref().and_then(|(_, p)| {
                        let b = p.pyr.base();
                        b.mean(0, 0, b.w, b.h)
                    });
                    match mean {
                        Some(c) if c.iter().all(|v| *v > 8.0) => {
                            let wb = app.proj.wb;
                            let grey = (c[0] + c[1] + c[2]) / 3.0;
                            let g = [wb[0] * grey / c[0], wb[1] * grey / c[1], wb[2] * grey / c[2]];
                            app.set_white(g.map(|v| v / g[1]));
                        }
                        _ => app.message = "no live picture to balance on".into(),
                    }
                }
                if ui.button("Off").clicked() {
                    app.set_white([1.0, 1.0, 1.0]);
                }
            });
            ui.end_row();
            ui.label("Motor step, um");
            changed |= ui.add(egui::DragValue::new(&mut app.proj.step_um).speed(0.001).range(0.001..=1000.0)).changed();
            ui.end_row();
            ui.label("Overlap quick / quality");
            ui.horizontal(|ui| {
                changed |= ui.add(egui::Slider::new(&mut app.proj.scan.overlap_quick, 0.05..=0.6)).changed();
                changed |= ui.add(egui::Slider::new(&mut app.proj.scan.overlap_quality, 0.05..=0.6)).changed();
            });
            ui.end_row();
            ui.label("Settle before a frame, ms");
            changed |= ui.add(egui::DragValue::new(&mut app.proj.scan.settle_ms).range(0.0..=2000.0)).changed();
            ui.end_row();
            ui.label("Backlash approach, um");
            changed |= ui.add(egui::DragValue::new(&mut app.proj.scan.back_um).range(0.0..=500.0)).changed();
            ui.end_row();
            ui.label("Autofocus while scanning");
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut app.proj.scan.autofocus, "").changed();
                ui.label("blurred below");
                changed |= ui.add(egui::DragValue::new(&mut app.proj.scan.focus_drop).speed(0.01).range(0.3..=0.95)).changed();
                ui.label("of the last sharp; look up to +-");
                changed |= ui.add(egui::DragValue::new(&mut app.proj.scan.focus_max_um).speed(1.0).range(5.0..=100.0)).changed();
                ui.label("um");
            });
            ui.end_row();
            ui.label("MStitch command");
            changed |= ui.text_edit_singleline(&mut app.proj.scan.mstitch).changed();
            ui.end_row();
            ui.label("Arrow step, part of the field");
            ui.add(egui::Slider::new(&mut app.arrow_frac, 0.1..=1.0));
            ui.end_row();
            ui.label("Mirror the view");
            ui.horizontal(|ui| {
                ui.checkbox(&mut app.flip_x, "x");
                ui.checkbox(&mut app.flip_y, "y");
            });
            ui.end_row();
            ui.label("Live preview fps");
            let mut fps = app.link.state.lock().unwrap().live_fps;
            if ui.add(egui::Slider::new(&mut fps, 1.0..=30.0)).changed() {
                app.link.state.lock().unwrap().live_fps = fps;
            }
            ui.end_row();
        });
        ui.separator();
        ui.label(RichText::new("Keys: arrows move the picture that way on the screen (Shift: 1/10, Ctrl: a whole field), Space stops, F follows the stage, Home shows the whole sample, E/S/L tools, Esc cancels a selection. Mouse: drag pans, wheel zooms, double click moves the stage there.").weak().size(11.5));
    });
    app.settings_open = open;
    if changed {
        app.changed();
    }
}

fn mmss(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// The status bar: the link, the objective, the scan running and its stitching, the queue and the time left.
pub fn status(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::bottom("status").exact_size(30.0).show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            let (connected, server, err, fps) = {
                let s = app.link.state.lock().unwrap();
                (s.connected, s.server.clone(), s.error.clone(), s.fps_in)
            };
            if connected {
                ui.label(RichText::new(format!("Connected: {server}")).color(Color32::from_rgb(90, 210, 120)));
                ui.label(RichText::new(format!("{fps:.0} fps")).weak());
            } else {
                ui.label(RichText::new("Not connected").color(Color32::from_rgb(230, 90, 70))).on_hover_text(err);
            }
            ui.separator();
            ui.label(format!("Objective {}", app.proj.optics().objective));
            ui.separator();
            if let Some(o) = app.wait_objective.clone() {
                ui.label(RichText::new(format!("Queue waits: put the {o} objective on")).strong().color(Color32::from_rgb(250, 190, 60)));
                if ui.button(format!("{o} is on: continue")).clicked() {
                    app.objective_changed(&o);
                }
                ui.separator();
            }
            let sels = app.proj.sample().selections.clone();
            let name = |id: u32| sels.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_else(|| "?".into());
            for j in &app.jobs {
                let j = j.lock().unwrap();
                match &j.phase {
                    crate::scan::Phase::Scanning => {
                        let f = j.done as f32 / j.total.max(1) as f32;
                        let left = (j.total - j.done) as f64 * app.sec_per_frame;
                        ui.label(format!("{} scan, {}", j.mode.name(), name(j.selection)));
                        ui.add(egui::ProgressBar::new(f).desired_width(220.0).text(format!("{}/{} frames, {} left", j.done, j.total, mmss(left))));
                    }
                    crate::scan::Phase::Stitching => {
                        let t = j.stitch_at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
                        ui.label(format!("stitching {}", name(j.selection)));
                        ui.add(egui::ProgressBar::new(((t / 2.0).fract()) as f32).desired_width(110.0).animate(true).text(mmss(t)));
                    }
                    _ => {}
                }
                ui.separator();
            }
            if let Some(c) = &app.calib {
                let c = c.lock().unwrap();
                ui.label("calibrating optics");
                ui.add(egui::ProgressBar::new(c.done as f32 / c.total.max(1) as f32).desired_width(160.0).text(format!("{} {}/{}", c.phase, c.done, c.total)));
                ui.separator();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if app.queue.is_empty() { "Queue: empty".to_string() } else {
                    let mut by: Vec<(String, usize)> = vec![];
                    for t in &app.queue {
                        match by.iter_mut().find(|b| b.0 == t.objective) {
                            Some(b) => b.1 += 1,
                            None => by.push((t.objective.clone(), 1)),
                        }
                    }
                    format!("Queue: {} ({})", app.queue.len(), by.iter().map(|(o, n)| format!("{o} x{n}")).collect::<Vec<_>>().join(", "))
                };
                if ui.button("Queue...").clicked() {
                    app.queue_open = !app.queue_open;
                }
                let left = app.time_left();
                if left > 0.0 {
                    ui.label(RichText::new(format!("all done in ~{}", mmss(left))).strong());
                }
                ui.label(label);
                if app.paused {
                    ui.label(RichText::new("paused").color(Color32::from_rgb(250, 190, 60)));
                }
            });
        });
    });
}

/// The queue: what will be scanned, in that order (grouped by objective), with the time each takes.
pub fn queue(app: &mut App, ctx: &egui::Context) {
    let mut open = app.queue_open;
    egui::Window::new("Queue").open(&mut open).default_width(460.0).show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui.button(if app.paused { "Resume" } else { "Pause after this scan" }).clicked() {
                app.paused = !app.paused;
            }
            if ui.button("Clear").clicked() {
                app.queue.clear();
                app.wait_objective = None;
            }
            if ui.button("Quick scan of every selection without a map").clicked() {
                let ids: Vec<u32> = app.proj.sample().selections.iter().filter(|s| s.best_scan().is_none()).map(|s| s.id).collect();
                for id in ids {
                    app.enqueue(id, Mode::Quick);
                }
            }
        });
        ui.label(RichText::new("Scans with the objective on the microscope go first; for the next objective the queue stops and asks for it, so each objective is changed once.").weak().size(11.5));
        ui.separator();
        let sels = app.proj.sample().selections.clone();
        let mut act: Option<(usize, i32)> = None;
        egui::Grid::new("q").striped(true).num_columns(6).show(ui, |ui| {
            for (k, t) in app.queue.iter().enumerate() {
                let name = sels.iter().find(|s| s.id == t.selection).map(|s| s.name.clone()).unwrap_or_default();
                ui.label(format!("{}.", k + 1));
                ui.label(&t.objective);
                ui.label(format!("{} {name}", t.mode.name()));
                ui.label(format!("{} frames, ~{}", t.frames, mmss(t.frames as f64 * app.sec_per_frame)));
                ui.horizontal(|ui| {
                    if ui.small_button("Up").clicked() {
                        act = Some((k, -1));
                    }
                    if ui.small_button("Down").clicked() {
                        act = Some((k, 1));
                    }
                    if ui.small_button("Remove").clicked() {
                        act = Some((k, 0));
                    }
                });
                ui.end_row();
            }
        });
        if app.queue.is_empty() {
            ui.label(RichText::new("Nothing waiting. Quick scan / Quality scan on a selection puts it here.").weak());
        }
        if let Some((k, d)) = act {
            match d {
                0 => {
                    app.queue.remove(k);
                }
                -1 if k > 0 => app.queue.swap(k, k - 1),
                1 if k + 1 < app.queue.len() => app.queue.swap(k, k + 1),
                _ => {}
            }
        }
    });
    app.queue_open = open;
}
