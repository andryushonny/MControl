//! Rust client for mc-server: discovery, control connection with time sync, data connection.

use mc_proto::clock::now_ns;
use mc_proto::{read_msg, split_frame, write_json, K_FRAME, K_JSON};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufReader};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Find servers on the LAN (UDP broadcast). Returns (address, answer).
pub fn discover(wait: Duration) -> Vec<(SocketAddr, Value)> {
    let mut out = Vec::new();
    let Ok(s) = UdpSocket::bind("0.0.0.0:0") else { return out };
    let _ = s.set_broadcast(true);
    let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
    for t in ["255.255.255.255", "127.0.0.1"] {
        let _ = s.send_to(mc_proto::DISCOVERY_ASK, (t, mc_proto::DISCOVERY_PORT));
    }
    let end = now_ns() + wait.as_nanos() as u64;
    let mut b = [0u8; 2048];
    while now_ns() < end {
        if let Ok((n, from)) = s.recv_from(&mut b) {
            if let Ok(v) = serde_json::from_slice::<Value>(&b[..n]) {
                let port = v["control_port"].as_u64().unwrap_or(mc_proto::CONTROL_PORT as u64) as u16;
                let a = SocketAddr::new(from.ip(), port);
                if !out.iter().any(|(x, w): &(SocketAddr, Value)| *x == a || w["name"] == v["name"]) {
                    out.push((a, v));
                }
            }
        }
    }
    // a LAN address is better than loopback for the same server
    out.sort_by_key(|(a, _)| a.ip().is_loopback());
    out
}

/// "auto", "host", "host:port" -> control address.
pub fn resolve(addr: &str) -> io::Result<SocketAddr> {
    if addr == "auto" || addr.is_empty() {
        return discover(Duration::from_millis(700))
            .first()
            .map(|x| x.0)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no mc-server found on the LAN (give its address)"));
    }
    let a = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{}", mc_proto::CONTROL_PORT) };
    a.to_socket_addrs()?.next().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, addr.to_string()))
}

pub struct Client {
    w: Mutex<TcpStream>,
    pending: Arc<Mutex<HashMap<u64, Sender<Value>>>>,
    next: AtomicU64,
    pub events: Mutex<Receiver<Value>>,
    pub session: String,
    pub server: Value,
    pub addr: SocketAddr,
    /// server clock - our clock, ns (min-RTT sample)
    pub offset_ns: Arc<AtomicI64>,
    pub rtt_ns: Arc<AtomicU64>,
}

impl Client {
    /// Connect the control channel and say hello. role: "operator" or "viewer".
    pub fn connect(addr: &str, role: &str, name: &str) -> io::Result<Arc<Client>> {
        let a = resolve(addr)?;
        let c = TcpStream::connect_timeout(&a, Duration::from_secs(5))?;
        c.set_nodelay(true)?;
        let mut r = BufReader::new(c.try_clone()?);
        let mut w = c;
        write_json(&mut w, &json!({"cmd": "hello", "id": 0, "role": role, "name": name, "client": "mc-client", "version": mc_proto::VERSION}))?;
        let mut buf = Vec::new();
        read_msg(&mut r, &mut buf)?;
        let v: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
        if v["ok"] != true {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, v["error"]["msg"].as_str().unwrap_or("refused").to_string()));
        }
        let pending: Arc<Mutex<HashMap<u64, Sender<Value>>>> = Arc::default();
        let (etx, erx) = channel();
        let p2 = pending.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            while let Ok(k) = read_msg(&mut r, &mut buf) {
                if k != K_JSON {
                    continue;
                }
                let v: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
                if v.get("event").is_some() {
                    let _ = etx.send(v);
                } else if let Some(id) = v["id"].as_u64() {
                    if let Some(tx) = p2.lock().unwrap().remove(&id) {
                        let _ = tx.send(v);
                    }
                }
            }
            // connection lost: every waiting call fails now, not after its timeout
            p2.lock().unwrap().clear();
            let _ = etx.send(json!({"event": "closed"}));
        });
        let cl = Arc::new(Client {
            w: Mutex::new(w),
            pending,
            next: AtomicU64::new(1),
            events: Mutex::new(erx),
            session: v["session"].as_str().unwrap_or("").to_string(),
            server: v["server"].clone(),
            addr: a,
            offset_ns: Arc::new(AtomicI64::new(0)),
            rtt_ns: Arc::new(AtomicU64::new(u64::MAX)),
        });
        // heartbeat + time sync: 4 pings a second, keep the least-delayed sample of the last 32
        let c2 = Arc::downgrade(&cl);
        std::thread::spawn(move || {
            let mut samples: std::collections::VecDeque<(u64, i64)> = Default::default();
            loop {
                let Some(c) = c2.upgrade() else { break };
                let t1 = now_ns();
                let rtt_us = c.rtt_ns.load(Ordering::Relaxed) / 1000;
                match c.call(json!({"cmd": "ping", "t1": t1, "rtt_us": rtt_us}), Duration::from_secs(2)) {
                    Ok(v) => {
                        let t4 = now_ns();
                        let (t2, t3) = (v["t2"].as_u64().unwrap_or(0) as i64, v["t3"].as_u64().unwrap_or(0) as i64);
                        let d = (t4 - t1) as i64 - (t3 - t2);
                        let off = ((t2 - t1 as i64) + (t3 - t4 as i64)) / 2;
                        samples.push_back((d.max(0) as u64, off));
                        if samples.len() > 32 {
                            samples.pop_front();
                        }
                        let best = samples.iter().min_by_key(|s| s.0).unwrap();
                        c.rtt_ns.store(best.0, Ordering::Relaxed);
                        c.offset_ns.store(best.1, Ordering::Relaxed);
                    }
                    Err(_) => break,
                }
                drop(c);
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        Ok(cl)
    }

    /// Send a command, wait for its reply (error replies become Err).
    pub fn call(&self, mut v: Value, timeout: Duration) -> io::Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        v["id"] = json!(id);
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        write_json(&mut *self.w.lock().unwrap(), &v)?;
        let r = rx.recv_timeout(timeout).map_err(|e| {
            self.pending.lock().unwrap().remove(&id);
            match e {
                std::sync::mpsc::RecvTimeoutError::Timeout => io::Error::new(io::ErrorKind::TimedOut, "E109 no reply from the server in time"),
                _ => io::Error::new(io::ErrorKind::ConnectionAborted, "E110 connection to the server lost"),
            }
        })?;
        if r["ok"] == true {
            Ok(r)
        } else {
            Err(io::Error::other(format!("{} {}", r["error"]["code"].as_str().unwrap_or(""), r["error"]["msg"].as_str().unwrap_or(""))))
        }
    }

    pub fn cmd(&self, v: Value) -> io::Result<Value> {
        self.call(v, Duration::from_secs(120))
    }

    /// Server time -> our time, ns.
    pub fn to_local(&self, t_server: u64) -> i64 {
        t_server as i64 - self.offset_ns.load(Ordering::Relaxed)
    }

    /// Open the data connection. want: "raw" (operator) or "preview".
    pub fn data(&self, want: &str, fps: f64) -> io::Result<DataConn> {
        let port = self.server["data_port"].as_u64().unwrap_or(mc_proto::DATA_PORT as u64) as u16;
        let c = TcpStream::connect_timeout(&SocketAddr::new(self.addr.ip(), port), Duration::from_secs(5))?;
        let _ = c.set_recv_buffer_size_hint();
        let mut w = c.try_clone()?;
        write_json(&mut w, &json!({"session": self.session, "want": want, "fps": fps}))?;
        let mut r = BufReader::with_capacity(1 << 20, c);
        let mut buf = Vec::new();
        read_msg(&mut r, &mut buf)?;
        let v: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
        if v["ok"] != true {
            return Err(io::Error::other(v["error"]["msg"].as_str().unwrap_or("refused").to_string()));
        }
        Ok(DataConn { r, buf: Vec::with_capacity(16 << 20) })
    }
}

trait Hint {
    fn set_recv_buffer_size_hint(&self) -> io::Result<()>;
}
impl Hint for TcpStream {
    fn set_recv_buffer_size_hint(&self) -> io::Result<()> {
        Ok(())
    }
}

pub struct DataConn {
    r: BufReader<TcpStream>,
    buf: Vec<u8>,
}

impl DataConn {
    /// Next frame: (header, pixels).
    pub fn next(&mut self) -> io::Result<(Value, &[u8])> {
        loop {
            let k = read_msg(&mut self.r, &mut self.buf)?;
            if k == K_FRAME {
                break;
            }
        }
        split_frame(&self.buf)
    }
}
