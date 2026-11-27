#![windows_subsystem = "windows"]
//! mc-operator: the operator's program on PC 2. A map of the stage with the live picture, a preview of where
//! the camera has been, selections, scans stitched by MStitch, the lattice of crosses, cutting and the DXF drawing.

mod align;
mod calib;
mod cross;
mod drawing;
mod export;
mod focus;
mod geom;
mod link;
mod map;
mod panels;
mod picture;
mod pixels;
mod project;
mod scan;

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
            queue: vec![],
            paused: false,
            wait_objective: None,
            sec_per_frame: 0.6,
            stitch_per_frame: 0.1,
            queue_open: false,
            cam_edit: None,
            auto_exposure: false,
            cam_applied: false,
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

    /// The view's turn that stands the lattice level: the smallest turn that makes its first axis horizontal.
    pub fn level_grid(&mut self) {
        let Some(l) = self.proj.sample().lattice.clone() else {
            self.message = "mark at least 2 crosses first (Lattice tool)".into();
            return;
        };
        let v = picture::View { center: [0.0, 0.0], scale: 1.0, rect: egui::Rect::ZERO, flip_x: self.flip_x, flip_y: self.flip_y, turn: 0.0 };
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
            match align::align(&l, &s.rect, p.pyr.base(), raw) {
                Ok(a) => {
                    report.push(format!("{}: {} crosses, moved {:.2} um, left {:.2} um", s.name, a.crosses, a.moved_um, a.rms_um));
                    if let Some(x) = self.proj.selection_mut(s.id) {
                        x.fix = Some(a.fix);
                    }
                }
                Err(e) => report.push(format!("{}: {e}, left as stitched", s.name)),
            }
        }
        self.message = if report.is_empty() { "no stitched maps to align".into() } else { format!("Align: {}", report.join("; ")) };
        self.changed();
    }

    pub fn start_calibration(&mut self) {
        self.calib = Some(calib::start(self.link.clone(), self.proj.optics().clone(), self.proj.scan.settle_ms.max(50.0), self.proj.scan.back_um));
    }

    fn poll_calibration(&mut self) {
        let Some(c) = &self.calib else { return };
        let r = c.lock().unwrap().result.clone();
        if let Some(r) = r {
            self.calib = None;
            match r {
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
        let orient = export::Orient::of(&lat, self.flip_x, self.flip_y, sample.turn_deg.to_radians());
        // what to cut: selections marked "cut" that have a map, top of the list first
        let mut items = vec![];
        let mut skipped = vec![];
        // every selection with a map: one layer each, all on the same lattice
        for s in sample.selections.iter() {
            match self.map_of(s) {
                Some((p, a)) => items.push((s.name.clone(), s.color, s.rect, p.pyr.clone(), a)),
                None => skipped.push(s.name.clone()),
            }
        }
        if !skipped.is_empty() {
            self.message = format!("not cut (no stitched map loaded): {}", skipped.join(", "));
        }
        if items.is_empty() {
            self.message = "nothing to cut: no selection has a stitched map yet".into();
            return;
        }
        let out = self.proj.path(&format!("export/{}_{}", export::layer_name(&sample.name), now_text().replace([':', ' '], "-")));
        let extent = sample.extent().unwrap();
        let st = Arc::new(Mutex::new(ExportState { done: 0, total: items.len(), stage: "cutting".into(), result: None }));
        self.export = Some(st.clone());
        std::thread::spawn(move || {
            let r = (|| -> Result<PathBuf, String> {
                let mut layers = vec![];
                for (k, (name, color, rect, pyr, a)) in items.iter().enumerate() {
                    st.lock().unwrap().stage = format!("cutting {name}");
                    let src = export::Source { rect: *rect, map: pyr.base(), to_stage: *a };
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
        storage.set_string("arrow_frac", self.arrow_frac.to_string());
    }
}
