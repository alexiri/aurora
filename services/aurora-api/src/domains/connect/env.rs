//! Outbound HTTP for connect flows: guarded client (https only, public addresses only)
//! and the hook-host client. `override_origin` redirects all guarded requests to a test
//! server (tests only).
use super::hooks::HookClient;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

pub struct Env {
    pub http: reqwest::Client,
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

/// Why an outbound URL was refused.
#[derive(Debug)]
pub struct Refused;

impl Env {
    pub fn new(
        hookhost_url: &str,
        internal_ring: &aurora_crypto::SecretRing,
    ) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .dns_resolver(std::sync::Arc::new(Guarded))
            .build()
            .map_err(|e| format!("connect http client: {e}"))?;
        Ok(Self {
            http,
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
