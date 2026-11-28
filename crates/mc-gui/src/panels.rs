//! Everything around the map: the toolbar, the selections, the lattice, the log and the settings.

use crate::project::{Mode, Sample};
use crate::{App, Tool};
use eframe::egui::{self, Color32, RichText};
use serde_json::json;

pub fn top(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("top").show(ui, |ui| {
        ui.horizontal(|ui| {

            use crate::keys::Act;
            for (t, name, act) in [(Tool::Explore, "Explore", Act::Explore), (Tool::Select, "Select", Act::Select), (Tool::Lattice, "Lattice", Act::Lattice)] {
                let k = app.keys.text(act);
                let label = if k.is_empty() { name.to_string() } else { format!("{name} ({k})") };
                if ui.selectable_label(app.tool == t, label).clicked() {
                    app.tool = t;
                    app.anchor = None;
                }
            }
            if !ui.ctx().text_edit_focused() && app.capturing.is_none() {
                for (k, t) in [(Act::Explore, Tool::Explore), (Act::Select, Tool::Select), (Act::Lattice, Tool::Lattice)] {
                    if app.keys.pressed(ui, k) {
                        app.tool = t;
                        app.anchor = None;
                    }
                }
            }
            ui.separator();
            if ui.button("Snap").on_hover_text("one frame, kept in the preview").clicked() {
                app.link.send(json!({"cmd": "snap"}));
            }
            if ui.button(RichText::new(with_key("Stop", app.keys.text(crate::keys::Act::Stop))).color(Color32::from_rgb(255, 120, 100))).on_hover_text("stops the stage and the scan; the queue pauses").clicked() {
                app.stop_all();
            }
            let (light, laser_mw, _) = app.link.lamps();
            let lt = if light > 0 { RichText::new(format!("Light {light}")).color(Color32::from_rgb(250, 220, 120)) } else { RichText::new("Light off") };
            if ui.button(lt).on_hover_text("F2: on / off (level in Settings)").clicked() {
                app.toggle_light();
            }
            let lz = if laser_mw > 0 { RichText::new(format!("LASER {laser_mw} mW")).strong().color(Color32::from_rgb(255, 70, 70)) } else { RichText::new("Laser off") };
            if ui.button(lz).on_hover_text("Shift+F3: on, F3 or Space: off (power in Settings)").clicked() {
                app.laser(laser_mw == 0);
            }
            let busy_f = app.focusing.is_some();
            if ui.add_enabled(!busy_f, egui::Button::new(if busy_f { "Focusing..." } else { "Focus" })).on_hover_text("find focus here (as a scan does); by hand: . and , keys (Shift: x5)").clicked() {
                app.focus_here();
            }
            if ui.button("Summary").on_hover_text("how well each stitch went: its numbers, targets and their history").clicked() {
                app.summary_open = !app.summary_open;
            }
            if ui.button("Keys").on_hover_text("every action and its key, to change").clicked() {
                app.keys_open = !app.keys_open;
            }
            if ui.button("Controls").on_hover_text("how the picture stands on the screen and which way the arrows move the stage, to your liking").clicked() {
                app.controls_open = !app.controls_open;
            }
            ui.menu_button("Calibrate", |ui| {
                let can = app.calib.is_none() && app.link.stage_pos().is_some();
                ui.label(RichText::new(format!("for {}, on a spot with texture (no repeating pattern)", app.proj.optics().objective)).weak().size(11.5));
                if ui.add_enabled(can, egui::Button::new("Orientation")).on_hover_text("3 frames: how the camera is turned or mirrored against the stage, and its scale").clicked() {
                    app.start_calibration(true);
                    ui.close();
                }
                if ui.add_enabled(can, egui::Button::new("Distortion")).on_hover_text("9 frames: the lens's distortion k1, k2 (and the orientation and scale again)").clicked() {
                    app.start_calibration(false);
                    ui.close();
                }
                if ui.button("White balance").on_hover_text("the live picture's average made grey; for a spot of your choice: Settings, Pick grey").clicked() {
                    app.auto_white();
                    ui.close();
                }
                ui.separator();
                let flat_can = app.flat_job.is_none() && app.calib.is_none() && app.link.stage_pos().is_some();
                if ui.add_enabled(flat_can, egui::Button::new("Flat field"))
                    .on_hover_text("the frame darker to its edges (vignetting, uneven light): 25 frames around here (about half a field apart) and 2 with the light off for a moment, best on an even, clean part of the substrate (or a little out of focus), not saturated. Then live pictures and scans with this objective are corrected.")
                    .clicked()
                {
                    app.start_flat();
                    ui.close();
                }
                let zcan = app.zcal.is_none() && app.calib.is_none() && app.link.stage_pos().is_some();
                if ui.add_enabled(zcan, egui::Button::new("Scale vs focus"))
                    .on_hover_text("the scale measured at z - dz, z, z + dz (Settings): how much the picture grows with focus; each scanned frame is then brought to the scale at the scan's start")
                    .clicked()
                {
                    app.start_zscale();
                    ui.close();
                }
                if app.proj.optics().flat.is_some() && ui.button("Flat field off").on_hover_text("no correction for this objective").clicked() {
                    app.flat_off();
                    ui.close();
                }
            });
            if ui.button(with_key("Follow", app.keys.text(crate::keys::Act::Follow))).clicked() {
                app.follow = true;
            }
            if ui.button(with_key("Fit", app.keys.text(crate::keys::Act::Fit))).on_hover_text("the whole sample").clicked() {
                app.fit = true;
            }
            if ui.button("Clear preview").on_hover_text("forget the small pictures of where the camera has been (stitched maps stay)").clicked() {
                app.thumbs.clear();
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
                    blocks(ui, e.done as f32 / e.total.max(1) as f32, 180.0, &e.stage);
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
            ui.add(egui::Label::new(RichText::new(&app.message).strong()).truncate()).on_hover_text(&app.message);
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
            ui.label(RichText::new("higher = drawn on top; new ones go first").weak());
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
                let nudging = app.nudging;
                let is_base = app.proj.sample().lattice_sel == Some(sid);
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
                        changed |= ui.checkbox(&mut s.visible, "").on_hover_text("shown on the map and exported (cut into squares)").changed();
                        if is_base {
                            ui.label(RichText::new("base").color(Color32::from_rgb(240, 90, 70)).size(11.0))
                                .on_hover_text("the lattice was marked on this map: it moves with it (Move map), the other maps are aligned onto it");
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Opacity").size(11.0));
                        changed |= ui.add(egui::Slider::new(&mut s.opacity, 0.0..=1.0).show_value(true)).on_hover_text("on the map and in the exported squares").changed();
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
                        // one line; the whole note on hover
                        let short: String = if w.chars().count() > 70 { w.chars().take(70).collect::<String>() + "..." } else { w.clone() };
                        ui.add(egui::Label::new(RichText::new(short).size(11.0).color(Color32::from_rgb(240, 170, 60))).truncate()).on_hover_text(w);
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
                        let moving = nudging == Some(s.id);
                        if ui.selectable_label(moving, "Move map").on_hover_text("the arrows move this map (1 px, Shift 10 px), not the stage; Enter: done").clicked() {
                            action = Some((k, "nudge"));
                        }
                        if !busy && s.scans.iter().any(|sc| !sc.frames.is_empty()) && ui.small_button("Stitch again").on_hover_text("the last scan's frames stitched once more (a newer MStitch, other settings), no scanning").clicked() {
                            action = Some((k, "restitch"));
                        }
                        if !is_base && ui.small_button("Make base").on_hover_text("the lattice belongs to this map: it moves with it, the others are aligned onto it").clicked() {
                            action = Some((k, "base"));
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
                "nudge" => {
                    app.nudging = if app.nudging == Some(id) { None } else { Some(id) };
                    if app.nudging.is_some() {
                        app.message = format!("moving the map of {}: arrows (Shift: x10), Enter: done", app.proj.sample().selections[k].name);
                    }
                }
                "restitch" => app.restitch(id),
                "base" => {
                    app.proj.sample_mut().lattice_sel = Some(id);
                    app.message = format!("{} is the base now: the lattice moves with its map, Align brings the others onto it", app.proj.sample().selections[k].name);
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
        ui.label(RichText::new("Click crosses on a stitched map, then sign each with its numbers (X, Y) from the sample. Once 2 are signed, the lattice and the numbers of all squares follow; 3 and more: fitted.").weak().size(11.5));
        // the sample's lithography crosses: chosen once, at the start
        {
            let designs = crate::project::Design::builtins();
            let now = app.proj.sample().design.clone();
            let signed = app.proj.sample().crosses.iter().any(|c| c.label.is_some());
            ui.horizontal(|ui| {
                ui.label("Crosses:");
                let text = now.as_ref().map(|d| d.name.clone()).unwrap_or_else(|| "not chosen".into());
                let mut pick: Option<Option<crate::project::Design>> = None;
                egui::ComboBox::from_id_salt("design").width(190.0).selected_text(RichText::new(text).color(if now.is_none() { Color32::from_rgb(250, 200, 80) } else { Color32::LIGHT_GRAY })).show_ui(ui, |ui| {
                    for d in &designs {
                        if ui.selectable_label(now.as_ref() == Some(d), &d.name).clicked() {
                            pick = Some(Some(d.clone()));
                        }
                    }
                });
                if let Some(p) = pick {
                    if p != now {
                        if signed && now.is_some() && app.design_confirm != p.as_ref().map(|d| d.name.clone()) {
                            // changing it later is not wanted: ask once more
                            app.design_confirm = p.as_ref().map(|d| d.name.clone());
                            app.message = "the sample's crosses are chosen already and crosses are signed: choose the other once more to change it".into();
                        } else {
                            app.design_confirm = None;
                            let smp = app.proj.sample_mut();
                            smp.design = p;
                            let _ = smp.refit();
                            app.changed();
                        }
                    }
                }
            });
            if now.is_none() {
                ui.label(RichText::new("Choose the sample's crosses first (once): their numbers then give drawing coordinates.").color(Color32::from_rgb(250, 200, 80)).size(11.0));
            }
            if let Some(d) = &now {
                let out: Vec<usize> = app.proj.sample().crosses.iter().enumerate().filter(|(_, c)| c.label.is_some_and(|l| l[0] < 1 || l[1] < 1 || l[0] > d.n[0] || l[1] > d.n[1])).map(|(k, _)| k + 1).collect();
                if !out.is_empty() {
                    ui.label(RichText::new(format!("crosses {out:?}: numbers outside 1..{} of this drawing", d.n[0])).color(Color32::from_rgb(240, 90, 70)).size(11.0));
                }
                if let Some(l) = &app.proj.sample().lattice {
                    let p = l.pitch();
                    ui.label(RichText::new(format!("pitch on the stage {:.3} um (drawing {:.3}): {:+.0} ppm", (p[0] + p[1]) / 2.0, d.pitch[0], ((p[0] + p[1]) / 2.0 / d.pitch[0] - 1.0) * 1e6)).monospace().size(11.0));
                }
            }
        }
        ui.separator();
        let mut remove = None;
        let mut changed = false;
        let mut bad = false;
        let edits = &mut app.label_edits;
        let s = app.proj.samples.get_mut(app.proj.active).unwrap();
        for (k, c) in s.crosses.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{}.", k + 1));
                match c.label.as_mut() {
                    Some(l) => {
                        ui.label("X");
                        changed |= ui.add(egui::DragValue::new(&mut l[0]).speed(0.1)).changed();
                        ui.label("Y");
                        changed |= ui.add(egui::DragValue::new(&mut l[1]).speed(0.1)).changed();
                    }
                    None => {
                        // not signed yet: type its numbers
                        let buf = edits.entry(k).or_insert_with(|| (String::new(), String::new()));
                        ui.label("X");
                        ui.add(egui::TextEdit::singleline(&mut buf.0).desired_width(34.0));
                        ui.label("Y");
                        let r = ui.add(egui::TextEdit::singleline(&mut buf.1).desired_width(34.0));
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.small_button("Sign").clicked() || enter {
                            match (buf.0.trim().parse::<i32>(), buf.1.trim().parse::<i32>()) {
                                (Ok(x), Ok(y)) => {
                                    c.label = Some([x, y]);
                                    edits.remove(&k);
                                    changed = true;
                                }
                                _ => bad = true,
                            }
                        }
                    }
                }
                if ui.small_button("Delete").clicked() {
                    remove = Some(k);
                }
            });
            ui.label(RichText::new(format!("   at {:.2}, {:.2} um", c.at[0], c.at[1])).monospace().size(11.0));
        }
        if let Some(k) = remove {
            s.crosses.remove(k);
            edits.clear();
            changed = true;
        }
        let fit = if changed { Some(s.refit()) } else { None };
        if bad {
            app.message = "X and Y are whole numbers, e.g. 3 and 7".into();
        }
        ui.separator();
        match &s.lattice {
            Some(l) => {
                let p = l.pitch();
                let ang = l.a[1].atan2(l.a[0]).to_degrees();
                ui.label(RichText::new(format!("pitch {:.3} x {:.3} um\nturn {:.4} deg\nfit rms {:.3} um", p[0], p[1], ang, l.rms_um)).monospace());
            }
            None => {
                ui.label("No lattice yet: sign at least 2 crosses with their X, Y.");
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
            if ui.button("Fit to all crosses").on_hover_text("every cross of the lattice looked for on the base map and the lattice fitted through all of them (2 signed crosses fix it only near them; far away a small error grows)").clicked() {
                app.fit_all_crosses();
            }
            if app.proj.sample().auto_valid() {
                let l = app.proj.sample().lattice.clone();
                let n = app.proj.sample().auto_marks.len();
                if let Some(l) = l {
                    ui.label(RichText::new(format!("fitted through {} more crosses, rms {:.2} um", n, l.rms_um)).monospace().size(11.0));
                }
                if ui.small_button("Signed crosses only").clicked() {
                    let smp = app.proj.sample_mut();
                    smp.auto_marks.clear();
                    smp.auto_for.clear();
                    let _ = smp.refit();
                    app.changed();
                }
            }
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
            ui.label("Light (F2)");
            ui.horizontal(|ui| {
                let (light, laser_mw, laser_max) = app.link.lamps();
                let mut on = light > 0;
                if ui.checkbox(&mut on, "on").changed() {
                    app.toggle_light();
                }
                ui.label("level");
                let mut lv = app.proj.light_level;
                if ui.add(egui::Slider::new(&mut lv, 1..=5)).changed() {
                    app.proj.light_level = lv;
                    changed = true;
                    if light > 0 {
                        app.link.send(json!({"cmd": "light", "level": lv}));
                    }
                }
                let _ = (laser_mw, laser_max);
            });
            ui.end_row();
            ui.label("Laser (Shift+F3 on, F3 off)");
            ui.horizontal(|ui| {
                let (_, laser_mw, laser_max) = app.link.lamps();
                let mut mw = app.proj.laser_mw;
                let top = if laser_max > 0 { laser_max } else { 100 };
                if ui.add(egui::DragValue::new(&mut mw).range(0..=top).suffix(" mW")).changed() {
                    app.proj.laser_mw = mw;
                    changed = true;
                }
                if laser_max > 0 {
                    ui.label(RichText::new(format!("max {laser_max}")).weak());
                }
                if laser_mw > 0 {
                    if ui.button(RichText::new(format!("Switch off ({laser_mw} mW on)")).color(Color32::from_rgb(255, 90, 90))).clicked() {
                        app.laser(false);
                    }
                } else if ui.button("Switch on").on_hover_text("needs mc-server started with --allow-laser").clicked() {
                    app.laser(true);
                }
            });
            ui.end_row();
            ui.label("Camera exposure, ms");
            camera(app, ui);
            ui.end_row();
            ui.label("Camera orientation");
            ui.horizontal(|ui| {
                // m P with P a signed permutation: the picture turned / mirrored against the stage
                let mut apply = |p: [f64; 4]| {
                    let [a, b, c, d] = app.proj.optics().m;
                    app.proj.optics_mut().m = [a * p[0] + b * p[2], a * p[1] + b * p[3], c * p[0] + d * p[2], c * p[1] + d * p[3]];
                    app.proj.optics_mut().profile_path = None;
                    app.thumbs.clear();
                    true
                };
                if ui.button("Rotate 90").clicked() {
                    changed |= apply([0.0, -1.0, 1.0, 0.0]);
                }
                if ui.button("Rotate 180").clicked() {
                    changed |= apply([-1.0, 0.0, 0.0, -1.0]);
                }
                if ui.button("Mirror x").clicked() {
                    changed |= apply([-1.0, 0.0, 0.0, 1.0]);
                }
                if ui.button("Mirror y").clicked() {
                    changed |= apply([1.0, 0.0, 0.0, -1.0]);
                }
                if ui.button("Straight").on_hover_text("no tilt: the nearest straight orientation, the scale kept").clicked() {
                    let m = crate::geom::Optics::straight(app.proj.optics().m);
                    app.proj.optics_mut().m = m;
                    app.thumbs.clear();
                    changed = true;
                }
                if ui.button("View like the camera").on_hover_text("turn the map so the live picture stands as the camera sees it (frame horizontal)").clicked() {
                    // the stage direction of the picture's +x, made to point right on the screen
                    let [a, b, c, d] = app.proj.optics().m;
                    let det = a * d - b * c;
                    let ux = [d / det, -c / det];
                    let v = crate::picture::View { center: [0.0, 0.0], scale: 1.0, rect: egui::Rect::ZERO, flip_x: app.flip_x, flip_y: app.flip_y, swap: app.swap, turn: (app.view_rot as f64 * 90.0).to_radians() };
                    let sd = v.dir_to_screen(ux);
                    app.proj.sample_mut().turn_deg = -sd[1].atan2(sd[0]).to_degrees();
                    app.fit = true;
                    changed = true;
                }
            });
            ui.end_row();
            ui.label("Lens distortion k1, k2");
            ui.horizontal(|ui| {
                changed |= ui.add(egui::DragValue::new(&mut app.proj.optics_mut().k[0]).speed(0.0005).range(-0.5..=0.5).fixed_decimals(4)).changed();
                changed |= ui.add(egui::DragValue::new(&mut app.proj.optics_mut().k[1]).speed(0.0005).range(-0.5..=0.5).fixed_decimals(4)).changed();
            });
            ui.end_row();
            ui.label("Lens k3, decentering p1, p2");
            ui.horizontal(|ui| {
                changed |= ui.add(egui::DragValue::new(&mut app.proj.optics_mut().k3).speed(0.0005).range(-0.5..=0.5).fixed_decimals(4)).changed();
                changed |= ui.add(egui::DragValue::new(&mut app.proj.optics_mut().p[0]).speed(0.0001).range(-0.05..=0.05).fixed_decimals(5)).changed();
                changed |= ui.add(egui::DragValue::new(&mut app.proj.optics_mut().p[1]).speed(0.0001).range(-0.05..=0.05).fixed_decimals(5)).changed();
            });
            ui.end_row();
            ui.label("Calibrate optics");
            ui.horizontal(|ui| {
                let busy = app.calib.is_some();
                if ui.add_enabled(!busy && app.link.stage_pos().is_some(), egui::Button::new("Measure here"))
                    .on_hover_text("9 frames of this spot, the stage moved by a third of the field between them: scale, turn and lens distortion. Needs texture in the field.")
                    .clicked()
                {
                    app.start_calibration(false);
                }
                if let Some(c) = &app.calib {
                    let c = c.lock().unwrap();
                    ui.label(format!("{} {}/{}", c.phase, c.done, c.total));
                }
            });
            ui.end_row();
            ui.label("Flat field");
            ui.horizontal(|ui| {
                match &app.proj.optics().flat {
                    Some(f) => ui.label(format!("on: edges up to {:.0} % darker, corrected", (1.0 - f.min as f64) * 100.0)),
                    None => ui.label(RichText::new("off").weak()),
                };
                let can = app.flat_job.is_none() && app.calib.is_none() && app.link.stage_pos().is_some();
                if ui.add_enabled(can, egui::Button::new("Measure here")).on_hover_text("25 frames around here (about half a field apart) and 2 with the light off for a moment, on an even part of the substrate, not saturated").clicked() {
                    app.start_flat();
                }
                if app.proj.optics().flat.is_some() && ui.button("Off").clicked() {
                    app.flat_off();
                }
            });
            ui.end_row();
            if app.proj.optics().flat.is_some() {
                ui.label("Flat field strength");
                ui.horizontal(|ui| {
                    let r = ui.add(egui::Slider::new(&mut app.proj.optics_mut().flat_strength, 0.5..=2.0).fixed_decimals(2));
                    if r.changed() {
                        app.changed();
                    }
                    ui.label(RichText::new("1 as measured; more if the edges stay dark, less if they turn bright").weak().size(11.0));
                });
                ui.end_row();
            }
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
                    app.auto_white();
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
                let word = if app.proj.scan.autofocus { "on" } else { "off" };
                changed |= ui
                    .checkbox(&mut app.proj.scan.autofocus, word)
                    .on_hover_text("off: every frame at the z the scan began at, nothing searched or followed")
                    .changed();
                ui.add_enabled_ui(app.proj.scan.autofocus, |ui| {
                    ui.label("blurred below");
                    changed |= ui.add(egui::DragValue::new(&mut app.proj.scan.focus_drop).speed(0.01).range(0.3..=0.95)).changed();
                    ui.label("of the last sharp; look up to +-");
                    changed |= ui.add(egui::DragValue::new(&mut app.proj.scan.focus_max_um).speed(1.0).range(5.0..=100.0)).changed();
                    ui.label("um");
                });
            });
            ui.end_row();
            ui.label("Scale vs focus");
            ui.horizontal(|ui| {
                changed |= ui.add(egui::DragValue::new(&mut app.proj.optics_mut().scale_per_um).speed(1e-7).range(-1e-3..=1e-3).custom_formatter(|v, _| format!("{:+.1} ppm/um", v * 1e6)).custom_parser(|t| t.trim().trim_end_matches("ppm/um").trim().parse::<f64>().ok().map(|v| v / 1e6))).changed();
                ui.label("measured +-");
                changed |= ui.add(egui::DragValue::new(&mut app.proj.scan.zscale_dz_um).speed(0.5).range(0.5..=100.0)).changed();
                ui.label("um");
            });
            ui.end_row();
            ui.label("Autofocus window, +- um");
            ui.horizontal(|ui| {
                changed |= ui.add(egui::DragValue::new(&mut app.proj.scan.focus_window_um).speed(1.0).range(0.0..=2000.0)).changed();
                ui.label(RichText::new("z never leaves this around where the scan started (or where Focus was pressed); 0: no limit").weak().size(11.0));
            });
            ui.end_row();
            ui.label("Z step (. and , keys / PageUp, PageDown), um");
            changed |= ui.add(egui::DragValue::new(&mut app.proj.scan.z_step_um).speed(0.5).range(0.5..=200.0)).on_hover_text("with Shift (> and <): five steps").changed();
            ui.end_row();
            ui.label("MStitch command");
            changed |= ui.text_edit_singleline(&mut app.proj.scan.mstitch).changed();
            ui.end_row();
            ui.label("Arrow step, part of the field");
            ui.add(egui::Slider::new(&mut app.arrow_frac, 0.1..=1.0));
            ui.end_row();
            ui.label("Stage on the screen: mirror");
            ui.horizontal(|ui| {
                ui.checkbox(&mut app.flip_x, "x");
                ui.checkbox(&mut app.flip_y, "y");
                ui.checkbox(&mut app.swap, "swap x and y").on_hover_text("the stage's x drawn up and down, y left and right: as the stage stands in front of you");
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
        ui.label(RichText::new("Keys: arrows move the picture that way on the screen (Shift: 1/10, Ctrl: a whole field), Space stops (and switches the laser off), . and , (or PageUp / PageDown) move z by the step, with Shift (> <) five steps, F2 light, Shift+F3 laser on / F3 off, F follows the stage, Home shows the whole sample, E/S/L tools, Esc cancels a selection. Mouse: drag pans, wheel zooms, double click moves the stage there.").weak().size(11.5));
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
            if let Some(p) = app.link.stage_pos() {
                ui.separator();
                ui.label(RichText::new(format!("x {:.1}  y {:.1}  z {:.1} um", p[0], p[1], p[2])).monospace());
            }
            let (_, laser_mw, _) = app.link.lamps();
            if laser_mw > 0 {
                ui.separator();
                ui.label(RichText::new(format!("LASER ON {laser_mw} mW")).strong().color(Color32::WHITE).background_color(Color32::from_rgb(200, 30, 30)));
            }
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
                        blocks(ui, f, 180.0, &format!("{}/{} frames, {} left", j.done, j.total, mmss(left)));
                    }
                    crate::scan::Phase::Stitching => {
                        let t = j.stitch_at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
                        ui.label(format!("stitching {}", name(j.selection)));
                        // MStitch's command line tells no progress: the time it took per frame before
                        let expect = (j.done.max(1) as f64 * app.stitch_per_frame).max(1.0);
                        blocks(ui, (t / expect).min(0.95) as f32, 120.0, &mmss(t));
                    }
                    _ => {}
                }
                ui.separator();
            }
            if let Some(j) = &app.zcal {
                let j = j.lock().unwrap();
                ui.label("scale vs focus");
                blocks(ui, j.done as f32 / j.total.max(1) as f32, 120.0, &format!("{}/{}", j.done, j.total));
                ui.separator();
            }
            if let Some(j) = &app.flat_job {
                let j = j.lock().unwrap();
                ui.label("flat field");
                blocks(ui, j.done as f32 / j.total.max(1) as f32, 120.0, &format!("{}/{}", j.done, j.total));
                ui.separator();
            }
            if let Some(c) = &app.calib {
                let c = c.lock().unwrap();
                ui.label("calibrating optics");
                blocks(ui, c.done as f32 / c.total.max(1) as f32, 120.0, &format!("{} {}/{}", c.phase, c.done, c.total));
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

/// Exposure, gain, auto exposure. Shown as the latest live frame was taken; sent when an edit is finished
/// (Enter, the mouse let go). Not while scanning: the frames of one map must look the same.
fn camera(app: &mut App, ui: &mut egui::Ui) {
    let now = app.link.state.lock().unwrap().live.as_ref().map(|l| (l.exposure_ms, l.gain));
    let scanning = app.jobs.iter().any(|j| j.lock().unwrap().phase == crate::scan::Phase::Scanning) || app.calib.is_some();
    let connected = app.link.state.lock().unwrap().connected;
    ui.add_enabled_ui(connected && !scanning, |ui| {
        ui.horizontal(|ui| {
            let (mut e, mut g) = app.cam_edit.or(now).unwrap_or((20.0, 1.0));
            let re = ui.add_enabled(!app.auto_exposure, egui::DragValue::new(&mut e).speed(0.1).range(0.05..=2000.0).fixed_decimals(2));
            ui.label("gain");
            let rg = ui.add(egui::DragValue::new(&mut g).speed(0.05).range(1.0..=16.0).fixed_decimals(0));
            if re.changed() || rg.changed() {
                app.cam_edit = Some((e, g));
            }
            let done = |r: &egui::Response| r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged() && !r.has_focus());
            if let Some((e, g)) = app.cam_edit {
                if done(&re) || done(&rg) {
                    if done(&re) {
                        app.link.send(json!({"cmd": "exposure", "ms": e}));
                    } else {
                        app.link.send(json!({"cmd": "gain", "value": g.round() as i64}));
                    }
                    // remembered for this objective: put on again later, it comes back by itself
                    app.proj.optics_mut().camera = Some([e, g.round()]);
                    app.auto_exposure = false;
                    app.cam_edit = None;
                    app.changed();
                }
            }
            if ui.checkbox(&mut app.auto_exposure, "auto").on_hover_text("the camera sets the exposure itself (not while scanning)").changed() {
                app.link.send(json!({"cmd": "auto_exposure", "on": app.auto_exposure}));
            }
        });
    })
    .response
    .on_disabled_hover_text(if scanning { "not while scanning: the frames of one map must have the same exposure" } else { "not connected" });
}

/// A plain green progress bar of blocks (as in old games): `frac` 0..1, the text beside it.
pub fn blocks(ui: &mut egui::Ui, frac: f32, width: f32, text: &str) {
    let h = 16.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, Color32::from_rgb(20, 24, 20));
    p.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, Color32::from_rgb(70, 110, 70)), egui::StrokeKind::Inside);
    let (bw, gap) = (7.0, 2.0);
    let inner = rect.shrink(3.0);
    let n = ((inner.width() + gap) / (bw + gap)).floor().max(1.0) as usize;
    let filled = (frac.clamp(0.0, 1.0) * n as f32).round() as usize;
    for k in 0..filled {
        let x = inner.left() + k as f32 * (bw + gap);
        p.rect_filled(egui::Rect::from_min_size(egui::pos2(x, inner.top()), egui::vec2(bw, inner.height())), 0.0, Color32::from_rgb(60, 200, 60));
    }
    ui.label(RichText::new(text).monospace());
}

/// A small notice in the corner of the map: the objective seems changed on the microscope, not here.
pub fn objective_hint(app: &mut App, ctx: &egui::Context) {
    let Some((text, suggest, at)) = app.objective_hint.clone() else { return };
    if at.elapsed().as_secs() > 60 {
        app.objective_hint = None;
        return;
    }
    egui::Area::new(egui::Id::unique("objective_hint")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -70.0)).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(Color32::from_rgb(60, 50, 20)).show(ui, |ui| {
            ui.set_max_width(330.0);
            ui.label(RichText::new(text).color(Color32::from_rgb(250, 220, 140)));
            ui.horizontal(|ui| {
                if let Some(o) = &suggest {
                    if ui.button(format!("Switch to {o}")).clicked() {
                        app.objective_changed(o);
                        app.objective_hint = None;
                    }
                }
                ui.label("On the microscope:");
                let cur = app.proj.optics().objective.clone();
                let names: Vec<String> = app.proj.objectives.iter().map(|o| o.objective.clone()).filter(|n| Some(n) != suggest.as_ref() && *n != cur).collect();
                for n in &names {
                    if ui.small_button(n).clicked() {
                        app.objective_changed(n);
                        app.objective_hint = None;
                    }
                }
                if ui.small_button("No").on_hover_text("nothing was changed").clicked() {
                    app.objective_hint = None;
                }
            });
        });
    });
}

/// One place to make the controls comfortable for this operator at this microscope: how the picture stands on
/// the screen, which way each arrow moves the stage (taught by watching the stage), the camera's orientation.
pub fn controls(app: &mut App, ctx: &egui::Context) {
    let mut open = app.controls_open;
    egui::Window::new("Controls").open(&mut open).default_width(470.0).show(ctx, |ui| {
        ui.heading("From a corner of the substrate");
        ui.label(RichText::new("All at once: put a corner of the substrate on the crosshair (the live picture's middle) and say which corner it is, as you see the substrate. The camera's orientation, the picture on the screen and the arrows are then set to match you.").weak().size(11.5));
        let can = app.link.stage_pos().is_some() && app.corner_job.is_none() && app.calib.is_none() && app.flat_job.is_none();
        let corner_btn = |ui: &mut egui::Ui, app: &mut App, label: &str, name: [f64; 2]| {
            if ui.add_enabled(can, egui::Button::new(label).min_size(egui::vec2(110.0, 0.0))).clicked() {
                app.start_corner(name);
            }
        };
        egui::Grid::new("corners").spacing([6.0, 4.0]).show(ui, |ui| {
            corner_btn(ui, app, "Top left", [1.0, -1.0]);
            corner_btn(ui, app, "Top right", [-1.0, -1.0]);
            ui.end_row();
            corner_btn(ui, app, "Bottom left", [1.0, 1.0]);
            corner_btn(ui, app, "Bottom right", [-1.0, 1.0]);
            ui.end_row();
        });
        ui.horizontal(|ui| {
            ui.label("The arrows move");
            ui.radio_value(&mut app.arrows_move_stage, false, "the view").on_hover_text("right: you look further to the right on the substrate (the stage itself goes left)");
            ui.radio_value(&mut app.arrows_move_stage, true, "the stage").on_hover_text("right: the stage itself goes to the right (the picture goes left)");
        });
        if let Some((_, j)) = &app.corner_job {
            let j = j.lock().unwrap();
            ui.label(format!("taking frames {}/{}...", j.done, j.total));
        }
        if app.corner.is_some() {
            ui.label(RichText::new("Corner found. Its two edges are drawn on the map, red and blue. Which one runs left-right for you? To see it: Show me, and watch the substrate on the stage itself, not the screen.").color(Color32::from_rgb(250, 200, 80)));
            ui.horizontal(|ui| {
                if ui.button("Show me").on_hover_text("the stage goes 1 mm along the red edge and comes back").clicked() {
                    app.corner_show();
                }
                if ui.button("Cancel").clicked() {
                    app.corner = None;
                    app.corner_shown = false;
                }
            });
            if app.corner_shown {
                ui.label("The substrate went:");
                ui.horizontal(|ui| {
                    if ui.button("left and right").clicked() {
                        app.corner_answer(0);
                    }
                    if ui.button("away and back towards me").clicked() {
                        app.corner_answer(1);
                    }
                    if ui.button("Again").clicked() {
                        app.corner_show();
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("or, if you can tell on the map:").weak().size(11.5));
                if ui.small_button(RichText::new("red runs left-right").color(Color32::from_rgb(240, 90, 70))).clicked() {
                    app.corner_answer(0);
                }
                if ui.small_button(RichText::new("blue runs left-right").color(Color32::from_rgb(90, 160, 255))).clicked() {
                    app.corner_answer(1);
                }
            });
        }
        ui.separator();
        ui.heading("1. The picture on the screen");
        ui.label(RichText::new("Only how the map is shown: turn and mirror it until it looks the way you like.").weak().size(11.5));
        ui.horizontal(|ui| {
            if ui.button("Rotate left").clicked() {
                app.view_rot = (app.view_rot + 3) % 4;
                app.fit = true;
            }
            if ui.button("Rotate right").clicked() {
                app.view_rot = (app.view_rot + 1) % 4;
                app.fit = true;
            }
            ui.checkbox(&mut app.flip_x, "mirror left-right");
            ui.checkbox(&mut app.flip_y, "mirror up-down");
        });
        ui.label(RichText::new(format!("turned {} deg", app.view_rot * 90)).monospace().size(11.0));
        ui.separator();

        ui.heading("2. The arrows");
        ui.label(RichText::new("Teach them by watching the stage itself: it moves a little and comes back; click where you saw it go. Then each arrow moves the stage the way it points, for you.").weak().size(11.5));
        let connected = app.link.stage_pos().is_some();
        for (axis, name) in [(0usize, "X"), (1, "Y")] {
            ui.horizontal(|ui| {
                let done = app.taught[axis].is_some();
                if ui.add_enabled(connected && app.teach.is_none(), egui::Button::new(format!("Move along {name}"))).clicked() {
                    app.teach_move(axis);
                }
                if app.teach == Some(axis) {
                    ui.label("it went:");
                    for (label, dir) in [("left", [-1.0, 0.0]), ("up (away)", [0.0, -1.0]), ("right", [1.0, 0.0]), ("down (to me)", [0.0, 1.0])] {
                        if ui.button(label).clicked() {
                            app.teach_answer(dir);
                        }
                    }
                } else if done {
                    ui.label(RichText::new("seen").color(Color32::from_rgb(120, 200, 140)));
                }
            });
        }
        // by hand: each arrow to an axis
        let axes = [("+X", [1.0, 0.0]), ("-X", [-1.0, 0.0]), ("+Y", [0.0, 1.0]), ("-Y", [0.0, -1.0])];
        let name_of = |v: [f64; 2]| axes.iter().find(|a| (a.1[0] - v[0]).abs() < 0.5 && (a.1[1] - v[1]).abs() < 0.5).map(|a| a.0).unwrap_or("?");
        ui.horizontal(|ui| {
            ui.label("By hand:");
            let mut dirs = app.arrow_dirs.unwrap_or([[1.0, 0.0], [0.0, -1.0]]);
            let mut set = false;
            for (k, arrow) in [(0usize, "right arrow"), (1, "up arrow")] {
                egui::ComboBox::from_id_salt(("arrow", k)).selected_text(format!("{arrow}: {}", name_of(dirs[k]))).show_ui(ui, |ui| {
                    for (n, v) in axes {
                        if ui.selectable_label(name_of(dirs[k]) == n, n).clicked() {
                            dirs[k] = v;
                            set = true;
                        }
                    }
                });
            }
            if set {
                app.arrow_dirs = Some(dirs);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Exchange:");
            ui.checkbox(&mut app.arrow_rev_x, "left with right");
            ui.checkbox(&mut app.arrow_rev_y, "up with down");
            ui.checkbox(&mut app.arrow_swap, "swap").on_hover_text("up moves right, down moves left, right moves up, left moves down");
        })
        .response
        .on_hover_text("only which way the arrows move the stage; the picture does not change");
        ui.horizontal(|ui| {
            match app.arrow_dirs {
                Some([r, u]) => ui.label(RichText::new(format!("right arrow: {}, up arrow: {} (left and down: the opposite)", name_of(r), name_of(u))).size(11.5)),
                None => ui.label(RichText::new("the arrows follow the screen").size(11.5)),
            };
            if app.arrow_dirs.is_some() && ui.small_button("Follow the screen again").clicked() {
                app.arrow_dirs = None;
            }
        });
        ui.separator();

        ui.heading("3. The camera");
        ui.label(RichText::new("How the camera's picture is turned or mirrored against the stage. Right when the sample in the live picture stands still on the map while the stage moves.").weak().size(11.5));
        ui.horizontal(|ui| {
            let can = app.calib.is_none() && app.link.stage_pos().is_some();
            if ui.add_enabled(can, egui::Button::new("Find it (3 frames)")).on_hover_text("on a spot with texture, no repeating pattern").clicked() {
                app.start_calibration(true);
            }
            let apply = |app: &mut App, p: [f64; 4]| {
                let [a, b, c, d] = app.proj.optics().m;
                app.proj.optics_mut().m = [a * p[0] + b * p[2], a * p[1] + b * p[3], c * p[0] + d * p[2], c * p[1] + d * p[3]];
                app.proj.optics_mut().profile_path = None;
                app.thumbs.clear();
                app.changed();
            };
            if ui.button("Rotate 90").clicked() {
                apply(app, [0.0, -1.0, 1.0, 0.0]);
            }
            if ui.button("Rotate 180").clicked() {
                apply(app, [-1.0, 0.0, 0.0, -1.0]);
            }
            if ui.button("Mirror x").clicked() {
                apply(app, [-1.0, 0.0, 0.0, 1.0]);
            }
            if ui.button("Mirror y").clicked() {
                apply(app, [1.0, 0.0, 0.0, -1.0]);
            }
            if ui.button("Straight").on_hover_text("no tilt: the nearest straight orientation, the scale kept").clicked() {
                let m = crate::geom::Optics::straight(app.proj.optics().m);
                app.proj.optics_mut().m = m;
                app.thumbs.clear();
                app.changed();
            }
        });
        let [a, b, c, d] = app.proj.optics().m;
        let ang = c.atan2(a).to_degrees();
        let mirror = a * d - b * c < 0.0;
        ui.label(RichText::new(format!("now: turned {ang:.1} deg{}, {:.4} um/px", if mirror { ", mirrored" } else { "" }, app.proj.optics().um_per_px())).monospace().size(11.0));
    });
    app.controls_open = open;
}

/// The keys: every action and the key that does it; Change, then press the key (with Shift / Ctrl if wanted).
pub fn keys_window(app: &mut App, ctx: &egui::Context) {
    use crate::keys::{Bind, ALL};
    let mut open = app.keys_open;
    // a key pressed while waiting for one: it becomes the binding
    if let Some(act) = app.capturing {
        let got = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some(Bind { key: *key, shift: modifiers.shift, ctrl: modifiers.command }),
                _ => None,
            })
        });
        if let Some(b) = got {
            app.keys.set(act, Some(b));
            app.capturing = None;
        }
    }
    egui::Window::new("Keys").open(&mut open).default_width(520.0).show(ctx, |ui| {
        ui.label(RichText::new("Change, then press the key (Shift / Ctrl with it if wanted). Red: the same key does two things. Kept for next time.").weak().size(11.5));
        let clashes = app.keys.clashes();
        egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
            egui::Grid::new("keys").striped(true).num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
                for a in ALL {
                    ui.label(a.label());
                    let t = app.keys.text(a);
                    let txt = if app.capturing == Some(a) { RichText::new("press a key...").color(Color32::from_rgb(250, 200, 80)) }
                        else if t.is_empty() { RichText::new("none").weak() }
                        else if clashes.contains(&a) { RichText::new(t).monospace().color(Color32::from_rgb(240, 90, 70)) }
                        else { RichText::new(t).monospace().strong() };
                    ui.label(txt);
                    ui.horizontal(|ui| {
                        if app.capturing == Some(a) {
                            if ui.small_button("Cancel").clicked() {
                                app.capturing = None;
                            }
                        } else if ui.small_button("Change").clicked() {
                            app.capturing = Some(a);
                        }
                        if ui.small_button("None").clicked() {
                            app.keys.set(a, None);
                        }
                        if ui.small_button("Default").clicked() {
                            app.keys.reset(a);
                        }
                    });
                    ui.end_row();
                }
            });
        });
        ui.separator();
        if ui.button("All back to the defaults").clicked() {
            app.keys = crate::keys::Keys::defaults();
            app.capturing = None;
        }
    });
    if !open {
        app.capturing = None;
    }
    app.keys_open = open;
}

/// "Stop (Space)", or "Stop" when the action has no key.
fn with_key(name: &str, key: String) -> String {
    if key.is_empty() { name.to_string() } else { format!("{name} ({key})") }
}

/// Summary: every stitch of the project, and for the one picked its numbers against their targets and their
/// history (mean +- 3 sigma of earlier stitches with the same objective and mode).
pub fn summary_window(app: &mut App, ctx: &egui::Context) {
    use crate::summary::{self, Verdict, METRICS};
    if !app.summary_open {
        return;
    }
    // every stitch with a map, newest first: (map folder, selection id, label)
    let mut all: Vec<(String, u32, String)> = vec![];
    for smp in &app.proj.samples {
        for s in &smp.selections {
            for sc in &s.scans {
                if let Some(m) = &sc.map {
                    all.push((m.clone(), s.id, format!("{} / {} - {} {}", smp.name, s.name, sc.mode.name(), sc.when)));
                }
            }
        }
    }
    all.sort_by(|a, b| b.2.split(" - ").nth(1).cmp(&a.2.split(" - ").nth(1)));
    if app.summary_pick.as_ref().is_none_or(|p| !all.iter().any(|a| &a.0 == p)) {
        app.summary_pick = all.first().map(|a| a.0.clone());
    }
    let mut open = app.summary_open;
    egui::Window::new("Summary").open(&mut open).default_width(820.0).default_height(560.0).show(ctx, |ui| {
        if all.is_empty() {
            ui.label("no stitched maps yet");
            return;
        }
        let pick = app.summary_pick.clone().unwrap();
        ui.horizontal(|ui| {
            ui.label("Stitch:");
            egui::ComboBox::from_id_salt("summary_pick").width(520.0).selected_text(all.iter().find(|a| a.0 == pick).map(|a| a.2.clone()).unwrap_or_default()).show_ui(ui, |ui| {
                for a in &all {
                    if ui.selectable_label(a.0 == pick, &a.2).clicked() {
                        app.summary_pick = Some(a.0.clone());
                    }
                }
            });
            if ui.button("Export CSV").on_hover_text("every stitch's numbers into metrics.csv in the project folder").clicked() {
                let path = app.proj.path("metrics.csv");
                let mut out = String::from("map,selection,mode,objective,when");
                for m in METRICS {
                    out += &format!(",{}", m.key);
                }
                out.push('\n');
                for a in &all {
                    let v = summary::read(&app.proj.path(&a.0));
                    let t = |k: &str| v[k].as_str().unwrap_or("").replace(',', ";");
                    out += &format!("{},{},{},{},{}", a.0, t("selection"), t("mode"), t("objective"), t("when"));
                    for m in METRICS {
                        out += &format!(",{}", summary::get(&v, m.key).map(|x| format!("{x}")).unwrap_or_default());
                    }
                    out.push('\n');
                }
                app.message = match std::fs::write(&path, out) {
                    Ok(()) => format!("metrics of {} stitches: {}", all.len(), path.display()),
                    Err(e) => format!("metrics.csv: {e}"),
                };
            }
        });
        let dir = app.proj.path(&pick);
        let mut v = summary::read(&dir);
        // the crosses: measured on the map where it is drawn now, once (again after Align)
        if v.get("crosses").is_none() {
            if let (Some(l), Some(sel)) = (app.proj.sample().lattice.clone(), all.iter().find(|a| a.0 == pick).and_then(|a| app.proj.samples.iter().flat_map(|s| s.selections.iter()).find(|s| s.id == a.1)).cloned()) {
                if sel.best_scan().and_then(|b| b.map.clone()).as_deref() == Some(pick.as_str()) {
                    if let Some((pic, place)) = app.map_of(&sel) {
                        if let Some(c) = summary::crosses(&l, &sel.rect, pic.pyr.base(), &place, app.proj.sample().design.as_ref().map(|d| d.pitch)) {
                            v["crosses"] = c;
                            summary::write(&dir, &v);
                        }
                    }
                }
            }
        }
        let (mode, objective) = (v["mode"].as_str().unwrap_or("").to_string(), v["objective"].as_str().unwrap_or("").to_string());
        ui.label(RichText::new(format!(
            "{} {}, {} frames, objective {}, scan {}, stitch {}",
            v["selection"].as_str().unwrap_or(""),
            mode,
            v["frames"].as_u64().unwrap_or(0),
            objective,
            v["scan_s"].as_f64().map(|t| mmss(t)).unwrap_or("-".into()),
            v["stitch_s"].as_f64().map(|t| mmss(t)).unwrap_or("-".into())
        )).size(12.0));
        // the history: earlier stitches with the same objective and mode
        let earlier: Vec<serde_json::Value> = all
            .iter()
            .filter(|a| a.0 != pick)
            .map(|a| summary::read(&app.proj.path(&a.0)))
            .filter(|h| h["mode"].as_str() == Some(mode.as_str()) && h["objective"].as_str() == Some(objective.as_str()))
            .collect();
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("summary").striped(true).num_columns(5).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["", "Value", "Target", "Usual (mean +- 3 sigma, n)", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for m in METRICS {
                    let val = summary::get(&v, m.key);
                    let hist_vals: Vec<f64> = earlier.iter().filter_map(|h| summary::get(h, m.key)).collect();
                    let hist = summary::history(&hist_vals);
                    ui.label(m.name).on_hover_text(m.help);
                    let unit = if m.unit.is_empty() { String::new() } else { format!(" {}", m.unit) };
                    let fmt = |x: f64| if x.abs() >= 100.0 { format!("{x:.0}") } else if x.abs() >= 1.0 { format!("{x:.2}") } else { format!("{x:.3}") };
                    match val {
                        Some(x) => {
                            let verdict = summary::verdict(m, x, hist);
                            let col = match verdict {
                                Verdict::Good => Color32::from_rgb(120, 200, 140),
                                Verdict::Unusual => Color32::from_rgb(250, 200, 80),
                                Verdict::Bad => Color32::from_rgb(240, 90, 70),
                                Verdict::Unknown => Color32::GRAY,
                            };
                            ui.label(RichText::new(format!("{}{unit}", fmt(x))).monospace().color(col));
                        }
                        None => {
                            ui.label(RichText::new("-").weak());
                        }
                    }
                    ui.label(match m.target {
                        Some(t) => format!("{} {}{unit}", if m.smaller { if m.abs { "|x| <=" } else { "<=" } } else { ">=" }, fmt(t)),
                        None => "watched".into(),
                    });
                    ui.label(match hist {
                        Some((mean, s)) => format!("{} +- {}{unit} (n {})", fmt(mean), fmt(3.0 * s), hist_vals.len()),
                        None => format!("(n {}, needs 3)", hist_vals.len()),
                    });
                    ui.label(match val.map(|x| summary::verdict(m, x, hist)) {
                        Some(Verdict::Good) => RichText::new("ok").color(Color32::from_rgb(120, 200, 140)),
                        Some(Verdict::Unusual) => RichText::new("unusual").color(Color32::from_rgb(250, 200, 80)),
                        Some(Verdict::Bad) => RichText::new("out of target").color(Color32::from_rgb(240, 90, 70)),
                        _ => RichText::new(""),
                    });
                    ui.end_row();
                }
            });
        });
        if let Some(ppm) = summary::get(&v, "mstitch.frame_scale_ppm") {
            if ppm.abs() > 20.0 && ui.button(format!("Correct um/px of {objective} by {ppm:+.0} ppm")).on_hover_text("the camera's scale measured against the drawing by the pinned crosses: the objective's um/px set from it (then scan or Stitch again)").clicked() {
                app.correct_scale(&objective, ppm);
            }
        }
        ui.label(RichText::new("Green: within the target. Yellow: within it, but out of this setup's usual range (mean +- 3 sigma of earlier stitches with the same objective and mode). Red: out of the target. Crosses: needs the lattice (2 signed crosses) and the map loaded; measured where the map is drawn now.").weak().size(11.0));
    });
    app.summary_open = open;
}
