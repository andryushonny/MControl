//! Who may connect, and who may control. Local networks only unless told otherwise.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::net::IpAddr;

/// One allowed range: an address with a prefix length.
#[derive(Clone, Debug)]
struct Net {
    ip: IpAddr,
    len: u8,
}

impl Net {
    fn parse(s: &str) -> Result<Net, String> {
        let (a, l) = s.split_once('/').unwrap_or((s, ""));
        let ip: IpAddr = a.trim().parse().map_err(|_| format!("not an address: {s}"))?;
        let max = if ip.is_ipv4() { 32 } else { 128 };
        let len = if l.is_empty() { max } else { l.parse().map_err(|_| format!("bad prefix: {s}"))? };
        if len > max {
            return Err(format!("bad prefix: {s}"));
        }
        Ok(Net { ip, len })
    }

    fn contains(&self, ip: &IpAddr) -> bool {
        let ip = canon(ip);
        match (self.ip, ip) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let m = if self.len == 0 { 0 } else { u32::MAX << (32 - self.len) };
                u32::from(n) & m == u32::from(a) & m
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let m = if self.len == 0 { 0 } else { u128::MAX << (128 - self.len) };
                u128::from(n) & m == u128::from(a) & m
            }
            _ => false,
        }
    }

    fn is_local(&self) -> bool {
        LOCAL.iter().any(|l| {
            let l = Net::parse(l).unwrap();
            l.len <= self.len && l.contains(&self.ip)
        })
    }
}

/// IPv4 seen through an IPv6 socket counts as IPv4.
fn canon(ip: &IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(*ip),
        _ => *ip,
    }
}

/// Loopback, private, link-local, and the VPN ranges people use (CGNAT 100.64/10: Tailscale).
const LOCAL: &[&str] = &["127.0.0.0/8", "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "100.64.0.0/10",
                         "::1/128", "fc00::/7", "fe80::/10"];

fn parse_list(s: &str) -> Result<Vec<Net>, String> {
    let mut v = Vec::new();
    for p in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match p {
            "local" => v.extend(LOCAL.iter().map(|l| Net::parse(l).unwrap())),
            "any" => v.extend(["0.0.0.0/0", "::/0"].iter().map(|l| Net::parse(l).unwrap())),
            _ => v.push(Net::parse(p)?),
        }
    }
    Ok(v)
}

pub struct Access {
    allow: Vec<Net>,
    operators: Vec<Net>,
    token: Option<String>,
    pub allow_text: String,
    pub operators_text: String,
}

impl Access {
    /// `allow`/`operators`: comma lists of `local`, `any`, addresses and ranges (`192.168.1.0/24`).
    pub fn new(allow: &str, operators: &str, token: Option<String>) -> Result<Access, String> {
        let a = parse_list(allow)?;
        let o = parse_list(operators)?;
        let open = a.iter().chain(o.iter()).any(|n| !n.is_local());
        if open && token.as_deref().map_or(true, str::is_empty) {
            return Err("E121".into());
        }
        Ok(Access { allow: a, operators: o, token: token.filter(|t| !t.is_empty()), allow_text: allow.into(), operators_text: operators.into() })
    }

    pub fn allowed(&self, ip: &IpAddr) -> bool {
        self.allow.iter().any(|n| n.contains(ip))
    }

    pub fn may_operate(&self, ip: &IpAddr) -> bool {
        self.operators.iter().any(|n| n.contains(ip))
    }

    pub fn needs_token(&self) -> bool {
        self.token.is_some()
    }

    /// Constant time, so the token cannot be guessed byte by byte from reply times.
    pub fn token_ok(&self, given: &str) -> bool {
        let Some(t) = &self.token else { return true };
        let (a, b) = (t.as_bytes(), given.as_bytes());
        let mut d = (a.len() ^ b.len()) as u8;
        for i in 0..a.len() {
            d |= a[i] ^ b.get(i).copied().unwrap_or(0);
        }
        d == 0
    }

    pub fn is_open(&self) -> bool {
        self.allow.iter().any(|n| !n.is_local())
    }
}

/// 128 random bits as hex: session ids that cannot be guessed (std only: RandomState is seeded by the OS).
pub fn random_id() -> String {
    let mut s = String::new();
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(mc_proto::clock::now_ns() ^ i);
        s += &format!("{:016x}", h.finish());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        let a = Access::new("local", "192.168.1.20, 127.0.0.1", None).unwrap();
        assert!(a.allowed(&"192.168.5.7".parse().unwrap()));
        assert!(a.allowed(&"::ffff:10.1.2.3".parse().unwrap()));
        assert!(!a.allowed(&"8.8.8.8".parse().unwrap()));
        assert!(a.may_operate(&"192.168.1.20".parse().unwrap()));
        assert!(!a.may_operate(&"192.168.1.21".parse().unwrap()));
        assert!(Access::new("any", "local", None).is_err()); // open without a token
        let b = Access::new("local,203.0.113.0/24", "local", Some("s3cret".into())).unwrap();
        assert!(b.allowed(&"203.0.113.9".parse().unwrap()) && b.is_open());
        assert!(b.token_ok("s3cret") && !b.token_ok("s3cre") && !b.token_ok("s3cretX"));
        assert_ne!(random_id(), random_id());
    }
}
