#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BYO admission (phase 2, RFE-025, SEC-52): posture, source address (TLS peer / PROXY protocol,
//! never HTTP headers), refusal logging, live-tunnel closure on invalidation.

use async_trait::async_trait;
use aurora_core::ApiError;
use aurora_core::cidr::Cidr;
use aurora_gateway::auth::{
    ByoAdmission, CredentialKind, CredentialResolver, RequestIdentity, ResolveError,
};
use aurora_gateway::byo::{self, Refusal, TunnelRegistry};
use aurora_gateway::ca;
use aurora_gateway::guard::Guard;
use aurora_gateway::invalidation::{Invalidatable, Invalidate, Scope};
use aurora_gateway::pipeline::RequestHandler;
use aurora_gateway::proxy::{ByoParts, Proxy, ProxyParts};
use aurora_llm::governance::{ByoInputs, ByoInstance, ByoPosture, OrgByo, effective_byo};
use base64::Engine;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use uuid::Uuid;

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn identity(token_kind: CredentialKind, byo: Option<ByoAdmission>) -> RequestIdentity {
    let id = Uuid::now_v7();
    RequestIdentity {
        credential_id: id,
        kind: token_kind,
        agent_id: Uuid::now_v7(),
        workspace_id: Uuid::now_v7(),
        org_id: Uuid::now_v7(),
        conversation_id: None,
        turn_id: None,
        context_label: None,
        speaker_user_id: None,
        speaker_group_ids: Vec::new(),
        capabilities: vec![],
        byo,
    }
}

fn admission(inst: &ByoInstance, i: &ByoInputs<'_>) -> ByoAdmission {
    ByoAdmission {
        effective: effective_byo(inst, i),
        org_slug: "acme".into(),
        ws_slug: "main".into(),
    }
}

fn enabled_inst() -> ByoInstance {
    ByoInstance {
        minimum_posture: ByoPosture::Enabled,
        ..ByoInstance::default()
    }
}

fn with_list(list: &[&str]) -> ByoAdmission {
    let l: Vec<String> = list.iter().map(|s| (*s).to_owned()).collect();
    admission(
        &enabled_inst(),
        &ByoInputs {
            agent_cidrs: Some(&l),
            ..ByoInputs::default()
        },
    )
}

// ---- pure admission --------------------------------------------------------

#[test]
fn admit_ignores_credentials_not_subject_to_byo() {
    let id = identity(CredentialKind::Boot, None);
    assert!(byo::admit(&id, ip("198.51.100.1"), "https://a.test").is_ok());
}

#[test]
fn source_check_ipv4_ipv6_mapped_and_boundaries() {
    let id = identity(
        CredentialKind::Agent,
        Some(with_list(&["203.0.113.0/24", "2001:db8:a::/48"])),
    );
    let ok = |a: &str| byo::admit(&id, ip(a), "https://a.test").is_ok();
    assert!(ok("203.0.113.0") && ok("203.0.113.255"));
    assert!(!ok("203.0.112.255") && !ok("203.0.114.0"));
    assert!(ok("2001:db8:a:ffff::1") && !ok("2001:db8:b::1"));
    assert!(ok("::ffff:203.0.113.9"), "IPv4-mapped peers are normalised");
    assert!(!ok("::ffff:198.51.100.9"));
    let (reason, err) = byo::admit(&id, ip("198.51.100.9"), "https://a.test/").unwrap_err();
    assert_eq!(reason, Refusal::Source);
    assert_eq!(err.status(), 403);
    assert_eq!(err.code(), "source_not_permitted");
    let d = err.details().unwrap();
    assert_eq!(d["source"], "198.51.100.9");
    assert_eq!(d["manage_url"], "https://a.test/o/acme/w/main/settings");
}

#[test]
fn unrestricted_agents_admit_any_address() {
    let id = identity(
        CredentialKind::Context,
        Some(admission(&enabled_inst(), &ByoInputs::default())),
    );
    assert!(byo::admit(&id, ip("8.8.8.8"), "https://a.test").is_ok());
    assert!(byo::admit(&id, ip("2001:4860::1"), "https://a.test").is_ok());
}

#[test]
fn posture_disabled_is_403_byo_disabled_with_the_level() {
    for (inst, ws, level) in [
        (
            ByoInstance {
                enabled: false,
                ..enabled_inst()
            },
            None,
            "instance",
        ),
        (enabled_inst(), Some("disabled"), "workspace"),
        (
            ByoInstance {
                minimum_posture: ByoPosture::Disabled,
                ..ByoInstance::default()
            },
            Some("enabled"),
            "instance",
        ),
    ] {
        let id = identity(
            CredentialKind::Agent,
            Some(admission(
                &inst,
                &ByoInputs {
                    workspace_posture: ws,
                    ..ByoInputs::default()
                },
            )),
        );
        let (reason, err) = byo::admit(&id, ip("127.0.0.1"), "https://a.test").unwrap_err();
        assert_eq!((reason, err.code()), (Refusal::Posture, "byo_disabled"));
        assert_eq!(err.details().unwrap()["level"], level);
    }
    // org layer
    let id = identity(
        CredentialKind::Agent,
        Some(admission(
            &enabled_inst(),
            &ByoInputs {
                org: OrgByo {
                    posture: Some(ByoPosture::Disabled),
                    ..OrgByo::default()
                },
                ..ByoInputs::default()
            },
        )),
    );
    let (_, err) = byo::admit(&id, ip("127.0.0.1"), "https://a.test").unwrap_err();
    assert_eq!(err.details().unwrap()["level"], "org");
    assert!(
        err.details().unwrap()["manage_url"]
            .as_str()
            .unwrap()
            .ends_with("/o/acme/governance")
    );
    // owners_only still admits existing credentials.
    let id = identity(
        CredentialKind::Agent,
        Some(admission(&ByoInstance::default(), &ByoInputs::default())),
    );
    assert!(byo::admit(&id, ip("127.0.0.1"), "https://a.test").is_ok());
}

#[test]
fn refusal_limiter_logs_once_per_credential_per_interval() {
    let l = byo::RefusalLimiter::default();
    let a = Uuid::now_v7();
    let b = Uuid::now_v7();
    assert!(l.should_log(a));
    assert!(!l.should_log(a));
    assert!(l.should_log(b));
    assert!(!l.should_log(b));
}

// ---- through the listener --------------------------------------------------

type Creds = Arc<Mutex<HashMap<String, RequestIdentity>>>;

/// Agents the fake store treats as effectively frozen (RFE-006): their credentials resolve to
/// `ResolveError::Frozen`, as the database-backed resolver does.
type FrozenAgents = Arc<Mutex<std::collections::HashSet<Uuid>>>;

struct MapResolver(Creds, FrozenAgents, Expiry);

/// (RFE-013) Agent tokens with an expiry: refused with `ResolveError::Expired` once it passed,
/// and reported to the tunnel registry as the deadline before that.
type Expiry = Arc<Mutex<HashMap<Uuid, std::time::Instant>>>;

impl MapResolver {
    fn check(&self, i: Option<RequestIdentity>) -> Result<Option<RequestIdentity>, ResolveError> {
        match i {
            Some(i) if self.1.lock().unwrap().contains(&i.agent_id) => {
                Err(ResolveError::Frozen { level: "agent" })
            }
            Some(i)
                if i.kind == CredentialKind::Agent
                    && self
                        .2
                        .lock()
                        .unwrap()
                        .get(&i.agent_id)
                        .is_some_and(|d| *d <= std::time::Instant::now()) =>
            {
                Err(ResolveError::Expired)
            }
            other => Ok(other),
        }
    }
}

#[async_trait]
impl CredentialResolver for MapResolver {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        self.check(self.0.lock().unwrap().get(token).cloned())
    }
    async fn resolve_credential(&self, id: Uuid) -> Result<Option<RequestIdentity>, ResolveError> {
        self.check(
            self.0
                .lock()
                .unwrap()
                .values()
                .find(|i| i.credential_id == id)
                .cloned(),
        )
    }
    fn token_deadline(&self, credential: Uuid) -> Option<std::time::Instant> {
        let agent = self
            .0
            .lock()
            .unwrap()
            .values()
            .find(|i| i.credential_id == credential && i.kind == CredentialKind::Agent)?
            .agent_id;
        self.2.lock().unwrap().get(&agent).copied()
    }
}

#[derive(Default)]
struct Capture(Mutex<Vec<(String, String, Refusal, String)>>);

struct Handler(Arc<Capture>);

#[async_trait]
impl RequestHandler for Handler {
    fn byo_refused(
        &self,
        _identity: &Arc<RequestIdentity>,
        method: &str,
        host: &str,
        reason: Refusal,
        err: &ApiError,
    ) {
        self.0
            .0
            .lock()
            .unwrap()
            .push((method.into(), host.into(), reason, err.code().into()));
    }
}

struct Env {
    proxy: SocketAddr,
    creds: Creds,
    frozen: FrozenAgents,
    expiry: Expiry,
    tunnels: Arc<TunnelRegistry>,
    log: Arc<Capture>,
    gw_ca_pem: String,
}

async fn start(trusted: &[&str]) -> Env {
    let creds: Creds = Arc::default();
    let frozen: FrozenAgents = Arc::default();
    let expiry: Expiry = Arc::default();
    let resolver: Arc<dyn CredentialResolver> =
        Arc::new(MapResolver(creds.clone(), frozen.clone(), expiry.clone()));
    let gw = ca::generate("gw").unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = l.local_addr().unwrap();
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy]).unwrap());
    let (gw_ca, tls) = crate::common::gateway_tls(&gw, proxy);
    let tunnels = TunnelRegistry::new(resolver.clone(), "https://aurora.test");
    let log = Arc::new(Capture::default());
    let p = Proxy::with_byo(
        ProxyParts {
            ca: gw_ca,
            tls,
            resolver,
            handler: Arc::new(Handler(log.clone())),
            guard,
            header_timeout: Duration::from_secs(5),
            extra_ca_pem: None,
            egress_proxy: None,
        },
        ByoParts {
            trusted_proxies: trusted.iter().map(|c| Cidr::parse(c).unwrap()).collect(),
            external_url: "https://aurora.test".into(),
            tunnels: Some(tunnels.clone()),
            ..ByoParts::default()
        },
    )
    .unwrap();
    tokio::spawn(p.run(l, std::future::pending()));
    Env {
        proxy,
        creds,
        frozen,
        expiry,
        tunnels,
        log,
        gw_ca_pem: gw.cert_pem,
    }
}

impl Env {
    fn add(&self, token: &str, kind: CredentialKind, byo: Option<ByoAdmission>) -> RequestIdentity {
        let i = identity(kind, byo);
        self.creds.lock().unwrap().insert(token.into(), i.clone());
        i
    }

    /// TCP connect, optional raw prefix (PROXY header), then the TLS handshake.
    async fn connect(&self, prefix: &[u8]) -> std::io::Result<TlsStream<TcpStream>> {
        let mut tcp = TcpStream::connect(self.proxy).await?;
        tcp.write_all(prefix).await?;
        TlsConnector::from(crate::common::client_config(
            &self.gw_ca_pem,
            &[b"http/1.1"],
        ))
        .connect(
            rustls::pki_types::ServerName::try_from("127.0.0.1".to_owned()).unwrap(),
            tcp,
        )
        .await
    }

    async fn request(&self, prefix: &[u8], token: &str, head: &str) -> Option<(u16, String)> {
        let mut s = self.connect(prefix).await.ok()?;
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{token}"));
        s.write_all(
            format!("{head}Proxy-Authorization: Basic {auth}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .ok()?;
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
        let text = String::from_utf8_lossy(&out).into_owned();
        let status = text.split_whitespace().nth(1)?.parse().ok()?;
        Some((status, text))
    }

    async fn connect_req(&self, prefix: &[u8], token: &str, extra: &str) -> Option<(u16, String)> {
        self.request(
            prefix,
            token,
            &format!("CONNECT 127.0.0.1:1 HTTP/1.1\r\nHost: 127.0.0.1:1\r\n{extra}"),
        )
        .await
    }
}

fn body_code(text: &str) -> String {
    let body = text.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    v["error"]["code"].as_str().unwrap_or("").to_owned()
}

fn v1(src: &str) -> Vec<u8> {
    format!("PROXY TCP4 {src} 10.0.0.1 40000 4102\r\n").into_bytes()
}

fn v2(src: [u8; 4]) -> Vec<u8> {
    let mut v = b"\r\n\r\n\0\r\nQUIT\n".to_vec();
    v.extend([0x21, 0x11, 0, 12]);
    v.extend(src);
    v.extend([10, 0, 0, 1]);
    v.extend(40000u16.to_be_bytes());
    v.extend(4102u16.to_be_bytes());
    v
}

#[tokio::test]
async fn posture_disabled_refuses_connect_and_absolute_form_with_403_not_407() {
    let env = start(&[]).await;
    let off = ByoInstance {
        enabled: false,
        ..enabled_inst()
    };
    env.add(
        "aug_off",
        CredentialKind::Agent,
        Some(admission(&off, &ByoInputs::default())),
    );
    env.add(
        "auc_off",
        CredentialKind::Context,
        Some(admission(&off, &ByoInputs::default())),
    );
    for t in ["aug_off", "auc_off"] {
        let (s, text) = env.connect_req(&[], t, "").await.unwrap();
        assert_eq!(
            (s, body_code(&text).as_str()),
            (403, "byo_disabled"),
            "{text}"
        );
        let (s, text) = env
            .request(
                &[],
                t,
                "GET http://127.0.0.1:1/ HTTP/1.1\r\nHost: 127.0.0.1:1\r\n",
            )
            .await
            .unwrap();
        assert_eq!(
            (s, body_code(&text).as_str()),
            (403, "byo_disabled"),
            "{text}"
        );
    }
    // A bad token still gets 407 (authentication comes first: a prober learns nothing).
    let (s, _) = env.connect_req(&[], "aug_nope", "").await.unwrap();
    assert_eq!(s, 407);
    // Hosted-style credentials (no BYO identity) are unaffected.
    env.add("aub_boot", CredentialKind::Boot, None);
    let (s, _) = env.connect_req(&[], "aub_boot", "").await.unwrap();
    assert_eq!(s, 200);
}

#[tokio::test]
async fn source_list_uses_the_tls_peer_and_ignores_forwarding_headers() {
    let env = start(&[]).await;
    // The client is 127.0.0.1: a list without it refuses, even if X-Forwarded-For claims better.
    env.add(
        "aug_out",
        CredentialKind::Agent,
        Some(with_list(&["203.0.113.0/24"])),
    );
    for extra in [
        "",
        "X-Forwarded-For: 203.0.113.5\r\n",
        "Forwarded: for=203.0.113.5\r\n",
        "X-Real-IP: 203.0.113.5\r\n",
    ] {
        let (s, text) = env.connect_req(&[], "aug_out", extra).await.unwrap();
        assert_eq!(
            (s, body_code(&text).as_str()),
            (403, "source_not_permitted"),
            "{extra}"
        );
        assert!(
            text.contains("127.0.0.1"),
            "details.source is the address the gateway saw"
        );
    }
    // A list containing the peer admits it, whatever the headers claim.
    env.add(
        "aug_in",
        CredentialKind::Agent,
        Some(with_list(&["127.0.0.0/8"])),
    );
    for extra in ["", "X-Forwarded-For: 198.51.100.1\r\n"] {
        let (s, _) = env.connect_req(&[], "aug_in", extra).await.unwrap();
        assert_eq!(s, 200, "{extra}");
    }
    // Absolute-form requests are checked too.
    let (s, text) = env
        .request(
            &[],
            "aug_out",
            "GET http://127.0.0.1:1/ HTTP/1.1\r\nHost: 127.0.0.1:1\r\nX-Forwarded-For: 203.0.113.5\r\n",
        )
        .await
        .unwrap();
    assert_eq!(
        (s, body_code(&text).as_str()),
        (403, "source_not_permitted")
    );
    // A bad token from the same address is 407, not 403.
    let (s, _) = env.connect_req(&[], "aug_unknown", "").await.unwrap();
    assert_eq!(s, 407);
}

#[tokio::test]
async fn refusals_are_logged_once_per_credential_per_minute() {
    let env = start(&[]).await;
    env.add(
        "aug_out",
        CredentialKind::Agent,
        Some(with_list(&["203.0.113.0/24"])),
    );
    for _ in 0..3 {
        let (s, _) = env.connect_req(&[], "aug_out", "").await.unwrap();
        assert_eq!(s, 403);
    }
    let rows = env.log.0.lock().unwrap().clone();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].2, Refusal::Source);
    assert_eq!(rows[0].3, "source_not_permitted");
    assert_eq!(rows[0].0, "CONNECT");
}

#[tokio::test]
async fn proxy_protocol_only_from_trusted_peers_and_mandatory_for_them() {
    let env = start(&["127.0.0.0/8"]).await;
    env.add(
        "aug_v",
        CredentialKind::Agent,
        Some(with_list(&["203.0.113.0/24"])),
    );
    // Trusted peer, v1 and v2 headers: the header's address decides.
    let (s, _) = env
        .connect_req(&v1("203.0.113.7"), "aug_v", "")
        .await
        .unwrap();
    assert_eq!(s, 200);
    let (s, _) = env
        .connect_req(&v2([203, 0, 113, 8]), "aug_v", "")
        .await
        .unwrap();
    assert_eq!(s, 200);
    let (s, text) = env
        .connect_req(&v1("198.51.100.1"), "aug_v", "")
        .await
        .unwrap();
    assert_eq!(
        (s, body_code(&text).as_str()),
        (403, "source_not_permitted")
    );
    assert!(text.contains("198.51.100.1"));
    let (s, text) = env
        .connect_req(&v2([198, 51, 100, 2]), "aug_v", "")
        .await
        .unwrap();
    assert_eq!(
        (s, body_code(&text).as_str()),
        (403, "source_not_permitted")
    );
    // Trusted peer without a header: closed before the TLS handshake (NAT/LB fail-closed).
    assert!(
        env.connect(&[]).await.is_err(),
        "no PROXY header from a trusted peer"
    );
    // Malformed header: closed.
    assert!(env.connect(b"PROXY TCP4 nonsense\r\n").await.is_err());
    // PROXY LOCAL / UNKNOWN (health checks) carry no client: closed.
    assert!(env.connect(b"PROXY UNKNOWN\r\n").await.is_err());

    // Untrusted peer sending a PROXY header: closed before the handshake.
    let untrusted = start(&[]).await;
    untrusted.add(
        "aug_v",
        CredentialKind::Agent,
        Some(with_list(&["203.0.113.0/24"])),
    );
    assert!(untrusted.connect(&v1("203.0.113.7")).await.is_err());
    assert!(untrusted.connect(&v2([203, 0, 113, 8])).await.is_err());
    // …and the spoofed address never admits anything.
    let (s, _) = untrusted.connect_req(&[], "aug_v", "").await.unwrap();
    assert_eq!(s, 403);
}

async fn alive(s: &mut TlsStream<TcpStream>, within: Duration) -> bool {
    let mut b = [0u8; 16];
    match tokio::time::timeout(within, s.read(&mut b)).await {
        Err(_) => true,
        Ok(Ok(0) | Err(_)) => false,
        Ok(Ok(_)) => true,
    }
}

async fn open_tunnel(env: &Env, token: &str) -> TlsStream<TcpStream> {
    let mut s = env.connect(&[]).await.unwrap();
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{token}"));
    s.write_all(
        format!("CONNECT 127.0.0.1:1 HTTP/1.1\r\nHost: 127.0.0.1:1\r\nProxy-Authorization: Basic {auth}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert!(s.read(&mut b).await.unwrap() > 0);
        head.push(b[0]);
    }
    assert!(
        String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"),
        "tunnel opened"
    );
    s
}

/// RFE-006 (SEC-45): a freeze of the agent, its workspace or its org refuses new connections
/// with 403 `frozen` (not 407) and closes live tunnels of every credential kind — hosted ones
/// included, which carry no BYO controls — within the cache refresh; unfreezing restores the
/// same token at once.
#[tokio::test]
async fn freeze_refuses_credentials_403_and_closes_live_tunnels_of_every_kind() {
    for (token, kind, scope_of) in [
        ("aub_boot", CredentialKind::Boot, 0u8),
        ("auc_ctx", CredentialKind::Context, 1),
        ("aug_agent", CredentialKind::Agent, 2),
    ] {
        let env = start(&[]).await;
        let id = env.add(token, kind, None);
        let mut tunnel = open_tunnel(&env, token).await;
        assert_eq!(env.tunnels.len(), 1, "non-BYO tunnels are registered too");
        // Unrelated invalidations keep it open.
        env.tunnels.invalidate(&Invalidate {
            scope: Scope::Org(Uuid::now_v7()),
        });
        env.tunnels.invalidate(&Invalidate {
            scope: Scope::Agent(id.agent_id),
        });
        assert!(alive(&mut tunnel, Duration::from_millis(300)).await);
        env.frozen.lock().unwrap().insert(id.agent_id);
        let started = std::time::Instant::now();
        env.tunnels.invalidate(&Invalidate {
            scope: match scope_of {
                0 => Scope::Org(id.org_id),
                1 => Scope::Workspace(id.workspace_id),
                _ => Scope::Agent(id.agent_id),
            },
        });
        assert!(
            !alive(&mut tunnel, Duration::from_secs(5)).await,
            "{token}: tunnel closed"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        let (s, text) = env.connect_req(&[], token, "").await.unwrap();
        assert_eq!((s, body_code(&text).as_str()), (403, "frozen"), "{text}");
        assert!(text.contains("\"level\":\"agent\""), "{text}");
        // Unfreeze: the same token works again and nothing was revoked.
        env.frozen.lock().unwrap().remove(&id.agent_id);
        let (s, _) = env.connect_req(&[], token, "").await.unwrap();
        assert_eq!(s, 200);
    }
}

/// (RFE-013, C-312, SEC-53) An expired agent token is 407 `proxy_auth_required` with
/// `details.reason = "agent_token_expired"` on both HTTP credential paths that authenticate a
/// request (CONNECT, which opens the MITM tunnel, and the absolute-form forward request); an
/// unknown token keeps the plain 407.
#[tokio::test]
async fn expired_agent_token_is_407_agent_token_expired_on_connect_and_absolute_form() {
    let env = start(&[]).await;
    let id = env.add("aug_old", CredentialKind::Agent, None);
    env.add("aug_ok", CredentialKind::Agent, None);
    env.expiry.lock().unwrap().insert(
        id.agent_id,
        std::time::Instant::now() - Duration::from_secs(1),
    );
    let absolute = "GET http://127.0.0.1:1/x HTTP/1.1\r\nHost: 127.0.0.1:1\r\n";
    for (path, head) in [
        (
            "CONNECT",
            "CONNECT 127.0.0.1:1 HTTP/1.1\r\nHost: 127.0.0.1:1\r\n",
        ),
        ("absolute-form", absolute),
    ] {
        let (s, text) = env.request(&[], "aug_old", head).await.unwrap();
        assert_eq!(s, 407, "{path}: {text}");
        assert!(
            text.contains("\"reason\":\"agent_token_expired\""),
            "{path}: {text}"
        );
        assert_eq!(body_code(&text), "proxy_auth_required", "{path}");
        assert!(
            text.to_lowercase()
                .contains("proxy-authenticate: basic realm=\"aurora\""),
            "{path}: {text}"
        );
        // Unknown tokens are told nothing about expiry.
        let (s, text) = env.request(&[], "aug_nope", head).await.unwrap();
        assert_eq!(s, 407, "{path}: {text}");
        assert!(!text.contains("agent_token_expired"), "{path}: {text}");
    }
    // A token that has not expired yet is admitted.
    let (s, _) = env.connect_req(&[], "aug_ok", "").await.unwrap();
    assert_eq!(s, 200);
}

/// (RFE-013) Authentication happens at CONNECT, so the agent token's expiry also ends the tunnel
/// it admitted: the tunnel is closed at the deadline and a new CONNECT is refused.
#[tokio::test]
async fn live_tunnel_closes_when_its_agent_token_expires() {
    let env = start(&[]).await;
    let id = env.add("aug_soon", CredentialKind::Agent, None);
    env.expiry.lock().unwrap().insert(
        id.agent_id,
        std::time::Instant::now() + Duration::from_millis(1500),
    );
    let mut tunnel = open_tunnel(&env, "aug_soon").await;
    assert_eq!(env.tunnels.len(), 1);
    assert!(alive(&mut tunnel, Duration::from_millis(500)).await);
    assert!(
        !alive(&mut tunnel, Duration::from_secs(5)).await,
        "tunnel closed at the token's expiry"
    );
    let (s, text) = env.connect_req(&[], "aug_soon", "").await.unwrap();
    assert_eq!(s, 407, "{text}");
    assert!(text.contains("agent_token_expired"), "{text}");
    // A tunnel of a token without an expiry is not touched by any deadline.
    env.add("aug_forever", CredentialKind::Agent, None);
    let mut t2 = open_tunnel(&env, "aug_forever").await;
    assert!(alive(&mut t2, Duration::from_millis(1800)).await);
}

#[tokio::test]
async fn live_tunnels_close_within_five_seconds_when_the_posture_tightens() {
    let env = start(&[]).await;
    let id = env.add(
        "aug_t",
        CredentialKind::Agent,
        Some(admission(&enabled_inst(), &ByoInputs::default())),
    );
    let mut tunnel = open_tunnel(&env, "aug_t").await;
    assert_eq!(env.tunnels.len(), 1);
    // An unrelated invalidation, and one that changes nothing, keep the tunnel open.
    env.tunnels.invalidate(&Invalidate {
        scope: Scope::Agent(Uuid::now_v7()),
    });
    env.tunnels.invalidate(&Invalidate {
        scope: Scope::Agent(id.agent_id),
    });
    assert!(
        alive(&mut tunnel, Duration::from_millis(500)).await,
        "unchanged → stays open"
    );
    // Tighten to disabled at workspace level and invalidate the workspace scope.
    let disabled = admission(
        &enabled_inst(),
        &ByoInputs {
            workspace_posture: Some("disabled"),
            ..ByoInputs::default()
        },
    );
    env.creds.lock().unwrap().get_mut("aug_t").unwrap().byo = Some(disabled);
    let started = std::time::Instant::now();
    env.tunnels.invalidate(&Invalidate {
        scope: Scope::Workspace(id.workspace_id),
    });
    assert!(
        !alive(&mut tunnel, Duration::from_secs(5)).await,
        "tunnel closed"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    // New connections are refused; nothing was deleted.
    let (s, text) = env.connect_req(&[], "aug_t", "").await.unwrap();
    assert_eq!((s, body_code(&text).as_str()), (403, "byo_disabled"));
    // Loosening restores service at once with the same token.
    env.creds.lock().unwrap().get_mut("aug_t").unwrap().byo =
        Some(admission(&enabled_inst(), &ByoInputs::default()));
    let (s, _) = env.connect_req(&[], "aug_t", "").await.unwrap();
    assert_eq!(s, 200);
}

#[tokio::test]
async fn live_tunnels_close_when_the_source_list_excludes_them_or_the_token_is_revoked() {
    let env = start(&[]).await;
    let a = env.add(
        "aug_a",
        CredentialKind::Agent,
        Some(admission(&enabled_inst(), &ByoInputs::default())),
    );
    let b = env.add(
        "aug_b",
        CredentialKind::Agent,
        Some(admission(&enabled_inst(), &ByoInputs::default())),
    );
    let mut ta = open_tunnel(&env, "aug_a").await;
    let mut tb = open_tunnel(&env, "aug_b").await;
    // a: a source list that excludes the peer is set.
    env.creds.lock().unwrap().get_mut("aug_a").unwrap().byo = Some(with_list(&["203.0.113.0/24"]));
    env.tunnels.invalidate(&Invalidate {
        scope: Scope::Agent(a.agent_id),
    });
    assert!(!alive(&mut ta, Duration::from_secs(5)).await);
    // b: the credential is revoked (regenerate): the identity disappears.
    env.creds.lock().unwrap().remove("aug_b");
    env.tunnels.invalidate(&Invalidate {
        scope: Scope::Credential(b.credential_id),
    });
    assert!(!alive(&mut tb, Duration::from_secs(5)).await);
}

#[tokio::test]
async fn context_required_turning_on_closes_aug_tunnels_only() {
    let env = start(&[]).await;
    let a = env.add(
        "aug_a",
        CredentialKind::Agent,
        Some(admission(&enabled_inst(), &ByoInputs::default())),
    );
    let mut ta = open_tunnel(&env, "aug_a").await;
    let req = admission(
        &enabled_inst(),
        &ByoInputs {
            agent_require_context: true,
            ..ByoInputs::default()
        },
    );
    env.creds.lock().unwrap().get_mut("aug_a").unwrap().byo = Some(req.clone());
    env.tunnels.invalidate(&Invalidate {
        scope: Scope::Org(a.org_id),
    });
    assert!(!alive(&mut ta, Duration::from_secs(5)).await);
    // A context credential is not affected by the flag.
    let c = env.add(
        "auc_c",
        CredentialKind::Context,
        Some(admission(&enabled_inst(), &ByoInputs::default())),
    );
    let mut tc = open_tunnel(&env, "auc_c").await;
    env.creds.lock().unwrap().get_mut("auc_c").unwrap().byo = Some(req);
    env.tunnels.invalidate(&Invalidate {
        scope: Scope::Agent(c.agent_id),
    });
    assert!(alive(&mut tc, Duration::from_millis(800)).await);
}

// ---- DB-backed identity: effective values from the stored layers -----------

#[tokio::test]
async fn db_resolver_computes_effective_byo_from_all_layers_and_fails_closed() {
    use aurora_gateway::resolver::DbCredentialResolver;
    let db = aurora_testkit::TestDb::new().await;
    let pool = db.pool().clone();
    let (org, ws, agent, hosted) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'acme','O','ext')",
    )
    .bind(org)
    .execute(&pool)
    .await
    .unwrap();
    let u = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1,'o@x.io','U')")
        .bind(u)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO org_members (org_id, user_id, role, status) VALUES ($1,$2,'owner','active')",
    )
    .bind(org)
    .bind(u)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'main','W')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await
        .unwrap();
    for (id, kind, handle) in [(agent, "byo", "a"), (hosted, "hosted", "h")] {
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,$4,'A',$5)")
            .bind(id)
            .bind(org)
            .bind(ws)
            .bind(handle)
            .bind(kind)
            .execute(&pool)
            .await
            .unwrap();
    }
    for (tok, kind, a) in [
        ("aug_db", "agent", agent),
        ("auc_db", "context", agent),
        ("auc_host", "context", hosted),
    ] {
        sqlx::query("INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id) VALUES ($1,$2,$3,'x',$4)")
            .bind(Uuid::now_v7())
            .bind(kind)
            .bind(aurora_core::token::hash(tok).to_vec())
            .bind(a)
            .execute(&pool)
            .await
            .unwrap();
    }
    let inst = ByoInstance {
        minimum_posture: ByoPosture::OwnersOnly,
        default_source_cidrs: vec![Cidr::parse("10.0.0.0/8").unwrap()],
        ..ByoInstance::default()
    };
    let r = DbCredentialResolver::with_ttls(
        pool.clone(),
        Duration::from_secs(60),
        Duration::from_secs(5),
    )
    .with_byo(inst);
    let byo_of = |i: &RequestIdentity| i.byo.clone().unwrap().effective;

    // Defaults: floor owners_only (instance), instance default list.
    let e = byo_of(&r.resolve("aug_db").await.unwrap().unwrap());
    assert_eq!(
        (e.posture, e.posture_level.as_str()),
        (ByoPosture::OwnersOnly, "instance")
    );
    assert_eq!(e.source_strings(), Some(vec!["10.0.0.0/8".to_owned()]));
    // Hosted agents and their context credentials carry no BYO admission.
    assert!(r.resolve("auc_host").await.unwrap().unwrap().byo.is_none());
    // BYO context credentials do.
    assert!(r.resolve("auc_db").await.unwrap().unwrap().byo.is_some());

    // All layers: org posture/list/require, workspace posture, agent list.
    sqlx::query("UPDATE organizations SET governance = $2 WHERE id = $1")
        .bind(org)
        .bind(
            serde_json::json!({"byo": {"posture": "owners_only", "source_cidrs": ["10.1.0.0/16"],
                                         "require_context_credentials": true}}),
        )
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE workspaces SET byo_posture = 'disabled' WHERE id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE agents SET byo_source_cidrs = ARRAY['10.1.2.0/24'], byo_require_context = false WHERE id = $1")
        .bind(agent)
        .execute(&pool)
        .await
        .unwrap();
    let fresh = DbCredentialResolver::new(pool.clone()).with_byo(ByoInstance {
        default_source_cidrs: vec![Cidr::parse("10.0.0.0/8").unwrap()],
        ..ByoInstance::default()
    });
    let i = fresh.resolve("aug_db").await.unwrap().unwrap();
    let b = i.byo.clone().unwrap();
    let e = &b.effective;
    assert_eq!(
        (e.posture, e.posture_level.as_str()),
        (ByoPosture::Disabled, "workspace")
    );
    assert_eq!(e.source_strings(), Some(vec!["10.1.2.0/24".to_owned()]));
    assert!(e.require_context, "org switched it on");
    assert_eq!((b.org_slug.as_str(), b.ws_slug.as_str()), ("acme", "main"));

    // Fail closed: an unparsable stored workspace value cannot be written (CHECK), but an
    // unparsable org document value is read as disabled.
    sqlx::query("UPDATE workspaces SET byo_posture = NULL WHERE id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE organizations SET governance = $2 WHERE id = $1")
        .bind(org)
        .bind(serde_json::json!({"byo": {"posture": 12}}))
        .execute(&pool)
        .await
        .unwrap();
    let again = DbCredentialResolver::new(pool.clone()).with_byo(enabled_inst());
    let e = byo_of(&again.resolve("aug_db").await.unwrap().unwrap());
    assert_eq!(e.posture, ByoPosture::Disabled);

    // Re-resolution by credential id (live tunnels) bypasses the cache.
    let id = i.credential_id;
    assert!(again.resolve_credential(id).await.unwrap().is_some());
    sqlx::query("UPDATE gateway_credentials SET revoked_at = now() WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(again.resolve_credential(id).await.unwrap().is_none());
}
