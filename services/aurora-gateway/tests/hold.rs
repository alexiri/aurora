#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! GW-004 approval holds against a real database; the policy source is in memory.

use async_trait::async_trait;
use aurora_config::{LlmSection, SecretsSection};
use aurora_core::Secret;
use aurora_db::repo::gateway::{PolicyRuleRow, RequestLogRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca::{self, Ca};
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::guard::Guard;
use aurora_gateway::hold::Holds;
use aurora_gateway::invalidation::Hub;
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::reqlog::{LogWriter, RequestLog};
use aurora_gateway::resolve::{PolicySource, Resolver};
use aurora_secrets::{
    MemoryStore, Purpose, SealContext, SecretRef, SecretStore, SecretVault, StoreCapabilities,
    StoreHealth, StoreRegistry,
};
use aurora_testkit::TestDb;
use base64::Engine;
use bytes::Bytes;
use http::{Request, Response};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

const TOKEN: &str = "aug_good";

struct World {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
}

struct Static(Arc<World>);

#[async_trait]
impl CredentialResolver for Static {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        if token != TOKEN {
            return Ok(None);
        }
        let w = &self.0;
        Ok(Some(RequestIdentity {
            credential_id: Uuid::nil(),
            kind: CredentialKind::Agent,
            agent_id: w.agent,
            workspace_id: w.ws,
            org_id: w.org,
            conversation_id: None,
            turn_id: None,
            context_label: None,
            principals: vec![w.agent],
        }))
    }
}

#[derive(Default)]
struct Rows {
    org: ScopePolicyRows,
    ws: ScopePolicyRows,
    secrets: Vec<SecretMetaRow>,
}

struct Src(Mutex<Rows>);

#[async_trait]
impl PolicySource for Src {
    async fn scope_policy(&self, _o: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        let r = self.0.lock().unwrap();
        Ok(if ws.is_some() {
            r.ws.clone()
        } else {
            r.org.clone()
        })
    }
    async fn secrets(&self, _o: Uuid, _w: Uuid) -> Result<Vec<SecretMetaRow>, String> {
        Ok(self.0.lock().unwrap().secrets.clone())
    }
    async fn slugs(&self, _w: Uuid) -> Result<Option<(String, String)>, String> {
        Ok(Some(("acme".into(), "main".into())))
    }
}

/// Counts reads, delegates to a memory store.
struct Counting {
    inner: Arc<MemoryStore>,
    gets: Arc<AtomicUsize>,
}

#[async_trait]
impl SecretStore for Counting {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn put(&self, c: &SealContext, p: &Secret<Vec<u8>>) -> aurora_secrets::Result<SecretRef> {
        self.inner.put(c, p).await
    }
    async fn get(&self, c: &SealContext, r: &SecretRef) -> aurora_secrets::Result<Secret<Vec<u8>>> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.inner.get(c, r).await
    }
    async fn delete(&self, c: &SealContext, r: &SecretRef) -> aurora_secrets::Result<()> {
        self.inner.delete(c, r).await
    }
    async fn resolve_reference(
        &self,
        c: &SealContext,
        p: &str,
        f: Option<&str>,
    ) -> aurora_secrets::Result<SecretRef> {
        self.inner.resolve_reference(c, p, f).await
    }
    async fn health(&self) -> StoreHealth {
        self.inner.health().await
    }
}

#[derive(Default)]
struct Capture(Mutex<Vec<RequestLogRow>>);

#[async_trait]
impl LogWriter for Capture {
    async fn write(&self, rows: &[RequestLogRow]) -> Result<(), String> {
        self.0.lock().unwrap().extend_from_slice(rows);
        Ok(())
    }
}

#[allow(dead_code)]
struct Env {
    w: Arc<World>,
    proxy: SocketAddr,
    up: SocketAddr,
    src: Arc<Src>,
    resolver: Arc<Resolver>,
    log: Arc<RequestLog>,
    captured: Arc<Capture>,
    gets: Arc<AtomicUsize>,
    pool: sqlx::PgPool,
    shutdown: tokio::sync::watch::Sender<bool>,
}

async fn upstream(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let mut out = format!(
        "path={}\nquery={}\n",
        req.uri().path(),
        req.uri().query().unwrap_or("")
    );
    for (k, v) in req.headers() {
        out.push_str(&format!("{}: {}\n", k, v.to_str().unwrap_or("?")));
    }
    let mut r = Response::new(Full::new(Bytes::from(out)));
    if req.uri().path() == "/401" {
        *r.status_mut() = http::StatusCode::UNAUTHORIZED;
    }
    Ok(r)
}

fn key_b64() -> String {
    base64::engine::general_purpose::STANDARD.encode([7u8; 32])
}

fn rule(name: &str, action: &str, targets: Value, extra: Value) -> PolicyRuleRow {
    let mut r = PolicyRuleRow {
        logical_id: Uuid::now_v7(),
        name: name.into(),
        enabled: true,
        identities: json!([]),
        targets,
        action: action.into(),
        require_approval: false,
        rate_limit: None,
        conditions: json!([]),
    };
    if let Some(v) = extra.get("rate_limit") {
        r.rate_limit = Some(v.clone());
    }
    if let Some(v) = extra.get("conditions") {
        r.conditions = v.clone();
    }
    r
}

fn secret(w: &World, name: &str, injection: Value, vref: &SecretRef) -> SecretMetaRow {
    SecretMetaRow {
        id: Uuid::now_v7(),
        workspace_id: Some(w.ws),
        name: name.into(),
        kind: "generic".into(),
        llm_provider: None,
        llm_auth_mode: None,
        llm_endpoint_id: None,
        host_pattern: "127.0.0.1".into(),
        path_pattern: None,
        injection,
        value_ref: vref.to_column(),
        value_version: 1,
        origin: None,
    }
}

fn world() -> Arc<World> {
    Arc::new(World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    })
}

/// `mk` builds the rows from the world and a secret ref pointing at a stored value.
async fn start(
    mk: impl FnOnce(&World, &SecretRef) -> Rows,
    llm: LlmSection,
    pool: sqlx::PgPool,
    timeout: Duration,
) -> Env {
    let w = world();
    let mem = Arc::new(MemoryStore::new("mem"));
    mem.preload("k1", None, b"s3cr3t");
    mem.allow_reference(w.org, "*");
    let gets = Arc::new(AtomicUsize::new(0));
    let keyring =
        Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{}", key_b64()), "k1").unwrap());
    let mut reg = StoreRegistry::from_config(&SecretsSection::default(), keyring).unwrap();
    reg.register(Arc::new(Counting {
        inner: mem,
        gets: gets.clone(),
    }));
    let _ = (Purpose::SecretValue,);
    let vref = SecretRef::ext("mem", "k1", None, "1").unwrap();
    let src = Arc::new(Src(Mutex::new(mk(&w, &vref))));
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
    let llm = Arc::new(aurora_llm::ProviderRegistry::from_config(&llm).unwrap());
    let resolver = Arc::new(Resolver::new(src.clone(), llm));
    let plaintext = Arc::new(PlaintextCache::new(Arc::new(SecretVault::new(reg))));
    let captured = Arc::new(Capture::default());
    let log = RequestLog::new(captured.clone());
    let hub = Hub::new();
    let (shutdown, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(hub.clone().run(pool.clone(), rx.clone()));
    while !hub.is_connected() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let holds = Arc::new(Holds::new(
        pool.clone(),
        hub,
        Uuid::now_v7(),
        timeout,
        Some(rx),
    ));
    let handler = Arc::new(PolicyHandler::new(PolicyHandlerParts {
        resolver: resolver.clone(),
        plaintext,
        log: log.clone(),
        external_url: "https://aurora.test/".into(),
        holds: Some(holds),
        touch_pool: None,
    }));
    let gw = ca::generate("gw").unwrap();
    let pl = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = pl.local_addr().unwrap();
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy]).unwrap());
    let p = Proxy::new(ProxyParts {
        ca: Arc::new(Ca::from_pem(&gw.cert_pem, &gw.key_pem).unwrap()),
        resolver: Arc::new(Static(w.clone())),
        handler,
        guard,
        header_timeout: Duration::from_secs(5),
        extra_ca_pem: None,
        egress_proxy: None,
    })
    .unwrap();
    tokio::spawn(p.run(pl, std::future::pending()));
    Env {
        w,
        proxy,
        up,
        src,
        resolver,
        log,
        captured,
        gets,
        pool,
        shutdown,
    }
}

fn no_llm() -> LlmSection {
    LlmSection {
        enabled_providers: vec![],
        provider_preference: vec![],
        endpoints: vec![],
    }
}

fn auth() -> String {
    format!(
        "Proxy-Authorization: Basic {}\r\n",
        base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"))
    )
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
}

/// Absolute-form GET through the proxy.
async fn get(env: &Env, path_and_query: &str, extra: &str) -> Reply {
    let mut s = TcpStream::connect(env.proxy).await.unwrap();
    let req = format!(
        "GET http://{}{path_and_query} HTTP/1.1\r\nHost: {}\r\n{}{extra}Connection: close\r\n\r\n",
        env.up,
        env.up,
        auth()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // Chunked bodies: keep only JSON/lines (tests search substrings).
    Reply {
        status,
        head: head.to_owned(),
        body: dechunk(body),
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

fn approval_env(w: &World, v: &SecretRef) -> Rows {
    let s = secret(w, "s", json!({"kind":"bearer"}), v);
    let mut rows = Rows::default();
    let mut r = allow_secret(&s);
    r.require_approval = true;
    rows.ws.rules.push(r);
    rows.secrets.push(s);
    rows
}

fn allow_secret(r: &SecretMetaRow) -> PolicyRuleRow {
    rule(
        "grant",
        "allow",
        json!([{"type":"secret","id":r.id}]),
        json!({}),
    )
}

/// Plays "the API": first decision wins, optionally with a NOTIFY.
async fn decide(pool: &sqlx::PgPool, id: Uuid, status: &str, ws: Uuid, notify: bool) {
    sqlx::query(
        "UPDATE approvals SET status=$2, decided_at=now() WHERE id=$1 AND status='pending'",
    )
    .bind(id)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
    if notify {
        sqlx::query("SELECT pg_notify('aurora_approval', $1)")
            .bind(json!({"id":id,"workspace_id":ws,"event":"decided"}).to_string())
            .execute(pool)
            .await
            .unwrap();
    }
}

async fn pending_for(pool: &sqlx::PgPool, ws: Uuid) -> (Uuid, Value, String) {
    for _ in 0..100 {
        if let Some(r) = sqlx::query_as::<_, (Uuid, Value, String)>(
            "SELECT id, summary, status FROM approvals WHERE workspace_id=$1",
        )
        .bind(ws)
        .fetch_optional(pool)
        .await
        .unwrap()
        {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no approval row");
}

async fn status_of(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM approvals WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn wait_status(pool: &sqlx::PgPool, id: Uuid, want: &str) {
    for _ in 0..100 {
        if status_of(pool, id).await == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("status never became {want}");
}

#[tokio::test]
async fn approve_injects_and_deny_refuses() {
    let db = TestDb::new().await;
    let env = Arc::new(
        start(
            approval_env,
            no_llm(),
            db.pool().clone(),
            Duration::from_secs(30),
        )
        .await,
    );
    let e2 = env.clone();
    let t = tokio::spawn(async move { get(&e2, "/x?token=abc", "").await });
    let (id, summary, status) = pending_for(&env.pool, env.w.ws).await;
    assert_eq!(status, "pending");
    assert_eq!(summary["source"], "generic");
    assert!(!summary.to_string().contains("abc"), "{summary}");
    // Policy before decryption: nothing read from the secret store while pending.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(env.gets.load(Ordering::SeqCst), 0);
    assert!(!t.is_finished());
    decide(&env.pool, id, "approved", env.w.ws, true).await;
    let r = t.await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("authorization: Bearer s3cr3t"),
        "{}",
        r.body
    );
    assert_eq!(env.gets.load(Ordering::SeqCst), 1);

    let env = Arc::new(
        start(
            approval_env,
            no_llm(),
            db.pool().clone(),
            Duration::from_secs(30),
        )
        .await,
    );
    let e2 = env.clone();
    let t = tokio::spawn(async move { get(&e2, "/x", "").await });
    let (id, _, _) = pending_for(&env.pool, env.w.ws).await;
    decide(&env.pool, id, "denied", env.w.ws, true).await;
    let r = t.await.unwrap();
    assert_eq!(r.status, 403);
    assert_eq!(r.json()["error"]["code"], "manual_approval_denied");
    assert_eq!(r.json()["error"]["details"]["approval_id"], id.to_string());
    assert_eq!(env.gets.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn deadline_expires() {
    let db = TestDb::new().await;
    let env = start(
        approval_env,
        no_llm(),
        db.pool().clone(),
        Duration::from_millis(600),
    )
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 403);
    assert_eq!(r.json()["error"]["code"], "manual_approval_expired");
    let (id, _, _) = pending_for(&env.pool, env.w.ws).await;
    assert_eq!(status_of(&env.pool, id).await, "expired");
}

#[tokio::test]
async fn client_disconnect_abandons() {
    let db = TestDb::new().await;
    let env = start(
        approval_env,
        no_llm(),
        db.pool().clone(),
        Duration::from_secs(30),
    )
    .await;
    let mut s = TcpStream::connect(env.proxy).await.unwrap();
    let req = format!(
        "GET http://{}/x HTTP/1.1\r\nHost: {}\r\n{}\r\n",
        env.up,
        env.up,
        auth()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let (id, _, _) = pending_for(&env.pool, env.w.ws).await;
    drop(s);
    wait_status(&env.pool, id, "abandoned").await;
}

#[tokio::test]
async fn decision_without_notify_is_polled() {
    let db = TestDb::new().await;
    let env = Arc::new(
        start(
            approval_env,
            no_llm(),
            db.pool().clone(),
            Duration::from_secs(30),
        )
        .await,
    );
    let e2 = env.clone();
    let t = tokio::spawn(async move { get(&e2, "/x", "").await });
    let (id, _, _) = pending_for(&env.pool, env.w.ws).await;
    decide(&env.pool, id, "approved", env.w.ws, false).await;
    let r = tokio::time::timeout(Duration::from_secs(5), t)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
}

#[tokio::test]
async fn two_instances_wake_only_their_own() {
    let db = TestDb::new().await;
    let a = Arc::new(
        start(
            approval_env,
            no_llm(),
            db.pool().clone(),
            Duration::from_secs(30),
        )
        .await,
    );
    let b = Arc::new(
        start(
            approval_env,
            no_llm(),
            db.pool().clone(),
            Duration::from_secs(30),
        )
        .await,
    );
    let (a2, b2) = (a.clone(), b.clone());
    let ta = tokio::spawn(async move { get(&a2, "/x", "").await });
    let tb = tokio::spawn(async move { get(&b2, "/x", "").await });
    let (ida, _, _) = pending_for(&a.pool, a.w.ws).await;
    let (idb, _, _) = pending_for(&b.pool, b.w.ws).await;
    decide(&a.pool, ida, "approved", a.w.ws, true).await;
    let ra = ta.await.unwrap();
    assert_eq!(ra.status, 200, "{}", ra.body);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!tb.is_finished());
    assert_eq!(status_of(&b.pool, idb).await, "pending");
    decide(&b.pool, idb, "denied", b.w.ws, true).await;
    assert_eq!(tb.await.unwrap().status, 403);
}

#[tokio::test]
async fn shutdown_answers_503_and_abandons() {
    let db = TestDb::new().await;
    let env = Arc::new(
        start(
            approval_env,
            no_llm(),
            db.pool().clone(),
            Duration::from_secs(30),
        )
        .await,
    );
    let e2 = env.clone();
    let t = tokio::spawn(async move { get(&e2, "/x", "").await });
    let (id, _, _) = pending_for(&env.pool, env.w.ws).await;
    env.shutdown.send(true).unwrap();
    let r = t.await.unwrap();
    assert_eq!(r.status, 503);
    assert_eq!(r.json()["error"]["code"], "gateway_restarting");
    assert!(
        r.head.to_ascii_lowercase().contains("retry-after: 1"),
        "{}",
        r.head
    );
    assert_eq!(status_of(&env.pool, id).await, "abandoned");
}

/// SEC-03: an approval that expires unanswered never opens the secret.
#[tokio::test]
async fn sec03_expired_hold_never_opens_the_secret() {
    let db = TestDb::new().await;
    let env = start(
        approval_env,
        no_llm(),
        db.pool().clone(),
        Duration::from_millis(600),
    )
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(r.json()["error"]["code"], "manual_approval_expired");
    assert_eq!(env.gets.load(Ordering::SeqCst), 0);
}
