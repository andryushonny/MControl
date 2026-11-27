//! The link to mc-server, run off the UI thread: connecting (and again after a drop), commands, events, frames.
//! The UI only reads `State` and sends commands; nothing here ever blocks a frame of the window.

use crate::pixels::{self, Rgb};
use eframe::egui;
use mc_client::Client;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A full frame as it came: header + packed payload (unpacked only when someone needs the pixels).
pub struct Packed {
    pub meta: Value,
    pub data: Vec<u8>,
}

impl Packed {
    pub fn seq(&self) -> u64 {
        self.meta["seq"].as_u64().unwrap_or(0)
    }
    pub fn xyz(&self) -> [f64; 3] {
        let a = &self.meta["xyz_um"];
        [a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a[2].as_f64().unwrap_or(0.0)]
    }
}

/// The newest live frame, small, for the screen.
pub struct Live {
    pub seq: u64,
    pub rgb: Arc<Rgb>,
    /// full-resolution pixels per pixel of `rgb`
    pub div: f64,
    pub xyz: [f64; 3],
    pub moving: bool,
    pub at: Instant,
    /// the camera's exposure (ms) and gain when this frame was taken
    pub exposure_ms: f64,
    pub gain: f64,
}

#[derive(Default)]
pub struct State {
    pub connected: bool,
    pub server: String,
    pub address: String,
    pub error: String,
    /// last telemetry: stage, camera, operator, clients, queue
    pub telemetry: Value,
    /// one line per event, newest last
    pub log: VecDeque<String>,
    pub live: Option<Live>,
    /// how many live frames to decode per second (the rest are skipped)
    pub live_fps: f64,
    pub fps_in: f64,
    pub frames_in: u64,
    /// how live pictures are made to look: the optics (distortion taken out) and the white balance
    pub look: Option<(crate::geom::Optics, [f64; 3])>,
    /// a command from the UI that failed, to show where the operator looks
    pub alert: Option<String>,
}

pub struct Link {
    pub state: Mutex<State>,
    client: Mutex<Option<Arc<Client>>>,
    /// recent full frames by number, for snaps: whoever asked waits on `arrived`
    recent: Mutex<VecDeque<Arc<Packed>>>,
    arrived: Condvar,
    ctx: egui::Context,
    want_live: Mutex<bool>,
}

impl Link {
    /// Start connecting to `address` ("auto" = discovery) as operator `name`; keeps reconnecting.
    pub fn start(ctx: egui::Context, address: String, name: String) -> Arc<Link> {
        let l = Arc::new(Link {
            state: Mutex::new(State { live_fps: 8.0, address: address.clone(), ..Default::default() }),
            client: Mutex::new(None),
            recent: Mutex::new(VecDeque::new()),
            arrived: Condvar::new(),
            ctx,
            want_live: Mutex::new(true),
        });
        let l2 = l.clone();
        std::thread::Builder::new().name("link".into()).spawn(move || l2.run(address, name)).unwrap();
        l
    }

    fn note(&self, line: String) {
        let mut s = self.state.lock().unwrap();
        s.log.push_back(line);
        while s.log.len() > 500 {
            s.log.pop_front();
        }
        drop(s);
        self.ctx.request_repaint();
    }

    fn run(self: Arc<Self>, address: String, name: String) {
        loop {
            match Client::connect(&address, "operator", &name) {
                Ok(c) => {
                    {
                        let mut s = self.state.lock().unwrap();
                        s.connected = true;
                        s.error.clear();
                        s.server = format!("{} ({})", c.server["name"].as_str().unwrap_or("?"), c.addr);
                    }
                    *self.client.lock().unwrap() = Some(c.clone());
                    self.note(format!("connected to {}", c.addr));
                    let live = *self.want_live.lock().unwrap();
                    let _ = c.cmd(json!({"cmd": "stream", "on": live}));
                    let me = self.clone();
                    let c2 = c.clone();
                    let data = std::thread::Builder::new().name("frames".into()).spawn(move || me.frames(c2)).unwrap();
                    self.events(&c);
                    let _ = data.join();
                    *self.client.lock().unwrap() = None;
                    let mut s = self.state.lock().unwrap();
                    s.connected = false;
                    s.error = "connection lost: reconnecting".into();
                }
                Err(e) => {
                    let mut s = self.state.lock().unwrap();
                    s.connected = false;
                    s.error = e.to_string();
                }
            }
            self.ctx.request_repaint();
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    fn events(&self, c: &Arc<Client>) {
        loop {
            let ev = c.events.lock().unwrap().recv_timeout(Duration::from_millis(500));
            match ev {
                Ok(ev) => {
                    let kind = ev["event"].as_str().unwrap_or("");
                    match kind {
                        "closed" => return,
                        "telemetry" => {
                            self.state.lock().unwrap().telemetry = ev;
                            self.ctx.request_repaint();
                        }
                        _ => {
                            if let Some(line) = describe(&ev) {
                                self.note(line);
                            }
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return,
            }
        }
    }

    fn frames(&self, c: Arc<Client>) {
        let Ok(mut d) = c.data_with(json!({"want": "raw", "codec": "auto"})) else { return };
        let mut last_decode = Instant::now() - Duration::from_secs(1);
        let mut count = (Instant::now(), 0u64);
        while let Ok((meta, px)) = d.next() {
            let f = Arc::new(Packed { meta, data: px.to_vec() });
            {
                let mut r = self.recent.lock().unwrap();
                r.push_back(f.clone());
                while r.len() > 24 {
                    r.pop_front();
                }
            }
            self.arrived.notify_all();
            count.1 += 1;
            if count.0.elapsed() > Duration::from_secs(1) {
                let mut s = self.state.lock().unwrap();
                s.fps_in = count.1 as f64 / count.0.elapsed().as_secs_f64();
                s.frames_in += count.1;
                count = (Instant::now(), 0);
            }
            // the screen gets a small picture, a few times a second
            let fps = self.state.lock().unwrap().live_fps.max(0.5);
            if last_decode.elapsed().as_secs_f64() >= 1.0 / fps {
                last_decode = Instant::now();
                if let Ok(sensor) = pixels::unpack(&f.meta, &f.data) {
                    let div = 2;
                    let mut rgb = sensor.rgb(div);
                    if let Some((o, wb)) = self.state.lock().unwrap().look.clone() {
                        rgb = rgb.undistorted(&o, 2.0 * div as f64);
                        rgb.balance(wb);
                    }
                    let rgb = Arc::new(rgb);
                    let xyz = f.xyz();
                    self.state.lock().unwrap().live = Some(Live {
                        seq: f.seq(),
                        rgb,
                        div: 2.0 * div as f64,
                        xyz,
                        moving: f.meta["moving"].as_bool().unwrap_or(false),
                        at: Instant::now(),
                        exposure_ms: f.meta["exp_us"].as_f64().unwrap_or(0.0) / 1000.0,
                        gain: f.meta["gain"].as_f64().unwrap_or(0.0),
                    });
                    self.ctx.request_repaint();
                }
            }
        }
    }

    pub fn client(&self) -> Option<Arc<Client>> {
        self.client.lock().unwrap().clone()
    }

    /// A command, waiting for the reply (call from a worker thread, not the UI).
    pub fn call(&self, v: Value) -> Result<Value, String> {
        let c = self.client().ok_or("not connected")?;
        c.cmd(v).map_err(|e| e.to_string())
    }

    /// A command from the UI: runs on its own thread; errors go to the log.
    pub fn send(self: &Arc<Self>, v: Value) {
        let me = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = me.call(v.clone()) {
                me.state.lock().unwrap().alert = Some(format!("{}: {e}", v["cmd"].as_str().unwrap_or("?")));
                me.note(format!("{}: {e}", v["cmd"].as_str().unwrap_or("?")));
            }
        });
    }

    /// The full frame with this number, as soon as it arrives (snaps reply with their number).
    pub fn frame(&self, seq: u64, timeout: Duration) -> Result<Arc<Packed>, String> {
        let end = Instant::now() + timeout;
        let mut r = self.recent.lock().unwrap();
        loop {
            if let Some(f) = r.iter().find(|f| f.seq() == seq) {
                return Ok(f.clone());
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("frame {seq} did not arrive"));
            }
            r = self.arrived.wait_timeout(r, left).unwrap().0;
        }
    }

    /// Move, settle, take a fresh still frame: (frame, reply).
    pub fn snap(&self, args: Value) -> Result<Arc<Packed>, String> {
        let mut v = args;
        v["cmd"] = json!("snap");
        let r = self.call(v)?;
        self.frame(r["seq"].as_u64().unwrap_or(0), Duration::from_secs(20))
    }

    pub fn set_live(self: &Arc<Self>, on: bool) {
        *self.want_live.lock().unwrap() = on;
        self.send(json!({"cmd": "stream", "on": on}));
    }

    /// Light level (0..5), laser mW and the laser's maximum, as the controller reports them.
    pub fn lamps(&self) -> (u32, u32, u32) {
        let s = self.state.lock().unwrap();
        let st = &s.telemetry["stage"];
        let n = |k: &str| st[k].as_u64().unwrap_or(0) as u32;
        (n("light"), n("laser_mw"), n("laser_max_mw"))
    }

    pub fn stage_pos(&self) -> Option<[f64; 3]> {
        let s = self.state.lock().unwrap();
        let p = &s.telemetry["stage"]["pos_um"];
        Some([p[0].as_f64()?, p[1].as_f64()?, p[2].as_f64()?])
    }
}

/// One readable line per event (the same shapes as the Python client's describe()).
pub fn describe(ev: &Value) -> Option<String> {
    let args = |a: &Value| {
        a.as_object().map(|o| o.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")).unwrap_or_default()
    };
    let n = ev["n"].as_u64().map(|n| format!("#{n} ")).unwrap_or_default();
    match ev["event"].as_str()? {
        "command" => {
            let head = format!("{n}{}: {} {}", ev["by"].as_str().unwrap_or("?"), ev["cmd"].as_str().unwrap_or("?"), args(&ev["args"]));
            match ev["state"].as_str()? {
                "queued" | "started" => None, // the result line says it all
                "failed" => Some(format!("{head}  FAILED {} {}", ev["error"]["code"].as_str().unwrap_or(""), ev["error"]["msg"].as_str().unwrap_or(""))),
                _ => Some(format!("{head}  {}  ({} ms)", ev["summary"].as_str().filter(|s| !s.is_empty()).unwrap_or("ok"), ev["ms"])),
            }
        }
        "client" => Some(format!("{} {} {} {}", ev["role"].as_str()?, ev["name"].as_str()?, ev["addr"].as_str()?, ev["action"].as_str()?)),
        "operator" => Some(format!("control: {}", ev["name"].as_str().unwrap_or("free"))),
        "safe" => Some(format!("SAFE STATE: {}", ev["why"].as_str().unwrap_or(""))),
        "error" => Some(format!("ERROR {} {}", ev["error"]["code"].as_str().unwrap_or(""), ev["error"]["msg"].as_str().unwrap_or(""))),
        "message" => Some(format!("{n}message {} -> {}: {}", ev["from"].as_str().unwrap_or("?"), ev["to"].as_str().unwrap_or("?"), ev["data"])),
        _ => None,
    }
}

#[allow(dead_code)]
pub type Frames = HashMap<u64, Arc<Packed>>;
