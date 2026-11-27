#![allow(dead_code)]
//! mc-server — run it on the PC wired to the microscope: `mc-server.exe` (or `mc-server.exe --sim` to try it without one).

#[macro_use]
mod log;
mod camera;
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
  --control-port N  control TCP port (default 5160)
  --data-port N     data TCP port (default 5161)
  --sdk DIR         folder of MVCAMSDK_X64.dll (default: the MindVision install folder)
  --no-camera       stage only
  --allow-laser     allow switching the laser on (off by default)
  --speed UM_S      real stage speed, um/s (default 3000)
  --ring N          frames kept for re-fetch (default 20)
  --dead-ms N       operator silent this long: stop stage, laser off (default 3000)
  --viewer-fps F    preview frames per second for viewers (default 3)
  --no-guard        do not start the guard process
  --log FILE        also write the log to FILE
  --no-color        plain log
  --debug           more log lines
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
        Some("--version") => {
            println!("mc-server {}", mc_proto::VERSION);
            return;
        }
        _ => {}
    }
    let has = |k: &str| args.iter().any(|a| a == k);
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    log::init(!has("--no-color"), has("--debug"), arg("--log").as_deref());
    let sim = has("--sim");
    let name = arg("--name").unwrap_or_else(hostname);

    println!();
    println!("  {}  {}", paint(BOLD, &format!("MControl server {}", mc_proto::VERSION)), paint(DIM, "· Optosky ATR8300 + MindVision camera"));
    println!();

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
        control_port: arg("--control-port").and_then(|v| v.parse().ok()).unwrap_or(mc_proto::CONTROL_PORT),
        data_port: arg("--data-port").and_then(|v| v.parse().ok()).unwrap_or(mc_proto::DATA_PORT),
        dead_ms: arg("--dead-ms").and_then(|v| v.parse().ok()).unwrap_or(3000),
        viewer_fps: arg("--viewer-fps").and_then(|v| v.parse().ok()).unwrap_or(3.0),
    };
    let ip = primary_ip();
    println!("  {}  {}", paint(DIM, "Name      "), paint(BOLD, &name));
    println!("  {}  {}  {}", paint(DIM, "Control   "), paint(CYAN, &format!("tcp://{ip}:{}", ncfg.control_port)), paint(DIM, "Commands, replies, events"));
    println!("  {}  {}  {}", paint(DIM, "Data      "), paint(CYAN, &format!("tcp://{ip}:{}", ncfg.data_port)), paint(DIM, "Frames"));
    println!("  {}  {}  {}", paint(DIM, "Discovery "), paint(CYAN, &format!("udp {}", mc_proto::DISCOVERY_PORT)), paint(DIM, "Clients find the server by themselves"));
    println!("  {}  {}", paint(DIM, "Laser     "), if cfg.allow_laser { paint(YELLOW, "Allowed") } else { "Blocked (--allow-laser to allow)".into() });
    println!("  {}  {}", paint(DIM, "Stop      "), paint(GREEN, "Ctrl+C: stage stops, laser, motors and light off"));
    println!();
    let srv = Arc::new(net::Server {
        stage: st.clone(),
        cam: cam.clone(),
        hub,
        sessions: Mutex::new(Vec::new()),
        lease: Mutex::new(None),
        cfg: ncfg,
        started: now_ns(),
    });
    let _ = srv.started;

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

    if let Err(e) = srv.run() {
        error!("NET", "cannot listen: {e} (is another mc-server running?)");
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
