//! Stage, light, laser, cooling: one thread owns the controller. Moves are cut into 1 mm chunks so `stop` acts fast.
//! The controller has no encoders: the position is the sum of the steps sent (kept in a state file between runs).
//! Every chunk is recorded as a time segment, so the position at any moment (a frame's exposure) is known.

use crate::controller::{self as C, Controller};
use mc_proto::clock::now_ns;
use mc_proto::errors::{McError, McResult};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const AXES: [&str; 3] = ["x", "y", "z"];

#[derive(Clone, Debug)]
pub struct StageCfg {
    pub step_um: f64,
    pub speed_um_s: f64,
    pub move_pause_ms: u64,
    pub xy_limit_um: f64,
    pub z_range_um: f64,
    pub z_max_move_um: f64,
    pub reverse: [bool; 3],
    pub allow_laser: bool,
    pub chunk_steps: i64,
    pub state_file: std::path::PathBuf,
}

impl Default for StageCfg {
    fn default() -> Self {
        StageCfg {
            step_um: 1.0 / 0.18,
            speed_um_s: 3000.0,
            move_pause_ms: 100,
            xy_limit_um: 25_000.0,
            z_range_um: 50_000.0,
            z_max_move_um: 2_000.0,
            reverse: [false; 3],
            allow_laser: false,
            chunk_steps: 180,
            state_file: "mc-server-state.json".into(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Seg {
    pub t0: u64,
    pub t1: u64,
    pub from: [f64; 3],
    pub to: [f64; 3],
}

#[derive(Debug)]
pub struct Shared {
    pub steps: [i64; 3],
    pub origin: [i64; 3],
    pub start: [i64; 3],
    pub moving: bool,
    pub uncertain: [bool; 3],
    pub light: u32,
    pub laser: u32,
    pub laser_max: u32,
    pub motors: bool,
    pub cooling: bool,
    pub ident: Value,
    pub segs: VecDeque<Seg>,
    pub settled_ns: u64,
    pub step_um: f64,
}

impl Shared {
    pub fn pos_um(&self) -> [f64; 3] {
        let mut p = [0.0; 3];
        for i in 0..3 {
            p[i] = (self.steps[i] - self.origin[i]) as f64 * self.step_um;
        }
        p
    }

    /// Position at server time t (ns), whether the stage was moving then, and ms since it last settled.
    pub fn at(&self, t: u64) -> ([f64; 3], bool, f64) {
        for s in self.segs.iter().rev() {
            if t >= s.t0 && t <= s.t1 {
                let f = if s.t1 > s.t0 { (t - s.t0) as f64 / (s.t1 - s.t0) as f64 } else { 1.0 };
                let mut p = [0.0; 3];
                for i in 0..3 {
                    p[i] = s.from[i] + (s.to[i] - s.from[i]) * f;
                }
                return (p, true, 0.0);
            }
            if t > s.t1 {
                let settled = t.saturating_sub(self.settled_ns.max(s.t1)) as f64 / 1e6;
                let moving = t < self.settled_ns && self.settled_ns > s.t1;
                return (s.to, moving, if moving { 0.0 } else { settled });
            }
        }
        (self.pos_um(), self.moving, t.saturating_sub(self.settled_ns) as f64 / 1e6)
    }

    pub fn json(&self) -> Value {
        let p = self.pos_um();
        json!({
            "pos_um": [round3(p[0]), round3(p[1]), round3(p[2])],
            "steps": self.steps,
            "moving": self.moving,
            "uncertain": AXES.iter().zip(self.uncertain).filter(|a| a.1).map(|a| a.0).collect::<Vec<_>>(),
            "light": self.light, "laser_mw": self.laser, "laser_max_mw": self.laser_max,
            "motors": self.motors, "cooling": self.cooling, "device": self.ident,
            "step_um": self.step_um,
        })
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

type Job = Box<dyn FnOnce(&mut Worker) -> McResult<Value> + Send>;

pub struct Stage {
    tx: Mutex<Sender<(Job, Sender<McResult<Value>>)>>,
    pub shared: Arc<Mutex<Shared>>,
    pub stop_gen: Arc<AtomicU64>,
    /// Bumped by the stage thread while it is alive (the guard watches it).
    pub tick: Arc<AtomicU64>,
    pub cfg: StageCfg,
    pub port: String,
}

pub struct Worker {
    ctl: Controller,
    cfg: StageCfg,
    shared: Arc<Mutex<Shared>>,
    stop_gen: Arc<AtomicU64>,
    tick: Arc<AtomicU64>,
    pub on_move: Option<Box<dyn Fn([f64; 3]) + Send>>,
}

impl Stage {
    pub fn open(port: &str, cfg: StageCfg) -> McResult<Arc<Stage>> {
        let ctl = Controller::open(port)?;
        let saved = load_state(&cfg.state_file);
        let mut sh = Shared {
            steps: saved.0,
            origin: saved.1,
            start: saved.0,
            moving: false,
            uncertain: [saved.2; 3],
            light: 0,
            laser: 0,
            laser_max: 0,
            motors: false,
            cooling: false,
            ident: Value::Null,
            segs: VecDeque::new(),
            settled_ns: now_ns(),
            step_um: cfg.step_um,
        };
        if saved.2 {
            warn!("STAGE", "The last run ended during a move: the position may be off. Check the stage, then `confirm`.");
        }
        sh.start = sh.steps;
        let shared = Arc::new(Mutex::new(sh));
        let stop_gen = Arc::new(AtomicU64::new(0));
        let tick = Arc::new(AtomicU64::new(now_ns()));
        let mut w = Worker { ctl, cfg: cfg.clone(), shared: shared.clone(), stop_gen: stop_gen.clone(), tick: tick.clone(), on_move: None };
        w.handshake()?;
        let (tx, rx) = channel::<(Job, Sender<McResult<Value>>)>();
        let st = Arc::new(Stage { tx: Mutex::new(tx), shared, stop_gen, tick, cfg, port: port.to_string() });
        std::thread::Builder::new().name("stage".into()).spawn(move || run(w, rx)).expect("stage thread");
        Ok(st)
    }

    /// Run a job on the stage thread and wait for its result.
    pub fn call<F>(&self, f: F) -> McResult<Value>
    where
        F: FnOnce(&mut Worker) -> McResult<Value> + Send + 'static,
    {
        let (rtx, rrx) = channel();
        self.tx.lock().unwrap().send((Box::new(f), rtx)).map_err(|_| McError::new("E300", "stage thread stopped"))?;
        rrx.recv().map_err(|_| McError::new("E300", "stage thread stopped"))?
    }

    /// Stop now: the running move ends after its current chunk, queued moves are dropped.
    pub fn stop(&self) {
        self.stop_gen.fetch_add(1, Ordering::SeqCst);
    }

    pub fn gen(&self) -> u64 {
        self.stop_gen.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> Value {
        self.shared.lock().unwrap().json()
    }
}

fn run(mut w: Worker, rx: Receiver<(Job, Sender<McResult<Value>>)>) {
    loop {
        w.tick.store(now_ns(), Ordering::Relaxed);
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok((job, rtx)) => {
                let r = job(&mut w);
                let _ = rtx.send(r);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
}

impl Worker {
    pub fn set_on_move(&mut self, f: Box<dyn Fn([f64; 3]) + Send>) {
        self.on_move = Some(f);
    }

    fn handshake(&mut self) -> McResult<()> {
        let mut sn = None;
        for i in 0..3 {
            match self.ctl.cmd(C::GET_SN, &[], 1000) {
                Ok(v) => {
                    sn = Some(v);
                    break;
                }
                Err(e) => warn!("STAGE", "No answer to GET_SN, attempt {} of 3 ({})", i + 1, e.code),
            }
        }
        let sn = sn.ok_or_else(|| McError::new("E301", "the controller does not answer: close the Optosky software, replug the controller USB"))?;
        let pn = self.ctl.cmd(C::GET_PN, &[], 5000)?;
        let ml = self.ctl.cmd(C::GET_MAX_LASER, &[], 5000)?;
        let _ = self.ctl.cmd(C::GET_CALIB, &[9], 5000);
        let ty = self.ctl.cmd(C::GET_TYPE, &[], 5000)?;
        let laser_max = if ml.len() >= 2 { u16::from_be_bytes([ml[ml.len() - 2], ml[ml.len() - 1]]) as u32 } else { 0 };
        let kind = ty.iter().find_map(|b| match b {
            0 => Some("AutoFocus"),
            1 => Some("Basic"),
            2 => Some("Mapping"),
            _ => None,
        });
        let ident = json!({"model": txt(&pn), "serial": txt(&sn), "type": kind.unwrap_or("?"), "laser_max_mw": laser_max, "port": self.ctl.name});
        info!("STAGE", "Controller {}: model {}, serial {}, type {}, laser max {} mW", self.ctl.name, txt(&pn), txt(&sn), kind.unwrap_or("?"), laser_max);
        let cooled = self.ctl.cmd(C::SET_TEC, &C::tec_data(C::COOL_C), 1000).is_ok();
        let mut sh = self.shared.lock().unwrap();
        sh.ident = ident;
        sh.laser_max = if laser_max <= 2000 { laser_max } else { 0 };
        sh.cooling = cooled;
        Ok(())
    }

    fn alive(&self) {
        self.tick.store(now_ns(), Ordering::Relaxed);
    }

    fn sleep(&self, ms: f64) {
        let end = now_ns() + (ms * 1e6) as u64;
        loop {
            self.alive();
            let left = end.saturating_sub(now_ns());
            if left == 0 {
                return;
            }
            std::thread::sleep(Duration::from_nanos(left.min(20_000_000)));
        }
    }

    /// Move by d (um). gen: the stop generation when the command was given.
    pub fn move_by(&mut self, d: [Option<f64>; 3], gen: u64) -> McResult<Value> {
        let cur = self.shared.lock().unwrap().pos_um();
        let mut t = [None; 3];
        for i in 0..3 {
            t[i] = d[i].map(|v| cur[i] + v);
        }
        self.move_to(t, gen)
    }

    /// Move to t (um, None = stay). Z away from the objective first, then X, Y, then Z towards it.
    pub fn move_to(&mut self, t: [Option<f64>; 3], gen: u64) -> McResult<Value> {
        let (cur_steps, start, origin) = {
            let s = self.shared.lock().unwrap();
            (s.steps, s.start, s.origin)
        };
        let step = self.cfg.step_um;
        let mut target = cur_steps;
        for i in 0..3 {
            if let Some(v) = t[i] {
                if !v.is_finite() {
                    return Err(McError::new("E101", format!("{} = {v}", AXES[i])));
                }
                target[i] = (v / step).round() as i64 + origin[i];
            }
        }
        for i in 0..2 {
            let off = (target[i] - start[i]) as f64 * step;
            if off.abs() > self.cfg.xy_limit_um {
                return Err(McError::new("E201", format!("{} {:.0} um from the start point, limit {:.0}", AXES[i], off, self.cfg.xy_limit_um)));
            }
        }
        let dz = (target[2] - cur_steps[2]) as f64 * step;
        if (target[2] - start[2]) as f64 * step > self.cfg.z_range_um || ((start[2] - target[2]) as f64 * step) > self.cfg.z_range_um {
            return Err(McError::new("E201", format!("z {:.0} um from the start point", (target[2] - start[2]) as f64 * step)));
        }
        if dz.abs() > self.cfg.z_max_move_um + 1e-6 {
            return Err(McError::new("E205", format!("{:.0} um, at most {:.0} um at a time", dz, self.cfg.z_max_move_um)));
        }
        let order: Vec<usize> = if dz < 0.0 { vec![2, 0, 1] } else { vec![0, 1, 2] };
        let t_start = now_ns();
        let mut moved = false;
        for ax in order {
            let n = target[ax] - self.shared.lock().unwrap().steps[ax];
            if n != 0 {
                self.axis(ax, n, gen)?;
                moved = true;
            }
        }
        if moved {
            self.sleep(self.cfg.move_pause_ms as f64);
        }
        let mut s = self.shared.lock().unwrap();
        s.moving = false;
        s.settled_ns = now_ns();
        let p = s.pos_um();
        drop(s);
        self.persist(false);
        if let Some(f) = &self.on_move {
            f(p);
        }
        Ok(json!({"pos_um": [round3(p[0]), round3(p[1]), round3(p[2])], "ms": (now_ns() - t_start) as f64 / 1e6}))
    }

    fn axis(&mut self, ax: usize, total: i64, gen: u64) -> McResult<()> {
        let mut left = total;
        let cmd = [C::MOTOR_X, C::MOTOR_Y, C::MOTOR_Z][ax];
        while left != 0 {
            if self.stop_gen.load(Ordering::SeqCst) != gen {
                let mut s = self.shared.lock().unwrap();
                s.moving = false;
                s.settled_ns = now_ns();
                drop(s);
                self.persist(false);
                return Err(McError::new("E203", "stopped"));
            }
            if !self.shared.lock().unwrap().motors {
                self.ctl.cmd(C::TURN_MOTOR, &[1], 1000)?;
                self.shared.lock().unwrap().motors = true;
                info!("STAGE", "Motors on (as the Optosky software before the first move)");
            }
            let n = left.clamp(-self.cfg.chunk_steps, self.cfg.chunk_steps);
            let mut pos = (n > 0) != (ax == 2); // Z: 0xFF = up
            if self.cfg.reverse[ax] {
                pos = !pos;
            }
            let nn = n.unsigned_abs() as u16;
            let data = [if pos { 0x00 } else { 0xFF }, (nn >> 8) as u8, (nn & 0xFF) as u8];
            self.persist(true);
            let from = self.shared.lock().unwrap().pos_um();
            let t_try_end = now_ns() + 5_000_000_000;
            let st = loop {
                self.alive();
                let r = match self.ctl.cmd(cmd, &data, 1000) {
                    Ok(r) => r,
                    Err(e) => {
                        let mut s = self.shared.lock().unwrap();
                        s.uncertain[ax] = true;
                        s.steps[ax] += n;
                        drop(s);
                        self.persist(false);
                        return Err(e);
                    }
                };
                if r.first() == Some(&C::MOTOR_MOVING) && now_ns() < t_try_end {
                    std::thread::sleep(Duration::from_millis(30));
                    continue;
                }
                break r;
            };
            let t0 = now_ns();
            let travel_ms = n.unsigned_abs() as f64 * self.cfg.step_um / self.cfg.speed_um_s * 1e3;
            {
                let mut s = self.shared.lock().unwrap();
                s.steps[ax] += n;
                s.moving = true;
                let to = s.pos_um();
                s.segs.push_back(Seg { t0, t1: t0 + (travel_ms * 1e6) as u64, from, to });
                while s.segs.len() > 4000 {
                    s.segs.pop_front();
                }
            }
            left -= n;
            let code = st.first().copied().unwrap_or(0);
            if code == C::MOTOR_FAIL || code == C::MOTOR_MOVING || code > 4 {
                self.shared.lock().unwrap().uncertain[ax] = true;
                return Err(McError::new("E306", format!("axis {}: status {code}", AXES[ax].to_uppercase())));
            }
            let side = match code {
                C::MOTOR_LIMIT_PLUS => 1,
                C::MOTOR_LIMIT_MINUS => -1,
                _ => 0,
            };
            self.sleep(travel_ms);
            if side != 0 && side == n.signum() {
                self.shared.lock().unwrap().uncertain[ax] = true;
                return Err(McError::new("E202", format!("axis {}, direction {}", AXES[ax].to_uppercase(), if side > 0 { "+" } else { "-" })));
            }
        }
        Ok(())
    }

    pub fn set_zero(&mut self) -> McResult<Value> {
        let mut s = self.shared.lock().unwrap();
        s.origin = s.steps;
        drop(s);
        self.persist(false);
        Ok(self.shared.lock().unwrap().json())
    }

    pub fn confirm(&mut self) -> McResult<Value> {
        self.shared.lock().unwrap().uncertain = [false; 3];
        self.persist(false);
        Ok(self.shared.lock().unwrap().json())
    }

    pub fn light(&mut self, level: u32) -> McResult<Value> {
        if level > 5 {
            return Err(McError::new("E101", "light 0..5"));
        }
        self.ctl.cmd(C::LIGHT, &(level as u16).to_be_bytes(), 1000)?;
        self.shared.lock().unwrap().light = level;
        Ok(json!({"light": level}))
    }

    pub fn laser(&mut self, mw: u32) -> McResult<Value> {
        if mw > 0 {
            if !self.cfg.allow_laser {
                return Err(McError::new("E308", ""));
            }
            let max = self.shared.lock().unwrap().laser_max;
            if max == 0 || mw > max {
                return Err(McError::new("E101", format!("laser {mw} mW, the maximum is {max} mW")));
            }
        }
        self.ctl.cmd(C::LASER, &(mw as u16).to_be_bytes(), 1000)?;
        self.shared.lock().unwrap().laser = mw; // logged by the command log (or by whoever switched it off)
        Ok(json!({"laser_mw": mw}))
    }

    pub fn motors(&mut self, on: bool) -> McResult<Value> {
        self.ctl.cmd(C::TURN_MOTOR, &[on as u8], 1000)?;
        self.shared.lock().unwrap().motors = on;
        Ok(json!({"motors": on}))
    }

    pub fn cooling(&mut self, on: bool) -> McResult<Value> {
        self.ctl.cmd(C::SET_TEC, &C::tec_data(if on { C::COOL_C } else { C::WARM_C }), 1000)?;
        self.shared.lock().unwrap().cooling = on;
        Ok(json!({"cooling": on}))
    }

    pub fn temperature(&mut self) -> McResult<Value> {
        let r = self.ctl.cmd(C::GET_TEC, &[], 1000)?;
        Ok(json!({"raw": r.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")}))
    }

    /// Safe state, as the Optosky software when it closes: laser 0, motors off, light 0, cooling 20 C.
    pub fn safe_off(&mut self, why: &str) {
        let motors = self.shared.lock().unwrap().motors;
        let steps: Vec<(u8, Vec<u8>)> = vec![
            (C::LASER, vec![0, 0]),
            (C::TURN_MOTOR, vec![0]),
            (C::LIGHT, vec![0, 0]),
            (C::SET_TEC, C::tec_data(C::WARM_C).to_vec()),
        ];
        for (c, d) in steps {
            if c == C::TURN_MOTOR && !motors {
                continue;
            }
            if let Err(e) = self.ctl.cmd(c, &d, 1000) {
                error!("STAGE", "{} while switching off: {e}", C::name(c));
            }
        }
        let mut s = self.shared.lock().unwrap();
        s.laser = 0;
        s.light = 0;
        s.motors = false;
        s.cooling = false;
        drop(s);
        self.persist(false);
        info!("STAGE", "Safe state ({why}): laser, motors and light off");
    }

    pub fn laser_off_now(&mut self) {
        if self.shared.lock().unwrap().laser > 0 {
            let _ = self.laser(0);
        }
    }

    fn persist(&self, moving: bool) {
        let s = self.shared.lock().unwrap();
        let v = json!({"steps": s.steps, "origin": s.origin, "moving": moving, "uncertain": s.uncertain.iter().any(|u| *u)});
        drop(s);
        let p = &self.cfg.state_file;
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
    }
}

fn txt(d: &[u8]) -> String {
    if d.iter().all(|b| (32..127).contains(b) || *b == 0) {
        String::from_utf8_lossy(d).trim_matches(|c| c == '\0' || c == ' ').to_string()
    } else {
        d.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }
}

fn load_state(p: &std::path::Path) -> ([i64; 3], [i64; 3], bool) {
    let v: Value = std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let arr = |k: &str| -> [i64; 3] {
        let a = v[k].as_array();
        let g = |i: usize| a.and_then(|a| a.get(i)).and_then(|x| x.as_i64()).unwrap_or(0);
        [g(0), g(1), g(2)]
    };
    let unsure = v["moving"].as_bool().unwrap_or(false) || v["uncertain"].as_bool().unwrap_or(false);
    (arr("steps"), arr("origin"), unsure)
}
