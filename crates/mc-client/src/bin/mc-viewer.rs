//! mc-viewer — watch a microscope server (read only): live preview frames, stage position, events.
//!
//! `mc-viewer [auto|HOST] [--save DIR] [--fps F] [--seconds S]`

use mc_client::Client;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let host = args.first().filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "auto".into());
    let fps: f64 = arg("--fps").and_then(|v| v.parse().ok()).unwrap_or(3.0);
    let secs: Option<f64> = arg("--seconds").and_then(|v| v.parse().ok());
    let save = arg("--save");
    let name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "viewer".into());
    let c = match Client::connect(&host, "viewer", &name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mc-viewer: {e}");
            std::process::exit(1);
        }
    };
    println!("connected to {} ({}) as a viewer", c.server["name"].as_str().unwrap_or("?"), c.addr);
    let mut d = c.data("preview", fps).expect("data connection");
    if let Some(dir) = &save {
        let _ = std::fs::create_dir_all(dir);
    }
    let c2 = c.clone();
    std::thread::spawn(move || loop {
        let ev = c2.events.lock().unwrap().recv();
        match ev {
            Ok(v) if v["event"] == "telemetry" => {
                let p = &v["stage"]["pos_um"];
                println!("  stage {p}  camera {:.1} fps  operator {}  rtt {:.2} ms", v["camera"]["fps"].as_f64().unwrap_or(0.0),
                         v["operator"].as_str().unwrap_or("-"), c2.rtt_ns.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6);
            }
            Ok(v) if v["event"] == "closed" => {
                println!("server closed the connection");
                std::process::exit(0);
            }
            Ok(v) => println!("  event {v}"),
            Err(_) => break,
        }
    });
    let t0 = Instant::now();
    loop {
        let (h, px) = match d.next() {
            Ok(x) => x,
            Err(e) => {
                eprintln!("data: {e}");
                break;
            }
        };
        let lat = (mc_proto::clock::now_ns() as i64 - c.to_local(h["t_exp_end"].as_u64().unwrap_or(0))) as f64 / 1e6;
        println!("frame {:>6}  {}x{} {}  {:>7} B  xyz {}  {}  latency {:.1} ms", h["seq"], h["w"], h["h"], h["fmt"].as_str().unwrap_or(""),
                 px.len(), h["xyz_um"], if h["moving"] == true { "moving" } else { "still" }, lat);
        if let Some(dir) = &save {
            let _ = std::fs::write(format!("{dir}/{:06}.jpg", h["seq"].as_u64().unwrap_or(0)), px);
        }
        if secs.map(|s| t0.elapsed() > Duration::from_secs_f64(s)).unwrap_or(false) {
            break;
        }
    }
}
