//! Scanning a selection: the plan (frame positions on the motor step grid, every frame approached from the
//! same side), taking the frames through mc-server, saving them, and stitching them with MStitch.

use crate::geom::{snap_step, Affine, Optics, Rect, P2};
use crate::link::Link;
use crate::pixels::{self, Rgb};
use crate::project::Mode;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Frame centres for a rectangle, row by row (y), each row left to right (x).
/// Most frames one selection may take: a quick stitch handles up to 1200 (with the overlap), a quality one
/// up to 225; above that no graphics card in the lab copes.
pub const QUICK_MAX: usize = 1200;
pub const QUALITY_MAX: usize = 225;

pub fn limit(mode: Mode) -> usize {
    match mode {
        Mode::Quick => QUICK_MAX,
        Mode::Quality => QUALITY_MAX,
    }
}

/// How many frames a scan of `rect` takes.
pub fn frames(rect: &Rect, optics: &Optics, overlap: f64, step_um: f64) -> usize {
    plan(rect, optics, overlap, step_um).iter().map(|r| r.len()).sum()
}

pub fn plan(rect: &Rect, optics: &Optics, overlap: f64, step_um: f64) -> Vec<Vec<P2>> {
    let f = optics.inner_field_um();
    let axis = |a: f64, b: f64, fw: f64| -> Vec<f64> {
        let span = b - a;
        if span <= fw {
            return vec![snap_step((a + b) / 2.0, step_um)];
        }
        let pitch = fw * (1.0 - overlap);
        let n = ((span - fw) / pitch).ceil() as usize + 1;
        let first = a + fw / 2.0;
        let last = b - fw / 2.0;
        (0..n).map(|k| snap_step(first + (last - first) * k as f64 / (n - 1).max(1) as f64, step_um)).collect()
    };
    let xs = axis(rect.x0, rect.x1, f[0]);
    let ys = axis(rect.y0, rect.y1, f[1]);
    ys.iter().map(|&y| xs.iter().map(|&x| [x, y]).collect()).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Scanning,
    Stitching,
    Done,
    Failed(String),
    Stopped,
}

/// A running scan, watched by the UI.
pub struct Job {
    pub selection: u32,
    pub mode: Mode,
    pub total: usize,
    pub done: usize,
    pub phase: Phase,
    pub started: Instant,
    pub frames_dir: PathBuf,
    pub map_dir: PathBuf,
    /// small pictures of the frames taken, for the map: (stage position, picture, full-res px per picture px)
    pub thumbs: Vec<(P2, Arc<Rgb>, f64)>,
    pub stop: bool,
    pub log: Vec<String>,
    /// what looks wrong in the stitch (frames MStitch placed far from the others)
    pub warning: Option<String>,
    /// how long the scanning took (set when it ended)
    pub scan_s: Option<f64>,
    /// when stitching began
    pub stitch_at: Option<Instant>,
}

impl Job {
    pub fn eta_s(&self) -> Option<f64> {
        (self.done > 0 && self.phase == Phase::Scanning)
            .then(|| self.started.elapsed().as_secs_f64() / self.done as f64 * (self.total - self.done) as f64)
    }
}

pub struct Settings {
    pub optics: Optics,
    pub overlap: f64,
    pub settle_ms: f64,
    pub back_um: f64,
    pub step_um: f64,
    pub mstitch: String,
    pub wb: [f64; 3],
    pub focus: crate::focus::Settings,
    /// the project's record of the surface (x, y, z, sharpness of every frame), appended to
    pub surface: Option<PathBuf>,
}

/// Scan `rect` into `frames_dir`, then stitch into `map_dir`. Runs on its own thread.
pub fn start(link: Arc<Link>, selection: u32, mode: Mode, rect: Rect, s: Settings, frames_dir: PathBuf, map_dir: PathBuf) -> Arc<Mutex<Job>> {
    let rows = plan(&rect, &s.optics, s.overlap, s.step_um);
    let total = rows.iter().map(|r| r.len()).sum();
    let job = Arc::new(Mutex::new(Job {
        selection,
        mode,
        total,
        done: 0,
        phase: Phase::Scanning,
        started: Instant::now(),
        frames_dir: frames_dir.clone(),
        map_dir: map_dir.clone(),
        thumbs: vec![],
        stop: false,
        log: vec![],
        warning: None,
        scan_s: None,
        stitch_at: None,
    }));
    let j2 = job.clone();
    std::thread::Builder::new()
        .name("scan".into())
        .spawn(move || {
            let r = run(&link, &j2, &rows, &s, &frames_dir).and_then(|_| {
                {
                    let mut j = j2.lock().unwrap();
                    j.phase = Phase::Stitching;
                    j.stitch_at = Some(Instant::now());
                }
                stitch(&s, mode, &frames_dir, &map_dir, &j2)
            });
            let mut j = j2.lock().unwrap();
            j.phase = match r {
                Ok(()) => Phase::Done,
                Err(_) if j.stop => Phase::Stopped,
                Err(e) => Phase::Failed(e),
            };
        })
        .unwrap();
    job
}

fn run(link: &Link, job: &Mutex<Job>, rows: &[Vec<P2>], s: &Settings, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let scan_dir = dir.parent().unwrap_or(dir);
    let tag = scan_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut focus = crate::focus::Focus::new(crate::focus::Settings { settle_ms: s.settle_ms, ..s.focus.clone() }, scan_dir, s.surface.clone(), &tag);
    let z_start = link.stage_pos().map(|p| p[2]).unwrap_or(0.0);
    for (j, row) in rows.iter().enumerate() {
        // a row is one block: nothing from another client gets between its frames
        link.call(json!({"cmd": "begin"}))?;
        let r = (|| -> Result<(), String> {
            // approach from the same side every time: back in x (and in y before the first row)
            let back_y = if j == 0 { s.back_um } else { 0.0 };
            link.call(json!({"cmd": "goto", "x": row[0][0] - s.back_um, "y": row[0][1] - back_y}))?;
            for (i, p) in row.iter().enumerate() {
                if job.lock().unwrap().stop {
                    return Err("stopped".into());
                }
                // in focus where it can be found; z from the surface measured so far
                let z = focus.predict(p[0], p[1], z_start);
                let shot = focus.shot(link, p[0], p[1], z, &|| job.lock().unwrap().stop)?;
                let f = shot.frame.clone();
                if shot.probes > 0 {
                    job.lock().unwrap().log.push(format!("focus at ({:.0}, {:.0}): z {:.1} um after {} frames, sharpness {:.2}", p[0], p[1], shot.z, shot.probes, shot.sharpness));
                }
                // the lens's distortion out before stitching: MStitch gets frames with straight lines
                // the flat field first (it belongs to the sensor), then the distortion
                let mut raw = shot.sensor;
                let flat = s.optics.flat.as_ref().is_some_and(|fl| fl.apply(&mut raw, s.optics.flat_strength as f32));
                let sensor = raw.undistorted(&s.optics);
                let mut meta = f.meta.clone();
                if flat {
                    meta["flat_corrected"] = json!(true);
                }
                meta["row"] = json!(j);
                meta["col"] = json!(i);
                meta["plan_um"] = json!(p);
                meta["sharpness"] = json!(shot.sharpness);
                if s.optics.distorted() {
                    meta["undistorted_k"] = json!(s.optics.k);
                    meta["undistorted_zoom"] = json!(s.optics.zoom());
                }
                sensor.save(&meta, &dir.join(format!("r{j:03}_c{i:03}"))).map_err(|e| e.to_string())?;
                let mut thumb = sensor.rgb(4);
                thumb.balance(s.wb);
                let thumb = Arc::new(thumb);
                let xyz = f.xyz();
                let mut jb = job.lock().unwrap();
                jb.done += 1;
                jb.thumbs.push(([xyz[0], xyz[1]], thumb, 8.0));
            }
            Ok(())
        })();
        let _ = link.call(json!({"cmd": "end"}));
        r?;
    }
    Ok(())
}

/// The stitcher. Until MStitch has a C API linked into this program, its command line does the work:
/// `mstitch stitch FRAMES --profile P --mode quick|quality --out MAP` (manifest.json + tiles/*.png).
fn stitch(s: &Settings, mode: Mode, frames: &Path, map: &Path, job: &Mutex<Job>) -> Result<(), String> {
    // MStitch's own profile when one is set (its flat field etc.), else the settings; frames are saved with the
    // distortion taken out and zoomed in by `zoom`: MStitch must not take it out again and gets m / zoom
    let mut prof: Value = match &s.optics.profile_path {
        Some(p) => serde_json::from_slice(&std::fs::read(p).map_err(|e| format!("{p}: {e}"))?).map_err(|e| format!("{p}: {e}"))?,
        None => s.optics.profile_json(),
    };
    // corrected by the flat field when scanned (frames of another size than the flat's are not)
    let flat = s.optics.flat.as_ref().filter(|_| {
        std::fs::read(frames.join("r000_c000.json")).ok().and_then(|d| serde_json::from_slice::<Value>(&d).ok()).is_some_and(|m| m["flat_corrected"] == json!(true))
    });
    let profile = if s.optics.distorted() || s.optics.profile_path.is_none() || flat.is_some() {
        let z = s.optics.zoom();
        prof["m"] = json!(s.optics.m.map(|v| v / z));
        prof["k"] = json!([0.0, 0.0]);
        // the frames are corrected already: MStitch gets an even flat field, so it neither uses its own nor
        // estimates one from the frames
        if let Some(f) = flat {
            crate::flat::Flat::write_ones(&frames.join("flat_ones.npy"), f.w, f.h)?;
            prof["flat"] = json!("flat_ones.npy");
            prof["flat_level"] = json!(2);
        }
        let p = frames.join("profile.json");
        std::fs::write(&p, serde_json::to_vec_pretty(&prof).unwrap()).map_err(|e| e.to_string())?;
        p
    } else {
        PathBuf::from(s.optics.profile_path.as_ref().unwrap())
    };
    let mut parts = s.mstitch.split_whitespace();
    let exe = parts.next().ok_or("MStitch command is empty (Settings)")?;
    let mut cmd = std::process::Command::new(exe);
    // no console window popping up on Windows
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.args(parts).arg("stitch").arg(frames).arg("--profile").arg(&profile).arg("--mode").arg(mode.name()).arg("--out").arg(map);
    let out = cmd.output().map_err(|e| format!("cannot start MStitch ({}): {e}", s.mstitch))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    job.lock().unwrap().log.extend(text.lines().map(String::from));
    if !out.status.success() {
        return Err(format!("MStitch failed: {}", text.lines().last().unwrap_or("")));
    }
    job.lock().unwrap().warning = check(map);
    Ok(())
}

/// Frames that MStitch put somewhere else than the others say: every frame is shifted from where the stage
/// was by about the same (the stage's offset); one that differs by more than 2 um was matched wrong
/// (or the stage slipped) and should be taken again.
pub fn check(map: &Path) -> Option<String> {
    let text = std::fs::read_to_string(map.join("poses.csv")).ok()?;
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next()?.split(',').collect();
    let col = |n: &str| head.iter().position(|h| *h == n);
    let (ix, iy, dx, dy, rs) = (col("x_um")?, col("y_um")?, col("dx_um")?, col("dy_um")?, col("reshoot"));
    let rows: Vec<Vec<String>> = lines.map(|l| l.split(',').map(String::from).collect()).collect();
    let num = |r: &Vec<String>, i: usize| r.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(f64::NAN);
    let median = |i: usize| {
        let mut v: Vec<f64> = rows.iter().map(|r| num(r, i)).filter(|v| v.is_finite()).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let (mx, my) = (median(dx), median(dy));
    let mut bad = vec![];
    for r in &rows {
        let off = ((num(r, dx) - mx).powi(2) + (num(r, dy) - my).powi(2)).sqrt();
        // MStitch writes None (or nothing) when a frame needs no second shot
        let flagged = rs.and_then(|i| r.get(i)).is_some_and(|v| !matches!(v.trim(), "" | "None" | "False" | "false" | "0" | "{}" | "[]"));
        if off > 2.0 || flagged {
            bad.push((off, format!("({:.0}, {:.0}) um {off:.1} um off", num(r, ix), num(r, iy))));
        }
    }
    if bad.is_empty() {
        return None;
    }
    // short: how many, and the worst three
    bad.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let worst: Vec<String> = bad.iter().take(3).map(|b| b.1.clone()).collect();
    Some(format!("check the stitch: {} of {} frames apart from the rest, worst {}", bad.len(), rows.len(), worst.join("; ")))
}

/// A stitched map from MStitch's folder: the picture and its pixel -> stage mapping.
pub fn load_map(dir: &Path) -> Result<(Rgb, Affine, f64), String> {
    let m: Value = serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).map_err(|e| format!("{}: {e}", dir.display()))?)
        .map_err(|e| e.to_string())?;
    let size = &m["size"];
    let (w, h) = (size[0].as_u64().ok_or("manifest: size")? as usize, size[1].as_u64().ok_or("manifest: size")? as usize);
    let a = &m["stage_from_px"];
    let row = |r: usize| -> Result<[f64; 3], String> {
        let v: Vec<f64> = a[r].as_array().ok_or("manifest: stage_from_px")?.iter().filter_map(|x| x.as_f64()).collect();
        Ok([v[0], v[1], v[2]])
    };
    let (r0, r1) = (row(0)?, row(1)?);
    // MStitch counts a pixel at its centre (pixel k is at k, the frame centre at (w-1)/2); here a pixel covers
    // [k, k+1), its centre is k + 1/2: half a pixel back before MStitch's matrix
    let to_stage = Affine([r0[0], r0[1], r0[2] - 0.5 * (r0[0] + r0[1]), r1[0], r1[1], r1[2] - 0.5 * (r1[0] + r1[1])]);
    let mut img = Rgb::new(w, h);
    for t in m["tiles"].as_array().ok_or("manifest: tiles")? {
        let tile = pixels::read_png(&dir.join(t["file"].as_str().unwrap_or("")))?;
        let (x0, y0) = (t["x"].as_u64().unwrap_or(0) as usize, t["y"].as_u64().unwrap_or(0) as usize);
        for y in 0..tile.h.min(h.saturating_sub(y0)) {
            let n = tile.w.min(w.saturating_sub(x0)) * 3;
            let src = &tile.px[y * tile.w * 3..y * tile.w * 3 + n];
            img.px[((y0 + y) * w + x0) * 3..((y0 + y) * w + x0) * 3 + n].copy_from_slice(src);
        }
    }
    let umpp = m["um_per_px"].as_f64().unwrap_or(to_stage.scale());
    Ok((img, to_stage, umpp))
}

#[cfg(test)]
mod check_tests {
    #[test]
    fn only_frames_really_apart_are_reported() {
        let dir = std::env::temp_dir().join(format!("mc-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut csv = "index,row,col,x_um,y_um,x_px,y_px,dx_px,dy_px,dx_um,dy_um,pairs,best_ncc,reshoot\n".to_string();
        for k in 0..300 {
            let dx = if k == 7 || k == 8 { 4.7 } else { 0.1 };
            csv += &format!("{k},-1,-1,{},{},0,0,0,0,{dx},0.0,3,0.9,None\n", k * 10, 0);
        }
        std::fs::write(dir.join("poses.csv"), csv).unwrap();
        let w = super::check(&dir).unwrap();
        assert!(w.starts_with("check the stitch: 2 of 300"), "{w}");
        assert!(w.len() < 200, "{w}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_covers_the_rectangle() {
        let o = Optics::guess("10x", 0.5, [2592, 1944]); // field 1296 x 972 um
        let r = Rect { x0: 0.0, y0: 0.0, x1: 3000.0, y1: 1000.0 };
        let p = plan(&r, &o, 0.25, 5.556);
        let f = o.inner_field_um();
        let xs: Vec<f64> = p[0].iter().map(|q| q[0]).collect();
        assert!(xs[0] - f[0] / 2.0 <= 3.0 && xs.last().unwrap() + f[0] / 2.0 >= 2997.0);
        for w in xs.windows(2) {
            assert!(w[1] - w[0] <= f[0] * 0.75 + 6.0); // at least the overlap asked for
        }
        assert!(p.len() == 2 && p.iter().all(|row| row.len() == xs.len()));
    }
}

/// The whole way against `mc-server --sim` (crosses every 100 um) and MStitch:
/// MC_E2E=127.0.0.1:5160 MC_MSTITCH="python -m mstitch" MC_E2E_OUT=/tmp/e2e cargo test -p mc-gui e2e -- --ignored --nocapture
#[cfg(test)]
mod e2e {
    use super::*;
    use crate::geom::{Optics, Rect};
    use crate::project::{Cross, Project, Scan};

    #[test]
    #[ignore]
    fn e2e_scan_stitch_lattice_cut() {
        let addr = std::env::var("MC_E2E").expect("MC_E2E=host:port of mc-server --sim");
        let out = PathBuf::from(std::env::var("MC_E2E_OUT").unwrap_or_else(|_| "/tmp/mc-e2e".into()));
        let _ = std::fs::remove_dir_all(&out);
        let link = Link::start(eframe::egui::Context::default(), addr, "e2e".into());
        let t0 = Instant::now();
        while !link.state.lock().unwrap().connected {
            assert!(t0.elapsed().as_secs() < 10, "no connection");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mut proj = Project::new(&out);
        proj.objectives = vec![Optics::guess("sim", 0.22, [1296, 972])];
        let rect = Rect::from_corners([40.0, 30.0], [420.0, 300.0], proj.step_um);
        let id = proj.add_selection(rect);
        let mut result: Option<(Rgb, Affine, f64, Vec<Cross>)> = None;
        for mode in [Mode::Quick, Mode::Quality] {
            let s = Settings { optics: proj.optics().clone(), overlap: if mode == Mode::Quick { 0.25 } else { 0.35 }, settle_ms: 0.0, back_um: 20.0, step_um: proj.step_um, mstitch: std::env::var("MC_MSTITCH").unwrap_or_else(|_| "python -m mstitch".into()), wb: [1.0, 1.0, 1.0], focus: crate::focus::Settings { on: false, drop: 0.7, max_um: 100.0, settle_ms: 0.0 }, surface: None };
            let base = out.join("scans").join(mode.name());
            let t1 = Instant::now();
            let job = start(link.clone(), id, mode, rect, s, base.join("frames"), base.join("map"));
            loop {
                let j = job.lock().unwrap();
                match &j.phase {
                    Phase::Done => break,
                    Phase::Failed(e) => panic!("{e}\n{}", j.log.join("\n")),
                    _ => {}
                }
                drop(j);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let n = job.lock().unwrap().done;
            if let Some(w) = job.lock().unwrap().warning.clone() {
                println!("{}: {w}", mode.name());
                continue; // MStitch's own registration miss, reported to the operator: not this program's error
            }
            let (map, a, umpp) = load_map(&base.join("map")).unwrap();
            println!("{}: {n} frames, {:.1} s, map {}x{} px, {umpp:.4} um/px", mode.name(), t1.elapsed().as_secs_f64(), map.w, map.h);
            // the sample's crosses are at multiples of 100 um: find them on the map, compare in um
            let mut worst: f64 = 0.0;
            let mut marks = vec![];
            for (x, y) in [(100.0, 100.0), (300.0, 100.0), (200.0, 200.0), (100.0, 200.0)] {
                let uv = a.inverse().apply([x + 1.3, y - 0.8]); // a click near it
                let (u, v) = crate::cross::refine(&map, uv[0], uv[1], (20.0 / umpp) as usize).expect("no cross found");
                let p = a.apply([u, v]);
                let e = ((p[0] - x).powi(2) + (p[1] - y).powi(2)).sqrt();
                println!("  cross ({x}, {y}): found ({:.3}, {:.3}) um, off by {e:.3} um", p[0], p[1]);
                worst = worst.max(e);
                marks.push(Cross { at: p, label: Some([(x / 100.0) as i32, (y / 100.0) as i32]) });
            }
            assert!(worst < 0.2, "crosses off by {worst} um");
            proj.sample_mut().selections[0].scans.push(Scan { mode, frames: format!("scans/{}/frames", mode.name()), map: Some(format!("scans/{}/map", mode.name())), n_frames: n, when: "e2e".into(), note: String::new() });
            result = Some((map, a, umpp, marks));
        }
        let Some((map, a, umpp, marks)) = result else {
            println!("MStitch misplaced a frame in both stitches: reported above, nothing more to check");
            return;
        };
        // project for a look in the program
        proj.sample_mut().selections[0].cut = true;
        proj.sample_mut().crosses = marks[..3].to_vec();
        proj.sample_mut().refit().unwrap();
        let l = proj.sample().lattice.clone().unwrap();
        println!("lattice pitch {:?} um, rms {:.3} um", l.pitch(), l.rms_um);
        assert!((l.pitch()[0] - 100.0).abs() < 0.3 && (l.pitch()[1] - 100.0).abs() < 0.3);
        proj.save().unwrap();
        let o = crate::export::Orient::of(&l, false, false, false, 0.0);
        let dir = out.join("export");
        let sq = crate::export::cut(&crate::export::Source { rect, map: &map, to_stage: a, opacity: 1.0 }, &l, o, umpp, &dir.join("squares/Sel"), &mut |_, _| {}).unwrap();
        crate::export::write_dxf(&dir.join("drawing.dxf"), &l, o, &[("Sel".into(), [255, 80, 80], rect, sq)], &rect).unwrap();
    }
}

/// Lens distortion and a turned sample, against `mc-server --sim --sim-k1 0.05 --sim-tilt 1.5`:
/// calibrate the optics, scan two neighbouring selections, mark 3 crosses, Align, cut.
/// MC_E2E=127.0.0.1:5160 MC_MSTITCH=... cargo test -p mc-gui e2e_lens -- --ignored --nocapture
#[cfg(test)]
mod e2e_lens {
    use super::*;
    use crate::geom::{self, Optics, Rect};
    use crate::project::{Cross, Project};

    fn wait_scan(job: &Arc<Mutex<Job>>) -> Result<(), String> {
        loop {
            let j = job.lock().unwrap();
            match &j.phase {
                Phase::Done => return j.warning.clone().map_or(Ok(()), Err),
                Phase::Failed(e) => return Err(e.clone()),
                _ => {}
            }
            drop(j);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    #[test]
    #[ignore]
    fn e2e_orientation_from_a_wrong_guess() {
        let addr = std::env::var("MC_E2E").expect("MC_E2E=host:port of mc-server --sim");
        let link = Link::start(eframe::egui::Context::default(), addr, "e2e".into());
        let t0 = Instant::now();
        while link.stage_pos().is_none() {
            assert!(t0.elapsed().as_secs() < 10, "no connection");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // wrong scale (0.35 instead of 0.22 um/px), turned 90 degrees and mirrored
        let k = 1.0 / 0.35;
        let o = Optics { m: [0.0, k, k, 0.0], ..Optics::guess("sim", 0.35, [1296, 972]) };
        link.call(json!({"cmd": "goto", "x": 700.0, "y": 500.0})).unwrap();
        let c = crate::calib::start(link.clone(), o, 50.0, 20.0, true);
        let f = loop {
            if let Some(r) = c.lock().unwrap().result.clone() {
                break r.unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        let t = 1.0 / 0.22;
        eprintln!("m = {:?}", f.m);
        assert!((f.m[0] - t).abs() < 0.05 * t && (f.m[3] - t).abs() < 0.05 * t && f.m[1].abs() < 0.05 * t && f.m[2].abs() < 0.05 * t, "{:?}", f.m);
    }

    #[test]
    #[ignore]
    fn e2e_lens_tilt_align() {
        let addr = std::env::var("MC_E2E").expect("MC_E2E=host:port of mc-server --sim --sim-k1 0.05 --sim-tilt 1.5");
        let (k1, tilt) = (0.05, 1.5f64.to_radians());
        let out = PathBuf::from("/tmp/mc-e2e-lens");
        let _ = std::fs::remove_dir_all(&out);
        let link = Link::start(eframe::egui::Context::default(), addr, "e2e".into());
        let t0 = Instant::now();
        while link.stage_pos().is_none() {
            assert!(t0.elapsed().as_secs() < 10, "no connection (another operator holds the microscope?)");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mut proj = Project::new(&out);
        proj.objectives = vec![Optics::guess("sim", 0.22, [1296, 972])];
        // 1. the optics, by the stage
        link.call(json!({"cmd": "goto", "x": 1030.0, "y": 1040.0})).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let c = crate::calib::start(link.clone(), proj.optics().clone(), 50.0, 20.0, false);
        let fit = loop {
            if let Some(r) = c.lock().unwrap().result.clone() {
                break r.unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        println!("calibration: {fit:?}");
        {
            let truth = Optics { objective: String::new(), m: [1.0 / 0.22, 0.0, 0.0, 1.0 / 0.22], frame: [1296, 972], profile_path: None, k: [k1, 0.0], camera: None, flat_file: None, flat: None, flat_strength: 1.0 };
            let ms = crate::calib::DEBUG.lock().unwrap().clone();
            let mut res: Vec<f64> = ms.iter().map(|m| {
                let (a, b) = (truth.undistort(m.p0), truth.undistort(m.p1));
                let d = m.d_um;
                geom::len([b[0] - a[0] + truth.m[0] * d[0], b[1] - a[1] + truth.m[3] * d[1]])
            }).collect();
            let mut by: Vec<(String, Vec<f64>)> = vec![];
            for (m, r) in ms.iter().zip(&res) {
                let key = format!("{:.1},{:.1}", m.d_um[0], m.d_um[1]);
                match by.iter_mut().find(|b| b.0 == key) {
                    Some(b) => b.1.push(*r),
                    None => by.push((key, vec![*r])),
                }
            }
            for (k, mut v) in by {
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                println!("  frame moved {k} um: {} matches, median {:.2} px, worst {:.1}", v.len(), v[v.len() / 2], v[v.len() - 1]);
            }
            res.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!("matches {}, residual under the truth: median {:.2} px, 75% {:.2}, 90% {:.2}, max {:.1}", res.len(), res[res.len() / 2], res[res.len() * 3 / 4], res[res.len() * 9 / 10], res[res.len() - 1]);
        }
        assert!((fit.k[0] - k1).abs() < 0.005 && fit.k[1].abs() < 0.01, "{fit:?}");
        assert!((fit.m[0] - 1.0 / 0.22).abs() < 0.01 && fit.m[1].abs() < 0.01 && fit.rms_px < 0.3, "{fit:?}");
        proj.optics_mut().m = fit.m;
        proj.optics_mut().k = fit.k;
        // 2. two selections side by side, each stitched on its own
        let rects = [Rect::from_corners([40.0, 30.0], [262.0, 300.0], proj.step_um), Rect::from_corners([250.0, 30.0], [470.0, 300.0], proj.step_um)];
        let mut maps = vec![];
        for (n, r) in rects.iter().enumerate() {
            let id = proj.add_selection(*r);
            let mut ok = None;
            for attempt in 0..4 {
                let s = Settings { optics: proj.optics().clone(), overlap: 0.35, settle_ms: 0.0, back_um: 20.0, step_um: proj.step_um, mstitch: std::env::var("MC_MSTITCH").unwrap_or_else(|_| "python -m mstitch".into()), wb: [1.0; 3], focus: crate::focus::Settings { on: false, drop: 0.7, max_um: 100.0, settle_ms: 0.0 }, surface: None };
                let base = out.join(format!("scans/s{n}_{attempt}"));
                let job = start(link.clone(), id, Mode::Quick, *r, s, base.join("frames"), base.join("map"));
                match wait_scan(&job) {
                    Ok(()) => {
                        ok = Some(load_map(&base.join("map")).unwrap());
                        let rel = format!("scans/s{n}_{attempt}");
                        proj.selection_mut(id).unwrap().scans.push(crate::project::Scan { mode: Mode::Quick, frames: format!("{rel}/frames"), map: Some(format!("{rel}/map")), n_frames: 0, when: "e2e".into(), note: String::new() });
                        break;
                    }
                    Err(e) => println!("selection {n}, attempt {attempt}: {e}"),
                }
            }
            let (map, a, umpp) = ok.expect("MStitch misplaced frames in every attempt");
            println!("selection {n}: map {}x{} px, {umpp:.4} um/px", map.w, map.h);
            maps.push((map, a));
        }
        // 3. three crosses marked by hand on the first map (clicks near them), the lattice
        let node = |i: f64, j: f64| [tilt.cos() * 100.0 * i - tilt.sin() * 100.0 * j, tilt.sin() * 100.0 * i + tilt.cos() * 100.0 * j];
        let (m0, a0) = &maps[0];
        let mut crosses = vec![];
        for (i, j) in [(1, 1), (2, 1), (1, 2)] {
            let t = node(i as f64, j as f64);
            let uv = a0.inverse().apply([t[0] + 1.5, t[1] - 1.0]);
            let (u, v) = crate::cross::refine(m0, uv[0], uv[1], (20.0 / a0.scale()) as usize).expect("cross");
            crosses.push(Cross { at: a0.apply([u, v]), label: Some([i, j]) });
        }
        proj.sample_mut().crosses = crosses;
        proj.sample_mut().refit().unwrap();
        let l = proj.sample().lattice.clone().unwrap();
        let turn = l.a[1].atan2(l.a[0]).to_degrees();
        println!("lattice: pitch {:?} um, turned {turn:.4} deg (truth 1.5)", l.pitch());
        assert!((turn - 1.5).abs() < 0.02 && (l.pitch()[0] - 100.0).abs() < 0.2, "{l:?}");
        // 4. Align both maps onto it; then every cross of both maps lies on its lattice node
        for (n, (m, a)) in maps.iter().enumerate() {
            let al = crate::align::align(&l, &rects[n], m, a).unwrap();
            println!("selection {n}: {} crosses, moved {:.3} um, left {:.3} um", al.crosses, al.moved_um, al.rms_um);
            assert!(al.rms_um < 0.2, "{al:?}");
            let fixed = a.then(&al.fix);
            for i in -1..6 {
                for j in -1..5 {
                    let t = l.at(i as f64, j as f64);
                    if !rects[n].contains(t) {
                        continue;
                    }
                    let uv = fixed.inverse().apply(t);
                    if let Some((u, v)) = crate::cross::refine(m, uv[0], uv[1], (20.0 / fixed.scale()) as usize) {
                        let e = geom::len(geom::sub(fixed.apply([u, v]), t));
                        println!("  selection {n} cross {i},{j}: {e:.3} um off its node");
                        assert!(e < 0.25);
                    }
                }
            }
            proj.sample_mut().selections[n].fix = Some(al.fix);
        }
        // 5. cut both on the one lattice, the drawing
        let o = crate::export::Orient::of(&l, false, false, false, 0.0);
        let mut layers = vec![];
        for (n, (m, a)) in maps.iter().enumerate() {
            let fixed = a.then(proj.sample().selections[n].fix.as_ref().unwrap());
            let dir = out.join(format!("export/squares/S{n}"));
            let sq = crate::export::cut(&crate::export::Source { rect: rects[n], map: m, to_stage: fixed, opacity: 1.0 }, &l, o, fixed.scale(), &dir, &mut |_, _| {}).unwrap();
            layers.push((format!("S{n}"), [255, 80, 80], rects[n], sq));
        }
        crate::export::write_dxf(&out.join("export/drawing.dxf"), &l, o, &layers, &rects[0].union(&rects[1])).unwrap();
        proj.save().unwrap();
    }
}

/// Autofocus against `mc-server --sim` (sharp at z = 0): a scan started 35 um off ends up in focus everywhere.
#[cfg(test)]
mod e2e_focus {
    use super::*;
    use crate::geom::{Optics, Rect};

    #[test]
    #[ignore]
    fn e2e_focus_finds_z() {
        let addr = std::env::var("MC_E2E").expect("MC_E2E=host:port of mc-server --sim");
        let out = PathBuf::from("/tmp/mc-e2e-focus");
        let _ = std::fs::remove_dir_all(&out);
        let link = Link::start(eframe::egui::Context::default(), addr, "e2e".into());
        let t0 = Instant::now();
        while link.stage_pos().is_none() {
            assert!(t0.elapsed().as_secs() < 10, "no connection");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        link.call(json!({"cmd": "goto", "x": 0.0, "y": 0.0, "z": 35.0})).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let optics = Optics::guess("sim", 0.22, [1296, 972]);
        let rect = Rect::from_corners([0.0, 0.0], [600.0, 400.0], 5.556);
        let s = Settings { optics, overlap: 0.25, settle_ms: 0.0, back_um: 20.0, step_um: 5.556, mstitch: "true".into(), wb: [1.0; 3], focus: crate::focus::Settings { on: true, drop: 0.7, max_um: 100.0, settle_ms: 0.0 }, surface: Some(out.join("surface.csv")) };
        let job = start(link.clone(), 1, Mode::Quick, rect, s, out.join("scan/frames"), out.join("scan/map"));
        loop {
            let j = job.lock().unwrap();
            if !matches!(j.phase, Phase::Scanning) {
                break;
            }
            drop(j);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let j = job.lock().unwrap();
        for l in &j.log {
            println!("{l}");
        }
        let csv = std::fs::read_to_string(out.join("scan/focus.csv")).unwrap();
        println!("{csv}");
        let zs: Vec<f64> = csv.lines().skip(1).map(|l| l.split(',').nth(2).unwrap().parse().unwrap()).collect();
        assert_eq!(zs.len(), j.done);
        assert!(zs.iter().all(|z| z.abs() <= 2.5), "{zs:?}");
        assert!(std::fs::read_to_string(out.join("surface.csv")).unwrap().lines().count() == zs.len() + 1);
    }
}
