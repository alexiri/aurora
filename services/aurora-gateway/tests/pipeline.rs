#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! GW-003 pipeline tests: policy view, selection, decision, injection, guidance, log.
//! The policy source is in memory; DB-backed loading is covered in `policy_db.rs`.

use async_trait::async_trait;
use aurora_config::{LlmEndpoint, LlmSection, SecretsSection};
use aurora_core::Secret;
use aurora_db::repo::gateway::{PolicyRuleRow, RequestLogRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca::{self, Ca};
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::guard::Guard;
use aurora_gateway::invalidation::{Invalidatable, Invalidate, Scope};
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::reqlog::{LogWriter, RequestLog};
use aurora_gateway::resolve::{PolicySource, Resolver};
use aurora_secrets::{
    MemoryStore, Purpose, SealContext, SecretRef, SecretStore, SecretVault, StoreCapabilities,
    StoreHealth, StoreRegistry,
};
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

struct Env {
    w: Arc<World>,
    proxy: SocketAddr,
    up: SocketAddr,
    src: Arc<Src>,
    resolver: Arc<Resolver>,
    log: Arc<RequestLog>,
    captured: Arc<Capture>,
    gets: Arc<AtomicUsize>,
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
async fn start(mk: impl FnOnce(&World, &SecretRef) -> Rows, llm: LlmSection) -> Env {
    start_with(mk, llm, None).await
}

async fn start_with(
    mk: impl FnOnce(&World, &SecretRef) -> Rows,
    llm: LlmSection,
    body_limit: Option<usize>,
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
    let mut handler = PolicyHandler::new(PolicyHandlerParts {
        resolver: resolver.clone(),
        plaintext,
        log: log.clone(),
        external_url: "https://aurora.test/".into(),
        holds: None,
        touch_pool: None,
    });
    if let Some(n) = body_limit {
        handler = handler.with_condition_body_limit(n);
    }
    let handler = Arc::new(handler);
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

fn allow_secret(r: &SecretMetaRow) -> PolicyRuleRow {
    rule(
        "grant",
        "allow",
        json!([{"type":"secret","id":r.id}]),
        json!({}),
    )
}

fn grant_env(recipe: Value) -> impl FnOnce(&World, &SecretRef) -> Rows {
    move |w, v| {
        let s = secret(w, "s", recipe, v);
        let mut rows = Rows::default();
        rows.ws.rules.push(allow_secret(&s));
        rows.secrets.push(s);
        rows
    }
}

#[tokio::test]
async fn injects_each_recipe_and_overwrites_agent_values() {
    let cases = [
        (json!({"kind":"bearer"}), "authorization: Bearer s3cr3t"),
        (
            json!({"kind":"header","name":"X-Api-Key","format":"k-{value}"}),
            "x-api-key: k-s3cr3t",
        ),
        (
            json!({"kind":"basic","username":"u"}),
            &format!(
                "authorization: Basic {}",
                base64::engine::general_purpose::STANDARD.encode("u:s3cr3t")
            ),
        ),
    ];
    for (recipe, expect) in cases {
        let env = start(grant_env(recipe), no_llm()).await;
        let r = get(
            &env,
            "/x",
            "Authorization: Bearer agent-supplied\r\nX-Api-Key: agent\r\n",
        )
        .await;
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(r.body.contains(expect), "{expect} not in {}", r.body);
        assert!(!r.body.contains("agent-supplied") || expect.contains("x-api-key"));
        assert!(r.body.matches("authorization:").count() <= 1);
        if expect.starts_with("x-api-key") {
            assert_eq!(r.body.matches("x-api-key:").count(), 1, "{}", r.body);
        }
    }
    let env = start(
        grant_env(json!({"kind":"query","name":"api_key","format":"{value}"})),
        no_llm(),
    )
    .await;
    let r = get(&env, "/x?api_key=agent&z=1", "").await;
    assert!(r.body.contains("query=z=1&api_key=s3cr3t"), "{}", r.body);
    let env = start(
        grant_env(json!({"kind":"path_template","prefix":"/bot"})),
        no_llm(),
    )
    .await;
    let r = get(&env, "/botPLACEHOLDER/send", "").await;
    assert!(r.body.contains("path=/bots3cr3t/send"), "{}", r.body);
}

#[tokio::test]
async fn ungranted_not_injected_and_guidance() {
    let env = start(
        |w, v| {
            let mut rows = Rows::default();
            rows.secrets
                .push(secret(w, "s", json!({"kind":"bearer"}), v));
            rows
        },
        no_llm(),
    )
    .await;
    let r = get(&env, "/401", "").await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json()["error"]["code"], "access_restricted", "{}", r.body);
    assert_eq!(env.gets.load(Ordering::SeqCst), 0);
    // Unknown host: nothing stored at all.
    let env = start(|_, _| Rows::default(), no_llm()).await;
    let r = get(&env, "/401", "").await;
    assert_eq!(r.status, 401);
    assert_eq!(
        r.json()["error"]["code"],
        "credential_not_found",
        "{}",
        r.body
    );
}

#[tokio::test]
async fn blocks_and_never_decrypts() {
    // block rule
    let env = start(
        |w, v| {
            let s = secret(w, "s", json!({"kind":"bearer"}), v);
            let mut rows = Rows::default();
            rows.ws
                .rules
                .push(rule("no-way", "block", json!([]), json!({})));
            rows.ws.rules.push(allow_secret(&s));
            rows.secrets.push(s);
            rows
        },
        no_llm(),
    )
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 403);
    let j = r.json();
    assert_eq!(j["error"]["code"], "blocked_by_policy", "{}", r.body);
    assert_eq!(j["error"]["details"]["rule_name"], "no-way");
    assert_eq!(
        env.gets.load(Ordering::SeqCst),
        0,
        "policy before decryption"
    );
    // default block with an injected credential (allow rule does not match: condition fails)
    let env = start(
        |w, v| {
            let s = secret(w, "s", json!({"kind":"bearer"}), v);
            let mut rows = Rows::default();
            rows.ws.default_action = "block".into();
            rows.ws.rules.push(rule(
                "grant",
                "allow",
                json!([{"type":"secret","id":s.id}]),
                json!({"conditions":[{"on":"body","op":"contains","value":"zzz"}]}),
            ));
            rows.secrets.push(s);
            rows
        },
        no_llm(),
    )
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(
        r.json()["error"]["code"],
        "blocked_by_default_policy",
        "{}",
        r.body
    );
    assert_eq!(env.gets.load(Ordering::SeqCst), 0);
    // egress posture
    let env = start(
        |_, _| {
            let mut rows = Rows::default();
            rows.ws.uncredentialed_default = "block".into();
            rows
        },
        no_llm(),
    )
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(
        r.json()["error"]["code"],
        "blocked_by_egress_policy",
        "{}",
        r.body
    );
    assert!(
        r.json()["error"]["details"]["manage_url"]
            .as_str()
            .unwrap()
            .starts_with("https://aurora.test/o/acme/")
    );
}

#[tokio::test]
async fn rate_limit_two_per_minute() {
    let env = start(
        |_, _| {
            let mut rows = Rows::default();
            rows.ws.rules.push(rule(
                "slow",
                "allow",
                json!([{"type":"network","host":"127.0.0.1"}]),
                json!({"rate_limit":{"count":2,"per":"minute"}}),
            ));
            rows
        },
        no_llm(),
    )
    .await;
    assert_eq!(get(&env, "/x", "").await.status, 200);
    assert_eq!(get(&env, "/x", "").await.status, 200);
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 429, "{}", r.body);
    assert!(r.head.to_lowercase().contains("retry-after:"), "{}", r.head);
    assert_eq!(r.json()["error"]["code"], "rate_limited");
}

#[tokio::test]
async fn multiple_providers_and_choice() {
    let ids = Arc::new(Mutex::new(Vec::new()));
    let ids2 = ids.clone();
    let env = start(
        move |w, v| {
            let a = secret(w, "a", json!({"kind":"bearer"}), v);
            let b = secret(
                w,
                "b",
                json!({"kind":"header","name":"x-b","format":"{value}"}),
                v,
            );
            ids2.lock().unwrap().extend([a.id, b.id]);
            let mut rows = Rows::default();
            rows.ws.rules.push(rule(
                "all",
                "allow",
                json!([{"type":"secrets","scope":"any"}]),
                json!({}),
            ));
            rows.secrets.extend([a, b]);
            rows
        },
        no_llm(),
    )
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 409, "{}", r.body);
    let j = r.json();
    assert_eq!(j["error"]["code"], "multiple_providers");
    assert_eq!(
        j["error"]["details"]["choices"].as_array().unwrap().len(),
        2
    );
    let id = ids.lock().unwrap()[1];
    let r = get(&env, "/x", &format!("x-aurora-connection-id: {id}\r\n")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("x-b: s3cr3t"));
    let r = get(
        &env,
        "/x",
        &format!("x-aurora-connection-id: {}\r\n", Uuid::now_v7()),
    )
    .await;
    assert_eq!(r.status, 404);
    assert_eq!(r.json()["error"]["code"], "connection_not_found");
}

#[tokio::test]
async fn llm_key_bearer_and_strips_x_api_key() {
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    // The LLM endpoint is the mock upstream's port, so start the upstream first.
    let llm = |p: u16| LlmSection {
        enabled_providers: vec!["litellm".into()],
        provider_preference: vec![],
        endpoints: vec![LlmEndpoint {
            id: "central".into(),
            provider: "litellm".into(),
            base_url: Some(format!("http://127.0.0.1:{p}")),
            ..Default::default()
        }],
    };
    let _ = port;
    // Two-phase: bind the upstream via a throwaway env to learn its port is not
    // possible, so reserve a port and serve the mock on it.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    let env = start(
        |w, v| {
            let mut s = secret(w, "llm", json!({}), v);
            s.kind = "llm".into();
            s.llm_provider = Some("litellm".into());
            s.llm_auth_mode = Some("api_key".into());
            s.llm_endpoint_id = Some("central".into());
            s.host_pattern = format!("127.0.0.1:{}", up.port());
            let mut rows = Rows::default();
            rows.ws.rules.push(rule(
                "llm",
                "allow",
                json!([{"type":"secrets","scope":"any","kind":"llm"}]),
                json!({}),
            ));
            rows.secrets.push(s);
            rows
        },
        llm(up.port()),
    )
    .await;
    // Serve the mock on the reserved port.
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
    let env = Env { up, ..env };
    let r = get(
        &env,
        "/v1/models",
        "x-api-key: agent\r\nAuthorization: Bearer agent\r\n",
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("authorization: Bearer s3cr3t"),
        "{}",
        r.body
    );
    assert!(!r.body.contains("x-api-key"), "{}", r.body);
    assert!(!r.body.contains("agent"), "{}", r.body);
}

#[tokio::test]
async fn invalidation_applies_new_rule_and_log_rows() {
    let env = start(grant_env(json!({"kind":"bearer"})), no_llm()).await;
    assert_eq!(get(&env, "/x", "").await.status, 200);
    // Publish a block rule: cached view still allows until invalidated.
    env.src
        .0
        .lock()
        .unwrap()
        .ws
        .rules
        .insert(0, rule("stop", "block", json!([]), json!({})));
    assert_eq!(get(&env, "/x", "").await.status, 200);
    env.resolver.cache().invalidate(&Invalidate {
        scope: Scope::Workspace(env.w.ws),
    });
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 403);
    env.log.flush().await;
    let rows = env.captured.0.lock().unwrap().clone();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].decision, "allow");
    assert_eq!(rows[0].injected, 1);
    assert_eq!(rows[0].status, 200);
    assert!(rows[0].secret_id.is_some());
    assert_eq!(rows[2].decision, "blocked");
    assert_eq!(rows[2].rule_name.as_deref(), Some("stop"));
    assert_eq!(rows[2].status, 403);
}

#[tokio::test]
async fn open_lane_policy_and_tunnel_row() {
    let env = start(
        |_, _| {
            let mut rows = Rows::default();
            rows.ws.rules.push(rule(
                "nope",
                "block",
                json!([{"type":"network","host":"blocked.example"}]),
                json!({}),
            ));
            rows
        },
        no_llm(),
    )
    .await;
    let connect = |target: String| {
        let auth = format!(
            "Proxy-Authorization: Basic {}\r\n",
            base64::engine::general_purpose::STANDARD.encode(format!("open:{TOKEN}"))
        );
        format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{auth}\r\n")
    };
    let mut s = TcpStream::connect(env.proxy).await.unwrap();
    s.write_all(connect("blocked.example:443".into()).as_bytes())
        .await
        .unwrap();
    let mut buf = vec![0u8; 2048];
    let n = s.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 403"));
    let mut s = TcpStream::connect(env.proxy).await.unwrap();
    s.write_all(connect(env.up.to_string()).as_bytes())
        .await
        .unwrap();
    let n = s.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    s.write_all(b"GET /x HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), s.read_to_end(&mut out)).await;
    drop(s);
    tokio::time::sleep(Duration::from_millis(300)).await;
    env.log.flush().await;
    let rows = env.captured.0.lock().unwrap().clone();
    let denied = rows.iter().find(|r| r.decision == "blocked").unwrap();
    assert_eq!(denied.lane, "open");
    let tunnel = rows.iter().find(|r| r.decision == "allow").unwrap();
    assert_eq!(tunnel.lane, "open");
    assert!(tunnel.bytes_up > 0 && tunnel.bytes_down > 0, "{tunnel:?}");
}

/// Absolute-form POST with `payload` through the proxy; returns the status code.
async fn post_status(env: &Env, payload: &str) -> u16 {
    let mut s = TcpStream::connect(env.proxy).await.unwrap();
    let req = format!(
        "POST http://{}/x HTTP/1.1\r\nHost: {}\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        env.up,
        env.up,
        auth(),
        payload.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out)
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// `gateway.condition_body_bytes`: a body condition beyond the buffered prefix does not
/// match; raising the limit makes it match.
#[tokio::test]
async fn condition_body_limit_is_configurable() {
    let mk = |w: &World, v: &SecretRef| {
        let s = secret(w, "s", json!({"kind":"bearer"}), v);
        let mut rows = Rows::default();
        rows.ws.default_action = "block".into();
        rows.ws.rules.push(rule(
            "grant",
            "allow",
            json!([{"type":"secret","id":s.id}]),
            json!({"conditions":[{"on":"body","op":"contains","value":"needle"}]}),
        ));
        rows.secrets.push(s);
        rows
    };
    let payload = format!("{}needle", "a".repeat(20_000));
    let small = start_with(mk, no_llm(), Some(4096)).await; // clamped up to APPROVAL_BODY_PEEK (16 KiB)
    assert_eq!(post_status(&small, &payload).await, 403, "limit 16 KiB");
    let big = start_with(mk, no_llm(), Some(32_768)).await;
    assert_eq!(post_status(&big, &payload).await, 200, "limit 32 KiB");
}
