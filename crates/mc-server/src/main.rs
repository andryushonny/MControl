#![allow(dead_code)]
//! mc-server — run it on the PC wired to the microscope: `mc-server.exe` (or `mc-server.exe --sim` to try it without one).

#[macro_use]
mod log;
mod access;
mod camera;
mod codec;
mod controller;
mod frames;
mod guard;
mod mindvision;
mod net;
mod stage;

use log::{paint, BOLD, CYAN, DIM, GREEN, YELLOW};
use mc_proto::clock::now_ns;
use serde_json::json;
use std::net::UdpSocket;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const HELP: &str = "mc-server — MControl server for the Optosky ATR8300 microscope and its MindVision camera

USAGE: mc-server [options]

  --port COM3       controller port (default: auto, the USB serial port that answers)
  --sim             simulated stage and camera (no hardware)
  --sim-size WxH    simulated camera size (default 1296x972)
  --name NAME       name shown to clients (default: computer name)
NETWORK (local network only unless told otherwise)
  --bind IP         listen on this address only, e.g. the PC's own LAN or VPN address (default: all)
  --control-port N  control TCP port (default 5160; if busy, the next free of the 9 after it)
  --data-port N     data TCP port (default 5161; same rule)
  --discovery-port N  UDP port for discovery (default 5159)
  --no-discovery    do not answer discovery: clients must give the address
  --allow LIST      who may connect: local, any, addresses, ranges (default: local =
                    loopback, 10/8, 172.16/12, 192.168/16, 169.254/16, 100.64/10, IPv6 local)
  --operators LIST  who may take control (default: same as --allow); others are viewers
  --token TEXT      shared secret clients must give (or env MC_TOKEN); required when --allow
                    goes beyond local networks

OTHER
  --sdk DIR         folder of MVCAMSDK_X64.dll (default: the MindVision install folder)
  --no-camera       stage only
  --allow-laser     allow switching the laser on (off by default)
  --speed UM_S      real stage speed, um/s (default 3000)
  --ring N          frames kept for re-fetch (default 20)
  --dead-ms N       operator silent this long: stop stage, laser off (default 3000)
  --viewer-fps F    preview frames per second for viewers (default 25, at most 30)
  --max-viewers N   viewers at the same time (default 5); the operator does not count
  --no-guard        do not start the guard process
  --log FILE        also write the log to FILE
  --no-color        plain log
  --debug           more log lines
  --config FILE     options from a file (default: mc-server.conf next to the exe, if it exists),
                    one per line: `bind = 192.168.1.10`, `allow-laser = true`; the command line wins

FRAMES (the client chooses: Microscope(codec=...))
  auto: snaps exact (lossless), live frames as good as the link carries: lossless, then JPEG 92 / 85 / 75
  raw | lossless (~2:1, exact) | jpeg (~5-10:1, error about the sensor noise)

COMMANDS
  mc-server errors       all error codes as a table
  mc-server codec-bench  how fast and how small each codec is on this PC
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("camera-worker") => camera::worker_main(&args[1..]),
        Some("guard") => guard::guard_main(&args[1..]),
        Some("-h") | Some("--help") | Some("help") => {
            print!("{HELP}");
            return;
        }
        Some("codec-bench") => {
            // mc-server codec-bench [FILE W H]: how fast and how small each codec is on this PC
            // (FILE: 8-bit Bayer, e.g. a frame saved with Frame.save("x.npy"); without it a synthetic 2592x1944 frame)
            let (w, h) = (args.get(2).and_then(|v| v.parse().ok()).unwrap_or(2592usize), args.get(3).and_then(|v| v.parse().ok()).unwrap_or(1944usize));
            let px = match args.get(1) {
                Some(f) => std::fs::read(f).expect("cannot read the file"),
                None => {
                    let mut r = 0x2545_F491_4F6C_DD1Du64;
                    (0..w * h)
                        .map(|i| {
                            let (x, y) = (i % w, i / w);
                            r ^= r << 13;
                            r ^= r >> 7;
                            r ^= r << 17;
                            let n = ((r & 7) + ((r >> 3) & 7)) as f64 / 2.0 - 3.5; // sensor noise, about +-2 DN
                            (120.0 + 60.0 * ((x as f64 / 300.0).sin() * (y as f64 / 200.0).cos()) + n) as u8
                        })
                        .collect()
                }
            };
            let px = px[px.len().saturating_sub(w * h)..].to_vec(); // .npy: skip its header
            for (name, run) in [("lossless", 0u8), ("jpeg 92", 92), ("jpeg 85", 85), ("jpeg 75", 75)] {
                let t = std::time::Instant::now();
                let mut n = 0;
                for _ in 0..10 {
                    n = if run == 0 { codec::lossless(&px, w, h, 1).len() } else { codec::jpeg_planes(&px, w, h, run).len() };
                }
                let ms = t.elapsed().as_secs_f64() * 100.0;
                println!("{name:9} {:6.1} ms/frame  {:5.2} MB  {:4.1} : 1", ms, n as f64 / 1e6, px.len() as f64 / n as f64);
            }
            return;
        }
        Some("errors") => {
            print!("{}", mc_proto::errors::markdown());
            return;
        }
        Some("--version") => {
            println!("mc-server {}", mc_proto::VERSION);
            return;
        }
        _ => {}
    }
    let args = with_config(args);
    let has = |k: &str| args.iter().any(|a| a == k);
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    log::init(!has("--no-color"), has("--debug"), arg("--log").as_deref());
    let sim = has("--sim");
    let name = arg("--name").unwrap_or_else(hostname);

    println!();
    println!("  {}  {}", paint(BOLD, &format!("MControl server {}", mc_proto::VERSION)), paint(DIM, "· Optosky ATR8300 + MindVision camera"));
    println!();

    // ---- network first: a wrong address or a busy port stops us before any hardware is touched
    let fail = |e: mc_proto::errors::McError| -> ! {
        error!("NET", "{e}");
        std::process::exit(2)
    };
    let bind: std::net::IpAddr = match arg("--bind").as_deref().unwrap_or("0.0.0.0").parse() {
        Ok(ip) => ip,
        Err(_) => fail(mc_proto::errors::McError::new("E101", format!("--bind {}", arg("--bind").unwrap_or_default()))),
    };
    let allow = arg("--allow").unwrap_or_else(|| "local".into());
    let operators = arg("--operators").unwrap_or_else(|| allow.clone());
    let token = arg("--token").or_else(|| std::env::var("MC_TOKEN").ok());
    let access = match access::Access::new(&allow, &operators, token) {
        Ok(a) => a,
        Err(e) if e == "E121" => fail(mc_proto::errors::McError::new("E121", format!("--allow {allow}"))),
        Err(e) => fail(mc_proto::errors::McError::new("E101", e)),
    };
    let (cl, control_port) = net::listen(bind, arg("--control-port").and_then(|v| v.parse().ok()).unwrap_or(mc_proto::CONTROL_PORT)).unwrap_or_else(|e| fail(e));
    let (dl, data_port) = net::listen(bind, arg("--data-port").and_then(|v| v.parse().ok()).unwrap_or(mc_proto::DATA_PORT)).unwrap_or_else(|e| fail(e));
    let discovery_port = if has("--no-discovery") { None } else { Some(arg("--discovery-port").and_then(|v| v.parse().ok()).unwrap_or(mc_proto::DISCOVERY_PORT)) };

    // ---- controller
    let mut cfg = stage::StageCfg::default();
    cfg.allow_laser = has("--allow-laser");
    if let Some(s) = arg("--speed").and_then(|v| v.parse().ok()) {
        cfg.speed_um_s = s;
    }
    if sim {
        cfg.move_pause_ms = 0;
        cfg.state_file = "mc-server-sim-state.json".into();
    }
    let port = if sim { "sim".to_string() } else { arg("--port").unwrap_or_else(|| "auto".into()) };
    let st = if port == "auto" {
        let ports = controller::Controller::ports();
        if ports.is_empty() {
            error!("STAGE", "E300 no USB serial port found: plug in the controller, or give --port COMx (try without hardware: --sim)");
            std::process::exit(1);
        }
        let mut found = None;
        for p in &ports {
            info!("STAGE", "Trying {p}");
            match stage::Stage::open(p, cfg.clone()) {
                Ok(s) => {
                    found = Some(s);
                    break;
                }
                Err(e) => warn!("STAGE", "{p}: {e}"),
            }
        }
        match found {
            Some(s) => s,
            None => {
                error!("STAGE", "E301 no controller answered on {}. Close the Optosky software; replug the USB", ports.join(", "));
                std::process::exit(1);
            }
        }
    } else {
        match stage::Stage::open(&port, cfg.clone()) {
            Ok(s) => s,
            Err(e) => {
                error!("STAGE", "{e}");
                std::process::exit(1);
            }
        }
    };

    // ---- camera (own process)
    let hub = frames::Hub::new(arg("--ring").and_then(|v| v.parse().ok()).unwrap_or(20));
    let cam_slot: Arc<Mutex<Option<Arc<camera::Camera>>>> = Arc::new(Mutex::new(None));
    let mut wargs = Vec::new();
    if sim {
        wargs.push("--sim".to_string());
        wargs.push(arg("--sim-size").unwrap_or_else(|| "1296x972".into()));
    }
    if let Some(d) = arg("--sdk") {
        wargs.push("--sdk".into());
        wargs.push(d);
    }
    let (h2, s2) = (hub.clone(), st.clone());
    let on_frame: Arc<dyn Fn(camera::CamFrame) + Send + Sync> = Arc::new(move |cf: camera::CamFrame| {
        let f = net::make_frame(&h2, &s2, cf.hdr, cf.px);
        h2.publish(Arc::new(f));
    });
    let cam = if has("--no-camera") {
        camera::Camera::start(vec!["--sim".into(), "2x2".into()], Arc::new(|_| {}))
    } else {
        camera::Camera::start(wargs, on_frame)
    };
    *cam_slot.lock().unwrap() = Some(cam.clone());
    if sim {
        let c2 = cam.clone();
        let _ = st.call(move |w| {
            w.set_on_move(Box::new(move |p| c2.tell(json!({"cmd": "sim_pos", "xyz": p}))));
            Ok(serde_json::Value::Null)
        });
    }

    // ---- guard (own process)
    let hb_port = free_udp_port();
    if !has("--no-guard") {
        let exe = std::env::current_exe().expect("exe");
        let mut g = std::process::Command::new(exe);
        g.args(["guard", "--pid", &std::process::id().to_string(), "--hb-port", &hb_port.to_string(), "--port", &st.port]);
        if has("--no-color") {
            g.arg("--no-color");
        }
        if let Some(l) = arg("--log") {
            g.args(["--log", &l]);
        }
        match g.spawn() {
            Ok(_) => {}
            Err(e) => warn!("GUARD", "could not start the guard: {e}"),
        }
        let tick = st.tick.clone();
        let s = UdpSocket::bind("127.0.0.1:0").expect("udp");
        // no clients before the guard is watching
        let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
        let mut up = false;
        for _ in 0..50 {
            let _ = s.send_to(b"hb", ("127.0.0.1", hb_port));
            let mut b = [0u8; 4];
            if s.recv(&mut b).is_ok() {
                up = true;
                break;
            }
        }
        if !up {
            warn!("GUARD", "the guard did not answer in 5 s: running without it");
        }
        std::thread::Builder::new()
            .name("heartbeat".into())
            .spawn(move || {
                loop {
                    // only while the stage thread is alive: a hung stage thread = no heartbeat = the guard acts
                    if now_ns().saturating_sub(tick.load(Ordering::Relaxed)) < 2_000_000_000 {
                        let _ = s.send_to(b"hb", ("127.0.0.1", hb_port));
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .ok();
    }

    // ---- network
    let ncfg = net::Cfg {
        name: name.clone(),
        control_port,
        data_port,
        discovery_port,
        bind,
        dead_ms: arg("--dead-ms").and_then(|v| v.parse().ok()).unwrap_or(3000),
        viewer_fps: arg("--viewer-fps").and_then(|v| v.parse().ok()).unwrap_or(25.0),
        max_viewers: arg("--max-viewers").and_then(|v| v.parse().ok()).unwrap_or(5),
        access,
    };
    let ip = if bind.is_unspecified() { primary_ip() } else { bind.to_string() };
    let row = |k: &str, v: String, note: &str| println!("  {}  {}  {}", paint(DIM, &format!("{k:<10}")), v, paint(DIM, note));
    let a = &ncfg.access;
    row("Name", paint(BOLD, &name), "");
    row("Control", paint(CYAN, &format!("tcp://{ip}:{control_port}")), "Commands, replies, events");
    row("Data", paint(CYAN, &format!("tcp://{ip}:{data_port}")), "Frames");
    match discovery_port {
        Some(p) => row("Discovery", paint(CYAN, &format!("udp {p}")), "Clients find the server by themselves"),
        None => row("Discovery", "Off".into(), "Clients give the address"),
    }
    let scope = if a.is_open() { paint(YELLOW, &a.allow_text) } else { a.allow_text.clone() };
    row("Access", scope, if a.needs_token() { "Token required" } else { "No token" });
    row("Viewers", format!("Up to {}", ncfg.max_viewers), &format!("Previews at {} fps", ncfg.viewer_fps));
    if a.operators_text != a.allow_text {
        row("Operators", a.operators_text.clone(), "Others connect as viewers");
    }
    row("Laser", if cfg.allow_laser { paint(YELLOW, "Allowed") } else { "Blocked".into() }, if cfg.allow_laser { "" } else { "--allow-laser to allow" });
    row("Stop", paint(GREEN, "Ctrl+C"), "Stage stops; laser, motors and light off");
    println!();
    let srv = net::Server::new(st.clone(), cam.clone(), hub, ncfg);

    // ---- Ctrl+C: safe state, tell the guard, leave
    let (st2, cam2) = (st.clone(), cam.clone());
    let _ = ctrlc::set_handler(move || {
        st2.stop();
        let _ = st2.call(|w| {
            w.safe_off("server closing");
            Ok(serde_json::Value::Null)
        });
        cam2.close();
        if let Ok(s) = UdpSocket::bind("127.0.0.1:0") {
            let _ = s.send_to(b"bye", ("127.0.0.1", hb_port));
        }
        info!("SRV", "Stopped");
        std::process::exit(0);
    });

    if let Err(e) = srv.run(cl, dl) {
        error!("NET", "E120 {e}");
        st.stop();
        let _ = st.call(|w| {
            w.safe_off("server cannot start");
            Ok(serde_json::Value::Null)
        });
        if let Ok(s) = UdpSocket::bind("127.0.0.1:0") {
            let _ = s.send_to(b"bye", ("127.0.0.1", hb_port));
        }
        cam.close();
        std::process::exit(1);
    }
}

/// Options from `--config FILE` or mc-server.conf next to the exe, after the command line (which wins).
fn with_config(mut args: Vec<String>) -> Vec<String> {
    let given = args.iter().position(|a| a == "--config").and_then(|i| args.get(i + 1)).cloned();
    let path = given.clone().or_else(|| {
        std::env::current_exe().ok().map(|p| p.with_file_name("mc-server.conf").to_string_lossy().into_owned())
    });
    let Some(text) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) else {
        if let Some(p) = given {
            eprintln!("mc-server: cannot read --config {p}");
            std::process::exit(2);
        }
        return args;
    };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (k, v) = line.split_once('=').map(|(k, v)| (k.trim(), v.trim())).unwrap_or((line, "true"));
        let k = format!("--{}", k.trim_start_matches('-'));
        match v {
            "true" | "yes" | "on" => args.push(k),
            "false" | "no" | "off" => {}
            _ => {
                args.push(k);
                args.push(v.trim_matches('"').to_string());
            }
        }
    }
    args
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0").and_then(|s| s.local_addr()).map(|a| a.port()).unwrap_or(5158)
}

/// The address other PCs reach this one at (no packet is sent).
fn primary_ip() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("10.255.255.255:1")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()))
        .unwrap_or_else(|| "mc-server".into())
}
