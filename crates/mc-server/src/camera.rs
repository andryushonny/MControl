//! The camera runs in its own process (`mc-server camera-worker`): if the vendor DLL crashes, the server, the stage
//! and the guard keep working and the worker is started again. Frames come over the worker's stdout
//! (same framing as the network), commands go to its stdin as JSON lines.

use crate::mindvision::{format_name, Sdk};
use mc_proto::clock::now_ns;
use mc_proto::errors::{McError, McResult};
use mc_proto::{read_msg, split_frame, write_frame, write_json, K_FRAME, K_JSON};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufWriter, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ============================================================== worker process

enum Dev {
    Mv(crate::mindvision::Cam),
    Sim(Sim),
}

struct Sim {
    w: usize,
    h: usize,
    px_um: f64,
    pos: [f64; 3],
    exp_us: f64,
    gain: i32,
    seq: u64,
    t0: u64,
}

/// `mc-server camera-worker [--sdk DIR] [--sim WxH] [--sim-px UM]`
pub fn worker_main(args: &[String]) -> ! {
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let out = std::io::stdout();
    let mut out = BufWriter::with_capacity(1 << 20, out.lock());
    let dev = if let Some(sz) = arg("--sim") {
        let (w, h) = sz.split_once('x').map(|(a, b)| (a.parse().unwrap_or(1296), b.parse().unwrap_or(972))).unwrap_or((1296, 972));
        let px = arg("--sim-px").and_then(|v| v.parse().ok()).unwrap_or(0.22);
        Ok(Dev::Sim(Sim { w: w & !1, h: h & !1, px_um: px, pos: [0.0; 3], exp_us: 20_000.0, gain: 1, seq: 0, t0: now_ns() }))
    } else {
        Sdk::load(arg("--sdk").as_deref()).and_then(|s| s.open_first()).map(Dev::Mv)
    };
    let dev = match dev {
        Ok(d) => Arc::new(Mutex::new(d)),
        Err(e) => {
            let _ = write_json(&mut out, &json!({"event": "fatal", "error": e.json()}));
            let _ = out.flush();
            std::process::exit(2);
        }
    };
    {
        let d = dev.lock().unwrap();
        let (model, sn) = match &*d {
            Dev::Mv(c) => (c.name.clone(), c.sn.clone()),
            Dev::Sim(s) => (format!("Simulated camera {}x{}", s.w, s.h), "SIM".into()),
        };
        let _ = write_json(&mut out, &json!({"event": "ready", "model": model, "serial": sn}));
        let _ = out.flush();
    }
    // commands from the server
    let (rtx, rrx) = channel::<Value>();
    let dev2 = dev.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let r = worker_cmd(&dev2, &v);
            let mut rep = match r {
                Ok(x) => json!({"ok": true, "result": x}),
                Err(e) => json!({"ok": false, "error": e.json()}),
            };
            rep["id"] = v["id"].clone();
            if rtx.send(rep).is_err() {
                break;
            }
        }
        std::process::exit(0); // server gone
    });
    // frames
    let mut off_min: Option<(u64, i128)> = None; // camera clock -> server clock, lower envelope
    let mut last_err = String::new();
    loop {
        while let Ok(rep) = rrx.try_recv() {
            let _ = write_json(&mut out, &rep);
        }
        let got = {
            let mut d = dev.lock().unwrap();
            match &mut *d {
                Dev::Mv(c) => c.grab(200, |h, px| {
                    let t_got = now_ns();
                    let (fmt, bits) = format_name(h.media_type);
                    (t_got, h.width, h.height, fmt, bits, h.timestamp, h.exp_time_us as f64, h.analog_gain as f64, px.to_vec())
                }),
                Dev::Sim(s) => Ok(sim_frame(s)),
            }
        };
        match got {
            Ok((t_got, w, h, fmt, bits, dev_ts, exp_us, gain, px)) => {
                last_err.clear();
                // the camera's own clock (0.1 ms) fixes when the frame was taken; the smallest (arrival - camera time)
                // of recent frames is the offset (the least delayed arrival). Re-measured every 10 s.
                let cam_ns = dev_ts as i128 * 100_000;
                let d = t_got as i128 - cam_ns;
                off_min = match off_min {
                    Some((t, o)) if t_got - t < 10_000_000_000 && o <= d => Some((t, o)),
                    _ => Some((t_got, d)),
                };
                let mapped = (cam_ns + off_min.unwrap().1).max(0) as u64;
                // no camera clock (simulator) or nonsense: the arrival time is the best we have
                let t_end = if dev_ts == 0 || mapped > t_got || t_got - mapped > 1_000_000_000 { t_got } else { mapped };
                let t_start = t_end.saturating_sub((exp_us * 1000.0) as u64);
                let hdr = json!({"w": w, "h": h, "fmt": fmt, "bits": bits, "exp_us": exp_us, "gain": gain,
                                 "t_exp_start": t_start, "t_exp_end": t_end, "t_got": t_got, "cam_ts": dev_ts});
                let hb = serde_json::to_vec(&hdr).unwrap();
                if write_frame(&mut out, &hb, &px).is_err() || out.flush().is_err() {
                    std::process::exit(0);
                }
            }
            Err(e) => {
                let s = e.to_string();
                if s != last_err {
                    let _ = write_json(&mut out, &json!({"event": "error", "error": e.json()}));
                    let _ = out.flush();
                    last_err = s;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn worker_cmd(dev: &Arc<Mutex<Dev>>, v: &Value) -> McResult<Value> {
    let mut d = dev.lock().unwrap();
    let cmd = v["cmd"].as_str().unwrap_or("");
    match &mut *d {
        Dev::Mv(c) => match cmd {
            "exposure" => {
                let ms = v["ms"].as_f64().ok_or_else(|| McError::new("E101", "ms"))?;
                c.set_auto_exposure(false)?;
                c.set_exposure_us(ms * 1000.0)?;
                Ok(json!({"exposure_ms": c.exposure_us() / 1000.0}))
            }
            "gain" => {
                c.set_gain(v["value"].as_i64().unwrap_or(1) as i32)?;
                Ok(json!({"gain": c.gain()}))
            }
            "auto_exposure" => {
                c.set_auto_exposure(v["on"].as_bool().unwrap_or(false))?;
                Ok(json!({}))
            }
            "white_balance" => {
                c.white_balance_once()?;
                Ok(json!({}))
            }
            "state" => Ok(json!({"exposure_ms": c.exposure_us() / 1000.0, "gain": c.gain(), "model": c.name})),
            _ => Ok(json!({})),
        },
        Dev::Sim(s) => match cmd {
            "exposure" => {
                s.exp_us = v["ms"].as_f64().unwrap_or(20.0) * 1000.0;
                Ok(json!({"exposure_ms": s.exp_us / 1000.0}))
            }
            "gain" => {
                s.gain = v["value"].as_i64().unwrap_or(1) as i32;
                Ok(json!({"gain": s.gain}))
            }
            "sim_pos" => {
                if let Some(a) = v["xyz"].as_array() {
                    for i in 0..3 {
                        s.pos[i] = a.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0);
                    }
                }
                Ok(json!({}))
            }
            "state" => Ok(json!({"exposure_ms": s.exp_us / 1000.0, "gain": s.gain, "model": "sim"})),
            _ => Ok(json!({})),
        },
    }
}

type Raw = (u64, i32, i32, String, u32, u32, f64, f64, Vec<u8>);

/// A procedural sample: lithographic crosses every 100 um plus specks, Bayer RG, 8 bit.
fn sim_frame(s: &mut Sim) -> Raw {
    let period = ((s.exp_us / 1000.0).max(1.0) as u64).max(40) * 1_000_000; // at most 25 fps
    let next = s.t0 + (s.seq + 1) * period;
    let now = now_ns();
    if next > now {
        std::thread::sleep(Duration::from_nanos(next - now));
    }
    s.seq += 1;
    let t_end = now_ns();
    let (w, h) = (s.w, s.h);
    let mut px = vec![0u8; w * h];
    let (cx, cy) = (s.pos[0], s.pos[1]);
    let blur = (s.pos[2].abs() / 20.0).min(1.0); // out of focus: lower contrast
    for j in 0..h {
        let wy = cy + (j as f64 - h as f64 / 2.0) * s.px_um;
        let fy = wy.rem_euclid(100.0);
        for i in 0..w {
            let wx = cx + (i as f64 - w as f64 / 2.0) * s.px_um;
            let fx = wx.rem_euclid(100.0);
            let mut v = 200.0;
            let near = |f: f64| f < 1.0 || f > 99.0;
            let arm = |f: f64| f < 8.0 || f > 92.0;
            if (near(fx) && arm(fy)) || (near(fy) && arm(fx)) {
                v = 60.0;
            }
            let hsh = ((wx * 0.7).floor() as i64).wrapping_mul(73856093) ^ ((wy * 0.7).floor() as i64).wrapping_mul(19349663);
            if hsh.rem_euclid(997) < 4 {
                v = 110.0;
            }
            v = 200.0 + (v - 200.0) * (1.0 - blur);
            // Bayer RG: R G / G B tints
            let c = match (j & 1, i & 1) {
                (0, 0) => 0.9,
                (1, 1) => 1.05,
                _ => 1.0,
            };
            px[j * w + i] = (v * c).clamp(0.0, 255.0) as u8;
        }
    }
    (t_end, w as i32, h as i32, "bayer_rg8".into(), 8, 0, s.exp_us, s.gain as f64, px)
}

// ============================================================== server side

pub struct CamFrame {
    pub hdr: Value,
    pub px: Vec<u8>,
}

pub struct Camera {
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    pending: Mutex<HashMap<u64, Sender<Value>>>,
    next: AtomicU64,
    pub ok: AtomicBool,
    pub info: Mutex<Value>,
    pub frames: AtomicU64,
    closing: AtomicBool,
}

impl Camera {
    /// Start the worker and keep it running; `on_frame` gets every frame.
    pub fn start(worker_args: Vec<String>, on_frame: Arc<dyn Fn(CamFrame) + Send + Sync>) -> Arc<Camera> {
        let cam = Arc::new(Camera {
            stdin: Mutex::new(None),
            child: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            ok: AtomicBool::new(false),
            info: Mutex::new(json!({})),
            frames: AtomicU64::new(0),
            closing: AtomicBool::new(false),
        });
        let c2 = cam.clone();
        std::thread::Builder::new()
            .name("camera".into())
            .spawn(move || c2.supervise(worker_args, on_frame))
            .expect("camera thread");
        cam
    }

    fn supervise(self: Arc<Self>, args: Vec<String>, on_frame: Arc<dyn Fn(CamFrame) + Send + Sync>) {
        let exe = std::env::current_exe().expect("exe");
        let mut fails = 0u32;
        let mut last_fatal = String::new();
        while !self.closing.load(Ordering::SeqCst) {
            let mut cmd = Command::new(&exe);
            cmd.arg("camera-worker").args(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    error!("CAM", "Cannot start the camera worker: {e}");
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };
            let mut out = child.stdout.take().unwrap();
            *self.stdin.lock().unwrap() = child.stdin.take();
            *self.child.lock().unwrap() = Some(child);
            let mut buf = Vec::with_capacity(16 << 20);
            let mut fatal = false;
            loop {
                let kind = match read_msg(&mut out, &mut buf) {
                    Ok(k) => k,
                    Err(_) => break,
                };
                if kind == K_FRAME {
                    if let Ok((hdr, px)) = split_frame(&buf) {
                        self.frames.fetch_add(1, Ordering::Relaxed);
                        fails = 0;
                        if !self.ok.swap(true, Ordering::SeqCst) {
                            let mut i = self.info.lock().unwrap();
                            i["w"] = hdr["w"].clone();
                            i["h"] = hdr["h"].clone();
                            i["fmt"] = hdr["fmt"].clone();
                            info!("CAM", "Streaming {}x{} {}", hdr["w"], hdr["h"], hdr["fmt"].as_str().unwrap_or("?"));
                        }
                        on_frame(CamFrame { hdr, px: px.to_vec() });
                    }
                } else if kind == K_JSON {
                    let v: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
                    match v["event"].as_str() {
                        Some("ready") => {
                            let mut i = self.info.lock().unwrap();
                            i["model"] = v["model"].clone();
                            i["serial"] = v["serial"].clone();
                            info!("CAM", "Camera {} (serial {}) opened in its own process", v["model"].as_str().unwrap_or("?"), v["serial"].as_str().unwrap_or("?"));
                            last_fatal.clear();
                        }
                        Some("fatal") => {
                            let m = v["error"]["msg"].as_str().unwrap_or("?").to_string();
                            if m != last_fatal {
                                error!("CAM", "{} {}", v["error"]["code"].as_str().unwrap_or(""), m);
                                last_fatal = m;
                            }
                            fatal = true;
                        }
                        Some("error") => warn!("CAM", "{} {}", v["error"]["code"].as_str().unwrap_or(""), v["error"]["msg"].as_str().unwrap_or("")),
                        _ => {
                            if let Some(id) = v["id"].as_u64() {
                                if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
                                    let _ = tx.send(v);
                                }
                            }
                        }
                    }
                }
            }
            self.ok.store(false, Ordering::SeqCst);
            *self.stdin.lock().unwrap() = None;
            if let Some(mut c) = self.child.lock().unwrap().take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            if self.closing.load(Ordering::SeqCst) {
                break;
            }
            fails += 1;
            if !fatal {
                error!("CAM", "E506 Camera worker stopped; starting it again");
            }
            std::thread::sleep(Duration::from_secs(if fatal { 10 } else { (fails as u64).min(5) }));
        }
    }

    /// Send a command to the worker, wait for its reply.
    pub fn cmd(&self, mut v: Value) -> McResult<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        v["id"] = json!(id);
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        {
            let mut g = self.stdin.lock().unwrap();
            let Some(s) = g.as_mut() else {
                self.pending.lock().unwrap().remove(&id);
                return Err(McError::new("E500", ""));
            };
            let line = format!("{}\n", v);
            if s.write_all(line.as_bytes()).and_then(|_| s.flush()).is_err() {
                self.pending.lock().unwrap().remove(&id);
                return Err(McError::new("E506", ""));
            }
        }
        let r = rx.recv_timeout(Duration::from_secs(3)).map_err(|_| {
            self.pending.lock().unwrap().remove(&id);
            McError::new("E505", "the camera did not answer")
        })?;
        if r["ok"].as_bool() == Some(true) {
            Ok(r["result"].clone())
        } else {
            let code = r["error"]["code"].as_str().unwrap_or("E503");
            let code: &'static str = mc_proto::errors::TABLE.iter().find(|t| t.0 == code).map(|t| t.0).unwrap_or("E503");
            Err(McError::new(code, r["error"]["msg"].as_str().unwrap_or("").to_string()))
        }
    }

    /// Fire and forget (simulator position updates).
    pub fn tell(&self, mut v: Value) {
        v["id"] = json!(0);
        if let Some(s) = self.stdin.lock().unwrap().as_mut() {
            let _ = s.write_all(format!("{}\n", v).as_bytes()).and_then(|_| s.flush());
        }
    }

    pub fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        *self.stdin.lock().unwrap() = None;
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
