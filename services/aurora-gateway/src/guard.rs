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
    /// Gateway listeners and database (address, port); an unspecified address
    /// means every loopback, unspecified and local interface address.
    own: Vec<(IpAddr, u16)>,
    /// (FIX-106) This host's own interface addresses, for a wildcard bind: with the default
    /// `0.0.0.0` bind the gateway's pod address is as much "own" as loopback is.
    local: Vec<IpAddr>,
}

/// (FIX-106) The addresses of this host's network interfaces. Linux reads `/proc/net`; where it
/// is unreadable the list is empty and only loopback and unspecified count as local.
#[must_use]
pub fn local_addresses() -> Vec<IpAddr> {
    let mut out = Vec::new();
    if let Ok(t) = std::fs::read_to_string("/proc/net/fib_trie") {
        out.extend(parse_fib_trie(&t).into_iter().map(IpAddr::V4));
    }
    if let Ok(t) = std::fs::read_to_string("/proc/net/if_inet6") {
        out.extend(parse_if_inet6(&t).into_iter().map(IpAddr::V6));
    }
    out
}

/// `/proc/net/fib_trie`: an address is local where its `|-- a.b.c.d` line is followed by
/// `/32 host LOCAL`.
fn parse_fib_trie(text: &str) -> Vec<Ipv4Addr> {
    let mut last = None;
    let mut out = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(a) = line.strip_prefix("|--") {
            last = a.trim().parse::<Ipv4Addr>().ok();
        } else if line.starts_with("/32 host LOCAL") {
            out.extend(last);
        }
    }
    out
}

/// `/proc/net/if_inet6`: 32 hex digits first on every line.
fn parse_if_inet6(text: &str) -> Vec<Ipv6Addr> {
    text.lines()
        .filter_map(|l| {
            let h = l.split_whitespace().next()?;
            u128::from_str_radix(h, 16)
                .ok()
                .filter(|_| h.len() == 32)
                .map(Ipv6Addr::from)
        })
        .collect()
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

// The refused-range table is shared with the API's probe guard: `aurora_core::ranges`.
use aurora_core::ranges::{is_metadata, is_special};

impl Guard {
    /// `allowed` entries: CIDR, IP, hostname or `*.suffix`. A wildcard `own` address covers this
    /// host's interface addresses ([`local_addresses`]).
    pub fn new(allowed: &[String], own: Vec<SocketAddr>) -> Result<Self, String> {
        Self::with_local(allowed, own, local_addresses())
    }

    /// [`Guard::new`] with the host's interface addresses given (tests inject one).
    pub fn with_local(
        allowed: &[String],
        own: Vec<SocketAddr>,
        local: Vec<IpAddr>,
    ) -> Result<Self, String> {
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
        let local = local.into_iter().map(canon).collect();
        Ok(Self { allow, own, local })
    }

    fn is_own(&self, ip: IpAddr, port: u16) -> bool {
        self.own.iter().any(|&(oip, oport)| {
            oport == port
                && (oip == ip
                    || (oip.is_unspecified()
                        && (ip.is_loopback() || ip.is_unspecified() || self.local.contains(&ip))))
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

    /// Name-only check: judges IP literals, accepts names (they are judged at dial time).
    pub fn check_name(&self, host: &str, port: u16) -> Result<(), DialError> {
        match normalize_host(host).parse::<IpAddr>() {
            Ok(ip) => self.check_ip(ip, host, port),
            Err(_) => Ok(()),
        }
    }

    /// (FIX-106, `06` §Destination guard) The local name check of egress-proxy mode: the name is
    /// resolved here (with the system resolver, so decimal and hex IP spellings resolve to what a
    /// dial would reach) and refused if **any** address it resolves to is refused. The egress
    /// proxy then resolves the name again; that window (DNS rebinding between the two lookups) is
    /// accepted and documented.
    pub async fn judge_name(&self, host: &str, port: u16) -> Result<(), DialError> {
        let h = normalize_host(host);
        if let Ok(ip) = h.parse::<IpAddr>() {
            return self.check_ip(ip, host, port);
        }
        let addrs = tokio::net::lookup_host((h.as_str(), port))
            .await
            .map_err(|e| DialError::Resolution(e.to_string()))?;
        let mut any = false;
        for a in addrs {
            any = true;
            self.check_ip(a.ip(), host, port)?;
        }
        if any {
            Ok(())
        } else {
            Err(DialError::Resolution("no addresses".into()))
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

    /// (FIX-106) With the default wildcard bind the gateway's routable (pod) address is "own" as
    /// well, even when the operator allow-lists the pod network for in-cluster LLMs.
    #[test]
    fn wildcard_bind_covers_the_local_interface_addresses() {
        let allow = vec!["10.0.0.0/8".to_owned()];
        let own: Vec<SocketAddr> = vec![
            "0.0.0.0:4102".parse().unwrap(),
            "0.0.0.0:4103".parse().unwrap(),
        ];
        let g =
            Guard::with_local(&allow, own.clone(), vec![ip("10.42.1.7"), ip("fd00::7")]).unwrap();
        for port in [4102, 4103] {
            assert!(g.check_ip(ip("10.42.1.7"), "x", port).is_err(), "{port}");
            assert!(g.check_ip(ip("fd00::7"), "x", port).is_err(), "{port}");
            assert!(
                g.check_ip(ip("::ffff:10.42.1.7"), "x", port).is_err(),
                "{port}"
            );
        }
        // Other ports of the pod network, and other pods, stay reachable.
        assert!(g.check_ip(ip("10.42.1.7"), "x", 8080).is_ok());
        assert!(g.check_ip(ip("10.42.1.8"), "x", 4103).is_ok());
        // Without the injected address the pod IP is allowed (the defect this fixes).
        let blind = Guard::with_local(&allow, own, vec![]).unwrap();
        assert!(blind.check_ip(ip("10.42.1.7"), "x", 4103).is_ok());
    }

    #[test]
    fn interface_addresses_are_parsed_from_proc() {
        let trie = "Main:\n  +-- 0.0.0.0/0 3 0 5\n     |-- 0.0.0.0\n        /0 universe UNICAST\n     +-- 10.42.0.0/24 2 0 2\n        |-- 10.42.0.0\n           /32 link BROADCAST\n           /24 link UNICAST\n        |-- 10.42.0.5\n           /32 host LOCAL\n     |-- 127.0.0.1\n        /32 host LOCAL\n";
        assert_eq!(
            parse_fib_trie(trie),
            vec![Ipv4Addr::new(10, 42, 0, 5), Ipv4Addr::LOCALHOST]
        );
        let v6 = "fd000000000000000000000000000007 02 40 00 80 eth0\nnot-an-address\n";
        assert_eq!(
            parse_if_inet6(v6),
            vec!["fd00::7".parse::<Ipv6Addr>().unwrap()]
        );
    }

    /// (FIX-106) Translated and reserved forms are refused unless an explicit entry covers them;
    /// an allowed IPv4 CIDR does not open its translated forms.
    #[test]
    fn translated_forms_are_refused() {
        let strict = g(&[]);
        let lax = g(&["10.0.0.0/8", "127.0.0.0/8"]);
        for p in [
            "64:ff9b::a00:1",
            "64:ff9b::7f00:1",
            "2002:a00:1::1",
            "::10.0.0.1",
            "fec0::1",
            "198.18.0.1",
            "198.19.1.1",
            "192.0.0.9",
            "240.0.0.1",
            "64:ff9b::a9fe:a9fe",
        ] {
            assert!(strict.check_ip(ip(p), "x", 80).is_err(), "{p}");
            assert!(
                lax.check_ip(ip(p), "x", 80).is_err(),
                "{p} via allowed CIDR"
            );
        }
        assert!(strict.check_ip(ip("64:ff9b::808:808"), "x", 80).is_ok());
    }

    /// (FIX-106) The local name check of egress-proxy mode resolves and judges every address.
    #[tokio::test]
    async fn judge_name_resolves_locally_and_refuses_any_bad_address() {
        let allow_localhost = g(&["localhost"]);
        let strict = g(&[]);
        assert!(matches!(
            strict.judge_name("localhost", 443).await,
            Err(DialError::Refused(_))
        ));
        // A decimal spelling of 169.254.169.254 is a "name" to the CONNECT parser; the system
        // resolver turns it into the metadata address.
        assert!(matches!(
            strict.judge_name("2852039166", 443).await,
            Err(DialError::Refused(_))
        ));
        assert!(matches!(
            strict.judge_name("10.0.0.1", 443).await,
            Err(DialError::Refused(_))
        ));
        assert!(strict.judge_name("93.184.216.34", 443).await.is_ok());
        // An allow-listed name passes.
        assert!(allow_localhost.judge_name("localhost", 443).await.is_ok());
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
