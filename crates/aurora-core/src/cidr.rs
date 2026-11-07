//! CIDR blocks and containment (RFE-025: BYO source lists, trusted proxies).
//!
//! Strict `cidr` syntax (like the PostgreSQL type): the host bits of the address must be zero, so
//! `10.0.0.1/24` is rejected instead of being silently widened. Matching is family-aware and
//! IPv4-mapped IPv6 addresses are normalised to IPv4 first, so a dual-stack listener cannot be
//! used to step around an IPv4 list.

use std::net::IpAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cidr {
    net: IpAddr,
    bits: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CidrError {
    #[error("not a CIDR (expected address/prefix)")]
    Syntax,
    #[error("host bits are set (use the network address)")]
    HostBits,
    #[error("a /0 prefix matches every address")]
    ZeroPrefix,
}

/// Normalises IPv4-mapped IPv6 (`::ffff:a.b.c.d`) to IPv4.
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

fn mask4(bits: u8) -> u32 {
    match bits {
        0 => 0,
        b => u32::MAX << (32 - u32::from(b.min(32))),
    }
}

fn mask6(bits: u8) -> u128 {
    match bits {
        0 => 0,
        b => u128::MAX << (128 - u32::from(b.min(128))),
    }
}

impl Cidr {
    /// Parses `addr/prefix`. `allow_zero` admits `/0` (never used for BYO lists).
    pub fn parse(s: &str) -> Result<Self, CidrError> {
        Self::parse_inner(s, false)
    }

    pub fn parse_allow_zero(s: &str) -> Result<Self, CidrError> {
        Self::parse_inner(s, true)
    }

    fn parse_inner(s: &str, allow_zero: bool) -> Result<Self, CidrError> {
        let (ip, len) = s.split_once('/').ok_or(CidrError::Syntax)?;
        if len.is_empty() || !len.bytes().all(|b| b.is_ascii_digit()) || len.len() > 3 {
            return Err(CidrError::Syntax);
        }
        let ip: IpAddr = ip.parse().map_err(|_| CidrError::Syntax)?;
        let bits: u8 = len.parse().map_err(|_| CidrError::Syntax)?;
        let max = if ip.is_ipv4() { 32 } else { 128 };
        if bits > max {
            return Err(CidrError::Syntax);
        }
        if bits == 0 && !allow_zero {
            return Err(CidrError::ZeroPrefix);
        }
        let host_bits_clear = match ip {
            IpAddr::V4(v4) => u32::from(v4) & !mask4(bits) == 0,
            IpAddr::V6(v6) => u128::from(v6) & !mask6(bits) == 0,
        };
        if !host_bits_clear {
            return Err(CidrError::HostBits);
        }
        Ok(Self { net: ip, bits })
    }

    pub fn prefix(&self) -> u8 {
        self.bits
    }

    pub fn network(&self) -> IpAddr {
        self.net
    }

    /// Whether `ip` (IPv4-mapped IPv6 normalised first) lies inside the block.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.net, canonical_ip(ip)) {
            (IpAddr::V4(n), IpAddr::V4(i)) => {
                u32::from(n) & mask4(self.bits) == u32::from(i) & mask4(self.bits)
            }
            (IpAddr::V6(n), IpAddr::V6(i)) => {
                u128::from(n) & mask6(self.bits) == u128::from(i) & mask6(self.bits)
            }
            _ => false,
        }
    }

    /// Whether every address of `inner` lies inside `self`.
    pub fn contains_cidr(&self, inner: &Cidr) -> bool {
        self.bits <= inner.bits && self.contains_raw(inner.net)
    }

    fn contains_raw(&self, ip: IpAddr) -> bool {
        match (self.net, ip) {
            (IpAddr::V4(n), IpAddr::V4(i)) => {
                u32::from(n) & mask4(self.bits) == u32::from(i) & mask4(self.bits)
            }
            (IpAddr::V6(n), IpAddr::V6(i)) => {
                u128::from(n) & mask6(self.bits) == u128::from(i) & mask6(self.bits)
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.net, self.bits)
    }
}

impl std::str::FromStr for Cidr {
    type Err = CidrError;
    fn from_str(s: &str) -> Result<Self, CidrError> {
        Self::parse(s)
    }
}

/// Whether `ip` lies inside any block of `list`.
pub fn any_contains(list: &[Cidr], ip: IpAddr) -> bool {
    list.iter().any(|c| c.contains(ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Cidr {
        Cidr::parse(s).unwrap()
    }
    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn parse_rules() {
        assert_eq!(Cidr::parse("0.0.0.0/0"), Err(CidrError::ZeroPrefix));
        assert_eq!(Cidr::parse("::/0"), Err(CidrError::ZeroPrefix));
        assert_eq!(Cidr::parse("10.0.0.1/24"), Err(CidrError::HostBits));
        assert_eq!(Cidr::parse("10.0.0.0/33"), Err(CidrError::Syntax));
        assert_eq!(Cidr::parse("10.0.0.0"), Err(CidrError::Syntax));
        assert_eq!(Cidr::parse("10.0.0.0/+8"), Err(CidrError::Syntax));
        assert_eq!(Cidr::parse("x/8"), Err(CidrError::Syntax));
        assert!(Cidr::parse_allow_zero("0.0.0.0/0").is_ok());
        assert_eq!(c("2001:db8::/32").to_string(), "2001:db8::/32");
        assert_eq!(c("203.0.113.7/32").to_string(), "203.0.113.7/32");
    }

    #[test]
    fn containment_boundaries_and_families() {
        let n = c("192.0.2.0/24");
        assert!(n.contains(ip("192.0.2.0")));
        assert!(n.contains(ip("192.0.2.255")));
        assert!(!n.contains(ip("192.0.3.0")));
        assert!(!n.contains(ip("191.255.255.255")));
        assert!(
            n.contains(ip("::ffff:192.0.2.9")),
            "mapped peers match IPv4 lists"
        );
        assert!(!n.contains(ip("2001:db8::1")));
        let v6 = c("2001:db8::/32");
        assert!(v6.contains(ip("2001:db8:ffff::1")));
        assert!(!v6.contains(ip("2001:db9::1")));
        assert!(!v6.contains(ip("192.0.2.1")));
    }

    #[test]
    fn subset() {
        assert!(c("10.0.0.0/8").contains_cidr(&c("10.1.0.0/16")));
        assert!(c("10.0.0.0/8").contains_cidr(&c("10.0.0.0/8")));
        assert!(!c("10.1.0.0/16").contains_cidr(&c("10.0.0.0/8")));
        assert!(!c("10.0.0.0/8").contains_cidr(&c("11.0.0.0/16")));
        assert!(!c("10.0.0.0/8").contains_cidr(&c("2001:db8::/32")));
    }
}
