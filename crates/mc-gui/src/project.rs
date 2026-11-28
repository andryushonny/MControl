//! What the operator builds, saved as project.json in the project folder after every change:
//! samples (a chip or substrate each), their selections (rectangles on the stage, in drawing order),
//! scans of each selection, the crosses marked and the lattice they give.

use crate::geom::{Affine, Lattice, Optics, Rect, P2};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Quick,
    Quality,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Quick => "quick",
            Mode::Quality => "quality",
        }
    }
}

/// One scan of a selection: the frames folder and, once stitched, the map folder (MStitch's output).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scan {
    pub mode: Mode,
    pub frames: String,
    pub map: Option<String>,
    pub n_frames: usize,
    pub when: String,
    #[serde(default)]
    pub note: String,
    /// a round of a quality scan in several
    #[serde(default)]
    pub variant: Option<crate::scan::Variant>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Selection {
    /// the map chosen by the rounds' vote (its folder)
    #[serde(default)]
    pub chosen: Option<String>,
    pub id: u32,
    pub name: String,
    pub rect: Rect,
    pub color: [u8; 3],
    pub visible: bool,
    /// (old projects; every selection with a map is cut now)
    #[serde(default)]
    pub cut: bool,
    pub scans: Vec<Scan>,
    /// Align's correction of the map: stage as the map says -> stage as the sample's lattice says
    #[serde(default)]
    pub fix: Option<Affine>,
    /// the objective it is scanned with (a name from the project's objectives)
    #[serde(default)]
    pub objective: String,
    /// how opaque its map is drawn and exported (0..1)
    #[serde(default = "opaque")]
    pub opacity: f32,
}

fn opaque() -> f32 {
    1.0
}

impl Selection {
    /// The map shown and cut: the newest quality scan, else the newest quick one.
    pub fn best_scan(&self) -> Option<&Scan> {
        // the round chosen by vote, while it is the latest group of rounds
        if let Some(c) = &self.chosen {
            if let Some(sc) = self.scans.iter().find(|s| s.map.as_deref() == Some(c.as_str())) {
                let later = self.scans.iter().rev().take_while(|s| s.map.as_deref() != Some(c.as_str())).any(|s| s.map.is_some() && s.variant.as_ref().map(|v| &v.group) != sc.variant.as_ref().map(|v| &v.group));
                if !later {
                    return Some(sc);
                }
            }
        }
        let done = |m: Mode| self.scans.iter().rev().find(|s| s.mode == m && s.map.is_some());
        done(Mode::Quality).or_else(|| done(Mode::Quick))
    }
}

/// A cross marked by hand on a stitched map, with its lattice number (X, Y).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cross {
    pub at: P2,
    /// its lattice numbers (X, Y), as the operator signed it; None until signed
    #[serde(default)]
    pub label: Option<[i32; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    pub name: String,
    pub selections: Vec<Selection>,
    #[serde(default)]
    pub crosses: Vec<Cross>,
    #[serde(default)]
    pub lattice: Option<Lattice>,
    /// the selection the lattice was marked on (its map is the base: the lattice moves with it, the other maps
    /// are aligned onto it)
    #[serde(default)]
    pub lattice_sel: Option<u32>,
    /// the lithography crosses on this sample (from its drawing): their numbers give exact drawing coordinates
    #[serde(default)]
    pub design: Option<Design>,
    /// crosses found on the base map by "Fit to all crosses" (numbered by the lattice), used in the fit too as
    /// long as the signed crosses are those it was done with (`auto_for`)
    #[serde(default)]
    pub auto_marks: Vec<(P2, [i32; 2])>,
    #[serde(default)]
    pub auto_for: Vec<(P2, [i32; 2])>,
    /// the view is turned by this much (degrees) so the lattice stands level on the screen
    #[serde(default)]
    pub turn_deg: f64,
}

impl Sample {
    /// The crosses found automatically still belong to these signed crosses.
    pub fn auto_valid(&self) -> bool {
        let signed: Vec<(P2, [i32; 2])> = self.crosses.iter().filter_map(|c| c.label.map(|l| (c.at, l))).collect();
        !self.auto_marks.is_empty()
            && signed.len() == self.auto_for.len()
            && signed.iter().zip(&self.auto_for).all(|(a, b)| a.1 == b.1 && (a.0[0] - b.0[0]).abs() < 1e-6 && (a.0[1] - b.0[1]).abs() < 1e-6)
    }

    /// How far around a cross's expected place to look for it (um): for the drawing's crosses (arms 5 um each
    /// way, numbers about 10 um off) small, so a number is not taken for the cross; else a part of the pitch.
    pub fn cross_radius_um(&self, l: &Lattice) -> f64 {
        match &self.design {
            Some(_) => 6.5,
            None => {
                let p = l.pitch();
                (0.3 * p[0].min(p[1])).min(25.0)
            }
        }
    }

    /// Where the crosses truly are, for pinning: the lattice, at the drawing's pitch when the sample has one.
    pub fn true_lattice(&self) -> Option<Lattice> {
        let l = self.lattice.clone()?;
        Some(match &self.design {
            Some(d) => {
                let mut marks: Vec<(P2, [i32; 2])> = self.crosses.iter().filter_map(|c| c.label.map(|k| (c.at, k))).collect();
                if self.auto_valid() {
                    marks.extend(self.auto_marks.iter().cloned());
                }
                l.with_pitch(&marks, d.pitch)
            }
            None => l,
        })
    }

    pub fn new(name: &str) -> Sample {
        Sample { name: name.into(), selections: vec![], crosses: vec![], lattice: None, lattice_sel: None, design: None, auto_marks: vec![], auto_for: vec![], turn_deg: 0.0 }
    }

    /// The box around all selections.
    pub fn extent(&self) -> Option<Rect> {
        self.selections.iter().map(|s| s.rect).reduce(|a, b| a.union(&b))
    }

    pub fn refit(&mut self) -> Result<(), String> {
        // only crosses the operator has signed: the numbers of all the others follow from them
        let mut marks: Vec<(P2, [i32; 2])> = self.crosses.iter().filter_map(|c| c.label.map(|l| (c.at, l))).collect();
        if marks.len() < 2 {
            self.lattice = None;
            return Ok(());
        }
        // the crosses found by "Fit to all crosses", while the signed ones are still those
        if self.auto_valid() {
            marks.extend(self.auto_marks.iter().cloned());
        }
        // the drawing's crosses are square: the lattice is too (shift, turn, one pitch; maybe mirrored)
        let fit = if self.design.is_some() { Lattice::fit_square(&marks) } else { Lattice::fit(&marks) };
        match fit {
            Ok(l) => {
                self.lattice = Some(l);
                Ok(())
            }
            Err(e) => {
                self.lattice = None;
                Err(e)
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanSettings {
    /// overlap of neighbouring frames, part of the field
    pub overlap_quick: f64,
    pub overlap_quality: f64,
    pub settle_ms: f64,
    /// every frame is approached from the same side, from this far back (no backlash in the data)
    pub back_um: f64,
    /// MStitch command line (until its C API is linked in)
    pub mstitch: String,
    /// look for focus when a frame is blurred (see focus.rs)
    #[serde(default = "yes")]
    pub autofocus: bool,
    /// the autofocus keeps z within +- this of where the scan started (0: no limit)
    #[serde(default = "focus_window")]
    pub focus_window_um: f64,
    /// how far up and down in z the scale is measured (Calibrate, Scale vs focus)
    #[serde(default = "zscale_dz")]
    pub zscale_dz_um: f64,
    /// rounds of a quality scan (each scanned and stitched anew, the best kept by vote); quick: always 1
    #[serde(default = "quality_rounds")]
    pub quality_rounds: usize,
    /// export: each square's picture reaches this far past its crosses (um), crosses whole, neighbours overlap
    #[serde(default = "export_pad")]
    pub export_pad_um: f64,
    /// blurred: less sharp than this part of the last sharp frames
    #[serde(default = "focus_drop")]
    pub focus_drop: f64,
    /// how far up and down it may look, um
    #[serde(default = "focus_max")]
    pub focus_max_um: f64,
    /// PageUp / PageDown move z by this much, um
    #[serde(default = "z_step")]
    pub z_step_um: f64,
}

fn z_step() -> f64 {
    5.0
}

fn yes() -> bool {
    true
}
fn focus_drop() -> f64 {
    0.5
}
fn focus_max() -> f64 {
    100.0
}

impl Default for ScanSettings {
    fn default() -> Self {
        ScanSettings { overlap_quick: 0.25, overlap_quality: 0.35, settle_ms: 30.0, back_um: 20.0, mstitch: "python -m mstitch".into(), autofocus: true, focus_window_um: focus_window(), zscale_dz_um: zscale_dz(), quality_rounds: quality_rounds(), export_pad_um: export_pad(), focus_drop: 0.5, focus_max_um: 100.0, z_step_um: 5.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    /// optics of each objective (scale, camera turn, distortion), calibrated one by one
    #[serde(default)]
    pub objectives: Vec<Optics>,
    /// the objective on the microscope now (live picture, calibration, new selections)
    #[serde(default)]
    pub current: usize,
    /// projects from before objectives: their one set of optics
    #[serde(default, skip_serializing)]
    optics: Option<Optics>,
    /// one motor step, um: selections snap to it
    pub step_um: f64,
    pub samples: Vec<Sample>,
    pub active: usize,
    pub scan: ScanSettings,
    pub next_id: u32,
    /// white balance: gains for red, green, blue (live, maps, exported squares; raw frames stay raw)
    #[serde(default = "no_balance")]
    pub wb: [f64; 3],
    /// light level used when the light is switched on (1..5), laser power when the laser is (mW)
    #[serde(default = "light_on")]
    pub light_level: u32,
    #[serde(default)]
    pub laser_mw: u32,
    #[serde(skip)]
    pub folder: PathBuf,
}

fn light_on() -> u32 {
    3
}

fn no_balance() -> [f64; 3] {
    [1.0, 1.0, 1.0]
}

/// Nominal optics until calibrated (MindVision 2592 x 1944, pixel 2.2 um behind the microscope's tube).
fn default_objectives() -> Vec<Optics> {
    [("10x", 0.5), ("20x", 0.25), ("50x", 0.1), ("100x", 0.05)].iter().map(|(n, u)| Optics::guess(n, *u, [2592, 1944])).collect()
}

const COLORS: [[u8; 3]; 8] =
    [[230, 80, 60], [60, 150, 230], [70, 190, 90], [230, 170, 40], [170, 90, 220], [40, 190, 190], [230, 100, 170], [140, 140, 140]];

impl Project {
    pub fn new(folder: &Path) -> Project {
        Project {
            name: folder.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into()),
            objectives: default_objectives(),
            current: 0,
            optics: None,
            step_um: 5.556,
            samples: vec![Sample::new("Sample 1")],
            active: 0,
            scan: ScanSettings::default(),
            next_id: 1,
            wb: no_balance(),
            light_level: light_on(),
            laser_mw: 0,
            folder: folder.to_path_buf(),
        }
    }

    pub fn open(folder: &Path) -> Result<Project, String> {
        let f = folder.join("project.json");
        if !f.exists() {
            std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
            let p = Project::new(folder);
            p.save()?;
            return Ok(p);
        }
        let s = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        let mut p: Project = serde_json::from_str(&s).map_err(|e| format!("{}: {e}", f.display()))?;
        p.folder = folder.to_path_buf();
        if p.objectives.is_empty() {
            p.objectives = p.optics.take().map(|o| vec![o]).unwrap_or_else(default_objectives);
        }
        p.current = p.current.min(p.objectives.len() - 1);
        if p.samples.is_empty() {
            p.samples.push(Sample::new("Sample 1"));
        }
        p.active = p.active.min(p.samples.len() - 1);
        // the objectives' flat fields (a missing or broken file: none)
        for o in p.objectives.iter_mut() {
            if let Some(f) = &o.flat_file {
                o.flat = crate::flat::Flat::load(&folder.join(f)).ok().map(std::sync::Arc::new);
            }
        }
        Ok(p)
    }

    /// Written to a temporary file first, then renamed: a crash never leaves half a project.
    pub fn save(&self) -> Result<(), String> {
        let tmp = self.folder.join("project.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.folder.join("project.json")).map_err(|e| e.to_string())
    }

    /// The objective on the microscope now.
    pub fn optics(&self) -> &Optics {
        &self.objectives[self.current]
    }
    pub fn optics_mut(&mut self) -> &mut Optics {
        &mut self.objectives[self.current]
    }
    /// A selection's objective (by name; the current one if it has none or an unknown one).
    pub fn optics_of(&self, s: &Selection) -> &Optics {
        self.objectives.iter().find(|o| o.objective == s.objective).unwrap_or(self.optics())
    }

    pub fn sample(&self) -> &Sample {
        &self.samples[self.active]
    }
    pub fn sample_mut(&mut self) -> &mut Sample {
        &mut self.samples[self.active]
    }

    pub fn add_selection(&mut self, rect: Rect) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let n = self.sample().selections.len();
        let color = COLORS[(id as usize - 1) % COLORS.len()];
        let objective = self.optics().objective.clone();
        self.sample_mut().selections.insert(0, Selection {
            chosen: None,
            id,
            name: format!("Selection {}", n + 1),
            rect,
            color,
            visible: true,
            cut: true,
            scans: vec![],
            fix: None,
            objective,
            opacity: 1.0,
        });
        id
    }

    pub fn selection_mut(&mut self, id: u32) -> Option<&mut Selection> {
        self.samples.iter_mut().flat_map(|s| s.selections.iter_mut()).find(|s| s.id == id)
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.folder.join(rel)
    }
}

fn focus_window() -> f64 {
    50.0
}

/// The crosses of a lithography drawing: cross (X, Y), numbered from 1, at origin + ((X - 1) px, (Y - 1) py)
/// in the drawing's coordinates (um; x right, y up).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Design {
    pub name: String,
    pub origin: P2,
    pub pitch: P2,
    pub n: [i32; 2],
}

impl Design {
    pub fn at(&self, x: f64, y: f64) -> P2 {
        [self.origin[0] + (x - 1.0) * self.pitch[0], self.origin[1] + (y - 1.0) * self.pitch[1]]
    }

    /// The layouts in use (from opt_7x7_with_el_lit_1.dxf and el_lit_mark.dxf).
    pub fn builtins() -> Vec<Design> {
        vec![
            Design { name: "Optical 30 x 30, 200 um (opt_7x7)".into(), origin: [-3372.4, -3364.48], pitch: [200.0, 200.0], n: [30, 30] },
            Design { name: "E-beam 56 x 56, 90 um (el_lit_mark)".into(), origin: [-2528.144, -2436.398], pitch: [90.0, 90.0], n: [56, 56] },
        ]
    }
}

fn zscale_dz() -> f64 {
    5.0
}

fn export_pad() -> f64 {
    20.0
}
fn quality_rounds() -> usize {
    3
}
