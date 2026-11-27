//! MControl wire protocol.
//!
//! Two TCP connections per client:
//! * **control** (port 5160): small JSON messages, `TCP_NODELAY`. Commands, replies, events, time sync.
//! * **data** (port 5161): frames. Big messages never delay commands.
//!
//! Every message on both connections: `[u32 LE length][u8 kind][payload]`, length = 1 + payload length.
//! * kind 1 — JSON (UTF-8 object);
//! * kind 2 — frame: `[u32 LE header length][header JSON][pixel bytes]`.
//!
//! Discovery: UDP broadcast `MCONTROL?` to port 5159, the server answers with one JSON object.

use std::io::{self, Read, Write};

pub mod clock;
pub mod errors;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CONTROL_PORT: u16 = 5160;
pub const DATA_PORT: u16 = 5161;
pub const DISCOVERY_PORT: u16 = 5159;
pub const DISCOVERY_ASK: &[u8] = b"MCONTROL?";

pub const K_JSON: u8 = 1;
pub const K_FRAME: u8 = 2;

/// Largest message accepted (a 12-bit 5 Mpx frame is ~10 MB).
pub const MAX_MSG: usize = 64 << 20;

/// Write one message made of several parts (no copy into one buffer).
pub fn write_msg<W: Write>(w: &mut W, kind: u8, parts: &[&[u8]]) -> io::Result<()> {
    let n: usize = 1 + parts.iter().map(|p| p.len()).sum::<usize>();
    let mut head = [0u8; 5];
    head[..4].copy_from_slice(&(n as u32).to_le_bytes());
    head[4] = kind;
    w.write_all(&head)?;
    for p in parts {
        w.write_all(p)?;
    }
    Ok(())
}

/// Write a JSON message.
pub fn write_json<W: Write>(w: &mut W, v: &serde_json::Value) -> io::Result<()> {
    let s = serde_json::to_vec(v).expect("json");
    write_msg(w, K_JSON, &[&s])
}

/// Write a frame message: header JSON + pixel bytes.
pub fn write_frame<W: Write>(w: &mut W, header: &[u8], pixels: &[u8]) -> io::Result<()> {
    let hl = (header.len() as u32).to_le_bytes();
    write_msg(w, K_FRAME, &[&hl, header, pixels])
}

/// Read one message into `buf` (reused between calls). Returns its kind.
pub fn read_msg<R: Read>(r: &mut R, buf: &mut Vec<u8>) -> io::Result<u8> {
    read_msg_max(r, buf, MAX_MSG)
}

/// Commands from clients are small: a server reads them with a low `max`, so nobody can make it allocate much.
pub const MAX_CMD: usize = 64 << 10;

pub fn read_msg_max<R: Read>(r: &mut R, buf: &mut Vec<u8>, max: usize) -> io::Result<u8> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let n = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
    if n == 0 || n > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bad message length {n}")));
    }
    buf.clear();
    buf.resize(n - 1, 0);
    r.read_exact(buf)?;
    Ok(head[4])
}

/// Split a frame payload into (header JSON, pixels).
pub fn split_frame(payload: &[u8]) -> io::Result<(serde_json::Value, &[u8])> {
    if payload.len() < 4 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short frame"));
    }
    let hl = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    if 4 + hl > payload.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad frame header length"));
    }
    let h = serde_json::from_slice(&payload[4..4 + hl])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok((h, &payload[4 + hl..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut v = Vec::new();
        write_json(&mut v, &serde_json::json!({"cmd": "ping", "id": 1})).unwrap();
        write_frame(&mut v, br#"{"seq":7}"#, &[1, 2, 3]).unwrap();
        let mut r = &v[..];
        let mut b = Vec::new();
        assert_eq!(read_msg(&mut r, &mut b).unwrap(), K_JSON);
        let j: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(j["cmd"], "ping");
        assert_eq!(read_msg(&mut r, &mut b).unwrap(), K_FRAME);
        let (h, px) = split_frame(&b).unwrap();
        assert_eq!(h["seq"], 7);
        assert_eq!(px, &[1, 2, 3]);
    }
}
