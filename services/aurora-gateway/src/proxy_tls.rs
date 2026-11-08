//! TLS for the proxy listener (4102): docs/06 §Proxy listener TLS (phase 1, RFE-002, D-44).
//!
//! The listener is TLS-only. The server certificate is either a leaf issued by the gateway CA
//! (default; regenerated in memory and hot-swapped before it expires) or an operator-supplied
//! chain + key reloaded when the files change. Established tunnels keep their session; only new
//! handshakes see a swapped certificate.

use crate::ca::Ca;
use arc_swap::ArcSwap;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

/// `PROXY_TLS_LEAF_VALIDITY` (docs/02): 30 days.
pub const LEAF_VALIDITY: Duration = Duration::from_secs(30 * 86_400);
/// The leaf is regenerated when fewer than this remain.
pub const RENEW_BELOW: Duration = Duration::from_secs(10 * 86_400);
/// How often renewal / operator-file changes are checked.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(300);
/// The gateway's documented fixed address on the podman `aurora-sandbox` network (10.89.250.0/24,
/// `docs/operator/runners.md`); added to the proxy certificate only when it is local. The compose
/// stack that introduced it was removed in OPS-017 (I-42), the constant keeps its old name.
pub const COMPOSE_GATEWAY_ADDR: &str = "10.89.250.2";
/// In-cluster service name of the gateway, also the host alias sandboxes get for it.
pub const SERVICE_NAME: &str = "aurora-gateway";

#[derive(Debug, thiserror::Error)]
pub enum ProxyTlsError {
    /// Configuration problem; names the offending key.
    #[error("{key}: {msg}")]
    Config { key: &'static str, msg: String },
    #[error("{0}")]
    Issue(String),
}

fn cfg_err(key: &'static str, msg: impl Into<String>) -> ProxyTlsError {
    ProxyTlsError::Config {
        key,
        msg: msg.into(),
    }
}

#[derive(Debug)]
struct SwapResolver(ArcSwap<CertifiedKey>);

impl ResolvesServerCert for SwapResolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.load_full())
    }
}

enum Source {
    Ca {
        ca: Arc<Ca>,
        names: Vec<String>,
        validity: Duration,
        renew_below: Duration,
    },
    Operator {
        cert: PathBuf,
        key: PathBuf,
        stamp: Mutex<Stamp>,
    },
}

type Stamp = (Option<SystemTime>, Option<SystemTime>);

pub struct ProxyTls {
    config: Arc<ServerConfig>,
    resolver: Arc<SwapResolver>,
    source: Source,
    not_after: Mutex<SystemTime>,
    generation: AtomicU64,
}

impl ProxyTls {
    /// CA-issued leaf for `names` (30 days, renewed below 10 days).
    pub fn from_ca(ca: Arc<Ca>, names: Vec<String>) -> Result<Arc<Self>, ProxyTlsError> {
        Self::from_ca_with(ca, names, LEAF_VALIDITY, RENEW_BELOW)
    }

    pub fn from_ca_with(
        ca: Arc<Ca>,
        names: Vec<String>,
        validity: Duration,
        renew_below: Duration,
    ) -> Result<Arc<Self>, ProxyTlsError> {
        let leaf = ca
            .issue_proxy_leaf(&names, validity)
            .map_err(|e| ProxyTlsError::Issue(e.to_string()))?;
        Self::build(
            leaf.key,
            leaf.not_after,
            Source::Ca {
                ca,
                names,
                validity,
                renew_below,
            },
        )
    }

    /// Operator-supplied certificate chain + key (`gateway.proxy_tls.cert_file`/`key_file`).
    /// Refuses unreadable files, a key that does not match, and an expired certificate.
    pub fn operator(cert: &Path, key: &Path) -> Result<Arc<Self>, ProxyTlsError> {
        let (ck, not_after) = load_operator(cert, key)?;
        let stamp = stamp_of(cert, key);
        Self::build(
            ck,
            not_after,
            Source::Operator {
                cert: cert.to_owned(),
                key: key.to_owned(),
                stamp: Mutex::new(stamp),
            },
        )
    }

    fn build(
        ck: Arc<CertifiedKey>,
        not_after: SystemTime,
        source: Source,
    ) -> Result<Arc<Self>, ProxyTlsError> {
        let resolver = Arc::new(SwapResolver(ArcSwap::new(ck)));
        let mut cfg = ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| ProxyTlsError::Issue(e.to_string()))?
        .with_no_client_auth()
        .with_cert_resolver(resolver.clone());
        // Proxy requests are HTTP/1.1; h2 is deliberately not offered on this listener.
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(Self {
            config: Arc::new(cfg),
            resolver,
            source,
            not_after: Mutex::new(not_after),
            generation: AtomicU64::new(1),
        }))
    }

    /// The listener's server configuration (the fallback for database logins whose SNI is not a
    /// connection host).
    pub fn server_config(&self) -> Arc<ServerConfig> {
        self.config.clone()
    }

    pub fn acceptor(&self) -> TlsAcceptor {
        TlsAcceptor::from(self.config.clone())
    }

    /// Expiry of the certificate currently presented.
    pub fn not_after(&self) -> SystemTime {
        *self.not_after.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Incremented each time the presented certificate is replaced.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// The certificate currently presented (leaf first).
    pub fn current_chain(&self) -> Vec<CertificateDer<'static>> {
        self.resolver.0.load().cert.clone()
    }

    fn swap(&self, ck: Arc<CertifiedKey>, not_after: SystemTime) {
        self.resolver.0.store(ck);
        *self.not_after.lock().unwrap_or_else(|e| e.into_inner()) = not_after;
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Renews the CA-issued leaf when fewer than `renew_below` remain, or reloads the operator
    /// certificate when its files changed. Returns whether the certificate was replaced. A failed
    /// reload keeps the previous certificate (and is reported).
    pub fn refresh(&self) -> Result<bool, ProxyTlsError> {
        match &self.source {
            Source::Ca {
                ca,
                names,
                validity,
                renew_below,
            } => {
                let remaining = self
                    .not_after()
                    .duration_since(SystemTime::now())
                    .unwrap_or(Duration::ZERO);
                if remaining >= *renew_below {
                    return Ok(false);
                }
                let leaf = ca
                    .issue_proxy_leaf(names, *validity)
                    .map_err(|e| ProxyTlsError::Issue(e.to_string()))?;
                self.swap(leaf.key, leaf.not_after);
                Ok(true)
            }
            Source::Operator { cert, key, stamp } => {
                let now = stamp_of(cert, key);
                {
                    let mut s = stamp.lock().unwrap_or_else(|e| e.into_inner());
                    if *s == now {
                        return Ok(false);
                    }
                    *s = now;
                }
                let (ck, not_after) = load_operator(cert, key)?;
                self.swap(ck, not_after);
                Ok(true)
            }
        }
    }

    /// Background renewal / reload until `shutdown` flips.
    pub async fn run_refresh(
        self: Arc<Self>,
        every: Duration,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => match self.refresh() {
                    Ok(true) => tracing::info!("proxy listener certificate replaced"),
                    Ok(false) => {}
                    Err(e) => tracing::error!(error = %e, "proxy listener certificate refresh failed; keeping the previous certificate"),
                },
                _ = shutdown.wait_for(|s| *s) => return,
            }
        }
    }
}

fn stamp_of(cert: &Path, key: &Path) -> Stamp {
    let m = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    (m(cert), m(key))
}

fn load_operator(
    cert: &Path,
    key: &Path,
) -> Result<(Arc<CertifiedKey>, SystemTime), ProxyTlsError> {
    let cert_pem = std::fs::read(cert).map_err(|e| {
        cfg_err(
            "gateway.proxy_tls.cert_file",
            format!("cannot read {}: {e}", cert.display()),
        )
    })?;
    let key_pem = std::fs::read(key).map_err(|e| {
        cfg_err(
            "gateway.proxy_tls.key_file",
            format!("cannot read {}: {e}", key.display()),
        )
    })?;
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| cfg_err("gateway.proxy_tls.cert_file", format!("invalid PEM: {e}")))?;
    let Some(leaf) = chain.first() else {
        return Err(cfg_err(
            "gateway.proxy_tls.cert_file",
            "no certificate found",
        ));
    };
    let (_, parsed) = x509_parser::parse_x509_certificate(leaf).map_err(|e| {
        cfg_err(
            "gateway.proxy_tls.cert_file",
            format!("invalid certificate: {e}"),
        )
    })?;
    let not_after = u64::try_from(parsed.validity().not_after.timestamp())
        .map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if not_after <= SystemTime::now() {
        return Err(cfg_err(
            "gateway.proxy_tls.cert_file",
            "the certificate has expired",
        ));
    }
    let pk = PrivateKeyDer::from_pem_slice(&key_pem)
        .map_err(|e| cfg_err("gateway.proxy_tls.key_file", format!("invalid key: {e}")))?;
    let signer = rustls::crypto::aws_lc_rs::sign::any_supported_type(&pk).map_err(|e| {
        cfg_err(
            "gateway.proxy_tls.key_file",
            format!("unsupported key: {e}"),
        )
    })?;
    let ck = CertifiedKey::new(chain, signer);
    if ck.keys_match().is_err() {
        return Err(cfg_err(
            "gateway.proxy_tls.key_file",
            "the key does not match gateway.proxy_tls.cert_file",
        ));
    }
    Ok((Arc::new(ck), not_after))
}

/// DNS names / IPs in a certificate's subjectAltName (lowercase DNS, textual IPs).
pub fn cert_names(der: &[u8]) -> Vec<String> {
    use x509_parser::extensions::GeneralName;
    let Ok((_, c)) = x509_parser::parse_x509_certificate(der) else {
        return vec![];
    };
    let Ok(Some(san)) = c.subject_alternative_name() else {
        return vec![];
    };
    san.value
        .general_names
        .iter()
        .filter_map(|g| match g {
            GeneralName::DNSName(d) => Some(d.to_ascii_lowercase()),
            GeneralName::IPAddress(b) => match b.len() {
                4 => Some(IpAddr::from([b[0], b[1], b[2], b[3]]).to_string()),
                16 => {
                    let mut a = [0u8; 16];
                    a.copy_from_slice(b);
                    Some(IpAddr::from(a).to_string())
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Whether certificate name `pattern` (possibly `*.example.org`) covers `host`.
pub fn name_covers(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => host
            .split_once('.')
            .is_some_and(|(label, rest)| !label.is_empty() && rest == suffix),
        None => pattern == host,
    }
}

/// Host part of a `scheme://[user@]host[:port]/…` URL, without brackets, lowercase.
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split(['/', '?', '#']).next()?;
    let rest = rest.rsplit('@').next()?;
    let host = if let Some(r) = rest.strip_prefix('[') {
        r.split(']').next()?
    } else {
        rest.rsplit_once(':')
            .filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit()))
            .map_or(rest, |(h, _)| h)
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Inputs of the SAN set of the CA-issued proxy certificate (docs/06 §Proxy listener TLS).
pub struct SanInputs<'a> {
    /// `gateway.advertised_proxy_url`; its host is also the sandbox-facing address in the boot
    /// document (docs/11).
    pub advertised_proxy_url: &'a str,
    /// Resolved `gateway.sandbox_proxy_url` (FIX-010, D-87): sandboxes verify the certificate
    /// against its host, so it is always a SAN.
    pub sandbox_proxy_url: &'a str,
    /// `gateway.proxy_tls.names`.
    pub extra_names: &'a [String],
    /// Kubernetes namespace when running in a cluster.
    pub namespace: Option<&'a str>,
    /// Whether the fixed compose address is a local address.
    pub compose_addr_local: bool,
}

/// The union of every SAN source, deduplicated, in a stable order.
pub fn san_set(i: &SanInputs<'_>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |n: String| {
        let n = n.trim().trim_end_matches('.').to_ascii_lowercase();
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
    };
    if let Some(h) = url_host(i.advertised_proxy_url) {
        add(h);
    }
    if let Some(h) = url_host(i.sandbox_proxy_url) {
        add(h);
    }
    add(SERVICE_NAME.to_owned());
    if let Some(ns) = i.namespace.filter(|n| !n.is_empty()) {
        add(format!("{SERVICE_NAME}.{ns}"));
        add(format!("{SERVICE_NAME}.{ns}.svc"));
        add(format!("{SERVICE_NAME}.{ns}.svc.cluster.local"));
    }
    if i.compose_addr_local {
        add(COMPOSE_GATEWAY_ADDR.to_owned());
    }
    for n in i.extra_names {
        add(n.clone());
    }
    out
}

/// The Kubernetes namespace this process runs in (downward API env, else the mounted file).
pub fn detect_namespace() -> Option<String> {
    std::env::var("POD_NAMESPACE")
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace").ok()
        })
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Whether the fixed compose address is assigned to this host (a UDP bind only succeeds on a
/// local address).
pub fn compose_addr_is_local() -> bool {
    COMPOSE_GATEWAY_ADDR
        .parse::<IpAddr>()
        .is_ok_and(|ip| UdpSocket::bind(SocketAddr::new(ip, 0)).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca;

    fn test_ca() -> Arc<Ca> {
        let g = ca::generate("t").unwrap();
        Arc::new(Ca::from_pem(&g.cert_pem, &g.key_pem).unwrap())
    }

    #[test]
    fn url_host_forms() {
        assert_eq!(
            url_host("https://GW.example.org:4102").as_deref(),
            Some("gw.example.org")
        );
        assert_eq!(url_host("https://x:tok@gw:4102/p").as_deref(), Some("gw"));
        assert_eq!(url_host("https://[::1]:4102").as_deref(), Some("::1"));
        assert_eq!(url_host("10.0.0.1:4102").as_deref(), Some("10.0.0.1"));
    }

    #[test]
    fn san_set_is_the_union_of_every_source() {
        let sans = san_set(&SanInputs {
            advertised_proxy_url: "https://lb.example.org:4102",
            sandbox_proxy_url: "https://sandbox-gw.example.org:4102",
            extra_names: &["Extra.Example.org".into(), "lb.example.org".into()],
            namespace: Some("aurora"),
            compose_addr_local: true,
        });
        assert_eq!(
            sans,
            [
                "lb.example.org",
                "sandbox-gw.example.org",
                "aurora-gateway",
                "aurora-gateway.aurora",
                "aurora-gateway.aurora.svc",
                "aurora-gateway.aurora.svc.cluster.local",
                "10.89.250.2",
                "extra.example.org",
            ]
        );
        let bare = san_set(&SanInputs {
            advertised_proxy_url: "https://aurora-gateway:4102",
            sandbox_proxy_url: "https://aurora-gateway:4102",
            extra_names: &[],
            namespace: None,
            compose_addr_local: false,
        });
        assert_eq!(bare, ["aurora-gateway"]);
    }

    #[test]
    fn issued_leaf_carries_every_san() {
        let ca = test_ca();
        let names = vec![
            "gw.test".to_owned(),
            "10.0.0.7".to_owned(),
            "aurora-gateway".to_owned(),
        ];
        let tls = ProxyTls::from_ca(ca, names.clone()).unwrap();
        let chain = tls.current_chain();
        assert_eq!(chain.len(), 1);
        let mut got = cert_names(&chain[0]);
        got.sort();
        let mut want = names;
        want.sort();
        assert_eq!(got, want);
        let left = tls.not_after().duration_since(SystemTime::now()).unwrap();
        assert!(left > Duration::from_secs(29 * 86_400) && left <= LEAF_VALIDITY);
    }

    #[test]
    fn leaf_is_renewed_only_below_the_threshold() {
        let ca = test_ca();
        let tls = ProxyTls::from_ca_with(
            ca.clone(),
            vec!["gw.test".into()],
            Duration::from_secs(3600),
            Duration::from_secs(60),
        )
        .unwrap();
        assert!(!tls.refresh().unwrap());
        assert_eq!(tls.generation(), 1);
        // Threshold above the validity: always due.
        let due = ProxyTls::from_ca_with(
            ca,
            vec!["gw.test".into()],
            Duration::from_secs(3600),
            Duration::from_secs(7200),
        )
        .unwrap();
        let before = due.current_chain();
        assert!(due.refresh().unwrap());
        assert_eq!(due.generation(), 2);
        assert_ne!(before[0].as_ref(), due.current_chain()[0].as_ref());
    }
}
