//! Client IP: rightmost-untrusted `X-Forwarded-For` hop when the socket peer is a
//! trusted proxy (`server.trusted_proxies`), else the socket peer.

use crate::app::AppState;
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use std::net::{IpAddr, SocketAddr};

/// Request extension set by [`middleware`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientIp(pub Option<IpAddr>);

#[derive(Debug, Clone, Copy)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// Parses `a.b.c.d`, `a.b.c.d/n`, or IPv6 equivalents.
    pub fn parse(s: &str) -> Option<Self> {
        let (a, p) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p.parse::<u8>().ok()?)),
            None => (s, None),
        };
        let addr: IpAddr = a.trim().parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = p.unwrap_or(max);
        (prefix <= max).then_some(Self { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(n), IpAddr::V4(i)) => mask_eq(
                u128::from(u32::from(n)),
                u128::from(u32::from(i)),
                32,
                self.prefix,
            ),
            (IpAddr::V6(n), IpAddr::V6(i)) => {
                mask_eq(u128::from(n), u128::from(i), 128, self.prefix)
            }
            _ => false,
        }
    }
}

fn mask_eq(a: u128, b: u128, bits: u32, prefix: u8) -> bool {
    let shift = bits - u32::from(prefix);
    if shift >= 128 {
        return true;
    }
    (a >> shift) == (b >> shift)
}

fn trusted(proxies: &[Cidr], ip: IpAddr) -> bool {
    proxies.iter().any(|c| c.contains(ip))
}

/// Pure resolution, exposed for tests.
pub fn resolve(peer: Option<IpAddr>, xff: &[&str], proxies: &[Cidr]) -> Option<IpAddr> {
    let peer = peer?;
    if !trusted(proxies, peer) {
        return Some(peer);
    }
    let mut candidate = peer;
    for entry in xff.iter().flat_map(|h| h.split(',')).rev() {
        let Ok(ip) = entry.trim().parse::<IpAddr>() else {
            break;
        };
        candidate = ip;
        if !trusted(proxies, ip) {
            break;
        }
    }
    Some(candidate)
}

pub async fn middleware(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let xff: Vec<&str> = req
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    let ip = resolve(peer, &xff, &state.net.trusted_proxies);
    req.extensions_mut().insert(ClientIp(ip));
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rightmost_untrusted() {
        let proxies = [Cidr::parse("10.0.0.0/8").unwrap()];
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        // untrusted peer: header ignored
        assert_eq!(
            resolve(Some(ip("1.2.3.4")), &["9.9.9.9"], &proxies),
            Some(ip("1.2.3.4"))
        );
        // trusted peer: rightmost untrusted wins (spoofed left entries ignored)
        assert_eq!(
            resolve(
                Some(ip("10.0.0.1")),
                &["6.6.6.6, 5.5.5.5, 10.0.0.2"],
                &proxies
            ),
            Some(ip("5.5.5.5"))
        );
        assert_eq!(
            resolve(Some(ip("10.0.0.1")), &[], &proxies),
            Some(ip("10.0.0.1"))
        );
    }
}
