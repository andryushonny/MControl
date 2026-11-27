//! Optosky ATR8300 controller: serial protocol, exactly as the Optosky software speaks it.
//!
//! Frame: `AA 55 | LEN (2, BE) | CMD | DATA | SUM8`, LEN = 4 + len(DATA), SUM8 = (LEN_hi + LEN_lo + CMD + sum(DATA)) & 0xFF.
//! The reply has the same form, CMD echoed; for moves DATA[0] is the status.
//! Only the commands below can ever reach the instrument (calibration writes etc. are blocked).

use mc_proto::clock::now_ns;
use mc_proto::errors::{McError, McResult};

use std::time::Duration;

pub const GET_PN: u8 = 0x03;
pub const GET_SN: u8 = 0x04;
pub const SET_TEC: u8 = 0x12;
pub const GET_TEC: u8 = 0x13;
pub const LASER: u8 = 0x20;
pub const TURN_MOTOR: u8 = 0x24;
pub const GET_TYPE: u8 = 0x26;
pub const MOTOR_Z: u8 = 0x2B;
pub const LIGHT: u8 = 0x2D;
pub const MOTOR_X: u8 = 0x2E;
pub const MOTOR_Y: u8 = 0x2F;
pub const GET_CALIB: u8 = 0x31;
pub const GET_MAX_LASER: u8 = 0x66;

pub const ALLOWED: &[u8] = &[GET_PN, GET_SN, SET_TEC, GET_TEC, LASER, TURN_MOTOR, GET_TYPE, MOTOR_Z, LIGHT, MOTOR_X, MOTOR_Y, GET_CALIB, GET_MAX_LASER];

pub const MOTOR_OK: u8 = 0;
pub const MOTOR_LIMIT_PLUS: u8 = 1;
pub const MOTOR_LIMIT_MINUS: u8 = 2;
pub const MOTOR_FAIL: u8 = 3;
pub const MOTOR_MOVING: u8 = 4;

/// Cooling of the spectrometer CCD, as the Optosky software: -5 C while connected, 20 C when closing.
pub const COOL_C: i32 = -5;
pub const WARM_C: i32 = 20;

pub fn name(cmd: u8) -> &'static str {
    match cmd {
        GET_PN => "GET_PN",
        GET_SN => "GET_SN",
        SET_TEC => "SET_TEC",
        GET_TEC => "GET_TEC",
        LASER => "LASER",
        TURN_MOTOR => "TURN_MOTOR",
        GET_TYPE => "GET_TYPE",
        MOTOR_Z => "MOTOR_Z",
        LIGHT => "LIGHT",
        MOTOR_X => "MOTOR_X",
        MOTOR_Y => "MOTOR_Y",
        GET_CALIB => "GET_CALIB",
        GET_MAX_LASER => "GET_MAX_LASER",
        _ => "?",
    }
}

pub fn tec_data(t: i32) -> [u8; 2] {
    [if t < 0 { 0xFF } else { 0x00 }, (t.unsigned_abs() & 0xFF) as u8]
}

pub fn pack(cmd: u8, data: &[u8]) -> Vec<u8> {
    let n = (4 + data.len()) as u16;
    let mut body = Vec::with_capacity(3 + data.len());
    body.extend_from_slice(&n.to_be_bytes());
    body.push(cmd);
    body.extend_from_slice(data);
    let sum = body.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    let mut out = vec![0xAA, 0x55];
    out.extend_from_slice(&body);
    out.push(sum);
    out
}

enum Port {
    Serial(Box<dyn serialport::SerialPort>),
    Sim,
}

pub struct Controller {
    port: Port,
    pub name: String,
    last_ns: u64,
    min_gap_ns: u64,
}

impl Controller {
    /// Open the port like the .NET SerialPort of the Optosky software: 115200 8N1, DTR and RTS off.
    pub fn open(port: &str) -> McResult<Self> {
        if port == "sim" {
            return Ok(Controller { port: Port::Sim, name: "sim".into(), last_ns: 0, min_gap_ns: 0 });
        }
        let mut p = serialport::new(port, 115_200)
            .timeout(Duration::from_millis(20))
            .open()
            .map_err(|e| McError::new("E307", format!("{port}: {e}. Close the Optosky software; check the USB cable")))?;
        let _ = p.write_data_terminal_ready(false);
        let _ = p.write_request_to_send(false);
        let _ = p.clear(serialport::ClearBuffer::All);
        Ok(Controller { port: Port::Serial(p), name: port.to_string(), last_ns: 0, min_gap_ns: 60_000_000 })
    }

    pub fn is_sim(&self) -> bool {
        matches!(self.port, Port::Sim)
    }

    /// One command and its reply (data without the echo). At least 60 ms between commands, as the controller needs.
    pub fn cmd(&mut self, cmd: u8, data: &[u8], timeout_ms: u64) -> McResult<Vec<u8>> {
        if !ALLOWED.contains(&cmd) {
            return Err(McError::new("E305", format!("0x{cmd:02X}")));
        }
        let wait = (self.last_ns + self.min_gap_ns).saturating_sub(now_ns());
        if wait > 0 {
            std::thread::sleep(Duration::from_nanos(wait));
        }
        let r = match &mut self.port {
            Port::Sim => Ok(sim_reply(cmd, data)),
            Port::Serial(p) => transact(p.as_mut(), cmd, data, timeout_ms),
        };
        self.last_ns = now_ns();
        r
    }

    /// List serial ports that look like the controller (USB serial).
    pub fn ports() -> Vec<String> {
        serialport::available_ports()
            .map(|v| v.into_iter().filter(|p| matches!(p.port_type, serialport::SerialPortType::UsbPort(_))).map(|p| p.port_name).collect())
            .unwrap_or_default()
    }
}

fn transact(p: &mut dyn serialport::SerialPort, cmd: u8, data: &[u8], timeout_ms: u64) -> McResult<Vec<u8>> {
    let _ = p.clear(serialport::ClearBuffer::Input);
    let frame = pack(cmd, data);
    p.write_all(&frame).map_err(|e| McError::new("E307", format!("{}: {e}", name(cmd))))?;
    let _ = p.flush();
    let deadline = now_ns() + timeout_ms * 1_000_000;
    let head = read_exact(p, 4, deadline)?;
    if head.len() < 4 {
        return Err(McError::new("E301", format!("{} (got {} bytes)", name(cmd), head.len())));
    }
    if head[0] != 0xAA || head[1] != 0x55 {
        return Err(McError::new("E302", format!("{}: {:02X?}", name(cmd), head)));
    }
    let n = u16::from_be_bytes([head[2], head[3]]) as usize;
    if !(4..=8200).contains(&n) {
        return Err(McError::new("E302", format!("length {n}")));
    }
    let rest = read_exact(p, n - 2, deadline + 1_000_000_000)?;
    if rest.len() != n - 2 {
        return Err(McError::new("E302", "truncated reply"));
    }
    let sum = head[2..].iter().chain(rest[..rest.len() - 1].iter()).fold(0u8, |a, b| a.wrapping_add(*b));
    if sum != rest[rest.len() - 1] {
        return Err(McError::new("E303", ""));
    }
    if rest[0] != cmd {
        return Err(McError::new("E304", format!("sent 0x{cmd:02X}, got 0x{:02X}", rest[0])));
    }
    Ok(rest[1..rest.len() - 1].to_vec())
}

/// Read n bytes by the deadline, not trusting the driver timeout (the old STM32 driver ignores it).
fn read_exact(p: &mut dyn serialport::SerialPort, n: usize, deadline: u64) -> McResult<Vec<u8>> {
    let mut buf = Vec::with_capacity(n);
    let mut tmp = [0u8; 512];
    while buf.len() < n {
        let want = (n - buf.len()).min(tmp.len());
        match p.read(&mut tmp[..want]) {
            Ok(k) if k > 0 => buf.extend_from_slice(&tmp[..k]),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(McError::new("E307", e.to_string())),
        }
        if buf.len() < n && now_ns() > deadline {
            break;
        }
    }
    Ok(buf)
}

fn sim_reply(cmd: u8, _data: &[u8]) -> Vec<u8> {
    match cmd {
        GET_SN => b"SIM-0001".to_vec(),
        GET_PN => b"ATR8300-SIM".to_vec(),
        GET_MAX_LASER => vec![0x00, 0x64],
        GET_TYPE => vec![2],
        GET_CALIB => vec![0; 16],
        _ => vec![0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_like_optosky() {
        // same bytes as MicroControl protocol.pack(0x2D, b'\x00\x05')
        assert_eq!(pack(LIGHT, &[0, 5]), vec![0xAA, 0x55, 0x00, 0x06, 0x2D, 0x00, 0x05, 0x38]);
        assert_eq!(tec_data(-5), [0xFF, 5]);
    }
}
