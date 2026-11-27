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
mod watch;

use eframe::egui;
use geom::P2;
use link::Link;
use picture::{Picture, Pyramid};
use project::Project;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
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
    /// the arrows move the stage itself the way they point (else the picture: the view goes that way)
    pub arrows_move_stage: bool,
    /// a flat field being taken
    pub flat_job: Option<Arc<Mutex<flat::Job>>>,
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
            corner_job: None,
            corner: None,
            arrows_move_stage: stored("arrows_move_stage").as_deref() == Some("true"),
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
    pub fn align_all(&mut self) {
        let Some(l) = self.proj.sample().lattice.clone() else {
            self.message = "mark at least 2 crosses first (Lattice tool)".into();
            return;
        };
        let mut report = vec![];
        let sels = self.proj.sample().selections.clone();
        for s in &sels {
            let Some(m) = s.best_scan().and_then(|sc| sc.map.clone()) else { continue };
            let (Some(p), Some(raw)) = (self.maps.get(&m), self.map_raw.get(&m)) else {
                report.push(format!("{}: map not loaded yet", s.name));
                continue;
            };
            // from where the map stands now (a shift by hand included), not from scratch
            let now = s.fix.map(|f| raw.then(&f)).unwrap_or(*raw);
            match align::align(&l, &s.rect, p.pyr.base(), &now) {
                Ok(a) => {
                    report.push(format!("{}: {} crosses, moved {:.2} um, left {:.2} um", s.name, a.crosses, a.moved_um, a.rms_um));
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
                self.follow = true;
                self.message = "corner found. Which of its two edges (red, blue) runs left-right for you? (Controls)".into();
                self.controls_open = true;
            }
            Err(e) => self.message = format!("corner: {e}"),
        }
    }

    /// The operator says which edge of the corner runs left-right for them (0 red, 1 blue): the screen is turned
    /// and mirrored to show the substrate as they see it, the arrows set the same way.
    pub fn corner_answer(&mut self, horizontal: usize) {
        let Some((name, c)) = self.corner.take() else { return };
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
                    self.proj.optics_mut().profile_path = None;
                    self.message = format!(
                        "optics calibrated on {} features: {:.4} um/px, distortion k1 {:+.4} k2 {:+.4}; residual {:.2} px (without distortion {:.2} px)",
                        f.matches, self.proj.optics().um_per_px(), f.k[0], f.k[1], f.rms_px, f.rms_plain_px
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
        }
    }

    pub fn start_scan(&mut self, id: u32, mode: project::Mode) {
        let Some(sel) = self.proj.sample().selections.iter().find(|s| s.id == id).cloned() else { return };
        let stamp = now_text().replace([':', ' '], "-");
        let base = format!("scans/{}_{}_{}", export::layer_name(&sel.name), mode.name(), stamp);
        let s = scan::Settings {
            optics: self.proj.optics_of(&sel).clone(),
            overlap: if mode == project::Mode::Quick { self.proj.scan.overlap_quick } else { self.proj.scan.overlap_quality },
            settle_ms: self.proj.scan.settle_ms,
            back_um: self.proj.scan.back_um,
            step_um: self.proj.step_um,
            mstitch: self.proj.scan.mstitch.clone(),
            wb: self.proj.wb,
            focus: focus::Settings { on: self.proj.scan.autofocus, drop: self.proj.scan.focus_drop, max_um: self.proj.scan.focus_max_um, settle_ms: 0.0 },
            surface: Some(self.proj.path("surface.csv")),
        };
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
        self.queue.push(Task { selection: id, mode, objective: self.proj.optics_of(&sel).objective.clone(), frames: n });
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
                self.start_scan(t.selection, t.mode);
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
        let s = focus::Settings { on: true, drop: 1.0, max_um: self.proj.scan.focus_max_um, settle_ms: self.proj.scan.settle_ms };
        let dir = self.proj.path("focus");
        let surface = Some(self.proj.path("surface.csv"));
        let out = Arc::new(Mutex::new(None));
        let o2 = out.clone();
        std::thread::spawn(move || {
            let mut f = focus::Focus::new(s, &dir, surface, "by hand");
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
        let orient = export::Orient::of(&lat, self.flip_x, self.flip_y, self.swap, self.view_turn());
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
