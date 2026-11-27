//! MindVision camera SDK (MVCAMSDK_X64.dll), loaded at run time. Only what MControl needs:
//! raw frames straight from the sensor (no ISP on the weak PC), exposure, gain, white balance, trigger.

use libloading::{Library, Symbol};
use mc_proto::errors::{McError, McResult};
use std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DevInfo {
    pub product_series: [u8; 32],
    pub product_name: [u8; 32],
    pub friendly_name: [u8; 32],
    pub link_name: [u8; 32],
    pub driver_version: [u8; 32],
    pub sensor_type: [u8; 32],
    pub port_type: [u8; 32],
    pub sn: [u8; 32],
    pub instance: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct FrameHead {
    pub media_type: u32,
    pub bytes: u32,
    pub width: i32,
    pub height: i32,
    pub width_zoom_sw: i32,
    pub height_zoom_sw: i32,
    pub is_trigger: i32,
    pub timestamp: u32, // 0.1 ms units, camera clock
    pub exp_time_us: u32,
    pub analog_gain: f32,
    pub gamma: i32,
    pub contrast: i32,
    pub saturation: i32,
    pub r_gain: f32,
    pub g_gain: f32,
    pub b_gain: f32,
}

#[cfg(windows)]
macro_rules! cfn { ($($t:ty),*) => { unsafe extern "system" fn($($t),*) -> i32 } }
#[cfg(not(windows))]
macro_rules! cfn { ($($t:ty),*) => { unsafe extern "C" fn($($t),*) -> i32 } }

pub struct Sdk {
    lib: Library,
}

pub struct Cam {
    sdk: Sdk,
    h: i32,
    pub name: String,
    pub sn: String,
}

fn cstr(b: &[u8]) -> String {
    let n = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).trim().to_string()
}

pub fn sdk_error(code: i32, what: &str) -> McError {
    match code {
        -45 | -18 => McError::new("E504", format!("{what}: SDK code {code}")),
        _ => McError::new("E503", format!("{what}: SDK code {code}")),
    }
}

impl Sdk {
    /// Load the DLL from `dir` or the usual install folders.
    pub fn load(dir: Option<&str>) -> McResult<Sdk> {
        #[cfg(windows)]
        let (name, dirs) = ("MVCAMSDK_X64.dll", vec![r"C:\Program Files\MindVision\SDK\X64", r"C:\Program Files (x86)\MindVision\SDK\X64"]);
        #[cfg(not(windows))]
        let (name, dirs) = ("libMVSDK.so", vec!["/lib", "/usr/lib"]);
        let mut tried = Vec::new();
        let mut cands: Vec<std::path::PathBuf> = Vec::new();
        if let Some(d) = dir {
            cands.push(std::path::Path::new(d).join(name));
        }
        cands.push(name.into());
        for d in dirs {
            cands.push(std::path::Path::new(d).join(name));
        }
        for c in cands {
            match unsafe { Library::new(&c) } {
                Ok(lib) => {
                    let s = Sdk { lib };
                    unsafe {
                        if let Ok(f) = s.lib.get::<cfn!(i32)>(b"CameraSdkInit") {
                            f(1);
                        }
                    }
                    return Ok(s);
                }
                Err(e) => tried.push(format!("{}: {e}", c.display())),
            }
        }
        Err(McError::new("E502", format!("{name} not loaded ({})", tried.join("; "))))
    }

    unsafe fn f<T>(&self, n: &[u8]) -> McResult<Symbol<'_, T>> {
        self.lib.get::<T>(n).map_err(|_| McError::new("E503", format!("no {} in the SDK", String::from_utf8_lossy(&n[..n.len() - 1]))))
    }

    pub fn open_first(self) -> McResult<Cam> {
        unsafe {
            let mut list = [std::mem::zeroed::<DevInfo>(); 8];
            let mut n: i32 = 8;
            let r = self.f::<cfn!(*mut DevInfo, *mut i32)>(b"CameraEnumerateDevice\0")?(list.as_mut_ptr(), &mut n);
            if r != 0 || n <= 0 {
                return Err(McError::new("E501", format!("plug in the camera USB (SDK code {r})")));
            }
            let mut h: i32 = 0;
            let r = self.f::<cfn!(*const DevInfo, i32, i32, *mut i32)>(b"CameraInit\0")?(&list[0], -1, -1, &mut h);
            if r != 0 {
                return Err(sdk_error(r, "CameraInit"));
            }
            let name = cstr(&list[0].friendly_name);
            let sn = cstr(&list[0].sn);
            let cam = Cam { sdk: self, h, name: if name.is_empty() { cstr(&list[0].product_name) } else { name }, sn };
            cam.call1(b"CameraSetTriggerMode\0", 0)?; // continuous
            cam.call1(b"CameraSetAeState\0", 0)?; // manual exposure
            let _ = cam.call1(b"CameraSetWbMode\0", 0); // manual white balance (raw anyway)
            let _ = cam.call1(b"CameraSetFrameSpeed\0", 2); // high frame speed
            cam.set_exposure_us(20_000.0)?; // 20 ms, as the Optosky software
            cam.call0(b"CameraPlay\0")?;
            Ok(cam)
        }
    }
}

impl Cam {
    fn call0(&self, n: &[u8]) -> McResult<()> {
        unsafe {
            let r = self.sdk.f::<cfn!(i32)>(n)?(self.h);
            if r != 0 {
                return Err(sdk_error(r, &String::from_utf8_lossy(&n[..n.len() - 1])));
            }
        }
        Ok(())
    }

    fn call1(&self, n: &[u8], v: i32) -> McResult<()> {
        unsafe {
            let r = self.sdk.f::<cfn!(i32, i32)>(n)?(self.h, v);
            if r != 0 {
                return Err(sdk_error(r, &String::from_utf8_lossy(&n[..n.len() - 1])));
            }
        }
        Ok(())
    }

    pub fn set_exposure_us(&self, us: f64) -> McResult<()> {
        unsafe {
            let r = self.sdk.f::<cfn!(i32, f64)>(b"CameraSetExposureTime\0")?(self.h, us);
            if r != 0 {
                return Err(sdk_error(r, "CameraSetExposureTime"));
            }
        }
        Ok(())
    }

    pub fn exposure_us(&self) -> f64 {
        let mut v = 0f64;
        unsafe {
            if let Ok(f) = self.sdk.f::<cfn!(i32, *mut f64)>(b"CameraGetExposureTime\0") {
                f(self.h, &mut v);
            }
        }
        v
    }

    pub fn set_gain(&self, g: i32) -> McResult<()> {
        self.call1(b"CameraSetAnalogGain\0", g)
    }

    pub fn gain(&self) -> i32 {
        let mut v = 0i32;
        unsafe {
            if let Ok(f) = self.sdk.f::<cfn!(i32, *mut i32)>(b"CameraGetAnalogGain\0") {
                f(self.h, &mut v);
            }
        }
        v
    }

    pub fn set_auto_exposure(&self, on: bool) -> McResult<()> {
        self.call1(b"CameraSetAeState\0", on as i32)
    }

    pub fn white_balance_once(&self) -> McResult<()> {
        self.call0(b"CameraSetOnceWB\0")
    }

    pub fn set_trigger(&self, soft: bool) -> McResult<()> {
        self.call1(b"CameraSetTriggerMode\0", if soft { 1 } else { 0 })
    }

    pub fn soft_trigger(&self) -> McResult<()> {
        self.call0(b"CameraSoftTrigger\0")
    }

    /// Next raw frame (sensor data, before the ISP). The closure sees the buffer before it is released.
    pub fn grab<R>(&self, timeout_ms: u32, f: impl FnOnce(&FrameHead, &[u8]) -> R) -> McResult<R> {
        unsafe {
            let mut head = FrameHead::default();
            let mut buf: *mut u8 = std::ptr::null_mut();
            let r = self.sdk.f::<cfn!(i32, *mut FrameHead, *mut *mut u8, u32)>(b"CameraGetImageBuffer\0")?(self.h, &mut head, &mut buf, timeout_ms);
            if r != 0 {
                return Err(sdk_error(r, "CameraGetImageBuffer"));
            }
            let data = std::slice::from_raw_parts(buf, head.bytes as usize);
            let out = f(&head, data);
            let _ = self.sdk.f::<cfn!(i32, *mut u8)>(b"CameraReleaseImageBuffer\0").map(|g| g(self.h, buf));
            Ok(out)
        }
    }
}

impl Drop for Cam {
    fn drop(&mut self) {
        unsafe {
            if let Ok(f) = self.sdk.f::<cfn!(i32)>(b"CameraUnInit\0") {
                f(self.h);
            }
        }
    }
}

/// Pixel format name from the SDK media type (CAMERA_MEDIA_TYPE_*).
pub fn format_name(mt: u32) -> (String, u32) {
    let bits = (mt >> 16) & 0xFF;
    let id = mt & 0xFFFF;
    let mono = mt & 0x0100_0000 != 0;
    let pat = match id {
        0x08 | 0x2E | 0x10 => Some("gr"),
        0x09 | 0x2F | 0x11 => Some("rg"),
        0x0A | 0x30 | 0x12 => Some("gb"),
        0x0B | 0x31 | 0x13 => Some("bg"),
        _ => None,
    };
    let name = match (mono, pat) {
        (true, Some(p)) => format!("bayer_{p}{bits}"),
        (true, None) => format!("mono{bits}"),
        _ => format!("raw_0x{mt:08x}"),
    };
    (name, bits)
}

#[allow(dead_code)]
fn _unused(_: *mut c_void) {}
