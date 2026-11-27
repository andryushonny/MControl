//! mc-cli — control the microscope from the command line.
//!
//! ```text
//! mc-cli [auto|HOST] status
//! mc-cli auto goto x=100 y=0          move / goto / snap take x= y= z= dx= dy= dz= settle_ms=
//! mc-cli auto move dz=-50
//! mc-cli auto snap x=0 y=0 out=frame.raw
//! mc-cli auto light level=3           laser mw=5, exposure ms=20, gain value=2, zero, stop, ...
//! mc-cli auto bench 10                full-frame stream for 10 s: fps, MB/s, latency
//! ```

use mc_client::Client;
use serde_json::{json, Value};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 || args[0] == "-h" || args[0] == "--help" {
        println!("{}", include_str!("mc-cli.rs").lines().skip(2).take(10).map(|l| l.trim_start_matches("//! ")).collect::<Vec<_>>().join("\n"));
        return;
    }
    let host = &args[0];
    let cmd = args[1].as_str();
    let name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "operator".into());
    let role = if cmd == "status" { "viewer" } else { "operator" };
    let c = match Client::connect(host, role, &name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mc-cli: {e}");
            std::process::exit(1);
        }
    };
    let mut v = json!({"cmd": cmd});
    let mut out = None;
    for a in &args[2..] {
        if let Some((k, x)) = a.split_once('=') {
            if k == "out" {
                out = Some(x.to_string());
                continue;
            }
            v[k] = match x {
                "on" | "true" => json!(true),
                "off" | "false" => json!(false),
                _ => x.parse::<f64>().map(|f| json!(f)).unwrap_or(json!(x)),
            };
        }
    }
    match cmd {
        "bench" => {
            let secs: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10.0);
            let mut d = c.data("raw", 0.0).expect("data");
            let t0 = Instant::now();
            let (mut n, mut bytes, mut lat) = (0u64, 0u64, Vec::new());
            while t0.elapsed().as_secs_f64() < secs {
                let (h, px) = d.next().expect("frame");
                n += 1;
                bytes += px.len() as u64;
                lat.push((mc_proto::clock::now_ns() as i64 - c.to_local(h["t_exp_end"].as_u64().unwrap_or(0))) as f64 / 1e6);
            }
            let dt = t0.elapsed().as_secs_f64();
            lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!("{n} frames in {dt:.1} s: {:.1} fps, {:.1} MB/s, latency median {:.1} ms, clock offset {:.3} ms, rtt {:.3} ms",
                     n as f64 / dt, bytes as f64 / dt / 1e6, lat.get(lat.len() / 2).copied().unwrap_or(0.0),
                     c.offset_ns.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6, c.rtt_ns.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6);
        }
        "snap" => {
            let mut d = c.data("raw", 0.0).expect("data");
            let _ = c.cmd(json!({"cmd": "stream", "on": false}));
            let r = c.cmd(v).unwrap_or_else(|e| fail(e));
            let seq = r["seq"].as_u64().unwrap_or(0);
            loop {
                let (h, px) = d.next().expect("frame");
                if h["seq"].as_u64() == Some(seq) {
                    println!("{}", serde_json::to_string_pretty(&h).unwrap());
                    if let Some(o) = &out {
                        std::fs::write(o, px).expect("write");
                        println!("saved {} bytes to {o}", px.len());
                    }
                    break;
                }
            }
        }
        _ => {
            let r: Value = c.cmd(v).unwrap_or_else(|e| fail(e));
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
        }
    }
}

fn fail(e: std::io::Error) -> Value {
    eprintln!("mc-cli: {e}");
    std::process::exit(2);
}
