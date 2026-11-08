//! Outbound HTTP for connect flows: guarded client (https only, public addresses only)
//! and the hook-host client. `override_origin` redirects all guarded requests to a test
//! server (tests only).
use super::hooks::HookClient;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

pub struct Env {
    pub http: reqwest::Client,
    /// The client for requests to a registered app instance (D-74): the same guard, plus the
    /// addresses the operator opened with `gateway.allowed_private`.
    pub http_instance: reqwest::Client,
    pub allow: PrivateAllow,
    pub hooks: HookClient,
    pub override_origin: Option<reqwest::Url>,
}

/// Publicly routable unicast addresses only.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_broadcast()
                || v.is_multicast()
                || v.is_documentation()
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                || o[0] == 0)
        }
        IpAddr::V6(v) => match v.to_ipv4_mapped() {
            Some(m) => is_public(IpAddr::V4(m)),
            None => {
                let s = v.segments();
                !(v.is_loopback()
                    || v.is_unspecified()
                    || v.is_multicast()
                    || (s[0] & 0xfe00) == 0xfc00
                    || (s[0] & 0xffc0) == 0xfe80)
            }
        },
    }
}

struct Guarded;

impl Resolve for Guarded {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|a| is_public(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err("destination refused".into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// `gateway.allowed_private` as the API's connect/probe flows honour it (same entry grammar as the
/// gateway's destination guard: CIDR, IP, hostname, `*.suffix`; the cloud metadata addresses only
/// by an exact IP entry).
#[derive(Debug, Clone, Default)]
pub struct PrivateAllow {
    cidrs: Vec<(IpAddr, u8)>,
    ips: Vec<IpAddr>,
    hosts: Vec<String>,
    suffixes: Vec<String>,
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
        IpAddr::V4(v4) => v4 == std::net::Ipv4Addr::new(169, 254, 169, 254),
        IpAddr::V6(v6) => v6 == std::net::Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254),
    }
}

impl PrivateAllow {
    /// Malformed entries are skipped (the gateway refuses to start on them; the API only narrows).
    #[must_use]
    pub fn new(entries: &[String]) -> Self {
        let mut a = Self::default();
        for e in entries.iter().map(|e| e.trim()) {
            if let Some((n, b)) = e.split_once('/') {
                if let (Ok(ip), Ok(bits)) = (n.parse::<IpAddr>(), b.parse::<u8>()) {
                    a.cidrs.push((canon(ip), bits));
                }
            } else if let Ok(ip) = e.parse::<IpAddr>() {
                a.ips.push(canon(ip));
            } else if let Some(s) = e.strip_prefix("*.") {
                a.suffixes
                    .push(s.trim_end_matches('.').to_ascii_lowercase());
            } else if !e.is_empty() {
                a.hosts.push(e.trim_end_matches('.').to_ascii_lowercase());
            }
        }
        a
    }

    /// May the connect/probe flow dial `ip` for the name `host`?
    #[must_use]
    pub fn permits(&self, ip: IpAddr, host: &str) -> bool {
        let ip = canon(ip);
        if is_metadata(ip) {
            return self.ips.contains(&ip);
        }
        if is_public(ip) {
            return true;
        }
        let h = host.trim_end_matches('.').to_ascii_lowercase();
        self.hosts.contains(&h)
            || self.suffixes.iter().any(|s| {
                h.len() > s.len()
                    && h.ends_with(s.as_str())
                    && h.as_bytes()[h.len() - s.len() - 1] == b'.'
            })
            || self.ips.contains(&ip)
            || self.cidrs.iter().any(|(n, b)| cidr_contains(*n, *b, ip))
    }
}

/// Resolver for an app instance's host: public addresses plus what `allowed_private` opens.
struct GuardedAllow(PrivateAllow);

impl Resolve for GuardedAllow {
    fn resolve(&self, name: Name) -> Resolving {
        let allow = self.0.clone();
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| allow.permits(a.ip(), &host))
                .collect();
            if addrs.is_empty() {
                return Err("destination refused".into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// Why an outbound URL was refused.
#[derive(Debug)]
pub struct Refused;

impl Env {
    pub fn new(
        hookhost_url: &str,
        internal_ring: &aurora_crypto::SecretRing,
        allowed_private: &[String],
    ) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .dns_resolver(std::sync::Arc::new(Guarded))
            .build()
            .map_err(|e| format!("connect http client: {e}"))?;
        let allow = PrivateAllow::new(allowed_private);
        let http_instance = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .dns_resolver(std::sync::Arc::new(GuardedAllow(allow.clone())))
            .build()
            .map_err(|e| format!("connect instance http client: {e}"))?;
        Ok(Self {
            http,
            http_instance,
            allow,
            hooks: HookClient::new(hookhost_url, internal_ring)?,
            override_origin: None,
        })
    }

    /// Validate an outbound URL (https, host not an IP literal in a private range) and apply
    /// the test override.
    pub fn prepare(&self, raw: &str) -> Result<reqwest::Url, Refused> {
        let u = reqwest::Url::parse(raw).map_err(|_| Refused)?;
        if let Some(o) = &self.override_origin {
            let mut t = o.clone();
            t.set_path(u.path());
            t.set_query(u.query());
            return Ok(t);
        }
        if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() {
            return Err(Refused);
        }
        let host = u.host_str().ok_or(Refused)?;
        match host.trim_matches(['[', ']']).parse::<IpAddr>() {
            Ok(ip) if !is_public(ip) => Err(Refused),
            _ => Ok(u),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// SEC-06 / SEC-59: a registered instance is dialled through the destination guard; private
    /// addresses only where `gateway.allowed_private` opens them.
    #[test]
    fn private_addresses_need_allowed_private() {
        let none = PrivateAllow::new(&[]);
        assert!(none.permits(ip("93.184.216.34"), "desk.acme.example"));
        for p in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.9",
            "172.16.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "::1",
            "fd12::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!none.permits(ip(p), "desk.acme.example"), "{p}");
        }
        let a = PrivateAllow::new(&[
            "10.0.0.0/8".into(),
            "192.168.0.9".into(),
            "*.corp.example".into(),
            "desk.lab.example".into(),
        ]);
        assert!(a.permits(ip("10.1.2.3"), "x.acme.example"));
        assert!(a.permits(ip("192.168.0.9"), "x.acme.example"));
        assert!(!a.permits(ip("192.168.0.10"), "x.acme.example"));
        assert!(!a.permits(ip("172.16.0.1"), "x.acme.example"));
        // a name entry opens whatever that name resolves to, and only that name
        assert!(a.permits(ip("172.16.0.1"), "git.corp.example"));
        assert!(a.permits(ip("172.16.0.1"), "desk.lab.example"));
        assert!(!a.permits(ip("172.16.0.1"), "corp.example"));
        assert!(!a.permits(ip("172.16.0.1"), "evilcorp.example"));
        // the metadata addresses only by an exact IP entry, never through a CIDR or a name
        let m = PrivateAllow::new(&["169.254.0.0/16".into(), "*.corp.example".into()]);
        assert!(!m.permits(ip("169.254.169.254"), "git.corp.example"));
        let m = PrivateAllow::new(&["169.254.169.254".into()]);
        assert!(m.permits(ip("169.254.169.254"), "x.example"));
    }
}
