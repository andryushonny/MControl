//! One time base for every process on a machine: the OS monotonic clock in nanoseconds
//! (Windows: QueryPerformanceCounter, Linux: CLOCK_MONOTONIC). The server, its camera worker and its guard
//! all read the same clock, so their timestamps can be compared directly. Clients convert server times with the
//! offset they measure (`ping`: t1, t2, t3, t4).

#[cfg(windows)]
mod imp {
    #[link(name = "kernel32")]
    extern "system" {
        fn QueryPerformanceCounter(c: *mut i64) -> i32;
        fn QueryPerformanceFrequency(f: *mut i64) -> i32;
    }

    pub fn now_ns() -> u64 {
        let (mut c, mut f) = (0i64, 0i64);
        unsafe {
            QueryPerformanceCounter(&mut c);
            QueryPerformanceFrequency(&mut f);
        }
        let (c, f) = (c as u128, f.max(1) as u128);
        (c * 1_000_000_000 / f) as u64
    }
}

#[cfg(unix)]
mod imp {
    pub fn now_ns() -> u64 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
    }
}

/// Monotonic time of this machine, ns (shared by all processes on it).
pub fn now_ns() -> u64 {
    imp::now_ns()
}

/// Wall clock, ms since 1970 (for logs and files only, never for measuring).
pub fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
