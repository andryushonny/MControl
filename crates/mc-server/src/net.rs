//! Network: control (commands, replies, events), data (frames), discovery.
//! One thread per connection: the server PC's cores serve clients in parallel, and a slow client blocks only itself.

use crate::access::{random_id, Access};
use crate::camera::Camera;
use crate::codec::Codec;
use crate::frames::{Frame, Hub, Outbox, Pack, Want};
use crate::log::{paint, rate, Level, BLUE, BOLD, DIM, GREEN};
use crate::stage::Stage;
use mc_proto::clock::now_ns;
use mc_proto::errors::{McError, McResult};
use mc_proto::{read_msg_max, write_frame, write_json, DISCOVERY_ASK, K_JSON, MAX_CMD};
use serde_json::{json, Value};
use std::io::BufWriter;
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Operator,
    Viewer,
    /// A helper of the operator (e.g. MStitch on the operator's PC): commands go into the operator's queue.
    Assistant,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Operator => "operator",
            Role::Viewer => "viewer",
            Role::Assistant => "assistant",
        }
    }
}

pub struct Session {
    pub id: String,
    pub role: Role,
    pub lease_key: String,               // an operator's key: an assistant shows it to join
    pub partner: Option<Arc<Session>>,   // an assistant's operator
    pub name: String,
    pub addr: SocketAddr,
    writer: Mutex<TcpStream>,
    pub last_seen: AtomicU64,
    pub outbox: Mutex<Option<Arc<Outbox>>>,
    pub alive: AtomicBool,
    pub refetch_ns: AtomicU64,
    pub rtt_us: AtomicU64,
    pub since: u64,
}

impl Session {
    pub fn who(&self) -> String {
        who(self.role, &self.name, &self.addr)
    }

    pub fn send(&self, v: &Value) {
        if !self.alive.load(Ordering::Relaxed) {
            return;
        }
        let mut w = self.writer.lock().unwrap();
        if write_json(&mut *w, v).is_err() {
            self.alive.store(false, Ordering::Relaxed);
            let _ = w.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// "operator NAME 1.2.3.4", coloured; the one way a client is named in the log.
pub fn who(role: Role, name: &str, addr: &SocketAddr) -> String {
    let (c, r) = match role {
        Role::Operator => (GREEN, "operator"),
        Role::Viewer => (BLUE, "viewer"),
        Role::Assistant => (GREEN, "assistant"),
    };
    format!("{} {} {}", paint(c, r), paint(BOLD, name), paint(DIM, &addr.ip().to_string()))
}

pub struct Cfg {
    pub name: String,
    pub control_port: u16,
    pub data_port: u16,
    pub discovery_port: Option<u16>,
    pub bind: IpAddr,
    pub dead_ms: u64,
    pub viewer_fps: f64,
    pub max_viewers: usize,
    pub access: Access,
}

/// Listen on `port`, or on the next free one of the 9 after it (another program may hold it).
pub fn listen(bind: IpAddr, port: u16) -> McResult<(TcpListener, u16)> {
    let mut last = String::new();
    for p in port..port.saturating_add(10) {
        match TcpListener::bind((bind, p)) {
            Ok(l) => {
                if p != port {
                    warn!("NET", "Port {port} busy: using {p} (discovery tells clients)");
                }
                return Ok((l, p));
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(McError::new("E120", format!("{bind}:{port}..{}: {last}", port.saturating_add(9))))
}

/// A command of the operator (or its assistant) waiting its turn.
pub struct Job {
    n: u64,
    sess: Arc<Session>,
    id: Value,
    cmd: String,
    v: Value,
}

pub struct Server {
    refused: Mutex<HashMap<IpAddr, u64>>,
    /// One queue for everything that changes the instrument: commands run one after another, in the order
    /// they came, each with a number `n`; inside a block (begin .. end) only that client's commands run.
    queue: Mutex<VecDeque<Job>>,
    queue_cv: Condvar,
    next_n: AtomicU64,
    cancel: AtomicU64, // bumped by stop: a running wait ends, queued commands are dropped
    exclusive: Mutex<Option<(Arc<Session>, u64)>>, // an open block: its client, and when it last did something
    history: Mutex<VecDeque<Value>>,
    pub stage: Arc<Stage>,
    pub cam: Arc<Camera>,
    pub hub: Arc<Hub>,
    pub sessions: Mutex<Vec<Arc<Session>>>,
    pub lease: Mutex<Option<Arc<Session>>>,
    pub cfg: Cfg,
    pub started: u64,
}

fn ok(id: &Value, mut v: Value) -> Value {
    if !v.is_object() {
        v = json!({"result": v});
    }
    v["ok"] = json!(true);
    v["id"] = id.clone();
    v
}

fn err(id: &Value, e: &McError) -> Value {
    json!({"ok": false, "id": id, "error": e.json()})
}

fn f3(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64())
}

impl Server {
    pub fn new(stage: Arc<Stage>, cam: Arc<Camera>, hub: Arc<Hub>, cfg: Cfg) -> Arc<Server> {
        Arc::new(Server { refused: Mutex::new(HashMap::new()), queue: Mutex::new(VecDeque::new()), queue_cv: Condvar::new(),
                          next_n: AtomicU64::new(0), cancel: AtomicU64::new(0), exclusive: Mutex::new(None), history: Mutex::new(VecDeque::new()),
                          stage, cam, hub, sessions: Mutex::new(Vec::new()), lease: Mutex::new(None), cfg, started: now_ns() })
    }

    /// Refuse a client before it gets a session; logged once a minute per address (scanners stay quiet).
    fn refuse(&self, w: &mut TcpStream, id: &Value, e: McError, who: String) {
        let _ = write_json(w, &err(id, &e));
        // close politely: half-close, then drain what the client still sends. Closing with unread data makes
        // Windows reset the connection, and the client would see "connection lost" instead of the reason.
        let _ = w.shutdown(std::net::Shutdown::Write);
        let _ = w.set_read_timeout(Some(Duration::from_millis(500)));
        let mut sink = [0u8; 512];
        while matches!(std::io::Read::read(w, &mut sink), Ok(n) if n > 0) {}
        let ip = w.peer_addr().map(|a| a.ip()).unwrap_or(IpAddr::from([0, 0, 0, 0]));
        let mut r = self.refused.lock().unwrap();
        let t = r.entry(ip).or_insert(0);
        if now_ns().saturating_sub(*t) > 60_000_000_000 {
            *t = now_ns();
            warn!("NET", "{who}  refused: {e}");
        }
    }

    pub fn run(self: &Arc<Self>, cl: TcpListener, dl: TcpListener) -> std::io::Result<()> {
        let s = self.clone();
        std::thread::Builder::new().name("data-accept".into()).spawn(move || {
            for c in dl.incoming().flatten() {
                let s = s.clone();
                std::thread::spawn(move || s.data_conn(c));
            }
        })?;
        let s = self.clone();
        std::thread::Builder::new().name("discovery".into()).spawn(move || s.discovery())?;
        let s = self.clone();
        std::thread::Builder::new().name("watchdog".into()).spawn(move || s.watchdog())?;
        let s = self.clone();
        std::thread::Builder::new().name("queue".into()).spawn(move || s.executor())?;
        let s = self.clone();
        std::thread::Builder::new().name("telemetry".into()).spawn(move || s.telemetry())?;
        for c in cl.incoming().flatten() {
            let s = self.clone();
            std::thread::spawn(move || s.control_conn(c));
        }
        Ok(())
    }

    fn holder(&self) -> Option<Arc<Session>> {
        self.lease.lock().unwrap().clone()
    }

    /// An event for every client (viewers see everything the operator does): `{"event": kind, "t_server", ...fields}`.
    pub fn event(&self, kind: &str, mut fields: Value) {
        fields["event"] = json!(kind);
        fields["t_server"] = json!(now_ns());
        self.broadcast(&fields);
    }

    pub fn broadcast(&self, v: &Value) {
        let list = self.sessions.lock().unwrap().clone();
        for s in list {
            s.send(v);
        }
    }

    // ---------------------------------------------------------------- control

    fn control_conn(self: Arc<Self>, mut c: TcpStream) {
        let addr = c.peer_addr().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
        if !self.cfg.access.allowed(&addr.ip()) {
            let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = read_msg_max(&mut c, &mut Vec::new(), MAX_CMD); // its hello: read, so the answer arrives
            let ip = addr.ip().to_string();
            self.refuse(&mut c, &Value::Null, McError::new("E111", ip.clone()), format!("client {ip}"));
            return;
        }
        let _ = c.set_nodelay(true);
        let _ = c.set_read_timeout(Some(Duration::from_secs(10)));
        let mut rd = match c.try_clone() {
            Ok(r) => r,
            Err(_) => return,
        };
        let mut buf = Vec::new();
        // hello
        let hello: Value = match read_msg_max(&mut rd, &mut buf, MAX_CMD) {
            Ok(K_JSON) => serde_json::from_slice(&buf).unwrap_or(Value::Null),
            _ => return,
        };
        let id = hello["id"].clone();
        let mut w = match c.try_clone() {
            Ok(w) => w,
            Err(_) => return,
        };
        if hello["cmd"] != "hello" {
            let _ = write_json(&mut w, &err(&id, &McError::new("E104", "")));
            return;
        }
        let role = match hello["role"].as_str() {
            Some("operator") => Role::Operator,
            Some("assistant") => Role::Assistant,
            _ => Role::Viewer,
        };
        let name = hello["name"].as_str().unwrap_or("?").chars().filter(|c| !c.is_control()).take(40).collect::<String>();
        let a = &self.cfg.access;
        let refusal = if !a.token_ok(hello["token"].as_str().unwrap_or("")) {
            Some(McError::new("E112", ""))
        } else if role != Role::Viewer && !a.may_operate(&addr.ip()) {
            Some(McError::new("E113", addr.ip().to_string()))
        } else if role == Role::Viewer
            && self.sessions.lock().unwrap().iter().filter(|s| s.role == Role::Viewer).count() >= self.cfg.max_viewers
        {
            Some(McError::new("E114", format!("{} at most", self.cfg.max_viewers)))
        } else {
            None
        };
        // an assistant joins the operator now in control: with its key, or from the operator's own PC
        let partner = if role == Role::Assistant { self.holder() } else { None };
        let refusal = refusal.or_else(|| {
            (role == Role::Assistant
                && !partner.as_ref().map_or(false, |h| {
                    hello["join"].as_str().map_or(h.addr.ip() == addr.ip(), |k| a.token_eq(k, &h.lease_key))
                }))
            .then(|| McError::new("E115", ""))
        });
        if let Some(e) = refusal {
            self.refuse(&mut w, &id, e, who(role, &name, &addr));
            return;
        }
        let sess = Arc::new(Session {
            id: random_id(),
            role,
            lease_key: random_id(),
            partner,
            name,
            addr,
            writer: Mutex::new(c),
            last_seen: AtomicU64::new(now_ns()),
            outbox: Mutex::new(None),
            alive: AtomicBool::new(true),
            refetch_ns: AtomicU64::new(0),
            rtt_us: AtomicU64::new(0),
            since: now_ns(),
        });
        if role == Role::Operator {
            let mut l = self.lease.lock().unwrap();
            if let Some(h) = l.as_ref() {
                if h.alive.load(Ordering::Relaxed) {
                    let e = McError::new("E103", format!("{} ({})", h.name, h.addr.ip()));
                    sess.send(&err(&id, &e));
                    warn!("NET", "{}  refused: {} holds the microscope", sess.who(), h.name);
                    return;
                }
            }
            *l = Some(sess.clone());
        }
        self.sessions.lock().unwrap().push(sess.clone());
        info!("NET", "{}  connected", sess.who());
        let mut reply = json!({"session": sess.id, "role": role.name(), "server": self.info(), "t_server": now_ns()});
        if role == Role::Operator {
            reply["lease_key"] = json!(sess.lease_key); // to hand to an assistant on another PC
        }
        sess.send(&ok(&id, reply));
        self.event("client", client_json(&sess, "connected")); // after the reply: hello's answer comes first
        if role == Role::Operator {
            self.event("operator", json!({"name": sess.name, "addr": sess.addr.ip().to_string()}));
        }
        let _ = rd.set_read_timeout(Some(Duration::from_millis(500)));
        loop {
            if !sess.alive.load(Ordering::Relaxed) {
                break;
            }
            match read_msg_max(&mut rd, &mut buf, MAX_CMD) {
                Ok(K_JSON) => {
                    let t2 = now_ns();
                    sess.last_seen.store(t2, Ordering::Relaxed);
                    let v: Value = match serde_json::from_slice(&buf) {
                        Ok(v) => v,
                        Err(_) => {
                            sess.send(&err(&Value::Null, &McError::new("E101", "not JSON")));
                            continue;
                        }
                    };
                    self.clone().command(&sess, v, t2);
                }
                Ok(_) => {}
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                Err(_) => break,
            }
        }
        self.drop_session(&sess, "disconnected");
    }

    fn drop_session(&self, sess: &Arc<Session>, why: &str) {
        sess.alive.store(false, Ordering::Relaxed);
        self.sessions.lock().unwrap().retain(|s| !Arc::ptr_eq(s, sess));
        if let Some(ob) = sess.outbox.lock().unwrap().take() {
            ob.close();
            self.hub.remove(&ob);
        }
        let was_op = {
            let mut l = self.lease.lock().unwrap();
            if l.as_ref().map(|h| Arc::ptr_eq(h, sess)).unwrap_or(false) {
                *l = None;
                true
            } else {
                false
            }
        };
        info!("NET", "{}  {} after {:.0} s", sess.who(), why, (now_ns() - sess.since) as f64 / 1e9);
        self.event("client", client_json(sess, why));
        if was_op {
            self.safe_stop("operator gone");
            self.event("operator", json!({"name": null}));
        }
    }

    /// Stop the stage and switch the laser off (operator lost).
    fn safe_stop(&self, why: &str) {
        self.stage.stop();
        let st = self.stage.clone();
        let laser = st.shared.lock().unwrap().laser;
        if laser > 0 || st.shared.lock().unwrap().moving {
            warn!("STAGE", "{why}: stage stopped, laser off");
            self.event("safe", json!({"why": why}));
        }
        std::thread::spawn(move || {
            let _ = st.call(|w| {
                w.laser_off_now();
                Ok(Value::Null)
            });
        });
    }

    fn info(&self) -> Value {
        json!({"name": self.cfg.name, "version": mc_proto::VERSION, "camera": *self.cam.info.lock().unwrap(),
               "camera_ok": self.cam.ok.load(Ordering::Relaxed), "stage": self.stage.status(), "controller": self.stage.port,
               "operator": self.holder().map(|h| h.name.clone()), "data_port": self.cfg.data_port, "dead_ms": self.cfg.dead_ms})
    }

    fn command(self: Arc<Self>, sess: &Arc<Session>, v: Value, t2: u64) {
        let id = v["id"].clone();
        let cmd = v["cmd"].as_str().unwrap_or("").to_string();
        // read-only commands: everybody
        match cmd.as_str() {
            "ping" => {
                sess.send(&json!({"ok": true, "id": id, "t1": v["t1"], "t2": t2, "t3": now_ns()}));
                if let Some(r) = v["rtt_us"].as_u64() {
                    sess.rtt_us.store(r, Ordering::Relaxed);
                }
                return;
            }
            "status" | "info" => {
                sess.send(&ok(&id, json!({"server": self.info(), "camera_fps": self.hub.fps(), "t_server": now_ns(), "clients": self.clients()})));
                return;
            }
            "frame" => {
                let seq = v["seq"].as_u64().unwrap_or(0);
                if sess.role == Role::Viewer {
                    let last = sess.refetch_ns.load(Ordering::Relaxed);
                    if now_ns() - last < 1_000_000_000 {
                        sess.send(&err(&id, &McError::new("E108", "viewers: one full frame per second")));
                        return;
                    }
                    sess.refetch_ns.store(now_ns(), Ordering::Relaxed);
                }
                let f = if seq == 0 { self.hub.latest() } else { self.hub.get(seq) };
                match (f, sess.outbox.lock().unwrap().clone()) {
                    (Some(f), Some(ob)) => {
                        let s = f.seq;
                        ob.push_reliable(f, v["preview"].as_bool().unwrap_or(false));
                        sess.send(&ok(&id, json!({"seq": s})));
                    }
                    (None, _) => sess.send(&err(&id, &McError::new("E107", format!("seq {seq}")))),
                    (_, None) => sess.send(&err(&id, &McError::new("E106", "open the data connection first"))),
                }
                return;
            }
            "history" => {
                // what was done, in order: commands with their numbers, results, errors; messages
                let since = v["since"].as_u64().unwrap_or(0);
                let limit = v["limit"].as_u64().unwrap_or(100).min(2000) as usize;
                let h = self.history.lock().unwrap();
                let list: Vec<Value> = h.iter().filter(|e| e["n"].as_u64().unwrap_or(0) > since).cloned().collect();
                let start = list.len().saturating_sub(limit);
                let queued: Vec<Value> = self.queue.lock().unwrap().iter()
                    .map(|j| json!({"n": j.n, "by": j.sess.name, "cmd": j.cmd, "args": cmd_args_json(&j.v)})).collect();
                sess.send(&ok(&id, json!({"history": list[start..].to_vec(), "queued": queued})));
                return;
            }
            "send" => {
                // a message to other clients (e.g. MStitch <-> the operator's script): "operator", "all", or a name
                let to = v["to"].as_str().unwrap_or("").to_string();
                let n = self.next_n.fetch_add(1, Ordering::SeqCst) + 1;
                let holder = self.holder();
                let targets: Vec<Arc<Session>> = self.sessions.lock().unwrap().iter()
                    .filter(|t| !Arc::ptr_eq(t, sess))
                    .filter(|t| match to.as_str() {
                        "all" => true,
                        "operator" => holder.as_ref().map_or(false, |h| Arc::ptr_eq(h, t)),
                        name => t.name == name,
                    })
                    .cloned().collect();
                let msg = json!({"event": "message", "n": n, "from": sess.name, "role": sess.role.name(), "to": to,
                                 "data": v["data"], "reply_to": v["reply_to"], "t_server": now_ns()});
                for t in &targets {
                    t.send(&msg);
                }
                let mut rec = msg.clone();
                rec["delivered"] = json!(targets.len());
                rec["t_wall_ms"] = json!(mc_proto::clock::wall_ms());
                self.remember(&rec);
                crate::debug!("NET", "Message #{n} {} -> {to}: {} recipients", sess.name, targets.len());
                sess.send(&ok(&id, json!({"n": n, "delivered": targets.len()})));
                return;
            }
            "stream" => {
                // pause / resume the live stream on this client's data connection (snaps still arrive)
                if let Some(ob) = sess.outbox.lock().unwrap().clone() {
                    ob.paused.store(!v["on"].as_bool().unwrap_or(true), Ordering::Relaxed);
                    if let Some(fps) = f3(&v, "fps") {
                        if let Want::Preview { .. } = *ob.want.lock().unwrap() {
                            *ob.want.lock().unwrap() = Want::Preview { gap_ns: (1e9 / fps.clamp(0.1, 30.0)) as u64 };
                        }
                    }
                }
                sess.send(&ok(&id, json!({})));
                return;
            }
            "bye" => {
                // control is free before the reply, so the client can hand over at once
                let mut l = self.lease.lock().unwrap();
                if l.as_ref().map_or(false, |h| Arc::ptr_eq(h, sess)) {
                    *l = None;
                    drop(l);
                    self.safe_stop("operator left");
                    self.event("operator", json!({"name": null}));
                }
                sess.send(&ok(&id, json!({})));
                sess.alive.store(false, Ordering::Relaxed);
                return;
            }
            _ => {}
        }
        // everything else changes the instrument: the operator holding control, or its assistant
        if !self.holds(sess) {
            let code = if sess.role == Role::Viewer { "E102" } else { "E103" };
            sess.send(&err(&id, &McError::new(code, cmd.clone())));
            return;
        }
        match cmd.as_str() {
            "stop" => {
                // not queued: at once. The stage stops, a block ends, waiting commands are dropped
                self.stage.stop();
                self.cancel.fetch_add(1, Ordering::SeqCst);
                *self.exclusive.lock().unwrap() = None;
                let dropped: Vec<Job> = self.queue.lock().unwrap().drain(..).collect();
                let n = self.next_n.fetch_add(1, Ordering::SeqCst) + 1;
                self.report(sess, &cmd, &v, Ok(&json!({"dropped": dropped.len()})), 0.0, n);
                for j in dropped {
                    let e = McError::new("E116", format!("by {}", sess.name));
                    self.report(&j.sess, &j.cmd, &j.v, Err(&e), 0.0, j.n);
                    j.sess.send(&err(&j.id, &e));
                }
                sess.send(&ok(&id, json!({"n": n})));
                return;
            }
            "release" if sess.role == Role::Operator => {
                *self.lease.lock().unwrap() = None;
                let n = self.next_n.fetch_add(1, Ordering::SeqCst) + 1;
                self.report(sess, &cmd, &v, Ok(&json!({})), 0.0, n);
                self.event("operator", json!({"name": null}));
                sess.send(&ok(&id, json!({"n": n})));
                return;
            }
            _ => {}
        }
        if is_read(&cmd) {
            // reads change nothing: not queued, so a long scan does not hold them up
            let (s, sess) = (self.clone(), sess.clone());
            std::thread::spawn(move || match s.run_cmd(&sess, &cmd, &v) {
                Ok(x) => sess.send(&ok(&id, x)),
                Err(e) => sess.send(&err(&id, &e)),
            });
            return;
        }
        let mut q = self.queue.lock().unwrap();
        if q.len() >= 1000 {
            sess.send(&err(&id, &McError::new("E117", "1000 waiting")));
            return;
        }
        let n = self.next_n.fetch_add(1, Ordering::SeqCst) + 1;
        let ahead = q.len();
        q.push_back(Job { n, sess: sess.clone(), id, cmd: cmd.clone(), v: v.clone() });
        drop(q);
        self.queue_cv.notify_all();
        self.event("command", json!({"state": "queued", "n": n, "by": sess.name, "cmd": cmd, "args": cmd_args_json(&v), "ahead": ahead}));
    }

    /// The operator, or an assistant whose operator still holds control.
    fn holds(&self, sess: &Arc<Session>) -> bool {
        let Some(h) = self.holder() else { return false };
        match sess.role {
            Role::Operator => Arc::ptr_eq(&h, sess),
            Role::Assistant => sess.partner.as_ref().map_or(false, |p| Arc::ptr_eq(&h, p)),
            Role::Viewer => false,
        }
    }

    /// The next job: the oldest one, or inside a block the oldest of the block's client. A block ends with
    /// "end", with stop, when its client leaves or loses control, or after 30 s without a command from it.
    fn take_job(&self) -> Job {
        let mut q = self.queue.lock().unwrap();
        loop {
            let ex = self.exclusive.lock().unwrap().clone();
            let pick = match &ex {
                None => (!q.is_empty()).then_some(0),
                Some((s, last)) => {
                    if !s.alive.load(Ordering::Relaxed) || !self.holds(s) || now_ns() - last > 30_000_000_000 {
                        warn!("NET", "Block of {} ended: {}", s.name,
                              if now_ns() - last > 30_000_000_000 { "no command for 30 s" } else { "its client left" });
                        *self.exclusive.lock().unwrap() = None;
                        continue;
                    }
                    q.iter().position(|j| Arc::ptr_eq(&j.sess, s))
                }
            };
            if let Some(i) = pick {
                return q.remove(i).unwrap();
            }
            q = self.queue_cv.wait_timeout(q, Duration::from_millis(200)).unwrap().0;
        }
    }

    /// Runs the queue: one command at a time, in order. Replies carry `n`, so the client can match them.
    fn executor(self: Arc<Self>) {
        loop {
            let job = self.take_job();
            if !job.sess.alive.load(Ordering::Relaxed) || !self.holds(&job.sess) {
                let e = McError::new("E103", "control changed hands before its turn");
                self.report(&job.sess, &job.cmd, &job.v, Err(&e), 0.0, job.n);
                job.sess.send(&err(&job.id, &e));
                continue;
            }
            self.event("command", json!({"state": "started", "n": job.n, "by": job.sess.name, "cmd": job.cmd, "args": cmd_args_json(&job.v)}));
            let t0 = now_ns();
            let r = match job.cmd.as_str() {
                // a block: from "begin" to "end" only this client's commands run (see take_job)
                "begin" => {
                    *self.exclusive.lock().unwrap() = Some((job.sess.clone(), now_ns()));
                    Ok(json!({}))
                }
                "end" => {
                    *self.exclusive.lock().unwrap() = None;
                    Ok(json!({}))
                }
                _ => self.run_cmd(&job.sess, &job.cmd, &job.v),
            };
            if let Some(e) = self.exclusive.lock().unwrap().as_mut() {
                e.1 = now_ns(); // the block is alive
            }
            let ms = (now_ns() - t0) as f64 / 1e6;
            self.report(&job.sess, &job.cmd, &job.v, r.as_ref(), ms, job.n);
            match r {
                Ok(mut x) => {
                    if x.is_object() {
                        x["n"] = json!(job.n);
                    }
                    job.sess.send(&ok(&job.id, x))
                }
                Err(e) => {
                    let mut m = err(&job.id, &e);
                    m["n"] = json!(job.n);
                    job.sess.send(&m)
                }
            }
        }
    }

    fn run_cmd(&self, sess: &Arc<Session>, cmd: &str, v: &Value) -> McResult<Value> {
        let gen = self.stage.gen();
        let xyz = |a: &str, b: &str, c: &str| [f3(v, a), f3(v, b), f3(v, c)];
        match cmd {
            "move" => {
                let d = xyz("dx", "dy", "dz");
                self.stage.call(move |w| w.move_by(d, gen))
            }
            "goto" => {
                let t = xyz("x", "y", "z");
                self.stage.call(move |w| w.move_to(t, gen))
            }
            "snap" => self.snap(sess, v, gen),
            "sleep" => {
                // a pause in the queue (e.g. let the sample settle); stop ends it
                let cancel = self.cancel.load(Ordering::SeqCst);
                let end = now_ns() + (f3(v, "ms").unwrap_or(0.0).clamp(0.0, 60_000.0) * 1e6) as u64;
                while now_ns() < end {
                    if self.cancel.load(Ordering::SeqCst) != cancel {
                        return Err(McError::new("E116", ""));
                    }
                    std::thread::sleep(Duration::from_millis(10).min(Duration::from_nanos(end - now_ns())));
                }
                Ok(json!({}))
            }
            "zero" => self.stage.call(|w| w.set_zero()),
            "confirm" => self.stage.call(|w| w.confirm()),
            "light" => {
                let l = v["level"].as_u64().unwrap_or(0) as u32;
                self.stage.call(move |w| w.light(l))
            }
            "laser" => {
                let mw = v["mw"].as_u64().unwrap_or(0) as u32;
                self.stage.call(move |w| w.laser(mw))
            }
            "motors" => {
                let on = v["on"].as_bool().unwrap_or(false);
                self.stage.call(move |w| w.motors(on))
            }
            "cooling" => {
                let on = v["on"].as_bool().unwrap_or(true);
                self.stage.call(move |w| w.cooling(on))
            }
            "temperature" => self.stage.call(|w| w.temperature()),
            "exposure" | "gain" | "auto_exposure" | "white_balance" | "camera_state" => {
                let mut c = v.clone();
                if cmd == "camera_state" {
                    c["cmd"] = json!("state");
                }
                let r = self.cam.cmd(c)?;
                if let Some(e) = r.get("exposure_ms") {
                    self.cam.info.lock().unwrap()["exposure_ms"] = e.clone();
                }
                Ok(r)
            }
            _ => Err(McError::new("E100", cmd.to_string())),
        }
    }

    /// Move (if asked), wait until the stage is still, deliver the first frame exposed after that — for sure.
    fn snap(&self, sess: &Arc<Session>, v: &Value, gen: u64) -> McResult<Value> {
        let t = [f3(v, "x"), f3(v, "y"), f3(v, "z")];
        let d = [f3(v, "dx"), f3(v, "dy"), f3(v, "dz")];
        if t.iter().any(|a| a.is_some()) {
            self.stage.call(move |w| w.move_to(t, gen))?;
        } else if d.iter().any(|a| a.is_some()) {
            self.stage.call(move |w| w.move_by(d, gen))?;
        }
        self.cam.wait_ok(3000)?;
        let settle = f3(v, "settle_ms").unwrap_or(0.0).max(0.0);
        let t_ready = self.stage.shared.lock().unwrap().settled_ns.max(now_ns()) + (settle * 1e6) as u64;   // a fresh frame, taken still
        let timeout = Duration::from_millis(f3(v, "timeout_ms").unwrap_or(5000.0).max(0.0) as u64);
        let f = self.hub.wait_after(t_ready, timeout).ok_or_else(|| McError::new("E505", "no frame after the stage settled"))?;
        if let Some(ob) = sess.outbox.lock().unwrap().clone() {
            ob.push_reliable(f.clone(), false);
        }
        Ok(json!({"seq": f.seq, "meta": f.meta}))
    }

    fn clients(&self) -> Value {
        let list = self.sessions.lock().unwrap().clone();
        Value::Array(
            list.iter()
                .map(|s| {
                    let ob = s.outbox.lock().unwrap().clone();
                    json!({"name": s.name, "role": s.role.name(),
                           "addr": s.addr.ip().to_string(), "rtt_us": s.rtt_us.load(Ordering::Relaxed),
                           "frames": ob.as_ref().map(|o| o.sent_frames.load(Ordering::Relaxed)),
                           "dropped": ob.as_ref().map(|o| o.dropped.load(Ordering::Relaxed))})
                })
                .collect(),
        )
    }

    // ---------------------------------------------------------------- data

    fn data_conn(self: Arc<Self>, c: TcpStream) {
        let addr = c.peer_addr().ok();
        if !addr.map_or(false, |a| self.cfg.access.allowed(&a.ip())) {
            return; // the control connection already told it why
        }
        let _ = c.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = c.set_nodelay(true);
        let mut rd = match c.try_clone() {
            Ok(r) => r,
            Err(_) => return,
        };
        let mut buf = Vec::new();
        let hello: Value = match read_msg_max(&mut rd, &mut buf, MAX_CMD) {
            Ok(K_JSON) => serde_json::from_slice(&buf).unwrap_or(Value::Null),
            _ => return,
        };
        let sid = hello["session"].as_str().unwrap_or("");
        let sess = self.sessions.lock().unwrap().iter().find(|s| s.id == sid).cloned();
        // a small send buffer: on a slow link a write waits instead of queueing seconds of frames,
        // so a snap goes out right behind the frame in flight and "auto" sees how busy the link really is
        let _ = socket2::SockRef::from(&c).set_send_buffer_size(1 << 20);
        let mut w = BufWriter::with_capacity(1 << 18, c);
        let Some(sess) = sess else {
            let _ = write_json(&mut w, &json!({"ok": false, "error": McError::new("E106", "").json()}));
            return;
        };
        let raw = hello["want"].as_str() == Some("raw");
        // full frames: a fixed codec, or "auto" (the default): live frames as good as the link carries
        let codec = hello["codec"].as_str().unwrap_or("auto");
        let fixed: Option<Pack> = match codec {
            "auto" => None,
            "jpeg" => Some(Pack { codec: Codec::Jpeg, quality: f3(&hello, "quality").unwrap_or(92.0).clamp(30.0, 100.0) as u8 }),
            c => match c.parse::<Codec>() {
                Ok(k) => Some(Pack { codec: k, quality: 0 }),
                Err(e) => {
                    let _ = write_json(&mut w, &json!({"ok": false, "error": McError::new("E101", e).json()}));
                    let _ = std::io::Write::flush(&mut w);
                    return;
                }
            },
        };
        let want = if raw {
            Want::Raw
        } else {
            let fps = f3(&hello, "fps").unwrap_or(self.cfg.viewer_fps).clamp(0.1, 30.0);
            Want::Preview { gap_ns: (1e9 / fps) as u64 }
        };
        let ob = Outbox::new(want);
        if let Some(old) = sess.outbox.lock().unwrap().replace(ob.clone()) {
            old.close();
            self.hub.remove(&old);
        }
        self.hub.add(ob.clone());
        let _ = write_json(&mut w, &json!({"ok": true}));
        use std::io::Write;
        let _ = w.flush();
        crate::debug!("DATA", "{} data link: {}", sess.who(), if raw { "full RAW stream" } else { "preview stream" });
        let _ = addr;
        let mut auto = AutoPack::new();
        while let Some((f, preview, reliable)) = ob.next() {
            if !sess.alive.load(Ordering::Relaxed) {
                break;
            }
            let r = if preview {
                let (h, j) = f.preview();
                write_frame(&mut w, h, j).map(|_| h.len() + j.len())
            } else {
                // snaps and re-fetches: exact pixels (unless a lossy codec was asked for); live: fixed or auto
                let p = match fixed {
                    Some(p) => p,
                    None if reliable => Pack::LOSSLESS,
                    None => auto.pack(),
                };
                *ob.pack.lock().unwrap() = p;
                let x = f.packed(p);
                write_frame(&mut w, &x.0, &x.1).map(|_| x.0.len() + x.1.len())
            };
            let r = r.and_then(|n| w.flush().map(|_| n));
            if !preview && !reliable && fixed.is_none() {
                if let Some(to) = auto.update(*r.as_ref().unwrap_or(&0), ob.dropped.load(Ordering::Relaxed)) {
                    info!("DATA", "{}  live frames now {}{}", sess.who(), to.label(),
                          if to == Pack::LOSSLESS { "" } else { " (link busy; snaps stay lossless)" });
                }
            }
            match r {
                Ok(n) => {
                    ob.sent_frames.fetch_add(1, Ordering::Relaxed);
                    ob.sent_bytes.fetch_add(n as u64 + 9, Ordering::Relaxed);
                    if !reliable {
                        ob.last_sent_ns.store(now_ns(), Ordering::Relaxed);
                    }
                }
                Err(_) => break,
            }
        }
        self.hub.remove(&ob);
        ob.close();
        crate::debug!("DATA", "{} data link closed", sess.who());
    }

    /// A finished operator command, said once in one shape: a log line for the console and a `command` event
    /// for every client: what was asked, what came out (or the error), how long it took, by whom.
    fn report(&self, sess: &Session, cmd: &str, v: &Value, r: Result<&Value, &McError>, ms: f64, n: u64) {
        if is_read(cmd) {
            return;
        }
        let tag_n = format!("#{n}");
        let mut args = cmd_args_json(v);
        if let Some(o) = args.as_object_mut() {
            o.remove("step");
        }
        let shown = cmd_args(&args);
        // after the number the line still starts with a capital, like every other log line
        let title: String = cmd.chars().take(1).flat_map(char::to_uppercase).chain(cmd.chars().skip(1)).collect();
        let by = paint(DIM, &format!("{ms:.0} ms  by {}", sess.name));
        let mut ev = json!({"n": n, "by": sess.name, "role": sess.role.name(), "cmd": cmd, "args": args,
                            "ms": ms.round(), "t_wall_ms": mc_proto::clock::wall_ms()});
        match r {
            Err(e) => {
                crate::log::log(Level::Warn, cmd_tag(cmd), &format!("{} {title} {shown}  {e}  {by}", paint(DIM, &tag_n)));
                ev["state"] = json!("failed");
                ev["error"] = e.json();
            }
            Ok(r) => {
                let xyz = |a: &Value| a.as_array().map(|p| p.iter().map(|x| format!("{:.1}", x.as_f64().unwrap_or(0.0))).collect::<Vec<_>>().join(", "));
                let out = if let Some(p) = r.get("pos_um").and_then(xyz) {
                    format!("at ({p})")
                } else if let Some(seq) = r.get("seq") {
                    format!("frame {seq} at ({})", xyz(&r["meta"]["xyz_um"]).unwrap_or_default())
                } else {
                    String::new()
                };
                let level = if cmd == "laser" && v["mw"].as_f64().unwrap_or(0.0) > 0.0 { Level::Warn } else { Level::Info }; // laser on stands out
                let line = [format!("{} {title} {shown}", paint(DIM, &tag_n)), if out.is_empty() { out.clone() } else { paint(BOLD, &out) }, by];
                crate::log::log(level, cmd_tag(cmd), &line.iter().filter(|p| !p.trim().is_empty()).cloned().collect::<Vec<_>>().join("  "));
                let mut result = r.clone();
                if let Some(o) = result.as_object_mut() {
                    o.remove("id");
                    o.remove("ok");
                }
                ev["state"] = json!("done");
                ev["result"] = result;
                ev["summary"] = json!(out);
            }
        }
        self.remember(&ev);
        self.event("command", ev);
    }

    /// The history: the last 2000 commands and messages, for `history` (anyone may read it).
    fn remember(&self, ev: &Value) {
        let mut h = self.history.lock().unwrap();
        let mut e = ev.clone();
        e["event"] = json!(if ev.get("data").is_some() { "message" } else { "command" });
        if let Some(r) = e.get_mut("result").and_then(|r| r.as_object_mut()) {
            r.remove("meta"); // the frame header is long; seq and the summary say enough
        }
        h.push_back(e);
        while h.len() > 2000 {
            h.pop_front();
        }
    }

    // ---------------------------------------------------------------- background

    fn watchdog(self: Arc<Self>) {
        loop {
            std::thread::sleep(Duration::from_millis(200));
            if let Some(h) = self.holder() {
                let silent = (now_ns() - h.last_seen.load(Ordering::Relaxed)) / 1_000_000;
                if silent > self.cfg.dead_ms {
                    error!("NET", "{}  silent for {} ms: E105 control released", h.who(), silent);
                    self.event("error", json!({"by": h.name, "error": McError::new("E105", format!("{silent} ms")).json()}));
                    *self.lease.lock().unwrap() = None;
                    self.safe_stop("operator silent");
                    self.event("operator", json!({"name": null}));
                }
            }
        }
    }

    fn telemetry(self: Arc<Self>) {
        let mut last_log = now_ns();
        let mut prev: std::collections::HashMap<String, (u64, u64)> = Default::default();
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let ev = json!({"event": "telemetry", "t_server": now_ns(), "stage": self.stage.status(),
                            "camera": {"ok": self.cam.ok.load(Ordering::Relaxed), "fps": (self.hub.fps() * 10.0).round() / 10.0,
                                       "info": *self.cam.info.lock().unwrap()},
                            "operator": self.holder().map(|h| h.name.clone()), "clients": self.clients(),
                            "queue": self.queue.lock().unwrap().len()});
            self.broadcast(&ev);
            if now_ns() - last_log >= 10_000_000_000 {
                let dt = (now_ns() - last_log) as f64 / 1e9;
                last_log = now_ns();
                let list = self.sessions.lock().unwrap().clone();
                for s in list {
                    if let Some(ob) = s.outbox.lock().unwrap().clone() {
                        let (fr, by) = (ob.sent_frames.load(Ordering::Relaxed), ob.sent_bytes.load(Ordering::Relaxed));
                        let (pf, pb) = prev.get(&s.id).copied().unwrap_or((0, 0));
                        prev.insert(s.id.clone(), (fr, by));
                        let what = if *ob.want.lock().unwrap() == Want::Raw { ob.pack.lock().unwrap().label() } else { "preview".into() };
                        info!("DATA", "{}  {:.1} fps  {}  {}  dropped {}  rtt {:.2} ms", s.who(), (fr - pf) as f64 / dt,
                              rate((by - pb) as f64 / dt), what, ob.dropped.load(Ordering::Relaxed), s.rtt_us.load(Ordering::Relaxed) as f64 / 1000.0);
                    }
                }
            }
        }
    }

    fn discovery(self: Arc<Self>) {
        let Some(port) = self.cfg.discovery_port else { return };
        let Ok(sock) = UdpSocket::bind((self.cfg.bind, port)) else {
            warn!("NET", "Discovery port {port} busy: clients must give the address");
            return;
        };
        let mut b = [0u8; 64];
        loop {
            if let Ok((n, from)) = sock.recv_from(&mut b) {
                if &b[..n] == DISCOVERY_ASK && self.cfg.access.allowed(&from.ip()) {
                    let v = json!({"mcontrol": mc_proto::VERSION, "name": self.cfg.name, "control_port": self.cfg.control_port,
                                   "data_port": self.cfg.data_port, "operator": self.holder().map(|h| h.name.clone()),
                                   "camera": self.cam.info.lock().unwrap()["model"], "controller": self.stage.port,
                                   "token": self.cfg.access.needs_token()});
                    let _ = sock.send_to(v.to_string().as_bytes(), from);
                    crate::debug!("NET", "discovery answer to {from}");
                }
            }
        }
    }
}

fn cmd_args(v: &Value) -> String {
    v.as_object()
        .map(|o| o.iter().filter(|(k, _)| !matches!(k.as_str(), "cmd" | "id")).map(|(k, x)| format!("{k}={x}")).collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

fn cmd_tag(cmd: &str) -> &'static str {
    if matches!(cmd, "exposure" | "gain" | "auto_exposure" | "white_balance") { "CAM" } else { "STAGE" }
}

fn cmd_args_json(v: &Value) -> Value {
    let mut a = v.clone();
    if let Some(o) = a.as_object_mut() {
        o.remove("cmd");
        o.remove("id");
    }
    a
}

/// Reads change nothing: they are not announced.
fn is_read(cmd: &str) -> bool {
    matches!(cmd, "camera_state" | "temperature")
}

fn client_json(sess: &Session, action: &str) -> Value {
    json!({"action": action, "role": sess.role.name(), "name": sess.name,
           "addr": sess.addr.ip().to_string(), "after_s": ((now_ns() - sess.since) as f64 / 1e9).round()})
}

/// "auto" for live full frames: one step down the ladder (smaller, lossy) when frames get dropped because the
/// link cannot keep up; one step back up when the better step would fit into what the link has shown it carries.
struct AutoPack {
    step: usize,
    since: u64,
    bytes: u64,
    frames: u64,
    dropped: u64,
    calm: u32,
    size: [f64; Pack::LADDER.len()], // average bytes per frame at each step (0: not seen yet)
    capacity: f64,                    // bytes/s the link carried while it was the bottleneck (0: not seen yet)
}

impl AutoPack {
    fn new() -> AutoPack {
        AutoPack { step: 0, since: now_ns(), bytes: 0, frames: 0, dropped: u64::MAX, calm: 0, size: [0.0; Pack::LADDER.len()], capacity: 0.0 }
    }

    fn pack(&self) -> Pack {
        Pack::LADDER[self.step]
    }

    /// After each live frame: its size and the outbox's drop counter. Returns the new pack if it changed.
    fn update(&mut self, bytes: usize, dropped: u64) -> Option<Pack> {
        self.bytes += bytes as u64;
        self.frames += 1;
        let s = &mut self.size[self.step];
        *s = if *s == 0.0 { bytes as f64 } else { 0.8 * *s + 0.2 * bytes as f64 };
        if self.dropped == u64::MAX {
            self.dropped = dropped;
        }
        let window = (now_ns() - self.since) as f64 / 1e9;
        if window < 1.0 {
            return None;
        }
        let (lost, rate, fps) = (dropped - self.dropped, self.bytes as f64 / window, self.frames as f64 / window);
        (self.since, self.bytes, self.frames, self.dropped) = (now_ns(), 0, 0, dropped);
        let before = self.step;
        if lost >= 2 {
            self.capacity = self.capacity.max(rate); // the link was full: this is what it carries
            self.calm = 0;
            if self.step + 1 < Pack::LADDER.len() {
                self.step += 1;
            }
        } else if lost == 0 && self.step > 0 {
            self.calm += 1;
            // the camera's rate at the better step, if it fits with a margin
            let better = self.size[self.step - 1];
            let need = (fps + lost as f64) * better;
            if self.calm >= 3 && (self.capacity == 0.0 || need < 0.8 * self.capacity) {
                self.step -= 1;
                self.calm = 0;
            }
        }
        (self.step != before).then(|| self.pack())
    }
}

/// Build a frame from the worker's header: number it and add where the stage was during the exposure.
pub fn make_frame(hub: &Hub, stage: &Stage, hdr: Value, px: Vec<u8>) -> Frame {
    let seq = hub.next_seq();
    let t0 = hdr["t_exp_start"].as_u64().unwrap_or(0);
    let t1 = hdr["t_exp_end"].as_u64().unwrap_or(t0);
    let mid = t0 + (t1 - t0) / 2;
    let sh = stage.shared.lock().unwrap();
    let (p0, m0, settled) = sh.at(t0);
    let (pm, mm, _) = sh.at(mid);
    let (_, m1, _) = sh.at(t1);
    drop(sh);
    let mut meta = hdr;
    meta["seq"] = json!(seq);
    meta["xyz_um"] = json!([r3(pm[0]), r3(pm[1]), r3(pm[2])]);
    meta["xyz_start_um"] = json!([r3(p0[0]), r3(p0[1]), r3(p0[2])]);
    meta["moving"] = json!(m0 || mm || m1);
    meta["settled_ms"] = json!((settled * 10.0).round() / 10.0);
    meta["t_server"] = json!(now_ns());
    Frame::new(seq, meta, px)
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}
