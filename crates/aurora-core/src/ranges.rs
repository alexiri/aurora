//! (FIX-106, `06` §Destination guard) The one table of addresses an agent-reachable outbound
//! connection may not target unless an operator opens them: the gateway's destination guard and
//! the API's connect/probe guard both judge with it, so they cannot drift apart.
//!
//! Refused (IPv4): `0.0.0.0/8`, `10.0.0.0/8`, `100.64.0.0/10` (CGNAT), `127.0.0.0/8`,
//! `169.254.0.0/16` (link-local; the cloud metadata address is stricter, see [`is_metadata`]),
//! `172.16.0.0/12`, `192.0.0.0/24`, `192.0.2.0/24`, `192.168.0.0/16`, `198.18.0.0/15`,
//! `198.51.100.0/24`, `203.0.113.0/24`, `224.0.0.0/4`, `240.0.0.0/4` (includes broadcast).
//!
//! Refused (IPv6): `::`, `::1`, `::/96` (IPv4-compatible), `fc00::/7`, `fe80::/10`,
//! `fec0::/10` (site-local), `ff00::/8`, `2001::/32` (Teredo), `2001:db8::/32`,
//! `64:ff9b:1::/48` (local-use NAT64), and every translated form that **embeds** a refused IPv4
//! address: IPv4-mapped `::ffff:0:0/96`, NAT64 `64:ff9b::/96` and 6to4 `2002::/16` (the embedded
//! address is judged; one embedding a public IPv4 address is public).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The cloud metadata addresses.
const METADATA_V4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);
const METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

/// The IPv4 address an IPv6 address carries, if its form carries one: IPv4-mapped, IPv4-compatible
/// (`::/96`), NAT64 (`64:ff9b::/96`) and 6to4 (`2002::/16`).
#[must_use]
pub fn embedded_ipv4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = v6.segments();
    let [hi, lo] = [s[6].to_be_bytes(), s[7].to_be_bytes()];
    let low = Ipv4Addr::new(hi[0], hi[1], lo[0], lo[1]);
    let mapped_or_compatible = s[..5] == [0, 0, 0, 0, 0] && (s[5] == 0xffff || s[5] == 0);
    if mapped_or_compatible || s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        Some(low)
    } else if s[0] == 0x2002 {
        let [a, b] = [s[1].to_be_bytes(), s[2].to_be_bytes()];
        Some(Ipv4Addr::new(a[0], a[1], b[0], b[1]))
    } else {
        None
    }
}

fn special_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_multicast()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_documentation()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || o[0] >= 240
}

/// Is `ip` something other than a publicly routable unicast address?
#[must_use]
pub fn is_special(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => special_v4(v4),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] & 0xffc0) == 0xfec0
                || s[..6] == [0, 0, 0, 0, 0, 0]
                || (s[0] == 0x2001 && (s[1] == 0 || s[1] == 0x0db8))
                || (s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1)
                || embedded_ipv4(v6).is_some_and(special_v4)
        }
    }
}

/// Is `ip` a cloud metadata address, or a translated form of one? Those are reachable only by an
/// exact-IP `allowed_private` entry, never by a CIDR or a name.
#[must_use]
pub fn is_metadata(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4 == METADATA_V4,
        IpAddr::V6(v6) => v6 == METADATA_V6 || embedded_ipv4(v6) == Some(METADATA_V4),
    }
}

/// (FIX-290, D-511) Is `ip` in a range an instance-registered self-hosted app's host may resolve
/// to without being listed in `gateway.allowed_private`? Only private unicast space: RFC 1918,
/// `100.64.0.0/10` and IPv6 `fc00::/7`. Everything else [`is_special`] names (loopback,
/// unspecified, link-local, multicast, reserved, and every translated form) is not, and neither is
/// a cloud metadata service that lives inside those ranges.
#[must_use]
pub fn app_host_may_reach(ip: IpAddr) -> bool {
    match canon(ip) {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            (v4.is_private() || (o[0] == 100 && (o[1] & 0xc0) == 64))
                // Alibaba Cloud's metadata service.
                && o != [100, 100, 100, 200]
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            (s[0] & 0xfe00) == 0xfc00
                // AWS (`fd00:ec2::/32`: instance metadata, pod identity, DNS) and GCP
                // (`fd20:ce::254`) metadata services.
                && !(s[0] == 0xfd00 && s[1] == 0x0ec2)
                && !(s[0] == 0xfd20 && s[1] == 0x00ce && s[2..7] == [0; 5] && s[7] == 0x0254)
        }
    }
}

/// (FIX-290, D-511) Does a self-hosted app's allowance open `ip`, one resolved address of its
/// host? It must be [`app_host_may_reach`], be an address of the fully qualified name too
/// (`absolute`: a name that only a DNS search domain completes has none there) and not belong
/// to another of Aurora's services (`services`). Shared by the gateway and the API.
#[must_use]
pub fn app_address_opens(ip: IpAddr, absolute: &[IpAddr], services: &[IpAddr]) -> bool {
    let ip = canon(ip);
    let has = |list: &[IpAddr]| list.iter().any(|a| canon(*a) == ip);
    app_host_may_reach(ip) && has(absolute) && !has(services)
}

/// (FIX-290, D-511) The one host and port an instance-registered self-hosted app may reach on a
/// private address. It is built per request from the registration that serves it and handed to
/// the destination guard with that request only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppHost {
    host: String,
    port: u16,
}

impl AppHost {
    /// The same name rules as the chart's `privateHosts.names`, without wildcards: lower case, at
    /// least two labels, never an address. Names of the cluster's own services and of this
    /// machine are refused too.
    #[must_use]
    pub fn new(host: &str, port: u16) -> Option<Self> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let labels: Vec<&str> = host.split('.').collect();
        let label_ok = |l: &str| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        let tld_is_name = labels
            .last()
            .is_some_and(|l| l.starts_with(|c: char| c.is_ascii_lowercase()));
        if labels.len() < 2 || !labels.iter().all(|l| label_ok(l)) || !tld_is_name {
            return None;
        }
        const OWN: [&str; 4] = [".svc", ".cluster.local", ".localhost", ".aurora.internal"];
        if OWN.iter().any(|s| host.ends_with(s)) || host == "aurora.internal" {
            return None;
        }
        Some(Self { host, port })
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Is `host:port` the name and port this allowance is for?
    #[must_use]
    pub fn covers(&self, host: &str, port: u16) -> bool {
        port == self.port && host.trim_end_matches('.').eq_ignore_ascii_case(&self.host)
    }
}

/// The host name (or literal address) a service URL, `host:port` or bare host names; `None` when
/// there is none. For listing Aurora's own services out of its configuration.
#[must_use]
pub fn service_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else if authority.matches(':').count() == 1 {
        authority.split(':').next().unwrap_or("")
    } else {
        authority
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// `gateway.allowed_private` as the API's connect/probe flows and the dashboard Vault stores
/// honour it (same entry grammar as the gateway's destination guard: CIDR, IP, hostname,
/// `*.suffix`; the cloud metadata addresses only by an exact IP entry).
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

    /// Does an entry name `host` (a host entry, a `*.suffix` entry, or the address when `host` is
    /// an IP literal that [`Self::permits`] accepts)? Used to ask whether an endpoint's own host
    /// is opened without resolving it (D-262).
    #[must_use]
    pub fn covers_host(&self, host: &str) -> bool {
        let h = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if let Ok(ip) = h.parse::<IpAddr>() {
            return self.permits(ip, &h);
        }
        self.hosts.contains(&h)
            || self.suffixes.iter().any(|s| {
                h.len() > s.len()
                    && h.ends_with(s.as_str())
                    && h.as_bytes()[h.len() - s.len() - 1] == b'.'
            })
    }

    /// May a flow dial `ip` for the name `host`?
    #[must_use]
    pub fn permits(&self, ip: IpAddr, host: &str) -> bool {
        let ip = canon(ip);
        if is_metadata(ip) {
            return self.ips.contains(&ip);
        }
        if !is_special(ip) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// (FIX-290, D-511) Only private unicast space is open to a self-hosted app's host: never
    /// loopback, link-local, a metadata service or a translated form.
    #[test]
    fn an_app_host_may_reach_private_space_only() {
        for p in [
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.0.9",
            "100.64.0.1",
            "fd12::1",
            "fc00::5",
            "::ffff:10.0.0.1",
        ] {
            assert!(app_host_may_reach(ip(p)), "{p}");
        }
        for p in [
            "127.0.0.1",
            "127.9.9.9",
            "::1",
            "0.0.0.0",
            "::",
            "169.254.169.254",
            "169.254.170.2",
            "169.254.1.1",
            "fe80::1",
            "fd00:ec2::254",
            "fd00:ec2::23",
            "fd20:ce::254",
            "100.100.100.200",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a00:1",
            "64:ff9b::a9fe:a9fe",
            "2002:a00:1::1",
            "::10.0.0.1",
            "fec0::1",
            "224.0.0.1",
            "255.255.255.255",
            "192.0.0.192",
            "198.18.0.1",
            "240.0.0.1",
            "93.184.216.34",
        ] {
            assert!(!app_host_may_reach(ip(p)), "{p}");
        }
    }

    #[test]
    fn an_allowance_opens_a_private_address_of_the_qualified_name_that_is_no_service_of_ours() {
        let a = ip("10.1.2.3");
        assert!(app_address_opens(a, &[a], &[]));
        assert!(app_address_opens(ip("::ffff:10.1.2.3"), &[a], &[]));
        assert!(!app_address_opens(a, &[], &[]));
        assert!(!app_address_opens(a, &[ip("10.1.2.4")], &[]));
        assert!(!app_address_opens(a, &[a], &[ip("::ffff:10.1.2.3")]));
        let loopback = ip("127.0.0.1");
        assert!(!app_address_opens(loopback, &[loopback], &[]));
    }

    #[test]
    fn an_app_host_is_a_plain_name_with_two_labels_outside_the_cluster() {
        let h = AppHost::new("Wiki.Corp.Example.", 443).unwrap();
        assert_eq!((h.host(), h.port()), ("wiki.corp.example", 443));
        assert!(h.covers("WIKI.corp.example", 443));
        assert!(!h.covers("wiki.corp.example", 8443));
        assert!(!h.covers("a.wiki.corp.example", 443));
        assert!(!h.covers("corp.example", 443));
        for bad in [
            "wiki",
            "localhost",
            "10.0.0.1",
            "169.254.169.254",
            "127.1",
            "::1",
            "[fd12::1]",
            "*.corp.example",
            "10.0.0.0/8",
            "wiki.corp.example:443",
            "wiki.corp.example/x",
            "a@wiki.corp.example",
            "wiki..example",
            "api.aurora.svc",
            "api.aurora.svc.cluster.local",
            "api.aurora.svc.cluster.local.",
            "x.localhost",
            "aurora.internal",
            "link.aurora.internal",
            "",
        ] {
            assert!(AppHost::new(bad, 443).is_none(), "{bad}");
        }
    }

    #[test]
    fn service_hosts_are_read_from_urls_and_host_ports() {
        for (url, host) in [
            ("http://rel-api:4101", "rel-api"),
            (
                "postgres://u:p@DB.corp.example:5432/aurora?sslmode=require",
                "db.corp.example",
            ),
            ("redis://:pw@10.0.0.5:6379/0", "10.0.0.5"),
            ("https://vault.corp.example", "vault.corp.example"),
            ("proxy.corp.example:3128", "proxy.corp.example"),
            ("http://[fd12::1]:8080/x", "fd12::1"),
            ("kubernetes.default.svc", "kubernetes.default.svc"),
        ] {
            assert_eq!(service_host(url).as_deref(), Some(host), "{url}");
        }
        assert_eq!(service_host(""), None);
    }

    #[test]
    fn every_listed_form_is_special() {
        for p in [
            "0.1.2.3",
            "10.1.2.3",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.0.0.8",
            "192.0.2.1",
            "192.168.0.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::10.0.0.1",
            "::8.8.8.8",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001:db8::1",
            "64:ff9b:1::1",
            "64:ff9b::a00:1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "2002:a00:1::1",
            "2002:7f00:1::",
            "2002:c0a8:1::5",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_special(ip(p)), "{p} must be refused");
        }
    }

    #[test]
    fn public_forms_stay_public() {
        for p in [
            "93.184.216.34",
            "8.8.8.8",
            "198.20.0.1",
            "192.0.1.1",
            "2001:4860:4860::8888",
            "2606:4700::1111",
            "64:ff9b::808:808",
            "2002:808:808::1",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_special(ip(p)), "{p} is public");
        }
    }

    #[test]
    fn metadata_includes_translated_forms() {
        for p in [
            "169.254.169.254",
            "fd00:ec2::254",
            "64:ff9b::a9fe:a9fe",
            "2002:a9fe:a9fe::1",
            "::ffff:169.254.169.254",
        ] {
            assert!(is_metadata(ip(p)), "{p}");
        }
        assert!(!is_metadata(ip("169.254.1.1")));
        assert!(!is_metadata(ip("fd00:ec2::255")));
    }
}
