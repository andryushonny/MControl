//! Coloured console log: who talks to the server, what it sends, what the hardware does.
//!
//! `21:04:07.512  NET    + operator alice-pc 192.168.1.20  control connected`

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static COLOR: AtomicBool = AtomicBool::new(true);
static DEBUG: AtomicBool = AtomicBool::new(false);
static LOCK: Mutex<()> = Mutex::new(());
static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub const RESET: &str = "\x1b[0m";
pub const DIM: &str = "\x1b[2m";
pub const BOLD: &str = "\x1b[1m";
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const BLUE: &str = "\x1b[34m";
pub const MAGENTA: &str = "\x1b[35m";
pub const CYAN: &str = "\x1b[36m";
pub const WHITE: &str = "\x1b[37m";
pub const BRED: &str = "\x1b[1;31m";

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

/// Turn colours on (Windows: enable ANSI in the console) or off; write a plain copy of the log to `file`.
pub fn init(color: bool, debug: bool, file: Option<&str>) {
    let ok = color && enable_ansi();
    COLOR.store(ok, Ordering::Relaxed);
    DEBUG.store(debug, Ordering::Relaxed);
    if let Some(p) = file {
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            *FILE.lock().unwrap() = Some(f);
        }
    }
}

pub fn color() -> bool {
    COLOR.load(Ordering::Relaxed)
}

/// Paint `s` with `c` if colours are on.
pub fn paint(c: &str, s: &str) -> String {
    if color() {
        format!("{c}{s}{RESET}")
    } else {
        s.to_string()
    }
}

fn tag_color(tag: &str) -> &'static str {
    match tag {
        "NET" => CYAN,
        "OP" => GREEN,
        "VIEW" => BLUE,
        "CAM" => MAGENTA,
        "STAGE" => YELLOW,
        "GUARD" => BRED,
        "DATA" => CYAN,
        _ => WHITE,
    }
}

pub fn log(level: Level, tag: &str, msg: &str) {
    if level == Level::Debug && !DEBUG.load(Ordering::Relaxed) {
        return;
    }
    let t = local_time();
    let _g = LOCK.lock().unwrap();
    let (lc, lv) = match level {
        Level::Debug => (DIM, ""),
        Level::Info => ("", ""),
        Level::Warn => (YELLOW, "warning: "),
        Level::Error => (BRED, "ERROR: "),
    };
    let line = if color() {
        format!("{DIM}{t}{RESET}  {}{tag:<6}{RESET} {lc}{lv}{msg}{RESET}\n", tag_color(tag))
    } else {
        format!("{t}  {tag:<6} {lv}{msg}\n")
    };
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
    if let Some(f) = FILE.lock().unwrap().as_mut() {
        let plain = format!("{} {t}  {tag:<6} {lv}{}\n", today(), strip(msg));
        let _ = f.write_all(plain.as_bytes());
    }
}

fn strip(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut esc = false;
    for ch in s.chars() {
        if esc {
            if ch == 'm' {
                esc = false;
            }
        } else if ch == '\x1b' {
            esc = true;
        } else {
            out.push(ch);
        }
    }
    out
}

#[macro_export]
macro_rules! info { ($tag:expr, $($a:tt)*) => { $crate::log::log($crate::log::Level::Info, $tag, &format!($($a)*)) } }
#[macro_export]
macro_rules! warn { ($tag:expr, $($a:tt)*) => { $crate::log::log($crate::log::Level::Warn, $tag, &format!($($a)*)) } }
#[macro_export]
macro_rules! error { ($tag:expr, $($a:tt)*) => { $crate::log::log($crate::log::Level::Error, $tag, &format!($($a)*)) } }
#[macro_export]
macro_rules! debug { ($tag:expr, $($a:tt)*) => { $crate::log::log($crate::log::Level::Debug, $tag, &format!($($a)*)) } }

/// Human size of a byte rate.
pub fn rate(bytes_per_s: f64) -> String {
    if bytes_per_s >= 1e6 {
        format!("{:.1} MB/s", bytes_per_s / 1e6)
    } else {
        format!("{:.0} kB/s", bytes_per_s / 1e3)
    }
}

// ---------------------------------------------------------------- local time and ANSI, without extra crates

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct SystemTime {
    y: u16,
    mo: u16,
    dow: u16,
    d: u16,
    h: u16,
    mi: u16,
    s: u16,
    ms: u16,
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetLocalTime(t: *mut SystemTime);
    fn GetStdHandle(n: u32) -> isize;
    fn GetConsoleMode(h: isize, m: *mut u32) -> i32;
    fn SetConsoleMode(h: isize, m: u32) -> i32;
    fn SetConsoleOutputCP(cp: u32) -> i32;
}

#[cfg(windows)]
fn local_parts() -> (u16, u16, u16, u16, u16, u16, u16) {
    let mut t = SystemTime::default();
    unsafe { GetLocalTime(&mut t) };
    (t.y, t.mo, t.d, t.h, t.mi, t.s, t.ms)
}

#[cfg(windows)]
fn enable_ansi() -> bool {
    unsafe {
        SetConsoleOutputCP(65001);
        let h = GetStdHandle((-11i32) as u32);
        let mut m = 0u32;
        if GetConsoleMode(h, &mut m) == 0 {
            return false; // not a console (redirected): no colours
        }
        SetConsoleMode(h, m | 0x0004) != 0
    }
}

#[cfg(unix)]
fn local_parts() -> (u16, u16, u16, u16, u16, u16, u16) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    (
        (tm.tm_year + 1900) as u16,
        (tm.tm_mon + 1) as u16,
        tm.tm_mday as u16,
        tm.tm_hour as u16,
        tm.tm_min as u16,
        tm.tm_sec as u16,
        now.subsec_millis() as u16,
    )
}

#[cfg(unix)]
fn enable_ansi() -> bool {
    unsafe { libc::isatty(1) == 1 }
}

fn local_time() -> String {
    let (_, _, _, h, mi, s, ms) = local_parts();
    format!("{h:02}:{mi:02}:{s:02}.{ms:03}")
}

fn today() -> String {
    let (y, mo, d, ..) = local_parts();
    format!("{y}-{mo:02}-{d:02}")
}
