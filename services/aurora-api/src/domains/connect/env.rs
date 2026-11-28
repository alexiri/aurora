//! Outbound HTTP for connect flows: guarded client (https only, public addresses only)
//! and the hook-host client. `override_origin` redirects all guarded requests to a test
//! server (tests only).
use super::hooks::HookClient;
use arc_swap::ArcSwap;
use aurora_core::ranges::{AppHost, app_address_opens};
use aurora_db::repo::app_instances::InstanceRow;
use aurora_secrets::tls::with_extra_roots;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

pub struct Env {
    pub http: reqwest::Client,
    /// The client for requests to a registered app instance (D-74): the same guard, plus the
    /// addresses the operator opened with `gateway.allowed_private`.
    pub http_instance: reqwest::Client,
    /// `gateway.allowed_private`, swapped live when the `limits` settings change (SET-5); the
    /// instance client's resolver reads the same cell. Read it with `allow.load()`.
    pub allow: std::sync::Arc<arc_swap::ArcSwap<PrivateAllow>>,
    pub hooks: HookClient,
    pub override_origin: Option<reqwest::Url>,
    /// Hosts the skill-library reader dials for real even while `override_origin` is set, so a
    /// test can send a redirect to a **name** through the guarded resolver (tests only).
    pub override_bypass: Vec<String>,
    /// How long the registration probe waits for an answer (10 s; tests shorten it).
    probe_timeout_ms: std::sync::atomic::AtomicU64,
    /// (FIX-290, D-511) Hosts of Aurora's other services, from this process's configuration. A
    /// self-hosted app's host that resolves to an address of one of them gets no allowance.
    services: Arc<ArcSwap<Vec<String>>>,
    /// Fixed answers for names (tests only; empty in a running API).
    names: Arc<ArcSwap<StaticNames>>,
    /// `tls.extra_ca_bundle`, for the clients built per self-hosted request.
    extra_ca_bundle: Option<String>,
    admissions: Arc<std::sync::atomic::AtomicU64>,
}

type StaticNames = Vec<(String, Vec<IpAddr>)>;

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

/// The system resolver, or the fixed answer `names` holds for exactly this name.
async fn lookup(host: &str, names: &StaticNames) -> std::io::Result<Vec<SocketAddr>> {
    if let Some((_, ips)) = names.iter().find(|(n, _)| n == host) {
        return Ok(ips.iter().map(|ip| SocketAddr::new(*ip, 0)).collect());
    }
    Ok(tokio::net::lookup_host((host, 0)).await?.collect())
}

/// Look `host` up and keep what `keep` allows.
async fn resolve_guarded(
    host: &str,
    names: &StaticNames,
    keep: impl Fn(IpAddr) -> bool,
) -> Result<Vec<SocketAddr>, ResolveError> {
    let all = lookup(host, names)
        .await
        .map_err(|_| ResolveError::NotFound)?;
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
            let addrs = resolve_guarded(name.as_str(), &Vec::new(), is_public).await?;
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

pub use aurora_core::ranges::PrivateAllow;

/// Resolver for an app instance's host: public addresses plus what `allowed_private` opens.
struct GuardedAllow {
    allow: Arc<ArcSwap<PrivateAllow>>,
    names: Arc<ArcSwap<StaticNames>>,
}

impl Resolve for GuardedAllow {
    fn resolve(&self, name: Name) -> Resolving {
        // One load per lookup: a change of the list applies to the next connection.
        let allow = self.allow.load_full();
        let names = self.names.load_full();
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs = resolve_guarded(&host, &names, |ip| allow.permits(ip, &host)).await?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// (FIX-290) [`GuardedAllow`] for one request that carries a self-hosted app's allowance: the
/// private addresses `aurora_core::ranges::app_address_opens` opens, for the app's name only.
/// (The allowance is for port 443; the API listens on no such port.)
struct GuardedApp {
    app: AppHost,
    allow: Arc<PrivateAllow>,
    names: Arc<StaticNames>,
    services: Arc<Vec<String>>,
    admissions: Arc<std::sync::atomic::AtomicU64>,
}

/// The addresses of Aurora's other services; `None` when one of them cannot be told.
async fn service_addresses(services: &[String], names: &StaticNames) -> Option<Vec<IpAddr>> {
    let mut out = Vec::new();
    for s in services {
        if let Ok(ip) = s.parse::<IpAddr>() {
            out.push(ip);
            continue;
        }
        match lookup(s, names).await {
            Ok(a) => out.extend(a.into_iter().map(|a| a.ip())),
            Err(error) => {
                tracing::warn!(service = %s, %error,
                    "one of Aurora's own services did not resolve; self-hosted app hosts stay closed");
                return None;
            }
        }
    }
    Some(out)
}

impl Resolve for GuardedApp {
    fn resolve(&self, name: Name) -> Resolving {
        let (app, allow) = (self.app.clone(), self.allow.clone());
        let (names, services) = (self.names.clone(), self.services.clone());
        let admissions = self.admissions.clone();
        Box::pin(async move {
            let host = name.as_str().to_owned();
            // The fully qualified name, and where Aurora's other services are.
            let absolute: Vec<IpAddr> = lookup(&format!("{}.", app.host()), &names)
                .await
                .map(|a| a.into_iter().map(|a| a.ip()).collect())
                .unwrap_or_default();
            let services = service_addresses(&services, &names).await;
            let named = app.covers(&host, app.port());
            let addrs = resolve_guarded(&host, &names, |ip| {
                if allow.permits(ip, &host) {
                    return true;
                }
                let opened = named
                    && services
                        .as_ref()
                        .is_some_and(|s| app_address_opens(ip, &absolute, s));
                if opened {
                    admissions.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                opened
            })
            .await?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// A client for requests to registered instances: no redirects, no system proxy, `resolver` as
/// its destination guard, and `tls.extra_ca_bundle`.
fn guarded_client<R: Resolve + 'static>(
    resolver: Arc<R>,
    extra_ca_bundle: Option<&str>,
) -> Result<reqwest::Client, String> {
    with_extra_roots(
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            // (FIX-217) A system proxy would resolve the name itself: the guard must see it.
            .no_proxy()
            .dns_resolver(resolver),
        extra_ca_bundle,
    )?
    .build()
    .map_err(|e| e.to_string())
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
                // (FIX-217, SEC-04) A system proxy would resolve the name itself and the guarded
                // resolver below would never run.
                .no_proxy()
                .dns_resolver(std::sync::Arc::new(Guarded)),
            extra_ca_bundle,
        )?
        .build()
        .map_err(|e| format!("connect http client: {e}"))?;
        let allow = Arc::new(ArcSwap::from_pointee(PrivateAllow::new(allowed_private)));
        let names = Arc::new(ArcSwap::from_pointee(Vec::new()));
        let http_instance = guarded_client(
            Arc::new(GuardedAllow {
                allow: allow.clone(),
                names: names.clone(),
            }),
            extra_ca_bundle,
        )
        .map_err(|e| format!("connect instance http client: {e}"))?;
        Ok(Self {
            services: Arc::new(ArcSwap::from_pointee(Vec::new())),
            names,
            extra_ca_bundle: extra_ca_bundle.map(str::to_owned),
            admissions: Arc::default(),
            http,
            http_instance,
            allow,
            hooks: HookClient::new(hookhost_url, internal_ring, extra_ca_bundle)?,
            override_origin: None,
            override_bypass: Vec::new(),
            probe_timeout_ms: std::sync::atomic::AtomicU64::new(10_000),
        })
    }

    /// Replace the addresses `gateway.allowed_private` opens (SET-5): applies from the next lookup.
    pub fn set_allowed_private(&self, entries: &[String]) {
        self.allow
            .store(std::sync::Arc::new(PrivateAllow::new(entries)));
    }

    /// (P5-WEBHOOK-2, C-909) Resolve `host` and run the destination guard (private ranges only under
    /// `gateway.allowed_private`) without sending anything: an outgoing webhook is not called at
    /// connect.
    pub async fn check_destination(&self, host: &str) -> Result<(), ResolveError> {
        let allow = self.allow.load_full();
        let names = self.names.load_full();
        resolve_guarded(host, &names, |ip| allow.permits(ip, host))
            .await
            .map(|_| ())
    }

    /// (FIX-290) Name Aurora's other services (host names or addresses).
    pub fn set_own_services(&self, hosts: Vec<String>) {
        self.services.store(Arc::new(hosts));
    }

    /// Answer these names with these addresses instead of the system resolver (tests only). A
    /// name is matched exactly as asked, so a fully qualified `name.` is its own entry.
    #[doc(hidden)]
    pub fn set_static_names(&self, names: Vec<(String, Vec<IpAddr>)>) {
        self.names.store(Arc::new(names));
    }

    /// (FIX-290, D-511, SEC-169) What a request that belongs to this registration may reach
    /// beyond `gateway.allowed_private`: its own host on 443, only for an active registration of
    /// an instance administrator (instance-wide and not a package's preset). Read the row for
    /// each request, so a removed or disabled one opens nothing.
    #[must_use]
    pub fn self_hosted(&self, instance: &InstanceRow) -> Option<AppHost> {
        (instance.org_id.is_none() && !instance.builtin && instance.status == "active")
            .then(|| AppHost::new(&instance.host, 443))
            .flatten()
    }

    /// (FIX-290) [`Self::self_hosted`] for the registration an instance administrator is making
    /// for all organizations, before it is stored. Call it for that request only.
    #[must_use]
    pub fn registering(&self, host: &str) -> Option<AppHost> {
        AppHost::new(host, 443)
    }

    /// How many lookups a self-hosted app's allowance has admitted on a private address.
    #[doc(hidden)]
    #[must_use]
    pub fn self_hosted_admissions(&self) -> u64 {
        self.admissions.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The client for one request to `url` that belongs to a registered instance:
    /// [`Self::http_instance`], unless `app` covers an `https` `url`. That request gets a client
    /// of its own (FIX-290), so no other request can reuse a connection it opened.
    #[must_use]
    pub fn instance_client(&self, app: Option<&AppHost>, url: &reqwest::Url) -> reqwest::Client {
        let covered = app.filter(|a| {
            url.scheme() == "https"
                && url
                    .host_str()
                    .zip(url.port_or_known_default())
                    .is_some_and(|(h, p)| a.covers(h, p))
        });
        let Some(app) = covered else {
            return self.http_instance.clone();
        };
        let resolver = GuardedApp {
            app: app.clone(),
            allow: self.allow.load_full(),
            names: self.names.load_full(),
            services: self.services.load_full(),
            admissions: self.admissions.clone(),
        };
        match guarded_client(Arc::new(resolver), self.extra_ca_bundle.as_deref()) {
            Ok(c) => c,
            Err(error) => {
                tracing::error!(%error, "building a self-hosted app client failed");
                self.http_instance.clone()
            }
        }
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

    fn registration(org: bool, builtin: bool, status: &str) -> InstanceRow {
        InstanceRow {
            id: uuid::Uuid::now_v7(),
            org_id: org.then(uuid::Uuid::now_v7),
            package_id: "p".into(),
            app_id: "wiki".into(),
            host: "wiki.corp.example".into(),
            base_path: None,
            label: "Wiki".into(),
            status: status.into(),
            created_by: None,
            created_by_name: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            connections: 0,
            org_name: None,
            org_slug: None,
            builtin,
        }
    }

    /// (FIX-290, D-511) Only an active registration of an instance administrator (instance-wide,
    /// not a package's preset) carries an allowance: for its own host on 443.
    #[test]
    fn only_an_active_admin_registration_carries_an_allowance() {
        let env = Env::new(
            "http://hooks.example:4104",
            &aurora_crypto::SecretRing::default(),
            &[],
            None,
        )
        .unwrap();
        let a = env
            .self_hosted(&registration(false, false, "active"))
            .unwrap();
        assert_eq!((a.host(), a.port()), ("wiki.corp.example", 443));
        for (org, builtin, status) in [
            (true, false, "active"),
            (false, true, "active"),
            (false, false, "disabled"),
        ] {
            assert!(
                env.self_hosted(&registration(org, builtin, status))
                    .is_none(),
                "{org} {builtin} {status}"
            );
        }
    }

    /// (FIX-290, D-511) The client built with an allowance opens a private address for exactly
    /// `https://<registered host>:443`; every other URL gets the ordinary instance client.
    #[tokio::test]
    async fn the_allowance_covers_https_443_of_the_registered_host_only() {
        let env = Env::new(
            "http://hooks.example:4104",
            &aurora_crypto::SecretRing::default(),
            &[],
            None,
        )
        .unwrap();
        let point = |ip: &str, also_absolute: bool| {
            let ips = vec![ip.parse().unwrap()];
            let mut n = vec![
                ("wiki.corp.example".to_owned(), ips.clone()),
                ("other.corp.example".to_owned(), ips.clone()),
            ];
            let qualified = if also_absolute { ips } else { Vec::new() };
            n.push(("wiki.corp.example.".to_owned(), qualified.clone()));
            n.push(("other.corp.example.".to_owned(), qualified));
            env.set_static_names(n);
        };
        let app = env.self_hosted(&registration(false, false, "active"));
        let reason = |url: &'static str| {
            let url = reqwest::Url::parse(url).unwrap();
            let c = env.instance_client(app.as_ref(), &url);
            async move {
                let e = c
                    .get(url)
                    .timeout(Duration::from_millis(300))
                    .send()
                    .await
                    .unwrap_err();
                classify_send_error(&e)
            }
        };
        let opened = |r: &str| r == "timeout" || r == "connect";
        point("10.255.255.1", true);
        assert!(opened(reason("https://wiki.corp.example/x").await));
        for url in [
            "https://other.corp.example/x",
            "https://wiki.corp.example:8443/x",
            "http://wiki.corp.example/x",
            "http://wiki.corp.example:443/x",
        ] {
            assert_eq!(reason(url).await, "destination_refused", "{url}");
        }
        // Without the allowance the same URL is refused.
        let url = reqwest::Url::parse("https://wiki.corp.example/x").unwrap();
        let e = env
            .instance_client(None, &url)
            .get(url)
            .send()
            .await
            .unwrap_err();
        assert_eq!(classify_send_error(&e), "destination_refused");
        for bad in [
            "127.0.0.1",
            "::1",
            "169.254.169.254",
            "fd00:ec2::254",
            "100.100.100.200",
        ] {
            point(bad, true);
            assert_eq!(
                reason("https://wiki.corp.example/x").await,
                "destination_refused",
                "{bad}"
            );
        }
        // A name only a DNS search domain completes (no answer for the fully qualified name).
        point("10.255.255.1", false);
        assert_eq!(
            reason("https://wiki.corp.example/x").await,
            "destination_refused"
        );
        // One of Aurora's own services at the address.
        point("10.255.255.1", true);
        env.set_own_services(vec!["10.255.255.1".to_owned()]);
        assert_eq!(
            reason("https://wiki.corp.example/x").await,
            "destination_refused"
        );
    }

    /// (FIX-217, SEC-04) Proxy environment variables do not apply to the guarded clients: with
    /// `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY` pointing at a local listener, a name that resolves to
    /// loopback is still refused by the guard and the listener receives no connection. The variables
    /// are set only on a child process (this test re-runs itself), so they cannot leak into other
    /// tests of this process.
    #[test]
    fn proxy_environment_does_not_bypass_the_guard() {
        if let Ok(port) = std::env::var("FIX217_PROXY_PORT") {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let ring = aurora_crypto::SecretRing::default();
                let env = Env::new("http://hooks.example:4104", &ring, &[], None).unwrap();
                let url = format!("https://localhost:{port}/");
                for c in [&env.http, &env.http_instance] {
                    let e = c.get(&url).send().await.unwrap_err();
                    assert_eq!(classify_send_error(&e), "destination_refused", "{e:?}");
                }
            });
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let proxy = format!("http://127.0.0.1:{port}");
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "domains::connect::env::tests::proxy_environment_does_not_bypass_the_guard",
                "--nocapture",
            ])
            .env("FIX217_PROXY_PORT", port.to_string())
            .env("HTTPS_PROXY", &proxy)
            .env("HTTP_PROXY", &proxy)
            .env("ALL_PROXY", &proxy)
            .env("https_proxy", &proxy)
            .env("http_proxy", &proxy)
            .env("all_proxy", &proxy)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "child failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("1 passed"),
            "child did not run the test"
        );
        assert!(
            matches!(
                listener.accept().map(|_| ()).map_err(|e| e.kind()),
                Err(std::io::ErrorKind::WouldBlock)
            ),
            "the proxy listener received a connection"
        );
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
