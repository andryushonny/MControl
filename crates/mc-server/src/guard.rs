//! Guard: a small separate process. The server sends it a heartbeat every 250 ms while its stage thread is alive.
//! If the heartbeats stop for 3 s (server hung or crashed), the guard kills the server, opens the controller port
//! itself and switches the laser, motors and light off. A clean server exit sends "bye" and the guard just leaves.

use crate::controller::{self as C, Controller};
use mc_proto::clock::now_ns;
use std::net::UdpSocket;
use std::time::Duration;

/// `mc-server guard --pid N --hb-port P --port COM3`
pub fn guard_main(args: &[String]) -> ! {
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let pid: u32 = arg("--pid").and_then(|v| v.parse().ok()).unwrap_or(0);
    let hb: u16 = arg("--hb-port").and_then(|v| v.parse().ok()).unwrap_or(0);
    let port = arg("--port").unwrap_or_else(|| "sim".into());
    let dead_ms: u64 = arg("--dead-ms").and_then(|v| v.parse().ok()).unwrap_or(3000);
    crate::log::init(!args.iter().any(|a| a == "--no-color"), false, arg("--log").as_deref());
    let sock = match UdpSocket::bind(("127.0.0.1", hb)) {
        Ok(s) => s,
        Err(e) => {
            error!("GUARD", "cannot listen on 127.0.0.1:{hb}: {e}");
            std::process::exit(1);
        }
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(250)));
    let mut last = now_ns();
    let mut seen = false;
    let mut b = [0u8; 16];
    loop {
        match sock.recv_from(&mut b) {
            Ok((n, _)) if &b[..n] == b"bye" => std::process::exit(0),
            Ok((_, from)) => {
                if !seen {
                    // logged before the answer, so the line comes before the server's address table
                    info!("GUARD", "watching the server (pid {pid}): if it stops answering for {:.0} s, the guard stops the stage and switches the laser off", dead_ms as f64 / 1000.0);
                }
                let _ = sock.send_to(b"ok", from); // the server waits for this before it takes clients
                seen = true;
                last = now_ns();
            }
            Err(_) => {}
        }
        if (now_ns() - last) / 1_000_000 > dead_ms.max(if seen { 0 } else { 15_000 }) {
            break;
        }
    }
    error!("GUARD", "E600 server silent for {:.1} s: killing it, then laser off, motors off, light off", (now_ns() - last) as f64 / 1e9);
    kill(pid);
    std::thread::sleep(Duration::from_millis(500));
    for attempt in 0..10 {
        match Controller::open(&port) {
            Ok(mut c) => {
                for (cmd, data) in [(C::LASER, vec![0u8, 0]), (C::TURN_MOTOR, vec![0]), (C::LIGHT, vec![0, 0])] {
                    match c.cmd(cmd, &data, 1000) {
                        Ok(_) => info!("GUARD", "{} off", C::name(cmd)),
                        Err(e) => error!("GUARD", "{}: {e}", C::name(cmd)),
                    }
                }
                info!("GUARD", "safe state set. Start mc-server again.");
                std::process::exit(0);
            }
            Err(e) => {
                if attempt == 9 {
                    error!("GUARD", "cannot open {port}: {e}. SWITCH THE LASER OFF BY HAND.");
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
    std::process::exit(1);
}

fn kill(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).output();
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
}
