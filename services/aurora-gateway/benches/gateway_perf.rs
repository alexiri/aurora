//! Gateway performance budgets (docs/18 §Performance budgets), OPS-014.
//!
//! A plain `harness = false` benchmark (no extra dependency); `scripts/perf/gateway.sh` drives it.
//! Modes (first argument after `--`):
//! - `latency`            (a) added latency, cached + allowed + bodyless + injection, in process
//! - `resolve`            (c) connect-time resolution on a cache miss, 1000 rules / 500 connections
//! - `upstream`           mock upstream; prints `ADDR <ip:port>`; runs until killed
//! - `gateway <upstream>` the real proxy + policy pipeline; prints `ADDR <ip:port>`; runs until killed
//! - `load <proxy> <up>`  (b) closed-loop load generator; reports rps and latency percentiles
//!
//! Every mode prints `RESULT key=value ...` lines that the script turns into PASS/FAIL.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]

use async_trait::async_trait;
use aurora_config::SecretsSection;
use aurora_db::repo::gateway::{AppConnectionRow, PolicyRuleRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca::{self, Ca};
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::guard::Guard;
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::reqlog::{LogWriter, RequestLog};
use aurora_gateway::resolve::{PolicySource, Resolver};
use aurora_gateway::snapshot::{SnapshotStore, parse};
use aurora_secrets::{MemoryStore, SecretRef, SecretVault, StoreRegistry};
use base64::Engine;
use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use rustls::pki_types::pem::PemObject;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

const TOKEN: &str = "aug_good";

#[derive(Clone, Copy)]
struct World {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
}

struct Static(World);

#[async_trait]
impl CredentialResolver for Static {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        if token != TOKEN {
            return Ok(None);
        }
        let w = self.0;
        Ok(Some(RequestIdentity {
            credential_id: Uuid::nil(),
            kind: CredentialKind::Agent,
            agent_id: w.agent,
            workspace_id: w.ws,
            org_id: w.org,
            conversation_id: None,
            turn_id: None,
            context_label: None,
            speaker_user_id: None,
            speaker_group_ids: Vec::new(),
            capabilities: vec![],
            byo: None,
        }))
    }
}

/// In-memory policy source: no database, so measurements exclude store latency.
struct Src {
    rules: Vec<PolicyRuleRow>,
    conns: Vec<AppConnectionRow>,
}

#[async_trait]
impl PolicySource for Src {
    async fn scope_policy(&self, _o: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        let mut r = ScopePolicyRows::default();
        if ws.is_some() {
            r.rules = self.rules.clone();
        }
        Ok(r)
    }
    async fn secrets(&self, _o: Uuid, _w: Uuid) -> Result<Vec<SecretMetaRow>, String> {
        Ok(vec![])
    }
    async fn slugs(&self, _w: Uuid) -> Result<Option<(String, String)>, String> {
        Ok(Some(("acme".into(), "main".into())))
    }
    async fn connections(
        &self,
        _o: Uuid,
        _w: Uuid,
        app_ids: &[String],
    ) -> Result<Vec<AppConnectionRow>, String> {
        Ok(self
            .conns
            .iter()
            .filter(|c| app_ids.contains(&c.app_id))
            .cloned()
            .collect())
    }
    async fn app_availability(&self, _o: Uuid, _w: Uuid) -> Result<(bool, Vec<String>), String> {
        Ok((false, vec![]))
    }
}

struct Null;

#[async_trait]
impl LogWriter for Null {
    async fn write(&self, _rows: &[aurora_db::repo::gateway::RequestLogRow]) -> Result<(), String> {
        Ok(())
    }
}

fn snapshot_json(host: &str) -> Value {
    json!({
        "format": 1, "contract": 1,
        "packages": [{"package_id": "pkg", "version": "1.0.0", "digest": "d", "publisher": "p",
                      "hooks_digest": null, "egress": {}, "context": ["org_id"]}],
        "oauth_clients": [],
        "apps": [{
            "app_id": "demo", "package_id": "pkg", "about": {"id": "demo", "name": "demo"},
            "credentials": [{"name": "tok", "secret": true, "optional": false}],
            "metadata": [{"name": "sub"}], "connect": [], "token": null,
            "tools": null, "approvals": {}, "agent": null, "database": null
        }],
        "hosts": [{
            "host": host, "path_prefix": null, "app_id": "demo", "claim_status": "active",
            "tenant": null, "rewrite_host": null,
            "inject": [{"bearer": "{credentials.tok}"},
                       {"header": {"name": "X-Key", "value": "k-{credentials.tok}"}}],
            "finalize": null, "transform": null, "intercept": null, "passthrough": []
        }],
        "reserved_hosts": []
    })
}

fn conn(w: &World, n: usize) -> AppConnectionRow {
    AppConnectionRow {
        id: Uuid::now_v7(),
        workspace_id: Some(w.ws),
        app_id: "demo".into(),
        label: format!("demo-{n}"),
        status: "active".into(),
        credentials_ref: SecretRef::ext("mem", &format!("c{n}"), None, "1")
            .unwrap()
            .to_column(),
        credentials_version: 1,
        credentials_expires_at: None,
        metadata: json!({"sub": "tenant1"}),
        tenant_host: None,
        instance_id: None,
        connect_method: None,
    }
}

fn rule(c: &AppConnectionRow, n: usize) -> PolicyRuleRow {
    PolicyRuleRow {
        logical_id: Uuid::now_v7(),
        name: format!("grant-{n}"),
        enabled: true,
        identities: json!([]),
        targets: json!([{"type": "connection", "id": c.id, "tools": null}]),
        action: "allow".into(),
        require_approval: false,
        approval_timeout_secs: None,
        rate_limit: None,
        conditions: json!([]),
    }
}

fn world() -> World {
    World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    }
}

async fn upstream_svc(_req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    Ok(Response::new(Full::new(Bytes::from_static(b"ok"))))
}

async fn spawn_upstream() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let _ = s.set_nodelay(true);
            tokio::spawn(async move {
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(s), service_fn(upstream_svc))
                    .await;
            });
        }
    });
    up
}

struct Gw {
    proxy: SocketAddr,
    /// Trust anchor for the TLS-only proxy listener (docs/06 §Proxy listener TLS).
    ca_pem: String,
    resolver: Arc<Resolver>,
    world: World,
}

/// Builds the real proxy + policy pipeline in front of `up`. `n_conns`/`n_rules` seed the policy.
async fn start_gateway(up: SocketAddr, n_conns: usize, n_rules: usize) -> Gw {
    let w = world();
    let mem = Arc::new(MemoryStore::new("mem"));
    for n in 0..n_conns {
        mem.preload(&format!("c{n}"), None, br#"{"tok":"secret-token-value"}"#);
    }
    mem.allow_reference(w.org, "*");
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{key}"), "k1").unwrap());
    let mut reg = StoreRegistry::from_config(&SecretsSection::default(), keyring).unwrap();
    reg.register(mem);
    let conns: Vec<_> = (0..n_conns).map(|n| conn(&w, n)).collect();
    let rules: Vec<_> = (0..n_rules)
        .map(|n| rule(&conns[n % conns.len()], n))
        .collect();
    let src = Arc::new(Src { rules, conns });
    let store = SnapshotStore::new();
    store.install(parse("h1", snapshot_json("127.0.0.1")).unwrap());
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&aurora_config::LlmSection {
            enabled_providers: vec![],
            provider_preference: vec![],
            endpoints: vec![],
        })
        .unwrap(),
    );
    let resolver = Arc::new(Resolver::new(src, llm).with_snapshots(store));
    let plaintext = Arc::new(PlaintextCache::new(Arc::new(SecretVault::new(reg))));
    let handler = Arc::new(
        PolicyHandler::new(PolicyHandlerParts {
            resolver: resolver.clone(),
            plaintext,
            log: RequestLog::new(Arc::new(Null)),
            external_url: "https://aurora.test/".into(),
            holds: None,
            touch_pool: None,
        })
        .with_insecure_loopback_injection(true),
    );
    let gw = ca::generate("gw").unwrap();
    let pl = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = pl.local_addr().unwrap();
    let _ = up;
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy]).unwrap());
    let gw_ca = Arc::new(Ca::from_pem(&gw.cert_pem, &gw.key_pem).unwrap());
    let gw_tls =
        aurora_gateway::proxy_tls::ProxyTls::from_ca(gw_ca.clone(), vec!["127.0.0.1".into()])
            .unwrap();
    let p = Proxy::new(ProxyParts {
        ca: gw_ca,
        tls: gw_tls,
        resolver: Arc::new(Static(w)),
        handler,
        guard,
        header_timeout: Duration::from_secs(5),
        extra_ca_pem: None,
        egress_proxy: None,
    })
    .unwrap();
    tokio::spawn(p.run(pl, std::future::pending()));
    Gw {
        proxy,
        ca_pem: gw.cert_pem,
        resolver,
        world: w,
    }
}

/// One keep-alive HTTP/1 client connection; reconnects (and counts it) if the peer closed it.
struct Client {
    addr: SocketAddr,
    /// Gateway CA PEM when the peer is the (TLS-only) proxy listener.
    ca: Option<Arc<String>>,
    tx: hyper::client::conn::http1::SendRequest<Empty<Bytes>>,
    reconnects: u64,
}

async fn open(
    addr: SocketAddr,
    ca: Option<&str>,
) -> hyper::client::conn::http1::SendRequest<Empty<Bytes>> {
    let s = TcpStream::connect(addr).await.unwrap();
    s.set_nodelay(true).unwrap();
    let Some(ca) = ca else {
        let (tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(s))
            .await
            .unwrap();
        tokio::spawn(conn);
        return tx;
    };
    let mut roots = rustls::RootCertStore::empty();
    for c in rustls::pki_types::CertificateDer::pem_slice_iter(ca.as_bytes()) {
        roots.add(c.unwrap()).unwrap();
    }
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(
            rustls::pki_types::ServerName::IpAddress(addr.ip().into()),
            s,
        )
        .await
        .unwrap();
    let (tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(conn);
    tx
}

impl Client {
    async fn connect(addr: SocketAddr, ca: Option<Arc<String>>) -> Self {
        Self {
            addr,
            tx: open(addr, ca.as_deref().map(String::as_str)).await,
            ca,
            reconnects: 0,
        }
    }

    /// `auth`: Proxy-Authorization value; the request goes in absolute form.
    async fn get(&mut self, uri: &str, auth: Option<&str>) -> u16 {
        let host = uri.trim_start_matches("http://");
        let host = host.split('/').next().unwrap();
        for attempt in 0..3 {
            let mut b = Request::get(uri).header("host", host);
            if let Some(a) = auth {
                b = b.header("proxy-authorization", a);
            }
            match self.tx.send_request(b.body(Empty::new()).unwrap()).await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let _ = resp.into_body().collect().await.unwrap();
                    return status;
                }
                Err(e) if attempt < 2 => {
                    eprintln!(
                        "client: connection to {} lost ({e}); reconnecting",
                        self.addr
                    );
                    self.reconnects += 1;
                    self.tx = open(self.addr, self.ca.as_deref().map(String::as_str)).await;
                }
                Err(e) => panic!("request failed after reconnects: {e}"),
            }
        }
        unreachable!()
    }
}

fn auth_header() -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"))
    )
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[i]
}

fn sorted_us(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v
}

/// (a): the same request is timed direct to the upstream and through the gateway on a
/// keep-alive connection; added latency is the difference per percentile.
async fn mode_latency() {
    let n: usize = env_usize("PERF_LATENCY_N", 20_000);
    let up = spawn_upstream().await;
    let gw = start_gateway(up, 1, 1).await;
    let auth = auth_header();
    let uri = format!("http://{up}/v1/ping?x=1");
    let mut direct = Client::connect(up, None).await;
    let mut via = Client::connect(gw.proxy, Some(Arc::new(gw.ca_pem.clone()))).await;
    for _ in 0..500 {
        assert_eq!(direct.get(&uri, None).await, 200);
        assert_eq!(via.get(&uri, Some(&auth)).await, 200, "warmup via gateway");
    }
    let (mut d, mut v) = (Vec::with_capacity(n), Vec::with_capacity(n));
    // Interleave so both series see the same machine noise.
    for _ in 0..n {
        let t = Instant::now();
        direct.get(&uri, None).await;
        d.push(t.elapsed().as_secs_f64() * 1e6);
        let t = Instant::now();
        let s = via.get(&uri, Some(&auth)).await;
        v.push(t.elapsed().as_secs_f64() * 1e6);
        assert_eq!(s, 200);
    }
    let (d, v) = (sorted_us(d), sorted_us(v));
    let (p50, p99) = (
        (pct(&v, 0.50) - pct(&d, 0.50)) / 1000.0,
        (pct(&v, 0.99) - pct(&d, 0.99)) / 1000.0,
    );
    println!(
        "direct_us p50={:.0} p99={:.0}; via_gateway_us p50={:.0} p99={:.0}",
        pct(&d, 0.5),
        pct(&d, 0.99),
        pct(&v, 0.5),
        pct(&v, 0.99)
    );
    println!(
        "RESULT a_added_p50_ms={p50:.3} a_added_p99_ms={p99:.3} n={n} reconnects={}",
        direct.reconnects + via.reconnects
    );
}

/// (c): 1 000 rules, 500 connections in scope; every sample starts from an empty cache.
async fn mode_resolve() {
    let n: usize = env_usize("PERF_RESOLVE_N", 300);
    let up = spawn_upstream().await;
    let gw = start_gateway(up, 500, 1000).await;
    let id = RequestIdentity {
        credential_id: Uuid::nil(),
        kind: CredentialKind::Agent,
        agent_id: gw.world.agent,
        workspace_id: gw.world.ws,
        org_id: gw.world.org,
        conversation_id: None,
        turn_id: None,
        context_label: None,
        speaker_user_id: None,
        speaker_group_ids: Vec::new(),
        capabilities: vec![],
        byo: None,
    };
    let cache = gw.resolver.cache();
    let mut v = Vec::with_capacity(n);
    for i in 0..n + 20 {
        aurora_gateway::invalidation::Invalidatable::flush_all(&*cache);
        let t = Instant::now();
        let r = gw.resolver.resolve(&id, "127.0.0.1", 80).await.unwrap();
        let e = t.elapsed().as_secs_f64() * 1e3;
        assert_eq!(r.conns.len(), 500);
        assert_eq!(r.ws_policy.ordered_rules.len(), 1000);
        if i >= 20 {
            v.push(e);
        }
    }
    let v = sorted_us(v);
    println!(
        "resolve_ms min={:.2} p50={:.2} p99={:.2} max={:.2}",
        v[0],
        pct(&v, 0.5),
        pct(&v, 0.99),
        v[v.len() - 1]
    );
    println!(
        "RESULT c_p50_ms={:.3} c_p99_ms={:.3} n={n}",
        pct(&v, 0.5),
        pct(&v, 0.99)
    );
}

async fn mode_load(proxy: SocketAddr, up: SocketAddr, ca: Arc<String>) {
    let conns = env_usize("PERF_LOAD_CONNS", 32);
    let secs = env_usize("PERF_LOAD_SECS", 10) as u64;
    let auth = Arc::new(auth_header());
    let uri = Arc::new(format!("http://{up}/v1/ping?x=1"));
    // Warm the resolve + plaintext caches.
    Client::connect(proxy, Some(ca.clone()))
        .await
        .get(&uri, Some(&auth))
        .await;
    let end = Instant::now() + Duration::from_secs(secs);
    let mut tasks = Vec::new();
    for _ in 0..conns {
        let (auth, uri, ca) = (auth.clone(), uri.clone(), ca.clone());
        tasks.push(tokio::spawn(async move {
            let mut c = Client::connect(proxy, Some(ca)).await;
            let (mut lat, mut bad) = (Vec::new(), 0u64);
            while Instant::now() < end {
                let t = Instant::now();
                let s = c.get(&uri, Some(&auth)).await;
                lat.push(t.elapsed().as_secs_f64() * 1e6);
                if s != 200 {
                    bad += 1;
                }
            }
            (lat, bad, c.reconnects)
        }));
    }
    let (mut all, mut bad, mut reconnects) = (Vec::new(), 0, 0);
    for t in tasks {
        let (l, b, r) = t.await.unwrap();
        all.extend(l);
        bad += b;
        reconnects += r;
    }
    let total = all.len();
    let all = sorted_us(all);
    let rps = total as f64 / secs as f64;
    println!(
        "load conns={conns} secs={secs} requests={total} non200={bad} latency_us p50={:.0} p99={:.0} p999={:.0}",
        pct(&all, 0.5),
        pct(&all, 0.99),
        pct(&all, 0.999)
    );
    println!("RESULT b_rps={rps:.0} b_non200={bad} reconnects={reconnects}");
}

fn env_usize(k: &str, d: usize) -> usize {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--bench"))
        .collect();
    let addr = |i: usize| -> SocketAddr { args[i].parse().expect("socket address") };
    match args.first().map(String::as_str) {
        Some("latency") => mode_latency().await,
        Some("resolve") => mode_resolve().await,
        Some("upstream") => {
            let up = spawn_upstream().await;
            println!("ADDR {up}");
            std::future::pending::<()>().await;
        }
        Some("gateway") => {
            let gw = start_gateway(addr(1), 1, 1).await;
            // The listener is TLS-only: hand the load generator its trust anchor.
            let ca_path =
                std::env::temp_dir().join(format!("gateway-perf-ca-{}.pem", std::process::id()));
            std::fs::write(&ca_path, &gw.ca_pem).unwrap();
            println!("CA {}", ca_path.display());
            println!("ADDR {}", gw.proxy);
            std::future::pending::<()>().await;
        }
        Some("load") => {
            let ca = std::fs::read_to_string(&args[3]).expect("CA PEM file");
            mode_load(addr(1), addr(2), Arc::new(ca)).await;
        }
        _ => {
            eprintln!(
                "usage: gateway_perf latency|resolve|upstream|gateway <up>|load <proxy> <up> <ca.pem>"
            );
            std::process::exit(2);
        }
    }
}
