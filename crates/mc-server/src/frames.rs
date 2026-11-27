//! Frames: a ring of the last N (for snap delivery and re-fetch), one outbox per data client.
//! Each client has its own sending thread, so a slow viewer never holds back the operator or the camera:
//! the camera thread only swaps a pointer into every outbox. A preview JPEG is made at most once per frame,
//! by whichever viewer thread needs it first, and shared.

use crate::codec::{self, Codec};
use mc_proto::clock::now_ns;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

pub struct Frame {
    pub seq: u64,
    pub meta: Value,
    pub hdr: Vec<u8>,
    pub px: Vec<u8>,
    pub t_exp_start: u64,
    pub w: usize,
    pub h: usize,
    pub fmt: String,
    pub bits: u32,
    preview: OnceLock<(Vec<u8>, Vec<u8>)>,
    packed: Mutex<Vec<(Pack, Arc<(Vec<u8>, Vec<u8>)>)>>,
}

/// How a full frame goes over the link: a codec and, for JPEG, its quality.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pack {
    pub codec: Codec,
    pub quality: u8,
}

impl Pack {
    pub const RAW: Pack = Pack { codec: Codec::Raw, quality: 0 };
    pub const LOSSLESS: Pack = Pack { codec: Codec::Lossless, quality: 0 };
    /// What "auto" walks along: best first. Snaps and re-fetches never leave the top step.
    pub const LADDER: [Pack; 4] = [Pack::LOSSLESS, Pack { codec: Codec::Jpeg, quality: 92 },
                                   Pack { codec: Codec::Jpeg, quality: 85 }, Pack { codec: Codec::Jpeg, quality: 75 }];

    pub fn label(self) -> String {
        if self.codec == Codec::Jpeg { format!("jpeg {}", self.quality) } else { self.codec.name().into() }
    }
}

impl Frame {
    pub fn new(seq: u64, meta: Value, px: Vec<u8>) -> Frame {
        let hdr = serde_json::to_vec(&meta).unwrap();
        Frame {
            seq,
            t_exp_start: meta["t_exp_start"].as_u64().unwrap_or(0),
            w: meta["w"].as_u64().unwrap_or(0) as usize,
            h: meta["h"].as_u64().unwrap_or(0) as usize,
            fmt: meta["fmt"].as_str().unwrap_or("").to_string(),
            bits: meta["bits"].as_u64().unwrap_or(8) as u32,
            meta,
            hdr,
            px,
            preview: OnceLock::new(),
            packed: Mutex::new(Vec::new()),
        }
    }

    /// (header, payload) of the full frame packed with `p`; made once per pack, shared by all senders.
    pub fn packed(&self, mut p: Pack) -> Arc<(Vec<u8>, Vec<u8>)> {
        if p.codec == Codec::Jpeg && (self.bits > 8 || !self.fmt.starts_with("bayer")) {
            p = Pack::LOSSLESS; // JPEG planes are for 8-bit Bayer only
        }
        if let Some(x) = self.packed.lock().unwrap().iter().find(|x| x.0 == p) {
            return x.1.clone();
        }
        let bpp = if self.bits > 8 { 2 } else { 1 };
        let payload = match p.codec {
            Codec::Raw => self.px.clone(),
            Codec::Lossless => codec::lossless(&self.px, self.w, self.h, bpp),
            Codec::Jpeg => codec::jpeg_planes(&self.px, self.w, self.h, p.quality),
        };
        let mut m = self.meta.clone();
        m["codec"] = json!(p.codec.name());
        if p.codec == Codec::Jpeg {
            m["quality"] = json!(p.quality);
        }
        m["raw_bytes"] = json!(self.px.len());
        let x = Arc::new((serde_json::to_vec(&m).unwrap(), payload));
        self.packed.lock().unwrap().push((p, x.clone()));
        x
    }

    /// (header, JPEG) of the preview: 1/4 size colour picture.
    pub fn preview(&self) -> &(Vec<u8>, Vec<u8>) {
        self.preview.get_or_init(|| {
            let (rgb, w, h) = quarter_rgb(&self.px, self.w, self.h, &self.fmt, self.bits);
            let mut jpg = Vec::with_capacity(w * h / 4);
            let enc = jpeg_encoder::Encoder::new(&mut jpg, 80);
            let _ = enc.encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb);
            let mut m = self.meta.clone();
            m["preview"] = json!(true);
            m["fmt"] = json!("jpeg");
            m["w"] = json!(w);
            m["h"] = json!(h);
            m["full_w"] = json!(self.w);
            m["full_h"] = json!(self.h);
            (serde_json::to_vec(&m).unwrap(), jpg)
        })
    }
}

/// 4x smaller RGB: Bayer 2x2 cells (no interpolation), then 2x2 average. Mono: 4x4 average.
fn quarter_rgb(px: &[u8], w: usize, h: usize, fmt: &str, bits: u32) -> (Vec<u8>, usize, usize) {
    let (ow, oh) = (w / 4, h / 4);
    let mut out = vec![0u8; ow * oh * 3];
    let bpp = if bits > 8 { 2 } else { 1 };
    let shift = bits.saturating_sub(8);
    let get = |x: usize, y: usize| -> u32 {
        let i = (y * w + x) * bpp;
        if i + bpp > px.len() {
            return 0;
        }
        if bpp == 1 {
            px[i] as u32
        } else {
            (u16::from_le_bytes([px[i], px[i + 1]]) as u32) >> shift
        }
    };
    // where R and B sit in the 2x2 cell
    let (r_at, b_at) = match &fmt.get(6..8).unwrap_or("") {
        &"rg" => ((0, 0), (1, 1)),
        &"gr" => ((1, 0), (0, 1)),
        &"gb" => ((0, 1), (1, 0)),
        &"bg" => ((1, 1), (0, 0)),
        _ => ((9, 9), (9, 9)),
    };
    let bayer = fmt.starts_with("bayer") && r_at.0 < 2;
    for oy in 0..oh {
        for ox in 0..ow {
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for cy in 0..2 {
                for cx in 0..2 {
                    let x0 = ox * 4 + cx * 2;
                    let y0 = oy * 4 + cy * 2;
                    if bayer {
                        r += get(x0 + r_at.0, y0 + r_at.1);
                        b += get(x0 + b_at.0, y0 + b_at.1);
                        let (g1, g2) = ((1 - r_at.0, r_at.1), (r_at.0, 1 - r_at.1));
                        g += (get(x0 + g1.0, y0 + g1.1) + get(x0 + g2.0, y0 + g2.1)) / 2;
                    } else {
                        let v = (get(x0, y0) + get(x0 + 1, y0) + get(x0, y0 + 1) + get(x0 + 1, y0 + 1)) / 4;
                        r += v;
                        g += v;
                        b += v;
                    }
                }
            }
            let o = (oy * ow + ox) * 3;
            out[o] = (r / 4).min(255) as u8;
            out[o + 1] = (g / 4).min(255) as u8;
            out[o + 2] = (b / 4).min(255) as u8;
        }
    }
    (out, ow, oh)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Want {
    Raw,
    Preview { gap_ns: u64 },
}

pub struct OutState {
    pub latest: Option<Arc<Frame>>,
    pub reliable: VecDeque<(Arc<Frame>, bool)>, // (frame, as preview)
    pub closed: bool,
}

pub struct Outbox {
    pub want: Mutex<Want>,
    pub st: Mutex<OutState>,
    pub cond: Condvar,
    pub sent_frames: AtomicU64,
    pub sent_bytes: AtomicU64,
    pub dropped: AtomicU64,
    pub last_sent_ns: AtomicU64,
    pub paused: std::sync::atomic::AtomicBool,
    pub pack: Mutex<Pack>, // how the last full frame went: for the log
}

impl Outbox {
    pub fn new(want: Want) -> Arc<Outbox> {
        Arc::new(Outbox {
            want: Mutex::new(want),
            st: Mutex::new(OutState { latest: None, reliable: VecDeque::new(), closed: false }),
            cond: Condvar::new(),
            sent_frames: AtomicU64::new(0),
            sent_bytes: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            last_sent_ns: AtomicU64::new(0),
            paused: std::sync::atomic::AtomicBool::new(false),
            pack: Mutex::new(Pack::RAW),
        })
    }

    /// A frame that must arrive (snap, re-fetch).
    pub fn push_reliable(&self, f: Arc<Frame>, preview: bool) {
        let mut s = self.st.lock().unwrap();
        s.reliable.push_back((f, preview));
        self.cond.notify_all();
    }

    pub fn close(&self) {
        self.st.lock().unwrap().closed = true;
        self.cond.notify_all();
    }

    /// Next thing to send, or None when closed. Reliable frames first; then the newest frame (older ones dropped).
    pub fn next(&self) -> Option<(Arc<Frame>, bool, bool)> {
        let mut s = self.st.lock().unwrap();
        loop {
            if s.closed {
                return None;
            }
            if let Some((f, p)) = s.reliable.pop_front() {
                return Some((f, p, true));
            }
            let want = *self.want.lock().unwrap();
            let paused = self.paused.load(Ordering::Relaxed);
            if !paused {
                if let Some(f) = s.latest.as_ref() {
                    match want {
                        Want::Raw => {
                            let f = s.latest.take().unwrap();
                            return Some((f, false, false));
                        }
                        Want::Preview { gap_ns } => {
                            let due = self.last_sent_ns.load(Ordering::Relaxed) + gap_ns;
                            let now = now_ns();
                            if now >= due {
                                let f = f.clone();
                                s.latest = None;
                                return Some((f, true, false));
                            }
                            let (g, _) = self.cond.wait_timeout(s, Duration::from_nanos(due - now)).unwrap();
                            s = g;
                            continue;
                        }
                    }
                }
            }
            let (g, _) = self.cond.wait_timeout(s, Duration::from_millis(500)).unwrap();
            s = g;
        }
    }
}

pub struct Hub {
    ring: Mutex<VecDeque<Arc<Frame>>>,
    ring_cond: Condvar,
    cap: usize,
    outboxes: Mutex<Vec<Arc<Outbox>>>,
    pub seq: AtomicU64,
    pub times: Mutex<VecDeque<u64>>,
}

impl Hub {
    pub fn new(cap: usize) -> Arc<Hub> {
        Arc::new(Hub {
            ring: Mutex::new(VecDeque::new()),
            ring_cond: Condvar::new(),
            cap: cap.max(2),
            outboxes: Mutex::new(Vec::new()),
            seq: AtomicU64::new(0),
            times: Mutex::new(VecDeque::new()),
        })
    }

    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn publish(&self, f: Arc<Frame>) {
        {
            let mut t = self.times.lock().unwrap();
            t.push_back(now_ns());
            while t.len() > 60 {
                t.pop_front();
            }
        }
        {
            let mut r = self.ring.lock().unwrap();
            r.push_back(f.clone());
            while r.len() > self.cap {
                r.pop_front();
            }
            self.ring_cond.notify_all();
        }
        for ob in self.outboxes.lock().unwrap().iter() {
            let mut s = ob.st.lock().unwrap();
            if s.latest.is_some() && *ob.want.lock().unwrap() == Want::Raw && !ob.paused.load(Ordering::Relaxed) {
                ob.dropped.fetch_add(1, Ordering::Relaxed);
            }
            s.latest = Some(f.clone());
            ob.cond.notify_all();
        }
    }

    pub fn fps(&self) -> f64 {
        let t = self.times.lock().unwrap();
        if t.len() < 2 {
            return 0.0;
        }
        let span = (t[t.len() - 1] - t[0]) as f64 / 1e9;
        if span <= 0.0 || now_ns() - t[t.len() - 1] > 2_000_000_000 {
            0.0
        } else {
            (t.len() - 1) as f64 / span
        }
    }

    pub fn add(&self, ob: Arc<Outbox>) {
        self.outboxes.lock().unwrap().push(ob);
    }

    pub fn remove(&self, ob: &Arc<Outbox>) {
        self.outboxes.lock().unwrap().retain(|o| !Arc::ptr_eq(o, ob));
    }

    pub fn get(&self, seq: u64) -> Option<Arc<Frame>> {
        self.ring.lock().unwrap().iter().find(|f| f.seq == seq).cloned()
    }

    pub fn latest(&self) -> Option<Arc<Frame>> {
        self.ring.lock().unwrap().back().cloned()
    }

    /// First frame whose exposure started at or after t (ns), waiting up to timeout.
    pub fn wait_after(&self, t: u64, timeout: Duration) -> Option<Arc<Frame>> {
        let end = now_ns() + timeout.as_nanos() as u64;
        let mut r = self.ring.lock().unwrap();
        loop {
            if let Some(f) = r.iter().find(|f| f.t_exp_start >= t) {
                return Some(f.clone());
            }
            let now = now_ns();
            if now >= end {
                return None;
            }
            let (g, _) = self.ring_cond.wait_timeout(r, Duration::from_nanos(end - now)).unwrap();
            r = g;
        }
    }
}
