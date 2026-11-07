//! Destination guard (06 step 2): SEC-06.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[derive(Debug, thiserror::Error)]
pub enum DialError {
    #[error("destination refused: {0}")]
    Refused(String),
    #[error("resolution failed: {0}")]
    Resolution(String),
    #[error("connect failed: {0}")]
    Connect(#[source] io::Error),
}

#[derive(Debug, Clone)]
enum Allow {
    Cidr(IpAddr, u8),
    Ip(IpAddr),
    Host(String),
    Suffix(String),
}

#[derive(Debug, Clone)]
pub struct Guard {
    allow: Vec<Allow>,
    /// Gateway listeners and database (address, port); unspecified address
    /// means every loopback/unspecified address.
    own: Vec<(IpAddr, u16)>,
}

const METADATA_V4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);
fn metadata_v6() -> Ipv6Addr {
    Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254)
}

pub fn normalize_host(host: &str) -> String {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.trim_end_matches('.').to_ascii_lowercase()
}

fn canon(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

fn cidr_contains(net: IpAddr, bits: u8, ip: IpAddr) -> bool {
    match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(i)) => {
            let bits = u32::from(bits.min(32));
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(n) & mask == u32::from(i) & mask
        }
        (IpAddr::V6(n), IpAddr::V6(i)) => {
            let bits = u32::from(bits.min(128));
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            u128::from(n) & mask == u128::from(i) & mask
        }
        _ => false,
    }
}

fn is_metadata(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4 == METADATA_V4,
        IpAddr::V6(v6) => v6 == metadata_v6(),
    }
}

/// Addresses that are not publicly routable unicast.
fn is_special(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                || o[0] >= 240
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
        }
    }
}

impl Guard {
    /// `allowed` entries: CIDR, IP, hostname or `*.suffix`.
    pub fn new(allowed: &[String], own: Vec<SocketAddr>) -> Result<Self, String> {
        let mut allow = Vec::new();
        for e in allowed {
            let e = e.trim();
            if let Some((a, b)) = e.split_once('/') {
                let ip: IpAddr = a.parse().map_err(|_| format!("bad CIDR {e:?}"))?;
                let bits: u8 = b.parse().map_err(|_| format!("bad CIDR {e:?}"))?;
                let max = if ip.is_ipv4() { 32 } else { 128 };
                if bits > max {
                    return Err(format!("bad CIDR {e:?}"));
                }
                allow.push(Allow::Cidr(canon(ip), bits));
            } else if let Ok(ip) = e.parse::<IpAddr>() {
                allow.push(Allow::Ip(canon(ip)));
            } else if let Some(sfx) = e.strip_prefix("*.") {
                allow.push(Allow::Suffix(normalize_host(sfx)));
            } else if !e.is_empty() {
                allow.push(Allow::Host(normalize_host(e)));
            } else {
                return Err("empty allowed_private entry".into());
            }
        }
        let own = own.into_iter().map(|a| (canon(a.ip()), a.port())).collect();
        Ok(Self { allow, own })
    }

    fn is_own(&self, ip: IpAddr, port: u16) -> bool {
        self.own.iter().any(|&(oip, oport)| {
            oport == port
                && (oip == ip
                    || (oip.is_unspecified() && (ip.is_loopback() || ip.is_unspecified())))
        })
    }

    fn name_allowed(&self, host: &str) -> bool {
        self.allow.iter().any(|a| match a {
            Allow::Host(h) => h == host,
            Allow::Suffix(s) => {
                host.len() > s.len()
                    && host.ends_with(s.as_str())
                    && host.as_bytes()[host.len() - s.len() - 1] == b'.'
            }
            _ => false,
        })
    }

    /// Judges one resolved address. `host` is the name the client asked for.
    pub fn check_ip(&self, ip: IpAddr, host: &str, port: u16) -> Result<(), DialError> {
        let ip = canon(ip);
        let refused = || DialError::Refused(host.to_owned());
        if self.is_own(ip, port) {
            return Err(refused());
        }
        if is_metadata(ip) {
            return if self
                .allow
                .iter()
                .any(|a| matches!(a, Allow::Ip(x) if *x == ip))
            {
                Ok(())
            } else {
                Err(refused())
            };
        }
        if !is_special(ip) {
            return Ok(());
        }
        let ok = self.name_allowed(&normalize_host(host))
            || self.allow.iter().any(|a| match a {
                Allow::Ip(x) => *x == ip,
                Allow::Cidr(n, b) => cidr_contains(*n, *b, ip),
                _ => false,
            });
        if ok { Ok(()) } else { Err(refused()) }
    }

    /// Name-only check: judges IP literals, accepts names (they are judged at
    /// dial time, or at the egress proxy's resolution).
    pub fn check_name(&self, host: &str, port: u16) -> Result<(), DialError> {
        match normalize_host(host).parse::<IpAddr>() {
            Ok(ip) => self.check_ip(ip, host, port),
            Err(_) => Ok(()),
        }
    }

    /// Resolves through the guard; only checked addresses are returned, so the
    /// caller connects to exactly what was judged.
    pub async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, DialError> {
        let h = normalize_host(host);
        if let Ok(ip) = h.parse::<IpAddr>() {
            self.check_ip(ip, host, port)?;
            return Ok(vec![SocketAddr::new(ip, port)]);
        }
        let addrs = tokio::net::lookup_host((h.as_str(), port))
            .await
            .map_err(|e| DialError::Resolution(e.to_string()))?;
        let mut ok = Vec::new();
        let mut refused = false;
        for a in addrs {
            match self.check_ip(a.ip(), host, port) {
                Ok(()) => ok.push(a),
                Err(_) => refused = true,
            }
        }
        if ok.is_empty() {
            return Err(if refused {
                DialError::Refused(host.to_owned())
            } else {
                DialError::Resolution("no addresses".into())
            });
        }
        Ok(ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(allow: &[&str]) -> Guard {
        let v: Vec<String> = allow.iter().map(|s| (*s).to_owned()).collect();
        Guard::new(&v, vec!["0.0.0.0:4102".parse().unwrap()]).unwrap()
    }
    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn public_ok_private_refused() {
        let g = g(&[]);
        assert!(g.check_ip(ip("93.184.216.34"), "x", 443).is_ok());
        for p in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.1",
            "172.16.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "224.0.0.1",
            "0.0.0.0",
            "::1",
            "fd12::1",
            "fe80::1",
            "::",
            "ff02::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(g.check_ip(ip(p), "x", 80).is_err(), "{p}");
        }
    }

    #[test]
    fn allow_list() {
        let g = g(&["127.0.0.0/8", "10.0.0.5", "internal.test", "*.corp.test"]);
        assert!(g.check_ip(ip("127.0.0.1"), "x", 80).is_ok());
        assert!(g.check_ip(ip("10.0.0.5"), "x", 80).is_ok());
        assert!(g.check_ip(ip("10.0.0.6"), "x", 80).is_err());
        assert!(g.check_ip(ip("10.0.0.6"), "internal.test", 80).is_ok());
        assert!(g.check_ip(ip("10.0.0.6"), "a.corp.test", 80).is_ok());
        assert!(g.check_ip(ip("10.0.0.6"), "corp.test", 80).is_err());
        assert!(g.check_ip(ip("10.0.0.6"), "evilcorp.test", 80).is_err());
    }

    #[test]
    fn metadata_needs_exact_ip() {
        let cidr = g(&["169.254.0.0/16", "internal.test"]);
        assert!(cidr.check_ip(ip("169.254.169.254"), "x", 80).is_err());
        assert!(
            cidr.check_ip(ip("169.254.169.254"), "internal.test", 80)
                .is_err()
        );
        assert!(cidr.check_ip(ip("169.254.1.1"), "x", 80).is_ok());
        assert!(
            g(&["169.254.169.254"])
                .check_ip(ip("169.254.169.254"), "x", 80)
                .is_ok()
        );
        assert!(
            g(&["fd00:ec2::/32"])
                .check_ip(ip("fd00:ec2::254"), "x", 80)
                .is_err()
        );
        assert!(
            g(&["fd00:ec2::254"])
                .check_ip(ip("fd00:ec2::254"), "x", 80)
                .is_ok()
        );
    }

    #[test]
    fn own_listener_always_refused() {
        let g = g(&["127.0.0.0/8"]);
        assert!(g.check_ip(ip("127.0.0.1"), "x", 4102).is_err());
        assert!(g.check_ip(ip("127.0.0.1"), "x", 4103).is_ok());
    }

    /// SEC-06: the addresses returned for dialling are exactly the judged ones
    /// (a name is judged by what it resolved to, never by the name alone).
    #[tokio::test]
    async fn resolved_addresses_are_judged_and_returned() {
        let strict = g(&[]);
        assert!(matches!(
            strict.resolve("localhost", 80).await,
            Err(DialError::Refused(_))
        ));
        assert!(matches!(
            strict.resolve("127.0.0.1", 80).await,
            Err(DialError::Refused(_))
        ));
        let lax = g(&["127.0.0.0/8", "::1"]);
        let got = lax.resolve("localhost", 80).await.unwrap();
        assert!(!got.is_empty() && got.iter().all(|a| a.ip().is_loopback() && a.port() == 80));
        // Internal services are never allow-listable, even by name.
        let own = g(&["127.0.0.0/8", "localhost"]);
        assert!(own.resolve("localhost", 4102).await.is_err());
    }
}
