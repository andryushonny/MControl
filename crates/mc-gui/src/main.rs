#![windows_subsystem = "windows"]
//! mc-operator: the operator's program on PC 2. A map of the stage with the live picture, a preview of where
//! the camera has been, selections, scans stitched by MStitch, the lattice of crosses, cutting and the DXF drawing.

mod align;
mod calib;
mod corner;
mod cross;
mod drawing;
mod export;
mod focus;
mod flat;
mod geom;
mod keys;
mod link;
mod map;
mod panels;
mod picture;
mod pixels;
mod project;
mod scan;
mod summary;
mod watch;

use eframe::egui;
use geom::P2;
use link::Link;
use picture::{Picture, Pyramid};
use project::Project;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1500.0, 950.0]).with_title("MControl operator"),
        // wgpu by default (DirectX 12 / Vulkan / Metal); MC_RENDERER=glow for old OpenGL-only machines
        renderer: if std::env::var("MC_RENDERER").as_deref() == Ok("glow") { eframe::Renderer::Glow } else { eframe::Renderer::Wgpu },
        ..Default::default()
    };
    eframe::run_native("MControl operator", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// look around, move the stage (arrows, double click), pan and zoom the map
    Explore,
    /// two clicks: the corners of a new selection
    Select,
    /// clicks on crosses of a stitched map: the lattice
    Lattice,
    /// a click on something that should be grey: the white balance
    White,
}

/// A scan waiting in the queue.
#[derive(Clone)]
pub struct Task {
    pub selection: u32,
    pub mode: project::Mode,
    pub objective: String,
    pub frames: usize,
    /// a round of a quality scan in several: (group, round, of)
    pub round: Option<(String, usize, usize)>,
}

pub struct ExportState {
    pub done: usize,
    pub total: usize,
    pub stage: String,
    pub result: Option<Result<PathBuf, String>>,
}

pub struct App {
    pub link: Arc<Link>,
    pub proj: Project,
    pub dirty: bool,
    pub center: P2,
    pub scale: f64,
    pub flip_x: bool,
    pub flip_y: bool,
    pub swap: bool,
    /// the arrow keys: up/down and left/right exchanged, then reversed (both: turned 180 degrees)
    pub arrow_swap: bool,
    pub arrow_rev_x: bool,
    pub arrow_rev_y: bool,
    /// taught arrows: the stage direction (unit, um) of the right arrow and of the up arrow (left, down: the
    /// opposite), as the operator sees the stage; None: the arrows follow the screen
    pub arrow_dirs: Option<[[f64; 2]; 2]>,
    /// the picture turned on the screen by this many quarter turns (the operator's taste)
    pub view_rot: i32,
    pub controls_open: bool,
    /// the arrow teaching: which axis was just moved (and back), waiting for where it went
    pub teach: Option<usize>,
    /// what has been taught so far: the screen-like direction (as seen) of +x and of +y
    pub taught: [Option<[f64; 2]>; 2],
    pub tool: Tool,
    pub anchor: Option<P2>,
    pub focus: Option<u32>,
    /// stitched maps by folder
    pub maps: HashMap<String, Picture>,
    /// each map's placement as stitched (map pixel -> stage), before Align's correction
    pub map_raw: HashMap<String, geom::Affine>,
    loading: HashMap<String, ()>,
    load_tx: Sender<(String, Result<(Arc<Pyramid>, geom::Affine), String>)>,
    load_rx: Receiver<(String, Result<(Arc<Pyramid>, geom::Affine), String>)>,
    /// the preview map: small pictures of where the camera has been
    pub thumbs: Vec<Picture>,
    pub last_thumb: (P2, Instant),
    pub live_pic: Option<(u64, Picture)>,
    pub jobs: Vec<Arc<Mutex<scan::Job>>>,
    pub export: Option<Arc<Mutex<ExportState>>>,
    pub calib: Option<Arc<Mutex<calib::Calib>>>,
    pub calib_orientation: bool,
    /// scans waiting, grouped by objective (the current one first)
    pub queue: Vec<Task>,
    pub paused: bool,
    /// the queue waits for the operator to put this objective on
    pub wait_objective: Option<String>,
    /// seconds per frame, learned from the scans done (for the time left)
    pub sec_per_frame: f64,
    /// stitching seconds per frame, learned (for its progress bar)
    pub stitch_per_frame: f64,
    pub queue_open: bool,
    /// camera settings being edited (exposure ms, gain); None: show what the camera has
    pub cam_edit: Option<(f64, f64)>,
    pub auto_exposure: bool,
    /// the objective's remembered exposure was given to the camera on this connection
    pub cam_applied: bool,
    /// watches the live picture for an objective turned on the microscope but not here
    pub watch: watch::Watch,
    /// the notice about it: (text, the objective to switch to, when)
    pub objective_hint: Option<(String, Option<String>, Instant)>,
    /// X, Y being typed for crosses not signed yet (by cross index)
    pub label_edits: HashMap<usize, (String, String)>,
    /// a focus search by hand, running (None) or done
    pub focusing: Option<Arc<Mutex<Option<Result<(f64, f64, usize), String>>>>>,
    /// the selection whose map the arrows move by hand (Move map)
    pub nudging: Option<u32>,
    /// when the work was last told to the server (for its web page), and when the last scan ended
    pub told: Instant,
    pub finished: Option<String>,
    /// every action's key (the Keys window), and the action waiting for its new key
    pub keys: keys::Keys,
    pub keys_open: bool,
    pub capturing: Option<keys::Act>,
    /// learning from a corner of the substrate: its name ([+1 left / -1 right, +1 bottom / -1 top]) and the
    /// frames being taken; then the corner found, waiting for which edge runs left-right
    pub corner_job: Option<([f64; 2], Arc<Mutex<corner::Job>>)>,
    pub corner: Option<([f64; 2], corner::Corner)>,
    /// the stage went along the corner's red edge and back: waiting for which way the operator saw it go
    pub corner_shown: bool,
    /// the arrows move the stage itself the way they point (else the picture: the view goes that way)
    pub arrows_move_stage: bool,
    /// the scale being measured at three focus heights
    pub zcal: Option<Arc<Mutex<calib::ZCal>>>,
    /// a flat field being taken
    pub flat_job: Option<Arc<Mutex<flat::Job>>>,
    pub summary_open: bool,
    /// a change of the sample's crosses waiting for a second choice
    pub design_confirm: Option<String>,
    /// the stitch shown in Summary (its map folder, relative)
    pub summary_pick: Option<String>,
    pub settings_open: bool,
    pub frame_no: u64,
    /// arrow keys move by this part of the field
    pub arrow_frac: f64,
    pub address: String,
    pub message: String,
    pub follow: bool,
    /// zoom to the whole sample on the next frame
    pub fit: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> App {
        let stored = |k: &str| cc.storage.and_then(|s| s.get_string(k));
        let address = std::env::var("MCONTROL_HOST").ok().or_else(|| stored("address")).unwrap_or_else(|| "auto".into());
        let folder = std::env::var("MC_PROJECT").ok().or_else(|| stored("project")).map(PathBuf::from).unwrap_or_else(default_project);
        let (proj, message) = match Project::open(&folder) {
            Ok(p) => (p, String::new()),
            Err(e) => (Project::new(&folder), e),
        };
        let name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "operator".into());
        let link = Link::start(cc.egui_ctx.clone(), address.clone(), name);
        let (load_tx, load_rx) = channel();
        let mut app = App {
            link,
            proj,
            dirty: false,
            center: [0.0, 0.0],
            scale: 0.25,
            flip_x: stored("flip_x").as_deref() == Some("true"),
            flip_y: stored("flip_y").as_deref() == Some("true"),
            swap: stored("swap").as_deref() == Some("true"),
            arrow_swap: stored("arrow_swap").as_deref() == Some("true"),
            arrow_rev_x: stored("arrow_rev_x").as_deref() == Some("true"),
            arrow_rev_y: stored("arrow_rev_y").as_deref() == Some("true"),
            arrow_dirs: stored("arrow_dirs").and_then(|s| serde_json::from_str(&s).ok()),
            view_rot: stored("view_rot").and_then(|s| s.parse().ok()).unwrap_or(0),
            controls_open: false,
            teach: None,
            taught: [None, None],
            tool: Tool::Explore,
            anchor: None,
            focus: None,
            maps: HashMap::new(),
            map_raw: HashMap::new(),
            loading: HashMap::new(),
            load_tx,
            load_rx,
            thumbs: vec![],
            last_thumb: ([f64::MAX, f64::MAX], Instant::now()),
            live_pic: None,
            jobs: vec![],
            export: None,
            calib: None,
            calib_orientation: false,
            queue: vec![],
            paused: false,
            wait_objective: None,
            sec_per_frame: 0.6,
            stitch_per_frame: 0.1,
            queue_open: false,
            cam_edit: None,
            auto_exposure: false,
            cam_applied: false,
            watch: watch::Watch::new(),
            objective_hint: None,
            label_edits: HashMap::new(),
            focusing: None,
            nudging: None,
            told: Instant::now(),
            finished: None,
            keys: keys::Keys::load(stored("keys")),
            keys_open: false,
            capturing: None,
            flat_job: None,
            zcal: None,
            corner_job: None,
            corner: None,
            corner_shown: false,
            arrows_move_stage: stored("arrows_move_stage").as_deref() == Some("true"),
            summary_open: false,
            design_confirm: None,
            summary_pick: None,
            settings_open: false,
            frame_no: 0,
            arrow_frac: stored("arrow_frac").and_then(|s| s.parse().ok()).unwrap_or(0.5),
            address,
            message,
            follow: true,
            fit: false,
        };
        app.fit = app.proj.sample().extent().is_some();
        app.link.state.lock().unwrap().look = Some((app.proj.optics().clone(), app.proj.wb));
        app
    }

    pub fn changed(&mut self) {
        self.dirty = true;
        self.link.state.lock().unwrap().look = Some((self.proj.optics().clone(), self.proj.wb));
    }

    /// New white balance: the maps are loaded again with it (they are balanced when loaded).
    pub fn set_white(&mut self, g: [f64; 3]) {
        self.proj.wb = g;
        self.maps.clear();
        self.map_raw.clear();
        self.changed();
    }

    /// The map of a selection (its best scan) and where it stands now (as stitched, then Align's correction).
    pub fn map_of(&self, s: &project::Selection) -> Option<(&Picture, geom::Affine)> {
        let m = s.best_scan()?.map.as_ref()?;
        let raw = self.map_raw.get(m)?;
        Some((self.maps.get(m)?, s.fix.map(|f| raw.then(&f)).unwrap_or(*raw)))
    }

    /// Give the camera the exposure and gain last used with the objective on the microscope.
    pub fn apply_camera(&mut self) {
        if let Some([e, g]) = self.proj.optics().camera {
            self.link.send(serde_json::json!({"cmd": "exposure", "ms": e}));
            self.link.send(serde_json::json!({"cmd": "gain", "value": g.round() as i64}));
            self.auto_exposure = false;
        }
    }

    /// The whole turn of the view, radians: the operator's quarter turns and the sample's own (Level grid).
    pub fn view_turn(&self) -> f64 {
        (self.view_rot as f64 * 90.0 + self.proj.sample().turn_deg).to_radians()
    }

    /// Teaching the arrows: move the stage a little along x (axis 0) or y (1) and back, then ask where it went.
    pub fn teach_move(&mut self, axis: usize) {
        let f = self.proj.optics().field_um();
        let d = 0.4 * f[0].min(f[1]);
        let (dx, dy) = if axis == 0 { (d, 0.0) } else { (0.0, d) };
        self.link.send(serde_json::json!({"cmd": "move", "dx": dx, "dy": dy}));
        self.link.send(serde_json::json!({"cmd": "sleep", "ms": 700}));
        self.link.send(serde_json::json!({"cmd": "move", "dx": -dx, "dy": -dy}));
        self.teach = Some(axis);
    }

    /// The operator says where the stage went (as seen: [1,0] right, [0,-1] up ...). With both axes known,
    /// the arrows are set: each moves the stage the way it points, for this operator.
    pub fn teach_answer(&mut self, seen: [f64; 2]) {
        let Some(axis) = self.teach.take() else { return };
        self.taught[axis] = Some(seen);
        if let [Some(ex), Some(ey)] = self.taught {
            if (ex[0] * ey[0] + ex[1] * ey[1]).abs() > 0.5 {
                self.message = "x and y were seen going the same way: teach again".into();
                self.taught = [None, None];
                return;
            }
            // seen = ex * sx + ey * sy  ->  stage (sx, sy) for "right" (1,0) and for "up" (0,-1)
            let stage_for = |want: [f64; 2]| [want[0] * ex[0] + want[1] * ex[1], want[0] * ey[0] + want[1] * ey[1]];
            self.arrow_dirs = Some([stage_for([1.0, 0.0]), stage_for([0.0, -1.0])]);
            self.taught = [None, None];
            self.message = "arrows taught: each moves the stage the way it points, as you see it".into();
        }
    }

    /// The view's turn that stands the lattice level: the smallest turn that makes its first axis horizontal.
    pub fn level_grid(&mut self) {
        let Some(l) = self.proj.sample().lattice.clone() else {
            self.message = "mark at least 2 crosses first (Lattice tool)".into();
            return;
        };
        let v = picture::View { center: [0.0, 0.0], scale: 1.0, rect: egui::Rect::ZERO, flip_x: self.flip_x, flip_y: self.flip_y, swap: self.swap, turn: (self.view_rot as f64 * 90.0).to_radians() };
        let d = v.dir_to_screen(l.a);
        let mut t = -d[1].atan2(d[0]).to_degrees();
        while t > 45.0 {
            t -= 90.0;
        }
        while t < -45.0 {
            t += 90.0;
        }
        self.proj.sample_mut().turn_deg = t;
        self.fit = true;
        self.message = format!("view turned by {t:.3} deg: the lattice stands level (all selections together)");
        self.changed();
    }

    /// Align every map of the sample onto its lattice (see align.rs).
    /// Every cross of the lattice on the base map looked for (where the lattice puts it, numbered by it), and
    /// the lattice fitted through all found with the signed ones: two signed crosses fix it only between them,
    /// far from them a small error in either grows; all of them, least squares, hold it everywhere. What is left
    /// shows how true the map itself is.
    pub fn fit_all_crosses(&mut self) {
        let Some(l) = self.proj.sample().lattice.clone() else { return };
        let smp = self.proj.sample().clone();
        let Some(base) = smp.lattice_sel.and_then(|id| smp.selections.iter().find(|s| s.id == id)).or_else(|| smp.selections.iter().find(|s| s.visible && s.best_scan().and_then(|b| b.map.as_ref()).is_some())) else {
            self.message = "no stitched map to look for crosses on".into();
            return;
        };
        let Some((pic, place)) = self.map_of(base) else {
            self.message = "the base map is not loaded yet".into();
            return;
        };
        let p = l.pitch();
        let radius_um = smp.cross_radius_um(&l);
        let r_px = (radius_um / place.scale()).round().max(6.0) as usize;
        let inv = place.inverse();
        let c = base.rect.corners().map(|q| l.coords(q));
        let (i0, i1) = (c.iter().map(|q| q[0]).fold(f64::MAX, f64::min).ceil() as i32, c.iter().map(|q| q[0]).fold(f64::MIN, f64::max).floor() as i32);
        let (j0, j1) = (c.iter().map(|q| q[1]).fold(f64::MAX, f64::min).ceil() as i32, c.iter().map(|q| q[1]).fold(f64::MIN, f64::max).floor() as i32);
        let signed: Vec<(P2, [i32; 2])> = smp.crosses.iter().filter_map(|c| c.label.map(|k| (c.at, k))).collect();
        let _ = p;
        // first the crosses near the signed ones (where the 2-cross lattice is good), the lattice fitted through
        // them, then all: far away the first lattice may miss by more than the small search radius
        let near_signed = |i: i32, j: i32, reach: i32| signed.iter().any(|s| (s.1[0] - i).abs() <= reach && (s.1[1] - j).abs() <= reach);
        let mut l = l;
        let mut found = vec![];
        for reach in [4, 12, i32::MAX] {
        found.clear();
        for j in j0..=j1 {
            for i in i0..=i1 {
                if signed.iter().any(|s| s.1 == [i, j]) || !near_signed(i, j, reach) {
                    continue;
                }
                let node = l.at(i as f64, j as f64);
                if !base.rect.contains(node) {
                    continue;
                }
                let uv = inv.apply(node);
                if let Some((u, v)) = cross::refine(pic.pyr.base(), uv[0], uv[1], r_px) {
                    let f = place.apply([u, v]);
                    if geom::len(geom::sub(f, node)) < radius_um {
                        found.push((f, [i, j]));
                    }
                }
            }
        }
        let m: Vec<(P2, [i32; 2])> = signed.iter().cloned().chain(found.iter().cloned()).collect();
        if let Ok(nl) = if smp.design.is_some() { geom::Lattice::fit_square(&m) } else { geom::Lattice::fit(&m) } {
            l = nl;
        }
        }
        if found.is_empty() {
            self.message = "no other crosses found on the base map".into();
            return;
        }
        let l0 = self.proj.sample().lattice.clone().unwrap();
        // a speck taken for a cross: off the fit by far more than the others
        let fit = |m: &[(P2, [i32; 2])]| if smp.design.is_some() { geom::Lattice::fit_square(m) } else { geom::Lattice::fit(m) };
        let mut all: Vec<(P2, [i32; 2])> = signed.iter().cloned().chain(found.iter().cloned()).collect();
        for _ in 0..2 {
            let Ok(fl) = fit(&all) else { break };
            let r: Vec<f64> = all.iter().map(|(p, k)| geom::len(geom::sub(*p, fl.at(k[0] as f64, k[1] as f64)))).collect();
            let mut s = r.clone();
            s.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let lim = (4.0 * s[s.len() / 2]).max(0.5);
            let keep: Vec<(P2, [i32; 2])> = all.iter().zip(&r).filter(|(m, e)| **e <= lim || signed.iter().any(|s| s.1 == m.1)).map(|(m, _)| *m).collect();
            if keep.len() == all.len() {
                break;
            }
            all = keep;
        }
        let auto: Vec<(P2, [i32; 2])> = all.iter().filter(|m| !signed.iter().any(|s| s.1 == m.1)).cloned().collect();
        let before = {
            // where the 2-cross lattice put them against where they are
            let r: Vec<f64> = auto.iter().map(|(p, k)| geom::len(geom::sub(*p, l0.at(k[0] as f64, k[1] as f64)))).collect();
            (r.iter().map(|e| e * e).sum::<f64>() / r.len().max(1) as f64).sqrt()
        };
        let smp = self.proj.sample_mut();
        smp.auto_marks = auto.clone();
        smp.auto_for = signed;
        let _ = smp.refit();
        let after = smp.lattice.as_ref().map(|l| {
            let r: Vec<f64> = all.iter().map(|(p, k)| geom::len(geom::sub(*p, l.at(k[0] as f64, k[1] as f64)))).collect();
            ((r.iter().map(|e| e * e).sum::<f64>() / r.len() as f64).sqrt(), r.iter().cloned().fold(0.0, f64::max))
        });
        if let Some((rms, max)) = after {
            self.message = format!(
                "lattice through {} crosses ({} found on the map): they were {:.2} um off it (rms), now {:.2} um rms, {:.2} um the worst{}",
                all.len(),
                auto.len(),
                before,
                rms,
                max,
                if rms > 0.5 { ": the map itself is not true (scale, drift): Stitch again (crosses pinned), Summary, Correct um/px" } else { "" }
            );
        }
        self.changed();
    }

    pub fn align_all(&mut self) {
        let Some(l) = self.proj.sample().lattice.clone() else {
            self.message = "mark at least 2 crosses first (Lattice tool)".into();
            return;
        };
        let mut report = vec![];
        let sels = self.proj.sample().selections.clone();
        let base_id = self.proj.sample().lattice_sel;
        // the base: the selection the lattice was marked on; it stays, the others come onto it cross to cross
        let base = base_id.and_then(|id| sels.iter().find(|s| s.id == id)).and_then(|s| self.map_of(s).map(|(p, a)| (s.rect, p.pyr.base().clone(), a, s.objective.clone())));
        for s in &sels {
            if Some(s.id) == base_id {
                if base.is_some() {
                    report.push(format!("{}: the base, stays", s.name));
                }
                continue;
            }
            let Some(m) = s.best_scan().and_then(|sc| sc.map.clone()) else { continue };
            let (Some(p), Some(raw)) = (self.maps.get(&m), self.map_raw.get(&m)) else {
                report.push(format!("{}: map not loaded yet", s.name));
                continue;
            };
            // from where the map stands now (a shift by hand included), not from scratch
            let now = s.fix.map(|f| raw.then(&f)).unwrap_or(*raw);
            // the same objective as the base: its scale is right, only shift and turn
            let rigid = base.as_ref().is_none_or(|b| b.3 == s.objective);
            let b = base.as_ref().filter(|b| b.0.intersects(&s.rect)).map(|b| (&b.0, &b.1, &b.2));
            match align::align_to(&l, &s.rect, p.pyr.base(), &now, b, rigid) {
                Ok(a) => {
                    report.push(format!("{}: {} crosses{}, moved {:.2} um, left {:.2} um", s.name, a.crosses, if b.is_some() { " (on the base map)" } else { "" }, a.moved_um, a.rms_um));
                    let dir = self.proj.path(&m);
                    let mut v = summary::read(&dir);
                    v["align"] = json!({"rms_um": a.rms_um, "crosses": a.crosses, "moved_um": a.moved_um, "onto": if b.is_some() { "base map" } else { "lattice" }});
                    // the map moved: its crosses are measured again where it is drawn now
                    if let Some(o) = v.as_object_mut() {
                        o.remove("crosses");
                    }
                    summary::write(&dir, &v);
                    if let Some(x) = self.proj.selection_mut(s.id) {
                        x.fix = Some(match x.fix {
                            Some(f) => f.then(&a.fix),
                            None => a.fix,
                        });
                    }
                }
                Err(e) => report.push(format!("{}: {e}, left as stitched", s.name)),
            }
        }
        self.message = if report.is_empty() { "no stitched maps to align".into() } else { format!("Align: {}", report.join("; ")) };
        self.changed();
    }

    /// `orientation_only`: the camera's turn / mirror and scale (3 frames); else also the distortion (9 frames).
    pub fn start_calibration(&mut self, orientation_only: bool) {
        self.calib_orientation = orientation_only;
        self.calib = Some(calib::start(self.link.clone(), self.proj.optics().clone(), self.proj.scan.settle_ms.max(50.0), self.proj.scan.back_um, orientation_only));
    }

    /// White balance from the live picture: its average made grey.
    pub fn auto_white(&mut self) {
        let mean = self.live_pic.as_ref().and_then(|(_, p)| {
            let b = p.pyr.base();
            b.mean(0, 0, b.w, b.h)
        });
        match mean {
            Some(c) if c.iter().all(|v| *v > 8.0) => {
                let wb = self.proj.wb;
                let grey = (c[0] + c[1] + c[2]) / 3.0;
                let g = [wb[0] * grey / c[0], wb[1] * grey / c[1], wb[2] * grey / c[2]];
                let g = g.map(|v| v / g[1]);
                self.set_white(g);
                self.message = format!("white balance R {:.3}  G 1  B {:.3} (from the live picture)", g[0], g[2]);
            }
            _ => self.message = "white balance: no live picture (or too dark)".into(),
        }
    }

    /// Flat field for the objective on now: nine frames around here, half a field apart.
    pub fn start_flat(&mut self) {
        self.flat_job = Some(flat::start(self.link.clone(), self.proj.optics().clone(), self.proj.scan.settle_ms.max(50.0), self.proj.scan.back_um));
        self.message = "flat field: 9 frames around here...".into();
    }

    /// A corner of the substrate is on the crosshair; `name`: which, as the operator sees the substrate.
    pub fn start_corner(&mut self, name: [f64; 2]) {
        self.corner = None;
        self.corner_job = Some((name, corner::start(self.link.clone(), self.proj.optics().clone(), self.proj.scan.settle_ms.max(50.0), self.proj.scan.back_um)));
        self.message = "corner: 3 frames...".into();
    }

    fn poll_corner(&mut self) {
        let Some((name, j)) = &self.corner_job else { return };
        let r = j.lock().unwrap().result.take();
        let Some(r) = r else { return };
        let name = *name;
        self.corner_job = None;
        match r {
            Ok(c) => {
                if self.proj.optics().m != c.m {
                    self.proj.optics_mut().m = c.m;
                    self.proj.optics_mut().profile_path = None;
                    self.thumbs.clear();
                    self.changed();
                }
                self.corner = Some((name, c));
                self.corner_shown = false;
                self.follow = true;
                self.message = "corner found: now Controls, Show me (the stage goes 1 mm along the red edge and back)".into();
                self.controls_open = true;
            }
            Err(e) => self.message = format!("corner: {e}"),
        }
    }

    /// The stage goes 1 mm along the corner's red edge and back, slowly enough to be seen: does the substrate
    /// go left-right or towards / away from the operator?
    pub fn corner_show(&mut self) {
        let Some((_, c)) = &self.corner else { return };
        let e = c.edges()[0];
        let d = 1000.0;
        self.link.send(serde_json::json!({"cmd": "move", "dx": e[0] * d, "dy": e[1] * d}));
        self.link.send(serde_json::json!({"cmd": "move", "dx": -e[0] * d, "dy": -e[1] * d}));
        self.corner_shown = true;
    }

    /// The operator says which edge of the corner runs left-right for them (0 red, 1 blue): the screen is turned
    /// and mirrored to show the substrate as they see it, the arrows set the same way.
    pub fn corner_answer(&mut self, horizontal: usize) {
        let Some((name, c)) = self.corner.take() else { return };
        self.corner_shown = false;
        let [r, u] = corner::right_up(&c, name, horizontal);
        // the screen: right on the screen = right for the operator, up = up (screen y grows downwards)
        let mut best = (f64::MIN, (false, false, false, 0));
        for swap in [false, true] {
            for fx in [false, true] {
                for fy in [false, true] {
                    for rot in 0..4 {
                        let v = picture::View { center: [0.0, 0.0], scale: 1.0, rect: egui::Rect::ZERO, flip_x: fx, flip_y: fy, swap, turn: (rot as f64 * 90.0 + self.proj.sample().turn_deg).to_radians() };
                        let (a, b) = (v.dir_to_screen(r), v.dir_to_screen(u));
                        let score = a[0] / geom::len(a) - b[1] / geom::len(b);
                        if score > best.0 + 1e-9 {
                            best = (score, (swap, fx, fy, rot));
                        }
                    }
                }
            }
        }
        let (swap, fx, fy, rot) = best.1;
        (self.swap, self.flip_x, self.flip_y, self.view_rot) = (swap, fx, fy, rot);
        // the arrows: right / up for the operator; the stage itself moves the other way to the view
        let sign = if self.arrows_move_stage { -1.0 } else { 1.0 };
        self.arrow_dirs = Some([geom::mul(r, sign), geom::mul(u, sign)]);
        (self.arrow_swap, self.arrow_rev_x, self.arrow_rev_y) = (false, false, false);
        self.taught = [None, None];
        self.fit = true;
        self.message = format!(
            "set from the corner: the screen shows the substrate as you see it; the arrows move {}",
            if self.arrows_move_stage { "the stage the way they point" } else { "the view the way they point" }
        );
    }

    /// The scale at z - dz, z, z + dz: how much the picture grows with focus.
    pub fn start_zscale(&mut self) {
        let dz = self.proj.scan.zscale_dz_um;
        self.zcal = Some(calib::start_zscale(self.link.clone(), self.proj.optics().clone(), self.proj.scan.settle_ms.max(50.0), self.proj.scan.back_um, dz));
        self.message = format!("scale vs focus: 9 frames at z -{dz}, 0, +{dz} um...");
    }

    fn poll_zcal(&mut self) {
        let Some(j) = &self.zcal else { return };
        let r = j.lock().unwrap().result.take();
        let Some(r) = r else { return };
        self.zcal = None;
        match r {
            Ok((a, k)) => {
                self.proj.optics_mut().scale_per_um = a;
                self.message = format!(
                    "scale vs focus for {}: {:+.1} ppm per um (px/um {:.5}, {:.5}, {:.5} at -dz, 0, +dz); each frame of a scan is corrected to the scan's start",
                    self.proj.optics().objective, a * 1e6, k[0], k[1], k[2]
                );
                self.changed();
            }
            Err(e) => self.message = format!("scale vs focus: {e}"),
        }
    }

    /// The objective's flat field off (its file stays).
    pub fn flat_off(&mut self) {
        let o = self.proj.optics_mut();
        o.flat = None;
        o.flat_file = None;
        self.message = format!("flat field for {} off", self.proj.optics().objective);
        self.changed();
    }

    fn poll_flat(&mut self) {
        let Some(j) = &self.flat_job else { return };
        let r = j.lock().unwrap().result.take();
        let Some(r) = r else { return };
        self.flat_job = None;
        match r {
            Ok(f) => {
                let name = format!("flat/{}.flat", export::layer_name(&self.proj.optics().objective));
                match f.save(&self.proj.path(&name)) {
                    Ok(()) => {
                        self.message = format!("flat field for {}: the edges get up to {:.0} % brighter; used for live pictures and scans from now on", self.proj.optics().objective, (1.0 / f.min as f64 - 1.0) * 100.0);
                        let o = self.proj.optics_mut();
                        o.flat = Some(Arc::new(f));
                        o.flat_file = Some(name);
                        self.changed();
                    }
                    Err(e) => self.message = format!("flat field: {e}"),
                }
            }
            Err(e) => self.message = format!("flat field: {e}"),
        }
    }

    fn poll_calibration(&mut self) {
        let Some(c) = &self.calib else { return };
        let r = c.lock().unwrap().result.clone();
        if let Some(r) = r {
            self.calib = None;
            match r {
                Ok(f) if self.calib_orientation => {
                    // only the straight way it is turned or mirrored, and the scale: no small tilt
                    let f = calib::Fit { m: geom::Optics::straight(f.m), ..f };
                    self.proj.optics_mut().m = f.m;
                    self.proj.optics_mut().profile_path = None;
                    let [a, b, c, d] = f.m;
                    let mirror = a * d - b * c < 0.0;
                    let ang = c.atan2(a).to_degrees();
                    self.message = format!("orientation: camera turned {ang:.1} deg{} against the stage, {:.4} um/px ({} features)",
                        if mirror { ", mirrored" } else { "" }, self.proj.optics().um_per_px(), f.matches);
                    self.thumbs.clear();
                    self.changed();
                }
                Ok(f) => {
                    self.proj.optics_mut().m = f.m;
                    self.proj.optics_mut().k = f.k;
                    self.proj.optics_mut().k3 = f.k3;
                    self.proj.optics_mut().p = f.p;
                    self.proj.optics_mut().calib_rms_px = Some(f.rms_px);
                    self.proj.optics_mut().profile_path = None;
                    self.message = format!(
                        "optics calibrated on {} features: {:.4} um/px, distortion k1 {:+.4} k2 {:+.4} k3 {:+.4} p1 {:+.5} p2 {:+.5}; residual {:.2} px (without distortion {:.2} px)",
                        f.matches, self.proj.optics().um_per_px(), f.k[0], f.k[1], f.k3, f.p[0], f.p[1], f.rms_px, f.rms_plain_px
                    );
                    self.thumbs.clear();
                    self.changed();
                }
                Err(e) => self.message = format!("calibration: {e}"),
            }
        }
    }

    /// Stitched maps of the current sample: load the ones not loaded yet (in the background).
    fn load_maps(&mut self) {
        let wanted: Vec<String> = self.proj.sample().selections.iter().filter_map(|s| s.best_scan().and_then(|sc| sc.map.clone())).collect();
        for m in wanted {
            if self.maps.contains_key(&m) || self.loading.contains_key(&m) {
                continue;
            }
            self.loading.insert(m.clone(), ());
            let dir = self.proj.path(&m);
            let tx = self.load_tx.clone();
            let wb = self.proj.wb;
            std::thread::spawn(move || {
                let r = scan::load_map(&dir).map(|(mut img, a, _)| {
                    img.balance(wb);
                    (Arc::new(Pyramid::new(img)), a)
                });
                let _ = tx.send((m, r));
            });
        }
        while let Ok((m, r)) = self.load_rx.try_recv() {
            self.loading.remove(&m);
            match r {
                Ok((pyr, a)) => {
                    self.map_raw.insert(m.clone(), a);
                    self.maps.insert(m.clone(), Picture::new(&m, pyr, a));
                }
                Err(e) => self.message = format!("map {m}: {e}"),
            }
        }
    }

    /// Finished scans: record them in the project, load their maps; their frames go into the preview.
    fn poll_jobs(&mut self) {
        let mut finished = vec![];
        for (k, j) in self.jobs.iter().enumerate() {
            let mut jb = j.lock().unwrap();
            if jb.phase != scan::Phase::Scanning && jb.scan_s.is_none() && jb.done > 0 {
                let t = jb.started.elapsed().as_secs_f64();
                jb.scan_s = Some(t);
                self.sec_per_frame = 0.7 * self.sec_per_frame + 0.3 * t / jb.done as f64;
            }
            for (p, rgb, div) in jb.thumbs.drain(..) {
                self.thumbs.push(thumb_picture(&self.proj.optics(), p, rgb, div, self.proj.optics().zoom(), self.thumbs.len()));
            }
            if matches!(jb.phase, scan::Phase::Done | scan::Phase::Failed(_) | scan::Phase::Stopped) {
                if let (scan::Phase::Done, Some(t)) = (&jb.phase, jb.stitch_at) {
                    if jb.done > 0 {
                        self.stitch_per_frame = 0.6 * self.stitch_per_frame + 0.4 * t.elapsed().as_secs_f64() / jb.done as f64;
                    }
                }
                finished.push(k);
            }
        }
        for k in finished.into_iter().rev() {
            let j = self.jobs.remove(k);
            let jb = j.lock().unwrap();
            let rel = |p: &std::path::Path| p.strip_prefix(&self.proj.folder).unwrap_or(p).to_string_lossy().into_owned();
            let entry = project::Scan {
                mode: jb.mode,
                frames: rel(&jb.frames_dir),
                map: (jb.phase == scan::Phase::Done).then(|| rel(&jb.map_dir)),
                n_frames: jb.done,
                when: now_text(),
                variant: jb.variant.clone(),
                note: match &jb.phase {
                    scan::Phase::Failed(e) => e.clone(),
                    scan::Phase::Stopped => "stopped".into(),
                    _ => jb.warning.clone().unwrap_or_default(),
                },
            };
            self.message = match &jb.phase {
                scan::Phase::Done => match &jb.warning {
                    None => format!("{} scan done: {} frames", jb.mode.name(), jb.done),
                    Some(w) => format!("{} scan done, {w}", jb.mode.name()),
                },
                scan::Phase::Failed(e) => format!("scan failed: {e}"),
                _ => "scan stopped".into(),
            };
            let id = jb.selection;
            if jb.phase == scan::Phase::Done {
                let sel_name = self.proj.sample().selections.iter().find(|s| s.id == id).map(|s| (s.name.clone(), s.objective.clone()));
                let (name, objective) = sel_name.unwrap_or_default();
                let o = self.proj.objectives.iter().find(|o| o.objective == objective).cloned().unwrap_or_else(|| self.proj.optics().clone());
                let mut v = summary::scan_part(jb.frames_dir.parent().unwrap_or(&jb.frames_dir));
                v["selection"] = json!(name);
                v["selection_id"] = json!(id);
                v["sample"] = json!(self.proj.sample().name);
                v["mode"] = json!(jb.mode.name());
                v["objective"] = json!(objective);
                v["when"] = json!(now_text());
                v["frames"] = json!(jb.done);
                v["scan_s"] = json!(jb.scan_s);
                v["stitch_s"] = json!(jb.stitch_at.map(|t| t.elapsed().as_secs_f64()));
                v["optics"] = json!({"um_per_px": o.um_per_px(), "k": o.k, "calib_rms_px": o.calib_rms_px, "flat": o.flat.is_some()});
                if let Ok(d) = std::fs::read(jb.map_dir.join("metrics.json")) {
                    if let Ok(m) = serde_json::from_slice::<serde_json::Value>(&d) {
                        v["mstitch"] = m;
                    }
                }
                if let Some(var) = &jb.variant {
                    v["round"] = json!(var);
                }
                summary::write(&jb.map_dir, &v);
            }
            let group = jb.variant.as_ref().map(|v| v.group.clone());
            drop(jb);
            self.finished = Some(chrono::Local::now().format("%H:%M").to_string());
            if let Some(s) = self.proj.selection_mut(id) {
                // a new map: Align's correction was for the old one
                if entry.map.is_some() && s.fix.take().is_some() {
                    self.message.push_str("; align again before export");
                }
                s.scans.push(entry);
            }
            self.changed();
            // the last round of a group: vote
            if let Some(g) = group {
                self.vote(id, &g);
            }
        }
    }

    pub fn scan_settings(&self, sel: &project::Selection, mode: project::Mode) -> scan::Settings {
        scan::Settings {
            variant: None,
            optics: self.proj.optics_of(&sel).clone(),
            overlap: if mode == project::Mode::Quick { self.proj.scan.overlap_quick } else { self.proj.scan.overlap_quality },
            settle_ms: self.proj.scan.settle_ms,
            back_um: self.proj.scan.back_um,
            step_um: self.proj.step_um,
            mstitch: self.proj.scan.mstitch.clone(),
            wb: self.proj.wb,
            focus: focus::Settings { on: self.proj.scan.autofocus, drop: self.proj.scan.focus_drop, max_um: self.proj.scan.focus_max_um, settle_ms: 0.0, window_um: self.proj.scan.focus_window_um },
            surface: Some(self.proj.path("surface.csv")),
            anchors: self.anchors_for(sel),
            anchor_radius_um: self.proj.sample().lattice.as_ref().map(|l| self.proj.sample().cross_radius_um(l)).unwrap_or(10.0),
        }
    }

    /// The lattice's crosses around a selection (half a field more on each side), at their true places.
    pub fn anchors_for(&self, sel: &project::Selection) -> Vec<P2> {
        let Some(l) = self.proj.sample().true_lattice() else { return vec![] };
        let f = self.proj.optics_of(sel).field_um();
        let r = &sel.rect;
        let big = geom::Rect { x0: r.x0 - f[0] / 2.0, y0: r.y0 - f[1] / 2.0, x1: r.x1 + f[0] / 2.0, y1: r.y1 + f[1] / 2.0 };
        let c = big.corners().map(|q| l.coords(q));
        let (i0, i1) = (c.iter().map(|q| q[0]).fold(f64::MAX, f64::min).floor() as i32, c.iter().map(|q| q[0]).fold(f64::MIN, f64::max).ceil() as i32);
        let (j0, j1) = (c.iter().map(|q| q[1]).fold(f64::MAX, f64::min).floor() as i32, c.iter().map(|q| q[1]).fold(f64::MIN, f64::max).ceil() as i32);
        let mut v = vec![];
        for j in j0..=j1 {
            for i in i0..=i1 {
                let p = l.at(i as f64, j as f64);
                if big.contains(p) {
                    v.push(p);
                }
            }
        }
        v
    }

    /// The objective's um/px corrected by the camera's scale against the drawing that a pinned stitch measured.
    pub fn correct_scale(&mut self, objective: &str, ppm: f64) {
        let Some(o) = self.proj.objectives.iter_mut().find(|o| o.objective == objective) else { return };
        let k = 1.0 + ppm * 1e-6;
        o.m = o.m.map(|v| v * k);
        o.profile_path = None;
        self.message = format!("{objective}: scale corrected by {ppm:+.0} ppm, now {:.5} um/px (scan again or Stitch again)", o.um_per_px());
        self.thumbs.clear();
        self.changed();
    }

    /// The last scan's frames of this selection stitched again into a new map (nothing scanned).
    pub fn restitch(&mut self, id: u32) {
        let Some(sel) = self.proj.sample().selections.iter().find(|s| s.id == id).cloned() else { return };
        let Some(sc) = sel.scans.iter().rev().find(|sc| !sc.frames.is_empty()).cloned() else { return };
        let frames = self.proj.path(&sc.frames);
        let stamp = now_text().replace([':', ' '], "-");
        let map = frames.parent().unwrap_or(&frames).join(format!("map_{stamp}"));
        let s = self.scan_settings(&sel, sc.mode);
        let job = scan::restitch(id, sc.mode, s, frames, map);
        self.jobs.push(job);
        self.message = format!("{}: stitching the {} scan again", sel.name, sc.mode.name());
    }

    /// One round of a quality scan in several, each really different from the others: round 1 as usual; round 2
    /// column by column, a little more overlap, the grid shifted so the objects found so far sit well inside
    /// frames; round 3 from the far corner back, more overlap again, shifted elsewhere; further rounds go round
    /// these. A shift is never near one used before.
    pub fn start_round(&mut self, id: u32, group: String, k: usize, of: usize) {
        let Some(sel) = self.proj.sample().selections.iter().find(|s| s.id == id).cloned() else { return };
        let mut s = self.scan_settings(&sel, project::Mode::Quality);
        let base = self.proj.scan.overlap_quality;
        let f = s.optics.inner_field_um();
        let extra = [0.0, 0.05, 0.10][(k - 1) % 3];
        let mut v = scan::Variant { group: group.clone(), round: k, of, overlap: (base + extra).min(0.6), offset_um: [0.0, 0.0], column_major: k % 2 == 0, reverse: k % 3 == 0 };
        // the shifts used so far in this group, and the objects the earlier rounds found
        let earlier: Vec<scan::Variant> = sel.scans.iter().filter_map(|sc| sc.variant.clone()).filter(|w| w.group == group).collect();
        let mut objects: Vec<(P2, P2, bool)> = vec![];
        for sc in sel.scans.iter().filter(|sc| sc.variant.as_ref().is_some_and(|w| w.group == group)) {
            if let Some(m) = &sc.map {
                if let Ok(t) = std::fs::read_to_string(self.proj.path(m).join("objects.csv")) {
                    for l in t.lines().skip(1) {
                        let x: Vec<f64> = l.split(',').filter_map(|q| q.parse().ok()).collect();
                        if x.len() >= 5 {
                            objects.push(([x[0], x[1]], [x[2], x[3]], x[4] > 0.5));
                        }
                    }
                }
            }
        }
        if k > 1 {
            let pitch = [f[0] * (1.0 - v.overlap), f[1] * (1.0 - v.overlap)];
            let mut best = (f64::MAX, [0.0, 0.0]);
            for gy in 0..8 {
                for gx in 0..8 {
                    let off = [pitch[0] * (gx as f64 + 0.5) / 8.0, pitch[1] * (gy as f64 + 0.5) / 8.0];
                    // far from every shift used before (a fifth of a pitch on one axis at least)
                    let near = earlier.iter().any(|w| (w.offset_um[0] - off[0]).abs() < 0.2 * pitch[0] && (w.offset_um[1] - off[1]).abs() < 0.2 * pitch[1]);
                    if near {
                        continue;
                    }
                    // objects: each wants a frame holding it whole, as near its centre as can be
                    let trial = scan::Variant { offset_um: off, ..v.clone() };
                    let centres: Vec<P2> = scan::plan_variant(&sel.rect, &s.optics, &trial, self.proj.step_um).into_iter().flatten().collect();
                    let mut cost = 0.0;
                    for (c, size, _) in &objects {
                        let mut best_o = 4.0;
                        for fc in &centres {
                            let (dx, dy) = ((c[0] - fc[0]).abs(), (c[1] - fc[1]).abs());
                            let inside = dx + size[0] / 2.0 <= f[0] / 2.0 && dy + size[1] / 2.0 <= f[1] / 2.0;
                            let o = if inside { (dx / (f[0] / 2.0)).max(dy / (f[1] / 2.0)) } else { 4.0 };
                            if o < best_o {
                                best_o = o;
                            }
                        }
                        cost += best_o;
                    }
                    // with no objects known: as far from the earlier shifts as can be
                    let spread = earlier.iter().map(|w| ((w.offset_um[0] - off[0]) / pitch[0]).abs() + ((w.offset_um[1] - off[1]) / pitch[1]).abs()).fold(f64::MAX, f64::min);
                    let cost = cost - 0.01 * spread.min(10.0);
                    if cost < best.0 {
                        best = (cost, off);
                    }
                }
            }
            v.offset_um = best.1;
        }
        // within the frame limit: less overlap if it would take too many
        while v.overlap > base && scan::plan_variant(&sel.rect, &s.optics, &v, self.proj.step_um).iter().map(|r| r.len()).sum::<usize>() > scan::limit(project::Mode::Quality) {
            v.overlap = (v.overlap - 0.025).max(base);
        }
        s.overlap = v.overlap;
        s.variant = Some(v);
        let stamp = now_text().replace([':', ' '], "-");
        let base_dir = format!("scans/{}_{}_r{k}of{of}_{}", export::layer_name(&sel.name), project::Mode::Quality.name(), stamp);
        let job = scan::start(self.link.clone(), id, project::Mode::Quality, sel.rect, s, self.proj.path(&format!("{base_dir}/frames")), self.proj.path(&format!("{base_dir}/map")));
        self.jobs.push(job);
        self.message = format!("{}: quality round {k} of {of}", sel.name);
    }

    /// All rounds of a group done: each metric ranks the rounds (best 1), the ranks are averaged (the crosses' and
    /// the pairs' numbers count double), the lowest wins and becomes the selection's map. Every round's numbers,
    /// how it was scanned and the vote go to rounds.csv in the project, for finding the best settings later.
    fn vote(&mut self, id: u32, group: &str) {
        let Some(sel) = self.proj.sample().selections.iter().find(|s| s.id == id).cloned() else { return };
        let rounds: Vec<(String, scan::Variant)> = sel.scans.iter().filter_map(|sc| Some((sc.map.clone()?, sc.variant.clone()?))).filter(|(_, v)| v.group == group).collect();
        let Some(of) = rounds.first().map(|r| r.1.of) else { return };
        let all_ended = sel.scans.iter().filter(|sc| sc.variant.as_ref().is_some_and(|v| v.group == group)).count() >= of;
        if !all_ended || self.queue.iter().any(|t| t.round.as_ref().is_some_and(|r| r.0 == group)) {
            return;
        }
        let sums: Vec<serde_json::Value> = rounds.iter().map(|(m, _)| summary::read(&self.proj.path(m))).collect();
        let n = rounds.len();
        let mut score = vec![0.0f64; n];
        let mut weight = 0.0;
        for m in summary::METRICS.iter().filter(|m| m.target.is_some()) {
            let vals: Vec<Option<f64>> = sums.iter().map(|v| summary::get(v, m.key)).collect();
            if n < 2 || vals.iter().any(|x| x.is_none()) {
                continue;
            }
            // smaller is better, after turning the value so
            let key: Vec<f64> = vals.iter().map(|x| {
                let x = x.unwrap();
                let x = if m.abs { x.abs() } else { x };
                if m.smaller { x } else { -x }
            }).collect();
            let w = if m.key.starts_with("crosses.") || m.key == "mstitch.anchor_rms_px" || m.key == "mstitch.pair_rms_px" { 2.0 } else { 1.0 };
            for i in 0..n {
                // rank: 1 + how many are better, ties shared
                let better = key.iter().filter(|&&k| k < key[i] - 1e-12).count() as f64;
                let same = key.iter().filter(|&&k| (k - key[i]).abs() <= 1e-12).count() as f64;
                score[i] += w * (1.0 + better + (same - 1.0) / 2.0);
            }
            weight += w;
        }
        let win = (0..n).min_by(|&a, &b| score[a].partial_cmp(&score[b]).unwrap()).unwrap_or(0);
        // the vote into each round's summary, and every round into rounds.csv
        let csv = self.proj.path("rounds.csv");
        let new_file = !csv.exists();
        let mut text = String::new();
        if new_file {
            text += "when,group,selection,objective,round,of,overlap,offset_x_um,offset_y_um,column_major,reverse,frames,score,chosen";
            for m in summary::METRICS {
                text += &format!(",{}", m.key);
            }
            text.push('\n');
        }
        for (i, ((m, v), s)) in rounds.iter().zip(&sums).enumerate() {
            let sc = if weight > 0.0 { score[i] / weight } else { 0.0 };
            let mut s2 = s.clone();
            s2["vote"] = json!({"score": sc, "chosen": i == win, "rounds": n});
            s2["round"] = json!(v);
            summary::write(&self.proj.path(m), &s2);
            text += &format!(
                "{},{},{},{},{},{},{:.3},{:.2},{:.2},{},{},{},{:.3},{}",
                now_text(), v.group, sel.name.replace(',', ";"), sel.objective, v.round, v.of, v.overlap, v.offset_um[0], v.offset_um[1], v.column_major, v.reverse,
                s["frames"].as_u64().unwrap_or(0), sc, i == win
            );
            for mt in summary::METRICS {
                text += &format!(",{}", summary::get(s, mt.key).map(|x| format!("{x}")).unwrap_or_default());
            }
            text.push('\n');
        }
        use std::io::Write;
        if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(&csv) {
            let _ = fh.write_all(text.as_bytes());
        }
        let chosen = rounds[win].0.clone();
        if let Some(x) = self.proj.selection_mut(id) {
            x.chosen = Some(chosen);
            x.fix = None;
        }
        self.message = format!(
            "{}: {} rounds voted, round {} chosen (average rank {}); all in rounds.csv",
            sel.name,
            n,
            rounds[win].1.round,
            if weight > 0.0 { format!("{:.2}", score[win] / weight) } else { "-".into() }
        );
        self.changed();
    }

    pub fn start_scan(&mut self, id: u32, mode: project::Mode) {
        let Some(sel) = self.proj.sample().selections.iter().find(|s| s.id == id).cloned() else { return };
        let stamp = now_text().replace([':', ' '], "-");
        let base = format!("scans/{}_{}_{}", export::layer_name(&sel.name), mode.name(), stamp);
        let s = self.scan_settings(&sel, mode);
        let job = scan::start(self.link.clone(), id, mode, sel.rect, s, self.proj.path(&format!("{base}/frames")), self.proj.path(&format!("{base}/map")));
        self.jobs.push(job);
    }

    /// Frames a scan of this selection takes, with its objective.
    pub fn frames_for(&self, s: &project::Selection, mode: project::Mode) -> usize {
        let overlap = if mode == project::Mode::Quick { self.proj.scan.overlap_quick } else { self.proj.scan.overlap_quality };
        scan::frames(&s.rect, self.proj.optics_of(s), overlap, self.proj.step_um)
    }

    /// Put a scan in the queue, if the selection is within the limits.
    pub fn enqueue(&mut self, id: u32, mode: project::Mode) {
        let Some(sel) = self.proj.sample().selections.iter().find(|s| s.id == id).cloned() else { return };
        let n = self.frames_for(&sel, mode);
        if n > scan::limit(mode) {
            self.message = format!("{}: {n} frames for a {} scan with {}, at most {}: make it smaller", sel.name, mode.name(), sel.objective, scan::limit(mode));
            return;
        }
        if self.queue.iter().any(|t| t.selection == id && t.mode == mode) {
            return;
        }
        let objective = self.proj.optics_of(&sel).objective.clone();
        let rounds = if mode == project::Mode::Quality { self.proj.scan.quality_rounds.max(1) } else { 1 };
        if rounds == 1 {
            self.queue.push(Task { selection: id, mode, objective, frames: n, round: None });
        } else {
            let group = format!("{}_{}", export::layer_name(&sel.name), now_text().replace([':', ' '], "-"));
            for k in 1..=rounds {
                self.queue.push(Task { selection: id, mode, objective: objective.clone(), frames: n, round: Some((group.clone(), k, rounds)) });
            }
        }
        self.group_queue();
    }

    /// Objectives in blocks: the one on the microscope first, the others in the order they came in.
    pub fn group_queue(&mut self) {
        let cur = self.proj.optics().objective.clone();
        let order: Vec<String> = self.queue.iter().map(|t| t.objective.clone()).fold(vec![], |mut v, o| {
            if !v.contains(&o) {
                v.push(o);
            }
            v
        });
        let rank = |o: &String| if *o == cur { 0 } else { 1 + order.iter().position(|x| x == o).unwrap_or(0) };
        self.queue.sort_by_key(|t| rank(&t.objective));
    }

    /// One scan on the microscope at a time; stitching runs beside it. A different objective: wait for the
    /// operator to change it.
    fn run_queue(&mut self) {
        let cur = self.proj.optics().objective.clone();
        // scans for the objective already on came in after the queue began to wait: they go first, no change yet
        if self.wait_objective.is_some() && self.queue.iter().any(|t| t.objective == cur) {
            self.wait_objective = None;
        }
        let scanning = self.jobs.iter().any(|j| j.lock().unwrap().phase == scan::Phase::Scanning);
        if self.paused || scanning || self.calib.is_some() || self.queue.is_empty() || self.wait_objective.is_some() {
            return;
        }
        if !self.link.state.lock().unwrap().connected {
            return;
        }
        match self.queue.iter().position(|t| t.objective == cur) {
            Some(k) => {
                let t = self.queue.remove(k);
                match &t.round {
                    Some((g, k, of)) => self.start_round(t.selection, g.clone(), *k, *of),
                    None => self.start_scan(t.selection, t.mode),
                }
            }
            None => self.wait_objective = Some(self.queue[0].objective.clone()),
        }
    }

    /// The operator has put the objective on: carry on.
    pub fn objective_changed(&mut self, name: &str) {
        if let Some(k) = self.proj.objectives.iter().position(|o| o.objective == name) {
            self.proj.current = k;
            self.thumbs.clear();
            self.apply_camera();
            self.changed();
        }
        if self.wait_objective.as_deref() == Some(name) {
            self.wait_objective = None;
        }
        self.group_queue();
    }

    /// Light on (at the project's level) or off.
    pub fn toggle_light(&mut self) {
        let (light, _, _) = self.link.lamps();
        let level = if light > 0 { 0 } else { self.proj.light_level.clamp(1, 5) };
        self.link.send(serde_json::json!({"cmd": "light", "level": level}));
    }

    /// Laser on at the project's power, or off. Off always works; on needs a power set and a server started
    /// with --allow-laser.
    pub fn laser(&mut self, on: bool) {
        if !on {
            self.link.send(serde_json::json!({"cmd": "laser", "mw": 0}));
            return;
        }
        let (_, _, max) = self.link.lamps();
        if self.proj.laser_mw == 0 {
            self.message = "laser: set its power in Settings first".into();
            return;
        }
        let mw = if max > 0 { self.proj.laser_mw.min(max) } else { self.proj.laser_mw };
        self.link.send(serde_json::json!({"cmd": "laser", "mw": mw}));
    }

    fn watch_objective(&mut self) {
        let live = self.link.state.lock().unwrap().live.as_ref().map(|l| (l.seq, l.rgb.clone(), l.xyz, l.moving));
        let Some((seq, rgb, xyz, moving)) = live else { return };
        let busy = !self.jobs.iter().all(|j| j.lock().unwrap().phase != scan::Phase::Scanning) || self.calib.is_some();
        let now = self.proj.optics().clone();
        if let Some(h) = self.watch.frame(seq, &rgb, xyz, moving, busy, &now, &self.proj.objectives) {
            self.objective_hint = Some(match h {
                watch::Hint::Objective(o) => (format!("The picture looks like {o} now, the program has {}.", now.objective), Some(o)),
                watch::Hint::Changed => (format!("The picture changed while the stage stood still. Objective changed? The program has {}.", now.objective), None),
            }).map(|(t, o)| (t, o, Instant::now()));
        }
    }

    /// Find focus here, as a scan does (the steps out to +-100 um, then finer), and stay there.
    pub fn focus_here(&mut self) {
        let Some(p) = self.link.stage_pos() else { return };
        if self.focusing.as_ref().is_some_and(|f| f.lock().unwrap().is_none()) {
            return;
        }
        let link = self.link.clone();
        let s = focus::Settings { on: true, drop: 1.0, max_um: self.proj.scan.focus_max_um, settle_ms: self.proj.scan.settle_ms, window_um: self.proj.scan.focus_window_um };
        let dir = self.proj.path("focus");
        let surface = Some(self.proj.path("surface.csv"));
        let out = Arc::new(Mutex::new(None));
        let o2 = out.clone();
        std::thread::spawn(move || {
            let mut f = focus::Focus::new(s, &dir, surface, "by hand");
            f.center = Some(p[2]);
            let r = f.shot(&link, p[0], p[1], p[2], &|| false).map(|sh| (sh.z, sh.sharpness, sh.probes));
            // the last probe may have been elsewhere in z: back to the sharpest
            if let Ok((z, _, _)) = &r {
                let _ = link.call(serde_json::json!({"cmd": "goto", "x": p[0], "y": p[1], "z": z}));
            }
            *o2.lock().unwrap() = Some(r);
        });
        self.focusing = Some(out);
        self.message = "looking for focus...".into();
    }

    /// Move a selection's map by hand by `d` um on the stage (before Align, to bring it near).
    pub fn nudge(&mut self, id: u32, d: P2) {
        // the base map takes its lattice along: the crosses were marked on it
        if self.proj.sample().lattice_sel == Some(id) {
            let smp = self.proj.sample_mut();
            for c in smp.crosses.iter_mut() {
                c.at = [c.at[0] + d[0], c.at[1] + d[1]];
            }
            for m in smp.auto_marks.iter_mut().chain(smp.auto_for.iter_mut()) {
                m.0 = [m.0[0] + d[0], m.0[1] + d[1]];
            }
            let _ = smp.refit();
        }
        if let Some(s) = self.proj.selection_mut(id) {
            let t = geom::Affine([1.0, 0.0, d[0], 0.0, 1.0, d[1]]);
            s.fix = Some(match s.fix {
                Some(f) => f.then(&t),
                None => t,
            });
        }
        self.changed();
    }

    /// What the program is doing, for the server's web page: project, sample, objective, the task and its
    /// progress, the time left.
    fn tell_work(&mut self) {
        let sels = self.proj.sample().selections.clone();
        let name = |id: u32| sels.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default();
        let (mut state, mut task, mut done, mut total, mut busy, mut waiting) = ("Idle".to_string(), String::new(), 0usize, 0usize, false, false);
        for j in &self.jobs {
            let j = j.lock().unwrap();
            match j.phase {
                scan::Phase::Scanning => {
                    state = format!("Scanning ({})", j.mode.name());
                    task = format!("{}: frame {} of {}", name(j.selection), j.done, j.total);
                    (done, total, busy) = (j.done, j.total, true);
                }
                scan::Phase::Stitching if !busy => {
                    state = "Stitching".into();
                    task = format!("{} ({} frames)", name(j.selection), j.done);
                    busy = true;
                }
                _ => {}
            }
        }
        if let Some(c) = &self.calib {
            let c = c.lock().unwrap();
            state = "Calibrating".into();
            task = format!("{} {}/{}", c.phase, c.done, c.total);
            (done, total, busy) = (c.done, c.total, true);
        }
        if let Some(o) = &self.wait_objective {
            state = format!("Waiting: put the {o} objective on");
            waiting = true;
        } else if self.paused && !self.queue.is_empty() {
            state = "Paused".into();
            waiting = true;
        } else if !busy && self.focusing.is_some() {
            state = "Focusing".into();
            busy = true;
        }
        let eta = self.time_left();
        let w = serde_json::json!({"cmd": "work", "project": self.proj.name, "sample": self.proj.sample().name,
            "objective": self.proj.optics().objective, "queue": self.queue.len(), "state": state, "task": task,
            "done": done, "total": total, "busy": busy, "waiting": waiting, "eta_s": eta, "finished": self.finished});
        self.link.tell(w);
    }

    /// Stop: the stage, the scan running (its frames so far are kept), and the queue pauses.
    pub fn stop_all(&mut self) {
        self.link.send(serde_json::json!({"cmd": "stop"}));
        if self.link.lamps().1 > 0 {
            self.laser(false);
        }
        for j in &self.jobs {
            j.lock().unwrap().stop = true;
        }
        if let Some(c) = &self.calib {
            c.lock().unwrap().stop = true;
        }
        if !self.queue.is_empty() {
            self.paused = true;
        }
    }

    /// Time left: the scan running and the queue, seconds.
    pub fn time_left(&self) -> f64 {
        let running: f64 = self.jobs.iter().map(|j| {
            let j = j.lock().unwrap();
            if j.phase == scan::Phase::Scanning { (j.total - j.done) as f64 * self.sec_per_frame } else { 0.0 }
        }).sum();
        running + self.queue.iter().map(|t| t.frames as f64 * self.sec_per_frame).sum::<f64>()
    }

    pub fn start_export(&mut self) {
        let sample = self.proj.sample().clone();
        let Some(lat) = sample.lattice.clone() else {
            self.message = "mark at least 2 crosses first (Lattice tool)".into();
            return;
        };
        // with the sample's drawing chosen: in the drawing's coordinates (X right, Y up, crosses at their places)
        let orient = match &sample.design {
            Some(d) => export::Orient::drawing(d),
            None => export::Orient::of(&lat, self.flip_x, self.flip_y, self.swap, self.view_turn()),
        };
        // what to cut: selections marked "cut" that have a map, top of the list first
        let mut items = vec![];
        let mut skipped = vec![];
        // every ticked selection with a map: one layer each, all on the same lattice
        for s in sample.selections.iter().filter(|s| s.visible) {
            match self.map_of(s) {
                Some((p, a)) => items.push((s.name.clone(), s.color, s.rect, p.pyr.clone(), a, s.opacity)),
                None => skipped.push(s.name.clone()),
            }
        }
        if !skipped.is_empty() {
            self.message = format!("not cut (no stitched map loaded): {}", skipped.join(", "));
        }
        if items.is_empty() {
            self.message = "nothing to cut: no ticked selection has a stitched map yet".into();
            return;
        }
        let out = self.proj.path(&format!("export/{}_{}", export::layer_name(&sample.name), now_text().replace([':', ' '], "-")));
        let extent = sample.extent().unwrap();
        let st = Arc::new(Mutex::new(ExportState { done: 0, total: items.len(), stage: "cutting".into(), result: None }));
        self.export = Some(st.clone());
        std::thread::spawn(move || {
            let r = (|| -> Result<PathBuf, String> {
                let mut layers = vec![];
                for (k, (name, color, rect, pyr, a, opacity)) in items.iter().enumerate() {
                    st.lock().unwrap().stage = format!("cutting {name}");
                    let src = export::Source { rect: *rect, map: pyr.base(), to_stage: *a, opacity: *opacity };
                    let umpp = a.scale();
                    let dir = out.join("squares").join(export::layer_name(name));
                    let sq = export::cut(&src, &lat, orient, umpp, &dir, &mut |_, _| {})?;
                    layers.push((name.clone(), *color, *rect, sq));
                    st.lock().unwrap().done = k + 1;
                }
                st.lock().unwrap().stage = "drawing".into();
                let path = out.join("drawing.dxf");
                export::write_dxf(&path, &lat, orient, &layers, &extent)?;
                Ok(path)
            })();
            st.lock().unwrap().result = Some(r);
        });
    }
}

/// A small picture of a frame at stage position p, for the preview map: one pixel of it is `div` pixels of
/// the frame, and the frame is zoomed in by `zoom` about its centre (scans: see Optics::zoom; live: 1).
pub fn thumb_picture(o: &geom::Optics, p: P2, rgb: Arc<pixels::Rgb>, div: f64, zoom: f64, n: usize) -> Picture {
    let c = [o.frame[0] as f64 / 2.0, o.frame[1] as f64 / 2.0];
    // picture pixel (u, v) -> ideal frame pixel c + zoom (div (u, v) - c) -> stage
    let at = |u: f64, v: f64| o.pixel_to_stage(p, c[0] + zoom * (div * u - c[0]), c[1] + zoom * (div * v - c[1]));
    let (o0, ox, oy) = (at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0));
    let a = geom::Affine([ox[0] - o0[0], oy[0] - o0[0], o0[0], ox[1] - o0[1], oy[1] - o0[1], o0[1]]);
    Picture::new(&format!("thumb{n}"), Arc::new(Pyramid { levels: vec![(*rgb).clone()] }), a)
}

fn default_project() -> PathBuf {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("MControl").join(format!("project-{}", now_text().split(' ').next().unwrap_or("today")))
}

/// Local date and time, "2026-11-27 15:42:10".
pub fn now_text() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame_no += 1;
        self.load_maps();
        self.poll_jobs();
        self.poll_calibration();
        self.poll_flat();
        self.poll_zcal();
        self.poll_corner();
        if let Some(a) = self.link.state.lock().unwrap().alert.take() {
            self.message = a;
        }
        // a new connection: the camera gets this objective's exposure again
        let connected = self.link.state.lock().unwrap().connected;
        if connected && !self.cam_applied {
            self.cam_applied = true;
            self.apply_camera();
        } else if !connected {
            self.cam_applied = false;
        }
        self.watch_objective();
        if self.told.elapsed().as_secs_f64() >= 2.0 {
            self.told = Instant::now();
            self.tell_work();
        }
        let done = self.focusing.as_ref().and_then(|f| f.lock().unwrap().take());
        {
            if let Some(r) = done {
                self.message = match r {
                    Ok((z, sh, n)) => format!("focus: z {z:.1} um, sharpness {sh:.2} ({n} frames tried)"),
                    Err(e) => format!("focus: {e}"),
                };
                self.focusing = None;
            }
        }
        self.run_queue();
        panels::top(self, ui);
        panels::status(self, ui);
        panels::bottom(self, ui);
        panels::left(self, ui);
        if self.tool == Tool::Lattice {
            panels::right(self, ui);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(egui::Color32::from_gray(24))).show(ui, |ui| map::show(self, ui));
        panels::settings(self, ui.ctx());
        panels::queue(self, ui.ctx());
        panels::controls(self, ui.ctx());
        panels::keys_window(self, ui.ctx());
        panels::summary_window(self, ui.ctx());
        panels::objective_hint(self, ui.ctx());
        if self.dirty {
            self.dirty = false;
            if let Err(e) = self.proj.save() {
                self.message = format!("cannot save the project: {e}");
            }
        }
        if !self.jobs.is_empty() || !self.queue.is_empty() || self.calib.is_some() || self.export.as_ref().is_some_and(|e| e.lock().unwrap().result.is_none()) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("address", self.address.clone());
        storage.set_string("project", self.proj.folder.display().to_string());
        storage.set_string("flip_x", self.flip_x.to_string());
        storage.set_string("flip_y", self.flip_y.to_string());
        storage.set_string("swap", self.swap.to_string());
        storage.set_string("arrow_swap", self.arrow_swap.to_string());
        storage.set_string("arrow_rev_x", self.arrow_rev_x.to_string());
        storage.set_string("arrow_rev_y", self.arrow_rev_y.to_string());
        storage.set_string("arrow_dirs", serde_json::to_string(&self.arrow_dirs).unwrap_or_default());
        storage.set_string("view_rot", self.view_rot.to_string());
        storage.set_string("arrows_move_stage", self.arrows_move_stage.to_string());
        storage.set_string("keys", self.keys.save());
        storage.set_string("arrow_frac", self.arrow_frac.to_string());
    }
}
