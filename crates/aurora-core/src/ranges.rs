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

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
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
