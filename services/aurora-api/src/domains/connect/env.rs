//! Outbound HTTP for connect flows: guarded client (https only, public addresses only)
//! and the hook-host client. `override_origin` redirects all guarded requests to a test
//! server (tests only).
use super::hooks::HookClient;
use aurora_secrets::tls::with_extra_roots;
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
    /// How long the registration probe waits for an answer (10 s; tests shorten it).
    probe_timeout_ms: std::sync::atomic::AtomicU64,
}

/// Publicly routable unicast addresses only (FIX-106: the gateway's destination-guard table,
/// `aurora_core::ranges`, including the translated forms that embed a private IPv4 address).
pub fn is_public(ip: IpAddr) -> bool {
    !aurora_core::ranges::is_special(ip)
}

/// Why a guarded resolver gave no address (typed, so callers never match on message text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    /// The name resolved, but no address passed the destination guard.
    Refused,
    /// The name did not resolve to any address.
    NotFound,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Refused => "destination refused",
            Self::NotFound => "name not found",
        })
    }
}

impl std::error::Error for ResolveError {}

/// Look `host` up and keep what `keep` allows.
async fn resolve_guarded(
    host: &str,
    keep: impl Fn(IpAddr) -> bool,
) -> Result<Vec<SocketAddr>, ResolveError> {
    let all: Vec<SocketAddr> = tokio::net::lookup_host((host, 0))
        .await
        .map_err(|_| ResolveError::NotFound)?
        .collect();
    if all.is_empty() {
        return Err(ResolveError::NotFound);
    }
    let addrs: Vec<SocketAddr> = all.into_iter().filter(|a| keep(a.ip())).collect();
    if addrs.is_empty() {
        return Err(ResolveError::Refused);
    }
    Ok(addrs)
}

struct Guarded;

impl Resolve for Guarded {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addrs = resolve_guarded(name.as_str(), is_public).await?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// A rustls error, possibly inside nested `io::Error`s (an `io::Error`'s `source()` skips the
/// error it wraps, so the wrapped one is reached through `get_ref`).
fn is_tls_error(err: &(dyn std::error::Error + 'static)) -> bool {
    if err.downcast_ref::<rustls::Error>().is_some() {
        return true;
    }
    err.downcast_ref::<std::io::Error>()
        .and_then(std::io::Error::get_ref)
        .is_some_and(|inner| is_tls_error(inner))
}

/// Why a request to a registered instance failed before an answer (C-730 `reason`), from the typed
/// error and its source chain.
#[must_use]
pub fn classify_send_error(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        return "timeout";
    }
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    let mut connect = e.is_connect();
    while let Some(err) = cur {
        if let Some(r) = err.downcast_ref::<ResolveError>() {
            return match r {
                ResolveError::Refused => "destination_refused",
                ResolveError::NotFound => "dns",
            };
        }
        if is_tls_error(err) {
            return "tls";
        }
        if err
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::TimedOut)
        {
            return "timeout";
        }
        if let Some(h) = err.downcast_ref::<hyper_util::client::legacy::Error>() {
            connect |= h.is_connect();
        }
        cur = err.source();
    }
    if connect { "connect" } else { "unreachable" }
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

use aurora_core::ranges::is_metadata;

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
            let addrs = resolve_guarded(&host, |ip| allow.permits(ip, &host)).await?;
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
        extra_ca_bundle: Option<&str>,
    ) -> Result<Self, String> {
        // `tls.extra_ca_bundle` (docs/15): a registered instance may be served under a private CA.
        let http = with_extra_roots(
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .dns_resolver(std::sync::Arc::new(Guarded)),
            extra_ca_bundle,
        )?
        .build()
        .map_err(|e| format!("connect http client: {e}"))?;
        let allow = PrivateAllow::new(allowed_private);
        let http_instance = with_extra_roots(
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .dns_resolver(std::sync::Arc::new(GuardedAllow(allow.clone()))),
            extra_ca_bundle,
        )?
        .build()
        .map_err(|e| format!("connect instance http client: {e}"))?;
        Ok(Self {
            http,
            http_instance,
            allow,
            hooks: HookClient::new(hookhost_url, internal_ring, extra_ca_bundle)?,
            override_origin: None,
            probe_timeout_ms: std::sync::atomic::AtomicU64::new(10_000),
        })
    }

    /// The registration probe's timeout.
    pub fn probe_timeout(&self) -> Duration {
        Duration::from_millis(
            self.probe_timeout_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Shorten the registration probe's timeout (tests).
    pub fn set_probe_timeout(&self, d: Duration) {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.probe_timeout_ms
            .store(ms, std::sync::atomic::Ordering::Relaxed);
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

    /// (FIX-144) The probe's reason for a name the guard refuses (resolves to a private address
    /// without `allowed_private`), a name that does not resolve, and one that `allowed_private`
    /// opens (then the dial itself fails: nothing listens on port 1).
    #[tokio::test]
    async fn the_resolver_failures_are_classified_by_type() {
        let reason = |env: &Env, url: &'static str| {
            let c = env.http_instance.clone();
            async move {
                let e = c.get(url).send().await.unwrap_err();
                classify_send_error(&e)
            }
        };
        let ring = aurora_crypto::SecretRing::default();
        let strict = Env::new("http://hooks.example:4104", &ring, &[], None).unwrap();
        assert_eq!(
            reason(&strict, "https://localhost:1/").await,
            "destination_refused"
        );
        assert_eq!(reason(&strict, "https://nx.invalid:1/").await, "dns");
        let open = Env::new(
            "http://hooks.example:4104",
            &ring,
            &["localhost".to_owned()],
            None,
        )
        .unwrap();
        assert_eq!(reason(&open, "https://localhost:1/").await, "connect");
    }

    /// (FIX-106) Translated and reserved forms that embed or stand for a private IPv4 address are
    /// refused by the probe guard exactly as by the gateway's.
    #[test]
    fn translated_and_reserved_forms_are_not_public() {
        for p in [
            "64:ff9b::a00:1",
            "64:ff9b::a9fe:a9fe",
            "2002:a00:1::1",
            "::10.0.0.1",
            "fec0::1",
            "198.18.0.1",
            "192.0.0.1",
            "240.0.0.1",
        ] {
            assert!(!is_public(ip(p)), "{p}");
            assert!(!PrivateAllow::new(&[]).permits(ip(p), "x.example"), "{p}");
        }
        assert!(is_public(ip("64:ff9b::808:808")));
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
