#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! GW-003 pipeline tests: policy view, selection, decision, injection, guidance, log.
//! The policy source is in memory; DB-backed loading is covered in `policy_db.rs`.

use async_trait::async_trait;
use aurora_config::{LlmEndpoint, LlmSection, SecretsSection};
use aurora_core::Secret;
use aurora_db::repo::gateway::{PolicyRuleRow, RequestLogRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca;
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
use tokio::net::TcpListener;
use uuid::Uuid;

const TOKEN: &str = "aug_good";
const BOOT: &str = "aub_browser";

struct World {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
}

struct Static(Arc<World>);

#[async_trait]
impl CredentialResolver for Static {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        // `BOOT` is a boot credential minted with the `browser` capability (RFE-003).
        if ![TOKEN, BOOT, BYO_CTX_REQ, BYO_CONTEXT, SPK_ALICE, SPK_BOB].contains(&token) {
            return Ok(None);
        }
        // (RFE-005) hosted context credentials of a turn spoken by Alice / by Bob.
        let speaker = match token {
            SPK_ALICE => Some(ALICE),
            SPK_BOB => Some(BOB),
            _ => None,
        };
        let boot = token == BOOT;
        let w = &self.0;
        // (RFE-025) BYO credentials of an agent that requires context credentials.
        let byo = [BYO_CTX_REQ, BYO_CONTEXT].contains(&token).then(|| {
            aurora_gateway::auth::ByoAdmission {
                effective: aurora_llm::governance::effective_byo(
                    &aurora_llm::governance::ByoInstance::default(),
                    &aurora_llm::governance::ByoInputs {
                        agent_require_context: true,
                        ..Default::default()
                    },
                ),
                org_slug: "o".into(),
                ws_slug: "w".into(),
            }
        });
        Ok(Some(RequestIdentity {
            credential_id: Uuid::nil(),
            kind: if boot {
                CredentialKind::Boot
            } else if token == BYO_CONTEXT || speaker.is_some() {
                CredentialKind::Context
            } else {
                CredentialKind::Agent
            },
            agent_id: w.agent,
            workspace_id: w.ws,
            org_id: w.org,
            conversation_id: None,
            turn_id: None,
            context_label: (token == BYO_CONTEXT).then(|| "aurora run: make test".to_owned()),
            speaker_user_id: speaker,
            capabilities: if boot { vec!["browser".into()] } else { vec![] },
            byo,
        }))
    }
}

/// `aug_` of a BYO agent with `byo_require_context`, and one of its `auc_` credentials.
const BYO_CTX_REQ: &str = "aug_ctxreq";
const BYO_CONTEXT: &str = "auc_ctxreq";
/// (RFE-005) hosted context credentials of turns spoken by two different people.
const SPK_ALICE: &str = "auc_alice";
const SPK_BOB: &str = "auc_bob";
const ALICE: Uuid = Uuid::from_u128(0xa11ce);
const BOB: Uuid = Uuid::from_u128(0xb0b);

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
        approval_timeout_secs: None,
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
    start_backed(mk, llm, body_limit, |w| {
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
        (reg, vref, gets)
    })
    .await
}

/// `backing` builds the store registry, the ref the secret row points at, and a read counter.
async fn start_backed(
    mk: impl FnOnce(&World, &SecretRef) -> Rows,
    llm: LlmSection,
    body_limit: Option<usize>,
    backing: impl FnOnce(&World) -> (StoreRegistry, SecretRef, Arc<AtomicUsize>),
) -> Env {
    let w = world();
    let (reg, vref, gets) = backing(&w);
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
    let (gw_ca, gw_tls) = crate::common::gateway_tls(&gw, proxy);
    let p = Proxy::new(ProxyParts {
        ca: gw_ca,
        tls: gw_tls,
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

/// Registry with a real Vault store whose `reference_scopes` grant `scoped_org` the whole
/// `<unique>/` area; the secret row references `<unique>/api-key` (SEC-61).
fn vault_backing(
    addr: String,
    unique: String,
    scope_for_world_org: bool,
) -> impl FnOnce(&World) -> (StoreRegistry, SecretRef, Arc<AtomicUsize>) {
    move |w| {
        let scoped = if scope_for_world_org {
            w.org
        } else {
            Uuid::now_v7()
        };
        let cfg: aurora_config::SecretStoreConfig = serde_json::from_value(json!({
            "id": "vault", "kind": "hashicorp-vault", "address": addr,
            "managed_prefix": format!("{unique}-managed"),
            "auth": {"method": "token", "token_env": "AURORA_TEST_VAULT_TOKEN"},
            "reference_scopes": [{"org_id": scoped.to_string(), "paths": [format!("{unique}/*")]}],
        }))
        .unwrap();
        let keyring =
            Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{}", key_b64()), "k1").unwrap());
        let reg = StoreRegistry::from_config(
            &SecretsSection {
                stores: vec![cfg],
                ..SecretsSection::default()
            },
            keyring,
        )
        .unwrap();
        let vref = SecretRef::ext(
            "vault",
            &format!("{unique}/api-key"),
            Some("value"),
            "latest",
        )
        .unwrap();
        (reg, vref, Arc::new(AtomicUsize::new(0)))
    }
}

/// SEC-61: a reference outside the org's `reference_scopes` (here: scoped to another org id)
/// injects nothing. The in-scope control shows the same row does inject when the scope covers
/// the org. Runs against a Vault dev server when `AURORA_TEST_VAULT=1`.
#[tokio::test]
async fn refused_vault_reference_injects_nothing() {
    if std::env::var("AURORA_TEST_VAULT").as_deref() != Ok("1") {
        eprintln!("skipped: set AURORA_TEST_VAULT=1 (and the Vault dev server variables)");
        return;
    }
    let addr =
        std::env::var("AURORA_TEST_VAULT_ADDR").unwrap_or_else(|_| "http://127.0.0.1:18200".into());
    let root = std::env::var("AURORA_TEST_VAULT_TOKEN").expect("AURORA_TEST_VAULT_TOKEN");
    let unique = format!("gw-{}", Uuid::now_v7().simple());
    let w = reqwest::Client::new()
        .post(format!("{addr}/v1/secret/data/{unique}/api-key"))
        .header("X-Vault-Token", root)
        .json(&json!({"data": {"value": "VAULT-SECRET-VALUE"}}))
        .send()
        .await
        .unwrap();
    assert!(w.status().is_success());

    // Control: in scope → injected.
    let env = start_backed(
        grant_env(json!({"kind":"bearer"})),
        no_llm(),
        None,
        vault_backing(addr.clone(), unique.clone(), true),
    )
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("authorization: Bearer VAULT-SECRET-VALUE"),
        "{}",
        r.body
    );

    // Out of scope (the scope belongs to another org id) → nothing injected, nothing leaked.
    let env = start_backed(
        grant_env(json!({"kind":"bearer"})),
        no_llm(),
        None,
        vault_backing(addr, unique, false),
    )
    .await;
    let r = get(&env, "/x", "Authorization: Bearer agent-supplied\r\n").await;
    assert_eq!(r.status, 502, "{}", r.body);
    assert_eq!(
        r.json()["error"]["code"],
        "credential_unavailable",
        "{}",
        r.body
    );
    assert_eq!(r.json()["error"]["details"]["kind"], "secret", "{}", r.body);
    assert!(!r.body.contains("VAULT-SECRET-VALUE"), "{}", r.body);
    // The request never reached the upstream (its echo would list the headers).
    assert!(!r.body.contains("path="), "{}", r.body);
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
    get_as(env, TOKEN, path_and_query, extra).await
}

async fn get_as(env: &Env, token: &str, path_and_query: &str, extra: &str) -> Reply {
    let mut s = crate::common::connect(env.proxy).await;
    let auth = format!(
        "Proxy-Authorization: Basic {}\r\n",
        base64::engine::general_purpose::STANDARD.encode(format!("agent:{token}"))
    );
    let req = format!(
        "GET http://{}{path_and_query} HTTP/1.1\r\nHost: {}\r\n{auth}{extra}Connection: close\r\n\r\n",
        env.up, env.up
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
            base64::engine::general_purpose::STANDARD.encode(format!("open:{BOOT}"))
        );
        format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{auth}\r\n")
    };
    let mut s = crate::common::connect(env.proxy).await;
    s.write_all(connect("blocked.example:443".into()).as_bytes())
        .await
        .unwrap();
    let mut buf = vec![0u8; 2048];
    let n = s.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 403"));
    let mut s = crate::common::connect(env.proxy).await;
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
    let mut s = crate::common::connect(env.proxy).await;
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

/// SEC-03: a rate-limited request never opens the secret (only the admitted ones do).
#[tokio::test]
async fn sec03_rate_limited_request_never_opens_the_secret() {
    let env = start(
        |w, v| {
            let s = secret(w, "s", json!({"kind":"bearer"}), v);
            let mut rows = Rows::default();
            rows.ws.rules.push(rule(
                "slow",
                "allow",
                json!([{"type":"secret","id":s.id}]),
                json!({"rate_limit":{"count":1,"per":"minute"}}),
            ));
            rows.secrets.push(s);
            rows
        },
        no_llm(),
    )
    .await;
    assert_eq!(get(&env, "/x", "").await.status, 200);
    assert_eq!(env.gets.load(Ordering::SeqCst), 1);
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 429, "{}", r.body);
    assert_eq!(
        env.gets.load(Ordering::SeqCst),
        1,
        "a rate-limited request must not open the credential"
    );
}

/// SEC-04(a): a granted credential is injected only on the host/path its pattern serves.
#[tokio::test]
async fn sec04_injection_only_where_the_secret_pattern_serves() {
    // Host pattern names a different host: never opened, never injected.
    let env = start(
        |w, v| {
            let mut s = secret(w, "s", json!({"kind":"bearer"}), v);
            s.host_pattern = "other.example.invalid".into();
            let mut rows = Rows::default();
            rows.ws.rules.push(allow_secret(&s));
            rows.secrets.push(s);
            rows
        },
        no_llm(),
    )
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        !r.body.to_lowercase().contains("authorization:"),
        "{}",
        r.body
    );
    assert_eq!(env.gets.load(Ordering::SeqCst), 0);
    // Path pattern: served path injects, other path does not.
    let env = start(
        |w, v| {
            let mut s = secret(w, "s", json!({"kind":"bearer"}), v);
            s.path_pattern = Some("/api/**".into());
            let mut rows = Rows::default();
            rows.ws.rules.push(allow_secret(&s));
            rows.secrets.push(s);
            rows
        },
        no_llm(),
    )
    .await;
    let r = get(&env, "/other", "").await;
    assert!(
        !r.body.to_lowercase().contains("authorization:"),
        "{}",
        r.body
    );
    assert_eq!(env.gets.load(Ordering::SeqCst), 0);
    let r = get(&env, "/api/x", "").await;
    assert!(
        r.body.contains("authorization: Bearer s3cr3t"),
        "{}",
        r.body
    );
}

// ---- (phase 2, RFE-025, SEC-52) require context credentials ----------------

#[tokio::test]
async fn byo_require_context_refuses_aug_only_when_a_credential_would_be_injected() {
    let env = start(grant_env(json!({"kind":"bearer"})), no_llm()).await;
    // Plain agent credential: injected as before.
    assert_eq!(get(&env, "/x", "").await.status, 200);
    let fetched = env.gets.load(Ordering::SeqCst);
    // aug_ of an agent that requires context credentials: refused before any value is fetched.
    let r = get_as(&env, BYO_CTX_REQ, "/x", "").await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.json()["error"]["code"], "context_credential_required");
    assert_eq!(
        r.json()["error"]["details"]["mint"],
        "POST /v1/agent/context-credentials"
    );
    assert_eq!(env.gets.load(Ordering::SeqCst), fetched, "no value fetched");
    // auc_ credentials pass and receive the credential.
    let r = get_as(&env, BYO_CONTEXT, "/x", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("authorization: Bearer s3cr3t"),
        "{}",
        r.body
    );
    env.log.flush().await;
    let rows = env.captured.0.lock().unwrap().clone();
    // The labelled run is on the request-log row of the injected request.
    assert!(
        rows.iter().any(|x| x.status == 200
            && x.credential_kind == "context"
            && x.context_label.as_deref() == Some("aurora run: make test")),
        "context label in the request log: {rows:?}"
    );
    assert!(
        rows.iter().any(|x| x.status == 403
            && x.decision == "blocked"
            && x.error_code.as_deref() == Some("context_credential_required")),
        "refusal is in the request log"
    );
}

#[tokio::test]
async fn byo_require_context_leaves_uncredentialed_traffic_alone() {
    // A secret exists but no allow rule selects it: nothing would be injected.
    let env = start(
        |w, v| {
            let s = secret(w, "s", json!({"kind":"bearer"}), v);
            Rows {
                secrets: vec![s],
                ..Rows::default()
            }
        },
        no_llm(),
    )
    .await;
    let r = get_as(&env, BYO_CTX_REQ, "/x", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains("s3cr3t"));
}

// ---- (phase 2, RFE-005, SEC-44) policy `user` identities bind to the speaker -----------

/// An allow rule naming Alice injects the credential only on turns Alice spoke: same agent,
/// another speaker, an agent credential, boot-like and BYO-context traffic get nothing.
#[tokio::test]
async fn sec44_user_allow_rule_injects_only_for_its_speaker() {
    let env = start(
        |w, v| {
            let s = secret(w, "s", json!({"kind":"bearer"}), v);
            let mut grant = allow_secret(&s);
            grant.identities = json!([{"kind":"user","id":ALICE}]);
            let mut rows = Rows::default();
            rows.ws.rules.push(grant);
            rows.secrets.push(s);
            rows
        },
        no_llm(),
    )
    .await;
    let r = get_as(&env, SPK_ALICE, "/x", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("authorization: Bearer s3cr3t"),
        "alice's turn gets the credential: {}",
        r.body
    );
    let fetched = env.gets.load(Ordering::SeqCst);
    for (who, token) in [
        ("another speaker", SPK_BOB),
        ("agent credential", TOKEN),
        ("BYO context", BYO_CONTEXT),
    ] {
        let r = get_as(&env, token, "/x", "").await;
        assert!(
            !r.body.contains("s3cr3t"),
            "{who} must not receive alice's credential: {}",
            r.body
        );
    }
    assert_eq!(
        env.gets.load(Ordering::SeqCst),
        fetched,
        "the value is never even fetched for non-matching traffic"
    );
    env.log.flush().await;
    let rows = env.captured.0.lock().unwrap().clone();
    // The speaker is recorded on the request-log row (03 `request_log.speaker_user_id`).
    assert!(
        rows.iter()
            .any(|x| x.credential_kind == "context" && x.speaker_user_id == Some(ALICE)),
        "speaker in the request log: {rows:?}"
    );
    // Each row is attributed to its own turn's speaker; speakerless traffic carries none.
    assert!(
        rows.iter().all(|x| match x.credential_kind {
            "agent" => x.speaker_user_id.is_none(),
            _ => matches!(x.speaker_user_id, Some(ALICE | BOB) | None),
        }),
        "attribution: {rows:?}"
    );
    assert!(
        rows.iter().any(|x| x.speaker_user_id == Some(BOB)),
        "bob's request is attributed to bob, not alice"
    );
}

/// A block rule naming Alice hits only Alice's turns; speakerless traffic is not hers
/// (07 §Rule anatomy: to restrict it, write an agent or empty identity).
#[tokio::test]
async fn sec44_user_block_rule_does_not_match_speakerless_traffic() {
    let env = start(
        |w, v| {
            let s = secret(w, "s", json!({"kind":"bearer"}), v);
            let mut block = rule(
                "no-alice",
                "block",
                json!([{"type":"network","host":"127.0.0.1"}]),
                json!({}),
            );
            block.identities = json!([{"kind":"user","id":ALICE}]);
            let mut rows = Rows::default();
            rows.ws.rules.push(block);
            rows.ws.rules.push(allow_secret(&s));
            rows.secrets.push(s);
            rows
        },
        no_llm(),
    )
    .await;
    assert_eq!(get_as(&env, SPK_ALICE, "/x", "").await.status, 403);
    for token in [SPK_BOB, TOKEN, BYO_CONTEXT] {
        let r = get_as(&env, token, "/x", "").await;
        assert_eq!(r.status, 200, "{token}: {}", r.body);
    }
}
