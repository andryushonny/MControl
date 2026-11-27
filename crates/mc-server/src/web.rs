//! A web page for anyone in the lab: http://<this PC>:8080. The live picture (a small MJPEG stream, one viewer
//! place per open page), the state of the work the operator's program reports (project, sample, what runs,
//! how far, how long still), the stage, and the last 10 commands. Read only: nothing on it changes the
//! instrument. The page is inside the exe.

use crate::net::Server;
use mc_proto::clock::now_ns;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const PAGE: &str = include_str!("web/index.html");

/// The web port: the given one, or the next free of the 9 after it.
pub fn listen(bind: IpAddr, port: u16) -> Option<(TcpListener, u16)> {
    (port..port.saturating_add(10)).find_map(|p| TcpListener::bind((bind, p)).ok().map(|l| (l, p)))
}

pub fn serve(srv: Arc<Server>, l: TcpListener) {
    std::thread::Builder::new()
        .name("web".into())
        .spawn(move || {
            for s in l.incoming().flatten() {
                let srv = srv.clone();
                let _ = std::thread::Builder::new().name("web-client".into()).spawn(move || {
                    let _ = handle(&srv, s);
                });
            }
        })
        .ok();
}

fn handle(srv: &Arc<Server>, mut s: TcpStream) -> std::io::Result<()> {
    let ip = s.peer_addr()?.ip();
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut r = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    // the rest of the request head is not needed
    let mut h = String::new();
    while r.read_line(&mut h)? > 2 {
        h.clear();
    }
    if !srv.cfg.access.allowed(&ip) {
        return reply(&mut s, "403 Forbidden", "text/plain", b"not allowed from this address");
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    match path.split('?').next().unwrap_or("/") {
        "/" | "/index.html" => reply(&mut s, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()),
        "/status" => reply(&mut s, "200 OK", "application/json", status(srv).to_string().as_bytes()),
        "/stream" => stream(srv, s),
        "/favicon.ico" => reply(&mut s, "204 No Content", "text/plain", b""),
        _ => reply(&mut s, "404 Not Found", "text/plain", b"not found"),
    }
}

fn reply(s: &mut TcpStream, code: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(s, "HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len())?;
    s.write_all(body)?;
    s.flush()
}

/// What the page shows, once a second.
fn status(srv: &Arc<Server>) -> Value {
    let holder = srv.lease.lock().unwrap().clone();
    let work = srv.work.lock().unwrap().clone();
    // the operator's program reports every 2 s; older than 15 s: it is not running any more
    let work = match work {
        Some((t, w)) if now_ns().saturating_sub(t) < 15_000_000_000 => w,
        Some((_, mut w)) => {
            w["stale"] = json!(true);
            w
        }
        None => Value::Null,
    };
    let last: Vec<Value> = srv.recent_commands(10);
    json!({
        "name": srv.cfg.name, "version": mc_proto::VERSION,
        "uptime_s": (now_ns() - srv.started) / 1_000_000_000,
        "camera_fps": (srv.hub.fps() * 10.0).round() / 10.0,
        "stage": srv.stage.status(),
        "operator": holder.map(|h| json!({"name": h.name, "ip": h.addr.ip().to_string()})),
        "work": work,
        "queue": srv.queued(),
        "viewers": srv.viewer_count(), "max_viewers": srv.cfg.max_viewers,
        "commands": last,
        "t_wall_ms": mc_proto::clock::wall_ms(),
    })
}

/// The live picture as MJPEG: the quarter-size preview of each frame, at most 10 a second. One viewer place.
fn stream(srv: &Arc<Server>, mut s: TcpStream) -> std::io::Result<()> {
    if srv.viewer_count() >= srv.cfg.max_viewers {
        return reply(&mut s, "503 Service Unavailable", "text/plain", format!("{} viewers at most", srv.cfg.max_viewers).as_bytes());
    }
    srv.web_viewers.fetch_add(1, Ordering::SeqCst);
    let r = (|| -> std::io::Result<()> {
        write!(s, "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n")?;
        let mut last = 0u64;
        loop {
            match srv.hub.latest() {
                Some(f) if f.seq != last => {
                    last = f.seq;
                    let (_, jpg) = f.preview();
                    write!(s, "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n", jpg.len())?;
                    s.write_all(jpg)?;
                    s.write_all(b"\r\n")?;
                    s.flush()?;
                    std::thread::sleep(Duration::from_millis(100));
                }
                _ => std::thread::sleep(Duration::from_millis(40)),
            }
        }
    })();
    srv.web_viewers.fetch_sub(1, Ordering::SeqCst);
    r
}
