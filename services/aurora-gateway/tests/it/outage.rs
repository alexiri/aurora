#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RFE-014 / SEC-54 (`18 §Gateway under a database outage`): the gateway against a Postgres that
//! drops, stalls or loses its LISTEN connection. The database is the shared dev server's per-test
//! database reached through a fault-injecting TCP proxy (`Fault`): `Mode::Drop` closes every
//! connection at once and refuses new ones, `Mode::Stall` accepts and never answers. The hub's
//! LISTEN connection goes through its own proxy so a LISTEN-only loss can be injected.

use async_trait::async_trait;
use aurora_config::SecretsSection;
use aurora_core::Secret;
use aurora_db::repo::gateway::{PolicyRuleRow, RequestLogRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialResolver, ResolveError};
use aurora_gateway::ca;
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::guard::Guard;
use aurora_gateway::health::{Health, State};
use aurora_gateway::hold::Holds;
use aurora_gateway::invalidation::{Hub, Invalidatable, Invalidate, Scope};
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{ByoParts, Proxy, ProxyParts};
use aurora_gateway::reqlog::{DbWriter, RequestLog};
use aurora_gateway::resolve::{DbSource, PolicySource, Resolver};
use aurora_gateway::resolver::DbCredentialResolver;
use aurora_gateway::snapshot::{SnapshotStore, parse};
use aurora_gateway::status::{Outage, readyz_router, router_with};
use aurora_secrets::{Purpose, SealContext, SecretRef, SecretVault, StoreRegistry};
use aurora_testkit::TestDb;
use base64::Engine;
use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tower::ServiceExt;
use uuid::Uuid;

/// Every outage test takes the read side; the metrics test takes the write side, because the
/// state gauge is process-wide and written by every `Health`.
static SERIAL: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

const TOKEN: &str = "aug_outage";

// ---- fault-injecting TCP proxy ----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Pass,
    /// Close every connection, existing and new.
    Drop,
    /// Accept and never answer.
    Stall,
    /// Established connections stay open but stop delivering in both directions (a dead TCP
    /// path that never closes); new connections stall.
    Blackhole,
    /// A slow reconnect after a dirty loss: data on connections made before this mode is
    /// swallowed (closes still pass), and each *new* connection is held this long before it is
    /// connected through.
    Delay(Duration),
}

/// Copies `r` to `w`; while the proxy is in `Blackhole` the bytes are read and discarded.
async fn pump(
    mut r: tokio::net::tcp::OwnedReadHalf,
    mut w: tokio::net::tcp::OwnedWriteHalf,
    st: Arc<FaultState>,
    swallow_in_delay: bool,
) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let Ok(n) = r.read(&mut buf).await else {
            return;
        };
        if n == 0 {
            let _ = w.shutdown().await;
            return;
        }
        let mode = *st.mode.lock().unwrap();
        if mode == Mode::Blackhole || (swallow_in_delay && matches!(mode, Mode::Delay(_))) {
            continue;
        }
        if w.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}

struct FaultState {
    mode: Mutex<Mode>,
    conns: Mutex<Vec<tokio::task::AbortHandle>>,
    target: SocketAddr,
}

#[derive(Clone)]
struct Fault {
    addr: SocketAddr,
    st: Arc<FaultState>,
}

impl Fault {
    async fn start(target: SocketAddr) -> Fault {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let st = Arc::new(FaultState {
            mode: Mutex::new(Mode::Pass),
            conns: Mutex::new(Vec::new()),
            target,
        });
        let st2 = st.clone();
        tokio::spawn(async move {
            loop {
                let Ok((c, _)) = l.accept().await else { return };
                let mode = *st2.mode.lock().unwrap();
                let st = st2.clone();
                let handle = match mode {
                    Mode::Drop => {
                        drop(c);
                        continue;
                    }
                    Mode::Stall => tokio::spawn(async move {
                        let _c = c;
                        std::future::pending::<()>().await;
                    }),
                    // New connections in `Blackhole` stall; established ones are pumped by
                    // `pump`, which discards the data while the mode is `Blackhole`.
                    Mode::Blackhole => tokio::spawn(async move {
                        let _c = c;
                        std::future::pending::<()>().await;
                    }),
                    Mode::Pass | Mode::Delay(_) => tokio::spawn(async move {
                        if let Mode::Delay(d) = mode {
                            tokio::time::sleep(d).await;
                        }
                        if let Ok(s) = TcpStream::connect(st.target).await {
                            let (cr, cw) = c.into_split();
                            let (sr, sw) = s.into_split();
                            // Connections made before a `Delay` swallow data while it lasts (so a
                            // terminated backend's FATAL message never reaches the client, only
                            // the close does); connections made during it are forwarded.
                            let old = !matches!(mode, Mode::Delay(_));
                            tokio::join!(
                                pump(cr, sw, st.clone(), old),
                                pump(sr, cw, st.clone(), old)
                            );
                        }
                    }),
                };
                st2.conns.lock().unwrap().push(handle.abort_handle());
            }
        });
        Fault { addr, st }
    }

    /// Switches the mode; anything but `Pass` also cuts every open connection.
    fn set(&self, m: Mode) {
        *self.st.mode.lock().unwrap() = m;
        if matches!(m, Mode::Drop | Mode::Stall) {
            for h in self.st.conns.lock().unwrap().drain(..) {
                h.abort();
            }
        }
    }

    fn pool(&self, db: &TestDb, acquire: Duration) -> PgPool {
        let opts: PgConnectOptions = db.url().parse().unwrap();
        let opts = opts.host("127.0.0.1").port(self.addr.port());
        PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(acquire)
            .connect_lazy_with(opts)
    }
}

fn db_addr(db: &TestDb) -> SocketAddr {
    let opts: PgConnectOptions = db.url().parse().unwrap();
    let host = match opts.get_host() {
        "localhost" => "127.0.0.1",
        h => h,
    };
    format!("{host}:{}", opts.get_port()).parse().unwrap()
}

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..400 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

// ---- the gateway under test ---------------------------------------------------------------------

#[derive(Clone)]
struct Opts {
    id_ttl: Duration,
    host_ttl: Duration,
    plaintext_ttl: Duration,
    hold_timeout: Duration,
    snapshot: bool,
    probe: Duration,
    grace: Duration,
    log_cap: usize,
    /// Listener heartbeat period.
    heartbeat: Duration,
    /// Probe and flush timers off: only hot-path errors move the state.
    no_probe: bool,
    /// The org's uncredentialed default is `block` (`credentialed_only`).
    credentialed_only: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            id_ttl: Duration::from_secs(3),
            host_ttl: Duration::from_secs(3),
            plaintext_ttl: Duration::from_secs(3),
            hold_timeout: Duration::from_secs(30),
            snapshot: true,
            probe: Duration::from_millis(100),
            grace: Duration::from_millis(500),
            log_cap: 10_000,
            // Long enough that a loaded test machine never misses two in a row by accident.
            heartbeat: Duration::from_secs(3),
            no_probe: false,
            credentialed_only: false,
        }
    }
}

struct World {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
}

async fn seed(pool: &PgPool) -> World {
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o','O','ext')",
    )
    .bind(w.org)
    .execute(pool)
    .await
    .unwrap();
    // A live credential needs an organization with an active owner.
    let owner = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1,'owner@x.io','Owner')")
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO org_members (org_id, user_id, role, status) VALUES ($1,$2,'owner','active')",
    )
    .bind(w.org)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'w','W')")
        .bind(w.ws)
        .bind(w.org)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'a','A','hosted')",
    )
    .bind(w.agent)
    .bind(w.org)
    .bind(w.ws)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id) \
         VALUES ($1,'agent',$2,'x',$3)",
    )
    .bind(Uuid::now_v7())
    .bind(aurora_core::token::hash(TOKEN).to_vec())
    .bind(w.agent)
    .execute(pool)
    .await
    .unwrap();
    w
}

fn snapshot_json() -> Value {
    json!({
        "format": 1, "contract": 1,
        "packages": [{"package_id": "pkg", "version": "1.0.0", "digest": "d", "publisher": "p",
                      "hooks_digest": null, "egress": {}, "context": ["org_id"]}],
        "oauth_clients": [], "apps": [], "hosts": [], "reserved_hosts": []
    })
}

/// In-memory policy rows (the policy source is not under test; the cache and the refusals are).
struct Src {
    rows: Mutex<(ScopePolicyRows, ScopePolicyRows, Vec<SecretMetaRow>)>,
    loads: AtomicUsize,
}

#[async_trait]
impl PolicySource for Src {
    async fn scope_policy(&self, _o: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        let r = self.rows.lock().unwrap();
        Ok(if ws.is_some() {
            r.1.clone()
        } else {
            r.0.clone()
        })
    }
    async fn secrets(&self, _o: Uuid, _w: Uuid) -> Result<Vec<SecretMetaRow>, String> {
        Ok(self.rows.lock().unwrap().2.clone())
    }
    async fn slugs(&self, _w: Uuid) -> Result<Option<(String, String)>, String> {
        Ok(Some(("acme".into(), "main".into())))
    }
}

fn rule(name: &str, targets: Value, require_approval: bool) -> PolicyRuleRow {
    PolicyRuleRow {
        logical_id: Uuid::now_v7(),
        name: name.into(),
        enabled: true,
        identities: json!([]),
        targets,
        action: "allow".into(),
        require_approval,
        approval_timeout_secs: None,
        rate_limit: None,
        conditions: json!([]),
    }
}

fn secret_row(w: &World, name: &str, path: &str, vref: &SecretRef) -> SecretMetaRow {
    SecretMetaRow {
        id: Uuid::now_v7(),
        workspace_id: Some(w.ws),
        name: name.into(),
        kind: "generic".into(),
        llm_provider: None,
        llm_auth_mode: None,
        llm_endpoint_id: None,
        host_pattern: "127.0.0.1".into(),
        path_pattern: Some(path.into()),
        injection: json!({"kind":"bearer"}),
        value_ref: vref.to_column(),
        value_version: 1,
        origin: None,
    }
}

struct Gw {
    db: TestDb,
    w: World,
    fault: Fault,
    lfault: Fault,
    pool: PgPool,
    hub: Arc<Hub>,
    health: Arc<Health>,
    snapshots: Arc<SnapshotStore>,
    log: Arc<RequestLog>,
    holds: Arc<Holds>,
    src: Arc<Src>,
    proxy: SocketAddr,
    up: SocketAddr,
    ring_bearer: String,
    internal: axum::Router,
    /// Dropped with the gateway: stops its tasks.
    #[allow(dead_code)]
    shutdown: tokio::sync::watch::Sender<bool>,
    resolver: Arc<DbCredentialResolver>,
}

/// `/granted/**` is granted to the agent, `/ungranted/**` has a candidate but no grant, and
/// `/approve/**` is granted behind an approval; every other path is uncredentialed (allowed).
async fn start(o: Opts) -> Gw {
    let db = TestDb::new().await;
    let w = seed(db.pool()).await;
    let target = db_addr(&db);
    let fault = Fault::start(target).await;
    let lfault = Fault::start(target).await;
    // Generous: a loaded test machine must not read a slow connect as an outage.
    let pool = fault.pool(&db, Duration::from_secs(5));
    let lpool = lfault.pool(&db, Duration::from_secs(5));

    let snapshots = SnapshotStore::new();
    if o.snapshot {
        snapshots.install(parse("h1", snapshot_json()).unwrap());
    }
    let loaded = snapshots.clone();
    let health = Health::with_timing(
        move || loaded.is_loaded(),
        o.grace,
        o.probe,
        Duration::from_millis(1500),
    );
    let hub = Hub::new();
    hub.set_health(health.clone());
    hub.set_registry_poke(snapshots.poke());
    hub.set_backoff(Duration::from_millis(50), Duration::from_millis(400));
    // The heartbeat's NOTIFY goes through the main pool, so a blackholed LISTEN path is the only
    // thing that stops it from coming back.
    hub.set_heartbeat(o.heartbeat, Some(pool.clone()));

    // Secrets: inline-sealed values (they need the platform database).
    let keyring = Arc::new(
        aurora_crypto::Keyring::parse(
            &format!(
                "k1:{}",
                base64::engine::general_purpose::STANDARD.encode([7u8; 32])
            ),
            "k1",
        )
        .unwrap(),
    );
    let vault = Arc::new(SecretVault::new(
        StoreRegistry::from_config(&SecretsSection::default(), keyring).unwrap(),
    ));
    let mut secrets = Vec::new();
    for (name, path) in [
        ("granted", "/granted/**"),
        ("ungranted", "/ungranted/**"),
        ("approve", "/approve/**"),
    ] {
        let id = Uuid::now_v7();
        let ctx = SealContext {
            purpose: Purpose::SecretValue,
            owner_id: id,
            org_id: w.org,
            workspace_id: Some(w.ws),
        };
        let vref = vault
            .seal(&ctx, Secret::new(format!("value-of-{name}").into_bytes()))
            .await
            .unwrap();
        let mut row = secret_row(&w, name, path, &vref);
        row.id = id;
        secrets.push(row);
    }
    let org = ScopePolicyRows {
        uncredentialed_default: if o.credentialed_only { "block" } else { "none" }.into(),
        ..ScopePolicyRows::default()
    };
    let ws = ScopePolicyRows {
        rules: vec![
            rule(
                "grant",
                json!([{"type":"secret","id":secrets[0].id}]),
                false,
            ),
            PolicyRuleRow {
                action: "block".into(),
                ..rule(
                    "deny",
                    json!([{"type":"network","host":"127.0.0.1","path":"/ungranted/**"}]),
                    false,
                )
            },
            rule(
                "grant-approve",
                json!([{"type":"secret","id":secrets[2].id}]),
                true,
            ),
        ],
        ..ScopePolicyRows::default()
    };
    let src = Arc::new(Src {
        rows: Mutex::new((org, ws, secrets)),
        loads: AtomicUsize::new(0),
    });

    let id_resolver = Arc::new(
        DbCredentialResolver::with_ttls(pool.clone(), o.id_ttl, Duration::from_millis(10))
            .with_health(health.clone()),
    );
    hub.register(id_resolver.cache());
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&aurora_config::LlmSection {
            enabled_providers: vec![],
            provider_preference: vec![],
            endpoints: vec![],
        })
        .unwrap(),
    );
    let resolver = Arc::new(
        Resolver::with_ttl(src.clone(), llm, o.host_ttl)
            .with_snapshots(snapshots.clone())
            .with_health(health.clone()),
    );
    hub.register(resolver.cache());
    let plaintext =
        Arc::new(PlaintextCache::with_ttl(vault, o.plaintext_ttl).with_health(health.clone()));
    hub.register(plaintext.clone());
    let log = RequestLog::with_cap(Arc::new(DbWriter(pool.clone())), o.log_cap);
    log.set_health(health.clone());
    let (shutdown, rx) = tokio::sync::watch::channel(false);
    let holds = Arc::new(
        Holds::new(
            pool.clone(),
            hub.clone(),
            Uuid::now_v7(),
            o.hold_timeout,
            Some(rx.clone()),
        )
        .with_health(health.clone())
        .with_min_timeout(Duration::from_secs(1)),
    );
    let handler = Arc::new(
        PolicyHandler::new(PolicyHandlerParts {
            resolver: resolver.clone(),
            plaintext,
            log: log.clone(),
            external_url: "https://aurora.test/".into(),
            holds: Some(holds.clone()),
            touch_pool: None,
        })
        .with_health(health.clone()),
    );

    tokio::spawn(hub.clone().run(lpool, rx.clone()));
    if !o.snapshot {
        tokio::spawn(snapshots.clone().run(pool.clone(), rx.clone()));
    }
    if !o.no_probe {
        tokio::spawn(health.clone().run_probe(pool.clone(), rx.clone()));
    }
    tokio::spawn(log.clone().run(Duration::from_millis(200), rx.clone()));
    tokio::spawn(
        holds
            .clone()
            .run_reconcile(Duration::from_millis(200), rx.clone()),
    );

    // Upstream: echoes the request; `/stream` sends five chunks 300 ms apart.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(s), service_fn(upstream))
                    .await;
            });
        }
    });

    let tunnels =
        aurora_gateway::byo::TunnelRegistry::new(id_resolver.clone(), "https://aurora.test");
    hub.register(tunnels.clone());
    let gw = ca::generate("gw").unwrap();
    let pl = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = pl.local_addr().unwrap();
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy_addr]).unwrap());
    let (gw_ca, gw_tls) = crate::common::gateway_tls(&gw, proxy_addr);
    let proxy = Proxy::with_byo(
        ProxyParts {
            ca: gw_ca,
            tls: gw_tls,
            resolver: id_resolver.clone(),
            handler,
            guard,
            header_timeout: Duration::from_secs(5),
            extra_ca_pem: None,
            egress_proxy: None,
        },
        ByoParts {
            health: Some(health.clone()),
            external_url: "https://aurora.test".into(),
            tunnels: Some(tunnels),
            ..ByoParts::default()
        },
    )
    .unwrap();
    tokio::spawn(proxy.run(pl, std::future::pending()));

    let b = base64::engine::general_purpose::STANDARD.encode([9u8; 32]);
    let ring = Arc::new(
        aurora_crypto::SecretRing::parse(
            &format!("a1:{b}"),
            Some("a1"),
            aurora_crypto::RingKind::Internal,
        )
        .unwrap(),
    );
    let outage = Outage {
        health: health.clone(),
        hub: hub.clone(),
        reqlog: Some(log.clone()),
    };
    let internal = readyz_router(outage.clone()).merge(router_with(ring.clone(), Some(outage)));
    let g = Gw {
        ring_bearer: ring.active_bearer(),
        db,
        w,
        fault,
        lfault,
        pool,
        hub,
        health,
        snapshots,
        log,
        holds,
        src,
        proxy: proxy_addr,
        up,
        internal,
        shutdown,
        resolver: id_resolver,
    };
    if o.snapshot {
        until("the LISTEN connection", || {
            g.hub.is_subscribed() && g.health.listener_connected()
        })
        .await;
    }
    g
}

/// `/stream` requests that reached the upstream (only `open_streams_survive_an_outage` makes
/// them): the gateway has admitted and forwarded the request, so the stream is in flight.
static STREAMS_STARTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

async fn upstream(
    req: Request<Incoming>,
) -> Result<
    Response<
        http_body_util::Either<
            http_body_util::Full<Bytes>,
            StreamBody<futures::stream::BoxStream<'static, Result<Frame<Bytes>, Infallible>>>,
        >,
    >,
    Infallible,
> {
    use futures::StreamExt;
    if req.uri().path() == "/stream" {
        STREAMS_STARTED.fetch_add(1, Ordering::SeqCst);
        let s = futures::stream::unfold(0u8, |i| async move {
            if i >= 5 {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            Some((Ok(Frame::data(Bytes::from(format!("chunk{i};")))), i + 1))
        })
        .boxed();
        return Ok(Response::new(http_body_util::Either::Right(
            StreamBody::new(s),
        )));
    }
    let mut out = format!("path={}\n", req.uri().path());
    for (k, v) in req.headers() {
        out.push_str(&format!("{}: {}\n", k, v.to_str().unwrap_or("?")));
    }
    Ok(Response::new(http_body_util::Either::Left(
        http_body_util::Full::new(Bytes::from(out)),
    )))
}

struct Reply {
    status: u16,
    head: String,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
    fn code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    }
    fn reason(&self) -> String {
        self.json()["error"]["details"]["reason"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    }
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_owned())
        })
    }
}

fn dechunk(b: &str) -> String {
    let mut out = String::new();
    let mut rest = b;
    while let Some((len, tail)) = rest.split_once("\r\n") {
        let Ok(n) = usize::from_str_radix(len.trim(), 16) else {
            return b.to_owned();
        };
        if n == 0 || tail.len() < n {
            break;
        }
        out.push_str(&tail[..n]);
        rest = tail[n..].trim_start_matches("\r\n");
    }
    if out.is_empty() { b.to_owned() } else { out }
}

/// Absolute-form GET through the proxy as `token` to `host` (default: the upstream).
async fn get_as(g: &Gw, token: &str, host: Option<&str>, path: &str) -> Reply {
    let mut s = crate::common::connect(g.proxy).await;
    let host = host.map_or_else(|| g.up.to_string(), str::to_owned);
    let req = format!(
        "GET http://{host}{path} HTTP/1.1\r\nHost: {host}\r\nProxy-Authorization: Basic {}\r\nConnection: close\r\n\r\n",
        base64::engine::general_purpose::STANDARD.encode(format!("agent:{token}"))
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(8), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Reply {
        status,
        head: head.to_owned(),
        body: dechunk(body),
    }
}

async fn get(g: &Gw, path: &str) -> Reply {
    get_as(g, TOKEN, None, path).await
}

async fn internal(g: &Gw, uri: &str, bearer: bool) -> (u16, Value) {
    let mut r = Request::builder().uri(uri);
    if bearer {
        r = r
            .header("authorization", format!("Bearer {}", g.ring_bearer))
            .header("x-aurora-key-id", "a1");
    }
    let res = g
        .internal
        .clone()
        .oneshot(r.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status().as_u16();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// Cuts the database and waits until the gateway knows (two failed probes).
async fn outage(g: &Gw) {
    g.fault.set(Mode::Drop);
    until("the gateway to notice the outage", || {
        g.health.db_unreachable()
    })
    .await;
}

async fn recover(g: &Gw) {
    g.fault.set(Mode::Pass);
    until("the gateway to recover", || {
        !g.health.db_unreachable() && g.health.state() == State::Ready
    })
    .await;
}

// ---- (a) states and /readyz ------------------------------------------------------------------------

#[tokio::test]
async fn state_machine_ready_degraded_ready_and_readyz_bodies() {
    let _s = SERIAL.read().await;
    let g = start(Opts::default()).await;
    // ready
    assert_eq!(
        internal(&g, "/readyz", false).await,
        (200, json!({"state":"ready"}))
    );
    // Warm a cache entry so `cache_entries` is visible.
    assert_eq!(get(&g, "/plain").await.status, 200);

    outage(&g).await;
    let (st, body) = internal(&g, "/readyz", false).await;
    assert_eq!(st, 200, "degraded stays in rotation: {body}");
    assert_eq!(body["state"], "degraded");
    assert_eq!(body["reason"], "database_unavailable");
    assert!(
        body["since"].as_str().is_some_and(|s| s.contains('T')),
        "{body}"
    );
    assert!(body["cache_entries"].as_u64().unwrap() >= 2, "{body}");
    assert!(body["cache_oldest_age_secs"].is_u64(), "{body}");
    // ?strict=true: 503 for degraded, same body; any other value is ignored.
    let (st, strict) = internal(&g, "/readyz?strict=true", false).await;
    assert_eq!(st, 503);
    assert_eq!(strict["state"], "degraded");
    assert_eq!(internal(&g, "/readyz?strict=1", false).await.0, 200);
    assert_eq!(internal(&g, "/readyz?strict=false", false).await.0, 200);

    // /internal/v1/status carries the outage fields (C-381).
    let (st, status) = internal(&g, "/internal/v1/status", true).await;
    assert_eq!(st, 200);
    assert_eq!(status["state"], "degraded");
    assert!(status["db_unreachable_since"].is_string(), "{status}");
    assert_eq!(status["listener_connected"], true);
    assert!(
        status["cache"]["entries"].as_u64().unwrap() >= 2,
        "{status}"
    );
    assert!(status["cache"]["oldest_age_secs"].is_u64(), "{status}");
    assert!(status["request_log_buffered"].is_u64(), "{status}");

    recover(&g).await;
    assert_eq!(
        internal(&g, "/readyz?strict=true", false).await,
        (200, json!({"state":"ready"}))
    );
    let (_, status) = internal(&g, "/internal/v1/status", true).await;
    assert_eq!(status["state"], "ready");
    assert!(status["db_unreachable_since"].is_null(), "{status}");

    // down (draining): 503 with the reason; new connections get gateway_restarting.
    g.health.set_draining();
    let (st, body) = internal(&g, "/readyz", false).await;
    assert_eq!(
        (st, &body["state"], &body["reason"]),
        (503, &json!("down"), &json!("draining"))
    );
    let r = get(&g, "/plain").await;
    assert_eq!((r.status, r.code().as_str()), (503, "gateway_restarting"));
}

#[tokio::test]
async fn a_stalled_database_is_an_outage_too() {
    let _s = SERIAL.read().await;
    let g = start(Opts::default()).await;
    g.fault.set(Mode::Stall);
    until("two timed-out probes", || g.health.db_unreachable()).await;
    assert_eq!(g.health.state(), State::Degraded);
    recover(&g).await;
}

#[tokio::test]
async fn a_connection_error_on_the_hot_path_degrades_without_waiting_for_probes() {
    let _s = SERIAL.read().await;
    let g = start(Opts {
        no_probe: true,
        ..Opts::default()
    })
    .await;
    assert_eq!(g.health.state(), State::Ready);
    g.fault.set(Mode::Drop);
    // An uncached credential needs a database read, which fails with a connection error.
    let r = get_as(&g, "aug_never_seen", None, "/plain").await;
    assert_eq!(r.status, 503, "{}", r.body);
    assert_eq!(
        g.health.state(),
        State::Degraded,
        "no probe has run: {}",
        r.body
    );
    assert!(g.health.db_unreachable());
    // With no probe, nothing else clears it: only a successful probe recovers the gateway.
    g.fault.set(Mode::Pass);
    assert!(g.health.db_unreachable());
    g.health.probe_ok();
    assert_eq!(g.health.state(), State::Ready);
}

#[tokio::test]
async fn a_statement_error_is_not_an_outage() {
    let _s = SERIAL.read().await;
    let g = start(Opts::default()).await;
    // A reachable but empty database: every statement fails (no such relation), none of the
    // connections does.
    let name = format!("p3014_blank_{}", Uuid::now_v7().simple());
    let admin = g.db.pool();
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(admin)
        .await
        .unwrap();
    let opts: PgConnectOptions = g.db.url().parse().unwrap();
    let blank = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts.database(&name))
        .await
        .unwrap();
    let r = DbCredentialResolver::new(blank.clone()).with_health(g.health.clone());
    let e = r.resolve(TOKEN).await.unwrap_err();
    assert!(
        matches!(e, ResolveError::Unavailable(_)),
        "a statement-level error is a store error, not an outage: {e:?}"
    );
    assert_eq!(g.health.state(), State::Ready);
    assert!(!g.health.db_unreachable());
    blank.close().await;
    sqlx::query(&format!("DROP DATABASE \"{name}\" WITH (FORCE)"))
        .execute(admin)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_policy_read_that_cannot_connect_is_an_outage_and_is_refused_as_degraded() {
    let _s = SERIAL.read().await;
    let g = start(Opts {
        no_probe: true,
        ..Opts::default()
    })
    .await;
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&aurora_config::LlmSection {
            enabled_providers: vec![],
            provider_preference: vec![],
            endpoints: vec![],
        })
        .unwrap(),
    );
    let resolver =
        Resolver::new(Arc::new(DbSource(g.pool.clone())), llm).with_health(g.health.clone());
    let id = aurora_gateway::auth::RequestIdentity {
        credential_id: Uuid::now_v7(),
        kind: aurora_gateway::auth::CredentialKind::Agent,
        agent_id: g.w.agent,
        workspace_id: g.w.ws,
        org_id: g.w.org,
        conversation_id: None,
        turn_id: None,
        context_label: None,
        speaker_user_id: None,
            speaker_group_ids: Vec::new(),
        capabilities: vec![],
        byo: None,
    };
    // Reachable: loaded from the database.
    resolver.resolve(&id, "a.example", 443).await.unwrap();
    g.fault.set(Mode::Drop);
    let e = resolver.resolve(&id, "b.example", 443).await.unwrap_err();
    assert!(e.is_db_unavailable(), "{e}");
    assert_eq!(g.health.state(), State::Degraded);
    // Cached within its TTL: still served; uncached: refused at once, without asking.
    resolver.resolve(&id, "a.example", 443).await.unwrap();
    let t = std::time::Instant::now();
    let e = resolver.resolve(&id, "c.example", 443).await.unwrap_err();
    assert!(
        e.is_db_unavailable() && t.elapsed() < Duration::from_millis(200),
        "{e}"
    );
}

#[tokio::test]
async fn started_without_a_snapshot_or_database_is_down_until_one_loads() {
    let _s = SERIAL.read().await;
    // No snapshot installed; the loader runs against a database that is down.
    let g = {
        let g = start_down().await;
        g.fault.set(Mode::Drop);
        g
    };
    let (st, body) = internal(&g, "/readyz", false).await;
    assert_eq!(
        (st, &body["state"], &body["reason"]),
        (503, &json!("down"), &json!("snapshot_not_loaded"))
    );
    // New connections are refused with the documented code, whatever the credential.
    let r = get(&g, "/plain").await;
    assert_eq!(
        (r.status, r.code().as_str(), r.reason().as_str()),
        (503, "gateway_degraded", "snapshot_not_loaded")
    );
    assert_eq!(r.header("retry-after").as_deref(), Some("5"));
    assert_eq!(r.header("x-should-retry").as_deref(), Some("true"));
    // The database comes back and an active snapshot exists: the loader picks it up.
    sqlx::query(
        "INSERT INTO registry_snapshots (id, contract, content_hash, snapshot, compiled_by, activated_at) \
         VALUES ($1, 1, 'h-outage', $2, 'test', now())",
    )
    .bind(Uuid::now_v7())
    .bind(snapshot_json())
    .execute(g.db.pool())
    .await
    .unwrap();
    g.fault.set(Mode::Pass);
    until("the snapshot to load", || g.snapshots.is_loaded()).await;
    until("ready", || g.health.state() == State::Ready).await;
    assert_eq!(get(&g, "/plain").await.status, 200);
}

async fn start_down() -> Gw {
    start(Opts {
        snapshot: false,
        ..Opts::default()
    })
    .await
}

// ---- (a) serving during an outage ------------------------------------------------------------------

/// (RFE-013 x RFE-014) Expiry refusals respect the outage rules: with the database reachable an
/// expired agent token is 407 `agent_token_expired`; during an outage a credential the gateway
/// does not hold in cache is 503 `gateway_degraded`, never a 407 (it cannot tell an expired token
/// from an unreadable store), and `last_used_at` is not written.
#[tokio::test]
async fn an_expired_agent_token_not_in_cache_is_503_during_an_outage_never_407_and_use_is_not_written()
 {
    let _s = SERIAL.read().await;
    let g = start(Opts::default()).await;
    let p = g.db.pool();
    let mut tokens = vec![];
    for n in ["x", "y"] {
        let agent = Uuid::now_v7();
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,$4,$4,'byo')")
            .bind(agent)
            .bind(g.w.org)
            .bind(g.w.ws)
            .bind(n)
            .execute(p)
            .await
            .unwrap();
        let token = format!("aug_expired_{n}");
        sqlx::query(
            "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id, expires_at) \
             VALUES ($1,'agent',$2,'x',$3, now() - interval '1 minute')",
        )
        .bind(Uuid::now_v7())
        .bind(aurora_core::token::hash(&token).to_vec())
        .bind(agent)
        .execute(p)
        .await
        .unwrap();
        tokens.push(token);
    }
    // Healthy: 407 with the expiry reason.
    let r = get_as(&g, &tokens[0], None, "/plain").await;
    assert_eq!(r.status, 407, "{}", r.body);
    assert!(r.body.contains("agent_token_expired"), "{}", r.body);

    // A good credential's use is queued, then the outage drops it unwritten.
    assert_eq!(get(&g, "/plain").await.status, 200);
    outage(&g).await;
    for t in &tokens {
        // Even the one answered before: its negative answer is short-lived and the database is
        // unreachable now.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let r = get_as(&g, t, None, "/plain").await;
        assert_eq!(r.status, 503, "{t}: {}", r.body);
        assert!(r.body.contains("gateway_degraded"), "{}", r.body);
        assert!(!r.body.contains("agent_token_expired"), "{}", r.body);
    }
    assert_eq!(
        g.resolver.flush_last_used().await,
        0,
        "skipped while degraded"
    );
    recover(&g).await;
    let used: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_used_at FROM gateway_credentials WHERE token_hash = $1")
            .bind(aurora_core::token::hash(TOKEN).to_vec())
            .fetch_one(p)
            .await
            .unwrap();
    assert!(used.is_none(), "no write during the outage");
    // Recovered: the batch writer works again.
    assert_eq!(get(&g, "/plain").await.status, 200);
    g.resolver.flush_last_used().await;
}

#[tokio::test]
async fn cached_state_is_served_until_its_ttl_then_refused_and_unknown_credentials_are_503_never_407()
 {
    let _s = SERIAL.read().await;
    let g = start(Opts {
        id_ttl: Duration::from_millis(1800),
        host_ttl: Duration::from_millis(1800),
        plaintext_ttl: Duration::from_millis(1800),
        ..Opts::default()
    })
    .await;
    // Healthy: an unknown credential is a 407.
    let r = get_as(&g, "aug_unknown", None, "/plain").await;
    assert_eq!(r.status, 407, "{}", r.body);
    let warm = get(&g, "/granted/x").await;
    assert_eq!(warm.status, 200, "{}", warm.body);
    assert!(
        warm.body.contains("authorization: Bearer value-of-granted"),
        "{}",
        warm.body
    );

    outage(&g).await;
    // Identity, ResolvedHost and plaintext are all within TTL: served.
    let r = get(&g, "/granted/x").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("authorization: Bearer value-of-granted"),
        "{}",
        r.body
    );
    // A credential not in the identity cache is refused 503, never 407.
    let r = get_as(&g, "aug_unknown", None, "/plain").await;
    assert_eq!(r.status, 503, "{}", r.body);
    assert_eq!(
        (r.code().as_str(), r.reason().as_str()),
        ("gateway_degraded", "database_unavailable")
    );
    assert_eq!(r.header("retry-after").as_deref(), Some("5"));
    assert_eq!(r.header("x-should-retry").as_deref(), Some("true"));
    // Nothing is served past its TTL, and nothing is refreshed: after the TTL the same request
    // is refused.
    tokio::time::sleep(Duration::from_millis(1900)).await;
    let r = get(&g, "/granted/x").await;
    assert_eq!(r.status, 503, "{}", r.body);
    assert_eq!(r.code(), "gateway_degraded");
    assert_ne!(r.status, 407);
    recover(&g).await;
    assert_eq!(get(&g, "/granted/x").await.status, 200);
}

#[tokio::test]
async fn an_uncached_host_and_an_uncached_plaintext_are_refused_with_the_documented_codes() {
    let _s = SERIAL.read().await;
    let g = start(Opts {
        id_ttl: Duration::from_secs(30),
        host_ttl: Duration::from_secs(30),
        plaintext_ttl: Duration::from_millis(800),
        ..Opts::default()
    })
    .await;
    assert_eq!(get(&g, "/granted/x").await.status, 200);
    let other = format!("localhost:{}", g.up.port());
    outage(&g).await;
    // A host with no ResolvedHost entry.
    let r = get_as(&g, TOKEN, Some(&other), "/plain").await;
    assert_eq!(r.status, 503, "{}", r.body);
    assert_eq!(
        (r.code().as_str(), r.reason().as_str()),
        ("gateway_degraded", "database_unavailable")
    );
    // A plaintext not in the plaintext cache (expired): credential_store_unavailable.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let r = get(&g, "/granted/x").await;
    assert_eq!(r.status, 503, "{}", r.body);
    assert_eq!(
        (r.code().as_str(), r.reason().as_str()),
        ("gateway_degraded", "credential_store_unavailable")
    );
    assert_eq!(r.header("retry-after").as_deref(), Some("5"));
    assert_eq!(r.header("x-should-retry").as_deref(), Some("true"));
    // The secret never reached the upstream.
    assert!(!r.body.contains("value-of"), "{}", r.body);
    // Cached identity + host still serve what needs no lookup.
    assert_eq!(get(&g, "/plain").await.status, 200);
}

#[tokio::test]
async fn the_cached_policy_still_blocks_what_it_blocked() {
    let _s = SERIAL.read().await;
    let g = start(Opts {
        id_ttl: Duration::from_secs(30),
        host_ttl: Duration::from_secs(30),
        ..Opts::default()
    })
    .await;
    let before = get(&g, "/ungranted/x").await;
    assert_eq!(before.status, 403, "{}", before.body);
    assert_eq!(before.code(), "blocked_by_policy");
    assert_eq!(get(&g, "/granted/x").await.status, 200);
    outage(&g).await;
    // The cached view still denies what it denied: degradation never opens anything.
    let r = get(&g, "/ungranted/x").await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "blocked_by_policy");
    assert!(!r.body.contains("value-of"), "{}", r.body);
    // And what is granted keeps being granted, nothing more.
    assert_eq!(get(&g, "/granted/x").await.status, 200);
}

#[tokio::test]
async fn credentialed_only_still_applies_to_cached_state() {
    let _s = SERIAL.read().await;
    let g = start(Opts {
        id_ttl: Duration::from_secs(30),
        host_ttl: Duration::from_secs(30),
        credentialed_only: true,
        ..Opts::default()
    })
    .await;
    let before = get(&g, "/plain").await;
    assert_eq!(
        (before.status, before.code().as_str()),
        (403, "blocked_by_egress_policy")
    );
    assert_eq!(get(&g, "/granted/x").await.status, 200);
    outage(&g).await;
    // The org's uncredentialed default `block` still applies to the cached view; an
    // uncredentialed request is not let through because the database is down.
    for path in ["/plain", "/ungranted/x"] {
        let r = get(&g, path).await;
        assert_eq!(
            (r.status, r.code().as_str()),
            (403, "blocked_by_egress_policy"),
            "{path}: {}",
            r.body
        );
    }
    assert_eq!(get(&g, "/granted/x").await.status, 200);
}

#[tokio::test]
async fn a_new_approval_hold_is_refused_and_nothing_is_persisted() {
    let _s = SERIAL.read().await;
    let g = start(Opts {
        id_ttl: Duration::from_secs(30),
        host_ttl: Duration::from_secs(30),
        ..Opts::default()
    })
    .await;
    // Warm the identity and host caches with a request that needs no approval.
    assert_eq!(get(&g, "/granted/x").await.status, 200);
    outage(&g).await;
    let r = get(&g, "/approve/x").await;
    assert_eq!(r.status, 503, "{}", r.body);
    assert_eq!(r.code(), "approval_store_unavailable");
    assert_eq!(r.header("retry-after").as_deref(), Some("5"));
    assert_eq!(r.header("x-should-retry").as_deref(), Some("true"));
    recover(&g).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals")
        .fetch_one(g.db.pool())
        .await
        .unwrap();
    assert_eq!(n, 0, "no hold was recorded");
}

#[tokio::test]
async fn open_streams_survive_an_outage() {
    let _s = SERIAL.read().await;
    let g = Arc::new(start(Opts::default()).await);
    let g2 = g.clone();
    let t = tokio::spawn(async move { get(&g2, "/stream").await });
    // Mid-stream: wait until the request is admitted and streaming (not a fixed sleep: under
    // load the identity lookup can still be in flight, and an outage then rightly refuses it),
    // then let a chunk or two go by.
    until("the stream to start", || {
        STREAMS_STARTED.load(Ordering::SeqCst) > 0
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    g.fault.set(Mode::Drop);
    let r = t.await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    for i in 0..5 {
        assert!(r.body.contains(&format!("chunk{i};")), "{}", r.body);
    }
}

#[tokio::test]
async fn an_open_lane_tunnel_survives_an_outage() {
    let _s = SERIAL.read().await;
    let g = start(Opts::default()).await;
    // A boot credential holding the `browser` capability opens a tunnel (RFE-003).
    sqlx::query(
        "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id, capabilities) \
         VALUES ($1,'boot',$2,'x',$3, ARRAY['browser'])",
    )
    .bind(Uuid::now_v7())
    .bind(aurora_core::token::hash("aub_outage").to_vec())
    .bind(g.w.agent)
    .execute(g.db.pool())
    .await
    .unwrap();
    // A TCP echo as the tunnel's far end.
    let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
    // By name, so the policy's `127.0.0.1` network rule does not apply to it.
    let echo_addr = format!("localhost:{}", echo.local_addr().unwrap().port());
    tokio::spawn(async move {
        loop {
            let (mut s, _) = echo.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 256];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let mut s = crate::common::connect(g.proxy).await;
    let req = format!(
        "CONNECT {echo_addr} HTTP/1.1\r\nHost: {echo_addr}\r\nProxy-Authorization: Basic {}\r\n\r\n",
        base64::engine::general_purpose::STANDARD.encode("open:aub_outage")
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut b).await.unwrap();
        head.push(b[0]);
    }
    if !String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200") {
        let mut rest = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(1), s.read_to_end(&mut rest)).await;
        panic!(
            "{}{}",
            String::from_utf8_lossy(&head),
            String::from_utf8_lossy(&rest)
        );
    }
    s.write_all(b"before").await.unwrap();
    let mut got = [0u8; 6];
    s.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"before");
    outage(&g).await;
    // The tunnel needs no database: bytes keep flowing.
    s.write_all(b"during").await.unwrap();
    s.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"during");
    // A new tunnel for a credential that is not cached is refused, never admitted.
    let mut s2 = crate::common::connect(g.proxy).await;
    let req = format!(
        "CONNECT {echo_addr} HTTP/1.1\r\nHost: {echo_addr}\r\nProxy-Authorization: Basic {}\r\n\r\n",
        base64::engine::general_purpose::STANDARD.encode("open:aub_unknown")
    );
    s2.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s2.read_to_end(&mut out)).await;
    assert!(
        String::from_utf8_lossy(&out).starts_with("HTTP/1.1 503"),
        "{}",
        String::from_utf8_lossy(&out)
    );
}

#[tokio::test]
async fn an_existing_hold_expires_without_the_database_and_its_row_is_fixed_on_recovery() {
    let _s = SERIAL.read().await;
    let g = Arc::new(
        start(Opts {
            id_ttl: Duration::from_secs(30),
            host_ttl: Duration::from_secs(30),
            hold_timeout: Duration::from_secs(2),
            ..Opts::default()
        })
        .await,
    );
    assert_eq!(get(&g, "/granted/x").await.status, 200);
    let g2 = g.clone();
    let t = tokio::spawn(async move { get(&g2, "/approve/x").await });
    // The row exists, pending.
    let mut id = None;
    for _ in 0..100 {
        id = sqlx::query_scalar::<_, Uuid>("SELECT id FROM approvals WHERE status = 'pending'")
            .fetch_optional(g.db.pool())
            .await
            .unwrap();
        if id.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let id = id.expect("a pending approval");
    outage(&g).await;
    // The hold's deadline passes with the database down: 403 manual_approval_expired, and the
    // failed poll was not read as a decision.
    let r = t.await.unwrap();
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "manual_approval_expired");
    let status: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
        .bind(id)
        .fetch_one(g.db.pool())
        .await
        .unwrap();
    assert_eq!(
        status, "pending",
        "the row cannot be written while the database is down"
    );
    assert_eq!(g.holds.pending_expiry(), 1);
    recover(&g).await;
    for _ in 0..100 {
        let s: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
            .bind(id)
            .fetch_one(g.db.pool())
            .await
            .unwrap();
        if s == "expired" {
            assert_eq!(g.holds.pending_expiry(), 0);
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the approval row was never expired");
}

fn log_row(w: &World, n: usize) -> RequestLogRow {
    RequestLogRow {
        id: Uuid::now_v7(),
        ts: chrono::Utc::now(),
        org_id: w.org,
        workspace_id: w.ws,
        agent_id: w.agent,
        conversation_id: None,
        turn_id: None,
        credential_kind: "agent",
        lane: "mitm",
        method: "GET".into(),
        host: "h.example".into(),
        path: format!("/r{n:04}"),
        app_id: None,
        tool_id: None,
        connection_id: None,
        secret_id: None,
        decision: "allow",
        rule_logical_id: None,
        rule_name: None,
        upstream_status: Some(200),
        status: 200,
        error_code: None,
        injected: 0,
        latency_ms: 1,
        bytes_up: 0,
        bytes_down: 0,
        context_label: None,
        speaker_user_id: None,
    }
}

#[tokio::test]
async fn the_request_log_buffers_drops_the_oldest_beyond_the_cap_and_flushes_on_recovery() {
    let _s = SERIAL.read().await;
    let g = start(Opts {
        log_cap: 20,
        ..Opts::default()
    })
    .await;
    outage(&g).await;
    for n in 0..30 {
        g.log.record(log_row(&g.w, n));
    }
    // Bounded: the 20 newest remain; recording never blocks.
    assert_eq!(g.log.buffered(), 20);
    let (_, status) = internal(&g, "/internal/v1/status", true).await;
    assert_eq!(status["request_log_buffered"], 20);
    // Nothing was written while the database was unreachable.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM request_log")
        .fetch_one(g.db.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    recover(&g).await;
    let mut paths: Vec<String> = Vec::new();
    for _ in 0..200 {
        paths = sqlx::query_scalar("SELECT path FROM request_log ORDER BY path")
            .fetch_all(g.db.pool())
            .await
            .unwrap();
        if paths.len() >= 20 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(paths.len(), 20, "flushed on recovery");
    assert_eq!(g.log.buffered(), 0);
    assert_eq!(
        paths.first().unwrap(),
        "/r0010",
        "the oldest ten were dropped"
    );
    assert_eq!(paths.last().unwrap(), "/r0029");
}

// ---- (b) LISTEN loss ---------------------------------------------------------------------------------

/// Records what the hub does to a cache, in order.
#[derive(Default)]
struct Rec {
    events: Mutex<Vec<String>>,
    hub: Mutex<Option<Arc<Hub>>>,
    notify_in_flush: Mutex<Option<PgPool>>,
}

impl Invalidatable for Rec {
    fn invalidate(&self, ev: &Invalidate) {
        self.events
            .lock()
            .unwrap()
            .push(format!("invalidate:{:?}", ev.scope));
    }
    fn flush_all(&self) {
        let subscribed = self
            .hub
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|h| h.is_subscribed());
        self.events
            .lock()
            .unwrap()
            .push(format!("flush(subscribed={subscribed})"));
        // An invalidation sent while the flush runs is already subscribed for and must arrive.
        if let Some(pool) = self.notify_in_flush.lock().unwrap().take() {
            tokio::spawn(async move {
                let payload = json!({"scope":"agent","id":Uuid::nil()}).to_string();
                sqlx::query("SELECT pg_notify('aurora_gateway_invalidate', $1)")
                    .bind(payload)
                    .execute(&pool)
                    .await
                    .unwrap();
            });
        }
    }
}

async fn notify_agent(pool: &PgPool, agent: Uuid) {
    sqlx::query("SELECT pg_notify('aurora_gateway_invalidate', $1)")
        .bind(json!({"scope":"agent","id":agent}).to_string())
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn listen_only_loss_keeps_caches_does_not_flap_within_the_grace_and_flushes_after_resubscribing()
 {
    let _s = SERIAL.read().await;
    let g = start(Opts {
        id_ttl: Duration::from_secs(30),
        host_ttl: Duration::from_secs(30),
        grace: Duration::from_millis(600),
        ..Opts::default()
    })
    .await;
    let rec = Arc::new(Rec::default());
    *rec.hub.lock().unwrap() = Some(g.hub.clone());
    g.hub.register(rec.clone());
    assert_eq!(get(&g, "/plain").await.status, 200);

    // A brief loss (the backend is terminated, the hub reconnects at once) does not flap
    // readiness.
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE datname = current_database() AND query LIKE 'LISTEN%'",
    )
    .execute(g.db.pool())
    .await
    .unwrap();
    for _ in 0..40 {
        assert_eq!(g.health.state(), State::Ready, "flapped within the grace");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    until("the hub to reconnect", || {
        g.hub.is_subscribed() && g.health.listener_connected()
    })
    .await;
    assert!(
        rec.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e == "flush(subscribed=true)"),
        "the flush ran after the subscription: {:?}",
        rec.events.lock().unwrap()
    );

    // A LISTEN loss that outlasts the grace degrades (`listener_down`) while the database is
    // fine; the caches are kept and keep serving. (The reconnect above flushed them: warm again.)
    assert_eq!(get(&g, "/plain").await.status, 200);
    rec.events.lock().unwrap().clear();
    g.lfault.set(Mode::Drop);
    until("the listener to be seen down", || !g.hub.is_connected()).await;
    until("degraded after the grace", || {
        g.health.state() == State::Degraded
    })
    .await;
    let (st, body) = internal(&g, "/readyz", false).await;
    assert_eq!((st, &body["reason"]), (200, &json!("listener_down")));
    assert!(
        body["cache_entries"].as_u64().unwrap() >= 2,
        "caches kept: {body}"
    );
    assert!(!g.health.db_unreachable());
    assert_eq!(
        get(&g, "/plain").await.status,
        200,
        "served from the kept caches"
    );
    assert!(
        !rec.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.starts_with("flush")),
        "nothing is flushed while the listener is down"
    );

    // An invalidation sent while disconnected is missed (the identity stays cached) ...
    sqlx::query("UPDATE gateway_credentials SET revoked_at = now() WHERE agent_id = $1")
        .bind(g.w.agent)
        .execute(g.db.pool())
        .await
        .unwrap();
    notify_agent(g.db.pool(), g.w.agent).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        get(&g, "/plain").await.status,
        200,
        "the revocation notice was missed"
    );

    // ... and is honoured after the flush. An invalidation sent while the flush runs, i.e.
    // after the re-subscribe, is delivered too.
    *rec.notify_in_flush.lock().unwrap() = Some(g.db.pool().clone());
    g.lfault.set(Mode::Pass);
    until("the listener to come back", || {
        g.hub.is_connected() && g.health.listener_connected()
    })
    .await;
    until("ready", || g.health.state() == State::Ready).await;
    let r = get(&g, "/plain").await;
    assert_eq!(
        r.status, 407,
        "the flush made the revocation effective: {}",
        r.body
    );
    until("the in-flush invalidation", || {
        rec.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.starts_with("invalidate"))
    })
    .await;
    let ev = rec.events.lock().unwrap().clone();
    assert_eq!(
        ev.first().map(String::as_str),
        Some("flush(subscribed=true)"),
        "{ev:?}"
    );
    assert!(
        ev.iter()
            .any(|e| e == &format!("invalidate:{:?}", Scope::Agent(Uuid::nil()))),
        "{ev:?}"
    );
    // A notice sent after the reconnect reaches the caches.
    notify_agent(g.db.pool(), g.w.agent).await;
    until("a late notice", || {
        rec.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e == &format!("invalidate:{:?}", Scope::Agent(g.w.agent)))
    })
    .await;
}

/// SEC-54 / D-108 regression: a LISTEN connection that is lost while the reconnect is slow is
/// always *seen* as lost by the hub (cancel-safe: the hub owns reconnection, sqlx's
/// `eager_reconnect` is off), however the heartbeat ticks interleave. With eager reconnect the
/// reconnect was awaited inside `try_recv`, a tick cancelled it, and the loss went unreported
/// until two heartbeats were missed (or for ever, if the silent reconnect succeeded): no
/// `listener_down`, no flush after resubscribing, no reconnect metric. The reconnect here takes
/// 2.5 s against a 1 s heartbeat, so the ticks deterministically cancel an eager reconnect; the
/// plain-loss path must report the loss well before a heartbeat-based detection (3 periods).
/// (The reconnect *metric* is asserted by `metrics_and_the_state_change_log_line`, which owns
/// the process-wide recorder.)
#[tokio::test]
async fn a_terminated_listener_is_seen_lost_even_when_the_reconnect_outlasts_the_heartbeat() {
    let _s = SERIAL.read().await;
    let g = start(Opts {
        id_ttl: Duration::from_secs(30),
        host_ttl: Duration::from_secs(30),
        heartbeat: Duration::from_secs(1),
        grace: Duration::from_millis(600),
        ..Opts::default()
    })
    .await;
    let rec = Arc::new(Rec::default());
    *rec.hub.lock().unwrap() = Some(g.hub.clone());
    g.hub.register(rec.clone());
    assert_eq!(get(&g, "/plain").await.status, 200);
    until("the listener", || {
        g.hub.is_subscribed() && g.health.listener_connected()
    })
    .await;
    rec.events.lock().unwrap().clear();

    g.lfault.set(Mode::Delay(Duration::from_millis(2500)));
    let t = std::time::Instant::now();
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE datname = current_database() AND query LIKE 'LISTEN%'",
    )
    .execute(g.db.pool())
    .await
    .unwrap();
    // The loss is reported as a loss, not found later by missed heartbeats (about 2 s).
    until("the loss to be reported", || !g.hub.is_connected()).await;
    assert!(
        t.elapsed() < Duration::from_millis(1500),
        "loss reported only after {:?}",
        t.elapsed()
    );
    assert!(!g.health.listener_connected());
    assert!(
        !g.hub.is_subscribed(),
        "not subscribed while the reconnect is pending"
    );

    // An invalidation sent in the gap is missed, and honoured by the flush after resubscribing.
    sqlx::query("UPDATE gateway_credentials SET revoked_at = now() WHERE agent_id = $1")
        .bind(g.w.agent)
        .execute(g.db.pool())
        .await
        .unwrap();
    notify_agent(g.db.pool(), g.w.agent).await;
    until("degraded after the grace", || {
        g.health.state() == State::Degraded
    })
    .await;
    let (st, body) = internal(&g, "/readyz", false).await;
    assert_eq!((st, &body["reason"]), (200, &json!("listener_down")));

    g.lfault.set(Mode::Pass);
    until("the hub to reconnect", || {
        g.hub.is_subscribed() && g.health.listener_connected()
    })
    .await;
    until("ready", || g.health.state() == State::Ready).await;
    assert!(
        rec.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e == "flush(subscribed=true)"),
        "flushed after resubscribing: {:?}",
        rec.events.lock().unwrap()
    );
    let r = get(&g, "/plain").await;
    assert_eq!(
        r.status, 407,
        "the revocation sent in the gap is effective: {}",
        r.body
    );
}

/// A silently dead LISTEN path (the TCP connection stays open but delivers nothing) is detected
/// by the heartbeat; a freeze sent during the gap is honoured by the flush after the reconnect
/// and closes the live tunnel.
#[tokio::test]
async fn a_silently_dead_listener_is_detected_and_a_freeze_closes_the_live_tunnel() {
    let _s = SERIAL.read().await;
    let g = start(Opts {
        id_ttl: Duration::from_secs(30),
        host_ttl: Duration::from_secs(30),
        heartbeat: Duration::from_millis(300),
        ..Opts::default()
    })
    .await;
    sqlx::query(
        "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id, capabilities) \
         VALUES ($1,'boot',$2,'x',$3, ARRAY['browser'])",
    )
    .bind(Uuid::now_v7())
    .bind(aurora_core::token::hash("aub_outage").to_vec())
    .bind(g.w.agent)
    .execute(g.db.pool())
    .await
    .unwrap();
    let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = format!("localhost:{}", echo.local_addr().unwrap().port());
    tokio::spawn(async move {
        loop {
            let (mut s, _) = echo.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 256];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let mut s = crate::common::connect(g.proxy).await;
    let req = format!(
        "CONNECT {echo_addr} HTTP/1.1\r\nHost: {echo_addr}\r\nProxy-Authorization: Basic {}\r\n\r\n",
        base64::engine::general_purpose::STANDARD.encode("open:aub_outage")
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut b).await.unwrap();
        head.push(b[0]);
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"));
    let mut got = [0u8; 4];
    s.write_all(b"ping").await.unwrap();
    s.read_exact(&mut got).await.unwrap();

    // The LISTEN path goes silent without closing. Heartbeats are 300 ms apart: degraded
    // (`listener_down`) within three periods, though the connection never closed and the
    // database is fine.
    let t = std::time::Instant::now();
    g.lfault.set(Mode::Blackhole);
    until("the dead listener to be detected", || {
        g.health.state() == State::Degraded
    })
    .await;
    assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
    let (st, body) = internal(&g, "/readyz", false).await;
    assert_eq!((st, &body["reason"]), (200, &json!("listener_down")));
    assert!(!g.health.db_unreachable());

    // A freeze published during the gap is missed; the tunnel is still open.
    sqlx::query("UPDATE agents SET frozen_at = now() WHERE id = $1")
        .bind(g.w.agent)
        .execute(g.db.pool())
        .await
        .unwrap();
    notify_agent(g.db.pool(), g.w.agent).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.write_all(b"pong").await.unwrap();
    s.read_exact(&mut got).await.unwrap();

    // Recovery: reconnect, flush after re-subscribing, and the freeze closes the tunnel.
    g.lfault.set(Mode::Pass);
    until("ready again", || g.health.state() == State::Ready).await;
    let mut rest = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(8), s.read_to_end(&mut rest)).await;
    assert!(
        closed.is_ok(),
        "the live tunnel of the frozen agent was closed by the flush"
    );
}

#[tokio::test]
async fn misses_after_a_flush_are_coalesced_per_key() {
    let _s = SERIAL.read().await;
    let g = Arc::new(
        start(Opts {
            id_ttl: Duration::from_secs(30),
            host_ttl: Duration::from_secs(30),
            ..Opts::default()
        })
        .await,
    );
    assert_eq!(get(&g, "/plain").await.status, 200);
    let before = g.src.loads.load(Ordering::SeqCst);
    g.hub.flush_all();
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let g = g.clone();
        tasks.push(tokio::spawn(async move { get(&g, "/plain").await.status }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), 200);
    }
    // One ResolvedHost load = two scope reads (org + workspace): eight requests, one load.
    assert_eq!(g.src.loads.load(Ordering::SeqCst) - before, 2);
    assert!(!g.resolver.cache().is_empty());
}

// ---- (c) metrics and the log line -----------------------------------------------------------------

#[derive(Default, Clone)]
struct Captured(Arc<Mutex<Vec<std::collections::HashMap<String, String>>>>);

struct Capture(Captured);

struct Fields(std::collections::HashMap<String, String>);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        self.0.insert(
            f.name().to_owned(),
            format!("{v:?}").trim_matches('"').to_owned(),
        );
    }
    fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
        self.0.insert(f.name().to_owned(), v.to_owned());
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, e: &tracing::Event<'_>) {
        let mut f = Fields(Default::default());
        e.record(&mut f);
        self.0.0.lock().unwrap().push(f.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

fn metric(text: &str, name: &str, labels: &str) -> Option<f64> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .find(|l| l.starts_with(name) && l.contains(labels))
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|v| v.parse().ok())
}

#[tokio::test]
async fn metrics_and_the_state_change_log_line() {
    // Exclusive: the state gauge is process-wide.
    let _s = SERIAL.write().await;
    // Register the `gateway.state_change` callsite before the capturing subscriber exists. A
    // callsite first reached by *another* thread (a test outside this lock that trips a state
    // change) while the capturing subscriber is the only dispatcher is registered from that
    // thread's own (absent) subscriber and stays disabled for ours; installing a dispatcher
    // rebuilds the interest of every callsite that is already registered, over *all* registered
    // dispatchers. With no dispatcher at all `tracing::warn!` short-circuits on the global max
    // level and never registers its callsite, so the warm-up needs a (throwaway) dispatcher to
    // register it for real; the capturing subscriber installed afterwards then rebuilds it.
    {
        let _warm_dispatch = tracing::subscriber::set_default(Capture(Captured::default()));
        let warm = Health::new(|| true);
        warm.report_db_error();
        assert_eq!(warm.state(), State::Degraded);
    }
    // This is the only test of the binary that installs the process-wide recorder.
    let handle = aurora_core::service::install_metrics("gateway-outage-test")
        .expect("the test process owns the metrics recorder");
    let captured = Captured::default();
    let _guard = tracing::subscriber::set_default(Capture(captured.clone()));
    let g = start(Opts {
        id_ttl: Duration::from_secs(30),
        host_ttl: Duration::from_secs(30),
        log_cap: 5,
        ..Opts::default()
    })
    .await;
    assert_eq!(get(&g, "/plain").await.status, 200);
    let text = handle.render();
    assert_eq!(
        metric(&text, "aurora_gateway_state", "state=\"ready\""),
        Some(1.0),
        "{text}"
    );
    assert_eq!(
        metric(&text, "aurora_gateway_state", "state=\"degraded\""),
        Some(0.0)
    );
    assert_eq!(
        metric(&text, "aurora_gateway_state", "state=\"down\""),
        Some(0.0)
    );

    outage(&g).await;
    assert_eq!(get(&g, "/plain").await.status, 200);
    let r = get_as(&g, "aug_unknown", None, "/plain").await;
    assert_eq!(r.status, 503);
    for n in 0..8 {
        g.log.record(log_row(&g.w, n));
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    let text = handle.render();
    assert_eq!(
        metric(&text, "aurora_gateway_state", "state=\"degraded\""),
        Some(1.0),
        "{text}"
    );
    assert_eq!(
        metric(&text, "aurora_gateway_state", "state=\"ready\""),
        Some(0.0)
    );
    assert!(
        metric(&text, "aurora_gateway_db_unreachable_seconds", "").unwrap() > 0.0,
        "{text}"
    );
    assert!(
        metric(
            &text,
            "aurora_gateway_degraded_requests_total",
            "outcome=\"served_from_cache\""
        )
        .unwrap()
            >= 1.0,
        "{text}"
    );
    assert!(
        metric(
            &text,
            "aurora_gateway_degraded_requests_total",
            "outcome=\"refused\",reason=\"database_unavailable\""
        )
        .or_else(|| metric(
            &text,
            "aurora_gateway_degraded_requests_total",
            "reason=\"database_unavailable\""
        ))
        .unwrap()
            >= 1.0,
        "{text}"
    );
    assert!(
        metric(&text, "aurora_gateway_request_log_dropped_total", "").unwrap() >= 3.0,
        "{text}"
    );
    assert!(
        text.contains("aurora_gateway_request_log_buffered"),
        "{text}"
    );

    // A LISTEN reconnect is counted, and so is its flush.
    recover(&g).await;
    let base_reconnects = metric(
        &handle.render(),
        "aurora_gateway_listener_reconnects_total",
        "",
    )
    .unwrap_or(0.0);
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE datname = current_database() AND query LIKE 'LISTEN%'",
    )
    .execute(g.db.pool())
    .await
    .unwrap();
    until("a reconnect", || {
        metric(
            &handle.render(),
            "aurora_gateway_listener_reconnects_total",
            "",
        )
        .unwrap_or(0.0)
            > base_reconnects
    })
    .await;
    let text = handle.render();
    assert!(
        metric(
            &text,
            "aurora_gateway_cache_flush_total",
            "reason=\"listener_reconnect\""
        )
        .unwrap()
            >= 1.0,
        "{text}"
    );
    notify_all(g.db.pool()).await;
    until("an invalidate_all flush", || {
        metric(
            &handle.render(),
            "aurora_gateway_cache_flush_total",
            "reason=\"invalidate_all\"",
        )
        .unwrap_or(0.0)
            >= 1.0
    })
    .await;

    // One structured line per transition: ready -> degraded -> ready.
    let lines: Vec<_> = captured
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|f| f.get("event").is_some_and(|e| e == "gateway.state_change"))
        .cloned()
        .collect();
    let pair = |from: &str, to: &str| lines.iter().any(|l| l["from"] == from && l["to"] == to);
    assert!(pair("ready", "degraded"), "{lines:?}");
    assert!(pair("degraded", "ready"), "{lines:?}");
    assert!(
        lines.iter().any(|l| l["reason"] == "database_unavailable"),
        "{lines:?}"
    );
}

async fn notify_all(pool: &PgPool) {
    sqlx::query("SELECT pg_notify('aurora_gateway_invalidate', '{\"scope\":\"all\"}')")
        .execute(pool)
        .await
        .unwrap();
}
