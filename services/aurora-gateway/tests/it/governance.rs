#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! GW-005: LLM and egress governance (step 6a, org egress posture, origin preflight, backstop).
//! The mock provider is Anthropic-style: `GET /v1/models` reports the key's account in
//! `anthropic-organization-id`, `POST /v1/messages` does too.

use async_trait::async_trait;
use aurora_config::{EgressEntry, LlmEndpoint, LlmSection, SecretsSection};
use aurora_db::repo::gateway::{PolicyRuleRow, RequestLogRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca;
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::governance::{GovSource, Governance, instance_from_config};
use aurora_gateway::guard::Guard;
use aurora_gateway::invalidation::{Invalidatable, Invalidate, Scope};
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::reqlog::{LogWriter, RequestLog};
use aurora_gateway::resolve::{PolicySource, Resolver};
use aurora_secrets::{MemoryStore, SecretRef, SecretVault, StoreRegistry};
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
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

struct Src(Mutex<Vec<SecretMetaRow>>, Mutex<ScopePolicyRows>);

#[async_trait]
impl PolicySource for Src {
    async fn scope_policy(&self, _o: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        Ok(if ws.is_some() {
            self.1.lock().unwrap().clone()
        } else {
            ScopePolicyRows::default()
        })
    }
    async fn secrets(&self, _o: Uuid, _w: Uuid) -> Result<Vec<SecretMetaRow>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn slugs(&self, _w: Uuid) -> Result<Option<(String, String)>, String> {
        Ok(Some(("acme".into(), "main".into())))
    }
}

#[derive(Default)]
struct TestGov {
    doc: Mutex<Value>,
    writes: Mutex<Vec<(Uuid, Value, String)>>,
}

#[async_trait]
impl GovSource for TestGov {
    async fn org_governance(&self, _o: Uuid) -> Result<Option<Value>, String> {
        Ok(Some(self.doc.lock().unwrap().clone()))
    }
    async fn write_origin(&self, id: Uuid, origin: Value, status: &'static str) {
        self.writes
            .lock()
            .unwrap()
            .push((id, origin, status.to_owned()));
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

#[derive(Default)]
struct Counters {
    verifies: AtomicUsize,
    messages: AtomicUsize,
}

fn org_of(key: &str) -> &'static str {
    if key.contains("foreign") {
        "org_bad"
    } else {
        "org_ok"
    }
}

async fn mock(
    req: Request<Incoming>,
    c: Arc<Counters>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let key = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let mut r = Response::new(Full::new(Bytes::from(format!("key={key}"))));
    match req.uri().path() {
        "/v1/models" => {
            c.verifies.fetch_add(1, Ordering::SeqCst);
            if key.contains("down") {
                *r.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
            } else {
                r.headers_mut()
                    .insert("anthropic-organization-id", org_of(&key).parse().unwrap());
                *r.body_mut() = Full::new(Bytes::from("{\"data\":[]}"));
            }
        }
        "/v1/messages" => {
            c.messages.fetch_add(1, Ordering::SeqCst);
            let org = if key.contains("sneaky") {
                Some("org_bad")
            } else if key.contains("nohdr") {
                None
            } else {
                Some(org_of(&key))
            };
            if let Some(o) = org {
                r.headers_mut()
                    .insert("anthropic-organization-id", o.parse().unwrap());
            }
        }
        _ => {}
    }
    Ok(r)
}

struct Opts {
    /// Org governance document.
    doc: Value,
    llm_min: aurora_config::LlmPosture,
    egress_min: aurora_config::EgressPosture,
    instance_models: Vec<String>,
    /// Keys held by Aurora: (stored key, workspace scoped, stored origin).
    secrets: Vec<(&'static str, bool, Option<Value>)>,
    known_hosts: Vec<String>,
}

fn doc() -> Value {
    json!({"llm": {"posture": "open", "endpoints": {"anthropic": {"allowed": true}}}})
}

fn verified(org: &str) -> Value {
    json!({"status": "verified", "method": "provider_account", "organization_id": org,
           "value_version": 1, "checked_at": null})
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            doc: doc(),
            llm_min: aurora_config::LlmPosture::Open,
            egress_min: aurora_config::EgressPosture::Open,
            instance_models: vec![],
            secrets: vec![("sk-good", true, Some(verified("org_ok")))],
            known_hosts: vec![],
        }
    }
}

struct Env {
    proxy: SocketAddr,
    llm: SocketAddr,
    plain: SocketAddr,
    gov: Arc<Governance>,
    gsrc: Arc<TestGov>,
    resolver: Arc<Resolver>,
    counters: Arc<Counters>,
    mem: Arc<MemoryStore>,
    captured: Arc<Capture>,
    log: Arc<RequestLog>,
    secret_ids: Vec<Uuid>,
}

async fn serve_mock(c: Arc<Counters>) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let c = c.clone();
            tokio::spawn(async move {
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(s), service_fn(move |r| mock(r, c.clone())))
                    .await;
            });
        }
    });
    a
}

async fn start(o: Opts) -> Env {
    let w = Arc::new(World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    });
    let counters = Arc::new(Counters::default());
    let llm_addr = serve_mock(counters.clone()).await;
    let plain = serve_mock(Arc::new(Counters::default())).await;
    let mem = Arc::new(MemoryStore::new("mem"));
    mem.allow_reference(w.org, "*");
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
    let mut reg = StoreRegistry::from_config(&SecretsSection::default(), keyring).unwrap();
    reg.register(mem.clone());
    let mut rows = Vec::new();
    let mut policy = ScopePolicyRows::default();
    let mut secret_ids = Vec::new();
    for (i, (key, ws_scoped, origin)) in o.secrets.iter().enumerate() {
        let path = format!("k{i}");
        mem.preload(&path, None, key.as_bytes());
        let id = Uuid::now_v7();
        secret_ids.push(id);
        rows.push(SecretMetaRow {
            id,
            workspace_id: ws_scoped.then_some(w.ws),
            name: format!("key{i}"),
            kind: "llm".into(),
            llm_provider: Some("anthropic".into()),
            llm_auth_mode: Some("api_key".into()),
            llm_endpoint_id: Some("anthropic".into()),
            host_pattern: llm_addr.to_string(),
            path_pattern: None,
            injection: json!({}),
            value_ref: SecretRef::ext("mem", &path, None, "1").unwrap().to_column(),
            value_version: 1,
            origin: origin.clone(),
        });
        policy.rules.push(PolicyRuleRow {
            logical_id: Uuid::now_v7(),
            name: "grant".into(),
            enabled: true,
            identities: json!([]),
            targets: json!([{"type":"secret","id":id}]),
            action: "allow".into(),
            require_approval: false,
            rate_limit: None,
            conditions: json!([]),
        });
    }
    let src = Arc::new(Src(Mutex::new(rows), Mutex::new(policy)));
    let cfg = LlmSection {
        enabled_providers: vec!["anthropic".into()],
        provider_preference: vec![],
        endpoints: vec![LlmEndpoint {
            id: "anthropic".into(),
            provider: "anthropic".into(),
            allowed_models: o.instance_models.clone(),
            key_origin: Some(aurora_config::KeyOrigin {
                method: "provider_account".into(),
                allowed_organization_ids: vec!["org_ok".into()],
                ..Default::default()
            }),
            ..Default::default()
        }],
    };
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config_with_anthropic_base(
            &cfg,
            &format!("http://{llm_addr}"),
        )
        .unwrap(),
    );
    let gcfg = aurora_config::Governance {
        allowed_key_scopes: vec!["organization".into(), "workspace".into()],
        llm_minimum_posture: o.llm_min,
        require_key_origin: false,
        known_llm_hosts: o.known_hosts.clone(),
        origin_recheck_secs: 3600,
        egress_minimum_posture: o.egress_min,
        baseline_allowlist: Vec::<EgressEntry>::new(),
    };
    let gsrc = Arc::new(TestGov {
        doc: Mutex::new(o.doc.clone()),
        ..Default::default()
    });
    let gov = Arc::new(Governance::new(
        instance_from_config(&gcfg, &llm),
        llm.clone(),
        gsrc.clone(),
    ));
    let resolver = Arc::new(Resolver::new(src, llm).with_governance(gov.clone()));
    let plaintext = Arc::new(PlaintextCache::with_ttl(
        Arc::new(SecretVault::new(reg)),
        Duration::from_millis(1),
    ));
    let captured = Arc::new(Capture::default());
    let log = RequestLog::new(captured.clone());
    let handler = Arc::new(
        PolicyHandler::new(PolicyHandlerParts {
            resolver: resolver.clone(),
            plaintext,
            log: log.clone(),
            external_url: "https://aurora.test/".into(),
            holds: None,
            touch_pool: None,
        })
        .with_governance(gov.clone()),
    );
    let gw = ca::generate("gw").unwrap();
    let pl = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = pl.local_addr().unwrap();
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy]).unwrap());
    let (gw_ca, gw_tls) = crate::common::gateway_tls(&gw, proxy);
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
    Env {
        proxy,
        llm: llm_addr,
        plain,
        gov,
        gsrc,
        resolver,
        counters,
        mem,
        captured,
        log,
        secret_ids,
    }
}

struct Reply {
    status: u16,
    body: String,
}

fn auth(user: &str) -> String {
    format!(
        "Proxy-Authorization: Basic {}\r\n",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{TOKEN}"))
    )
}

async fn request(
    env: &Env,
    target: SocketAddr,
    method: &str,
    path: &str,
    extra: &str,
    body: &[u8],
) -> Reply {
    let mut s = crate::common::connect(env.proxy).await;
    let head = format!(
        "{method} http://{target}{path} HTTP/1.1\r\nHost: {target}\r\n{}{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        auth("agent"),
        body.len()
    );
    s.write_all(head.as_bytes()).await.unwrap();
    s.write_all(body).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Reply {
        status: head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        body: body.to_owned(),
    }
}

async fn msg(env: &Env, extra: &str, body: &Value) -> Reply {
    request(
        env,
        env.llm,
        "POST",
        "/v1/messages",
        extra,
        body.to_string().as_bytes(),
    )
    .await
}

async fn connect(env: &Env, target: SocketAddr) -> u16 {
    let mut s = crate::common::connect(env.proxy).await;
    s.write_all(
        format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{}\r\n",
            auth("open")
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = vec![0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8_lossy(&buf[..n])
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn has(r: &Reply, code: &str) -> bool {
    r.body.contains(&format!("\"{code}\""))
}

fn hello() -> Value {
    json!({"model": "claude-x", "messages": []})
}

async fn wait_writes(env: &Env, n: usize) -> Vec<(Uuid, Value, String)> {
    for _ in 0..100 {
        {
            let w = env.gsrc.writes.lock().unwrap();
            if w.len() >= n {
                return w.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("origin write-back did not happen");
}

// ---- step 6a ----

#[tokio::test]
async fn compliant_key_is_injected_and_verified_once() {
    let env = start(Opts::default()).await;
    let r = msg(&env, "x-api-key: agent-own\r\n", &hello()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("key=sk-good"), "{}", r.body);
    assert_eq!(env.counters.verifies.load(Ordering::SeqCst), 1);
    // Second request: verification cached by (secret, sha256(value)).
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 200);
    assert_eq!(env.counters.verifies.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn endpoint_not_permitted_and_known_host_under_sanctioned_only() {
    let o = Opts {
        doc: json!({"llm": {"posture": "open", "endpoints": {"anthropic": {"allowed": false}}}}),
        ..Opts::default()
    };
    let env = start(o).await;
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_provider_not_permitted"), "{}", r.body);
    assert_eq!(env.counters.messages.load(Ordering::SeqCst), 0);

    // Known LLM host (no configured endpoint): refused only under sanctioned_only.
    let known: SocketAddr = "127.0.0.1:59871".parse().unwrap();
    let mk = |posture: &str| Opts {
        doc: json!({"llm": {"posture": posture, "endpoints": {"anthropic": {"allowed": true}}}}),
        known_hosts: vec![known.to_string()],
        secrets: vec![],
        ..Opts::default()
    };
    let env = start(mk("sanctioned_only")).await;
    let r = request(&env, known, "GET", "/x", "", b"").await;
    assert!(has(&r, "llm_provider_not_permitted"), "{}", r.body);
    let env = start(mk("open")).await;
    let r = request(&env, known, "GET", "/x", "", b"").await;
    assert!(!has(&r, "llm_provider_not_permitted"), "{}", r.body);
}

#[tokio::test]
async fn key_scope_not_permitted() {
    let o = Opts {
        doc: json!({"llm": {"endpoints": {"anthropic": {"allowed": true, "key_scopes": ["organization"]}}}}),
        ..Opts::default()
    };
    let env = start(o).await; // the key is workspace scoped
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_key_not_permitted"), "{}", r.body);
    assert!(r.body.contains(&env.secret_ids[0].to_string()));
}

#[tokio::test]
async fn stored_origin_must_comply() {
    for (origin, code) in [
        (Some(verified("org_bad")), "llm_key_wrong_organization"),
        (
            Some(json!({"status":"mismatch","value_version":1})),
            "llm_key_wrong_organization",
        ),
        (
            Some(json!({"status":"pending","value_version":1})),
            "llm_key_unverifiable",
        ),
        (None, "llm_key_unverifiable"),
        // verified, but for an older value
        (
            Some(json!({"status":"verified","organization_id":"org_ok","value_version":0})),
            "llm_key_unverifiable",
        ),
    ] {
        let env = start(Opts {
            secrets: vec![("sk-good", true, origin)],
            ..Opts::default()
        })
        .await;
        let r = msg(&env, "", &hello()).await;
        assert_eq!(r.status, 403, "{code}: {}", r.body);
        assert!(has(&r, code), "{code}: {}", r.body);
        // Never forwarded without the key instead.
        assert_eq!(env.counters.messages.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn sanctioned_only_requires_an_aurora_key() {
    let sanctioned = json!({"llm": {"posture": "sanctioned_only", "endpoints": {"anthropic": {"allowed": true}}}});
    let env = start(Opts {
        doc: sanctioned.clone(),
        secrets: vec![],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "x-api-key: agent-own\r\n", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_key_required"), "{}", r.body);
    assert!(r.body.contains("anthropic"));
    // Instance minimum raises an open org to sanctioned_only.
    let env = start(Opts {
        llm_min: aurora_config::LlmPosture::SanctionedOnly,
        secrets: vec![],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &hello()).await;
    assert!(has(&r, "llm_key_required"), "{}", r.body);
    // Open posture: an agent-supplied key passes through untouched.
    let env = start(Opts {
        secrets: vec![],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "x-api-key: agent-own\r\n", &hello()).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("key=agent-own"));
    // With a sanctioned key everything works.
    let env = start(Opts {
        doc: sanctioned,
        ..Opts::default()
    })
    .await;
    assert_eq!(msg(&env, "", &hello()).await.status, 200);
}

#[tokio::test]
async fn model_allow_list_fails_closed() {
    let env = start(Opts {
        doc: json!({"llm": {"endpoints": {"anthropic": {"allowed": true, "allowed_models": ["claude-a", "claude-b"]}}}}),
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &json!({"model": "claude-c"})).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    assert!(r.body.contains("claude-a") && r.body.contains("claude-c"));
    // Unparsable body.
    let r = request(&env, env.llm, "POST", "/v1/messages", "", b"{not json").await;
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    // Body without a model.
    let r = request(&env, env.llm, "POST", "/v1/messages", "", b"{}").await;
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    // Truncated body (larger than the condition buffer).
    let big = format!(
        "{{\"pad\":\"{}\",\"model\":\"claude-a\"}}",
        "a".repeat(300 * 1024)
    );
    let r = request(&env, env.llm, "POST", "/v1/messages", "", big.as_bytes()).await;
    assert!(has(&r, "llm_model_not_allowed"), "{}", r.body);
    // Allowed model and model-less calls pass.
    assert_eq!(
        msg(&env, "", &json!({"model": "claude-b"})).await.status,
        200
    );
    assert_eq!(
        request(&env, env.llm, "GET", "/v1/models", "", b"")
            .await
            .status,
        200
    );
    // Instance list intersects with the org list.
    let env = start(Opts {
        instance_models: vec!["claude-a".into()],
        doc: json!({"llm": {"endpoints": {"anthropic": {"allowed": true, "allowed_models": ["claude-a", "claude-b"]}}}}),
        ..Opts::default()
    })
    .await;
    assert!(has(
        &msg(&env, "", &json!({"model": "claude-b"})).await,
        "llm_model_not_allowed"
    ));
    assert_eq!(
        msg(&env, "", &json!({"model": "claude-a"})).await.status,
        200
    );
}

#[tokio::test]
async fn refusals_are_logged_and_malformed_governance_fails_closed() {
    let env = start(Opts {
        secrets: vec![],
        doc: json!({"llm": {"posture": "sanctioned_only", "endpoints": {"anthropic": {"allowed": true}}}}),
        ..Opts::default()
    })
    .await;
    msg(&env, "", &hello()).await;
    env.log.flush().await;
    let rows = env.captured.0.lock().unwrap().clone();
    assert!(rows.iter().any(|r| r.decision == "governance_refused"
        && r.error_code.as_deref() == Some("llm_key_required")));
    let env = start(Opts {
        doc: json!({"llm": {"posture": 7}}),
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 503);
    assert!(has(&r, "policy_unavailable"), "{}", r.body);
}

#[tokio::test]
async fn effective_governance_is_cached_until_invalidated() {
    let env = start(Opts::default()).await;
    assert_eq!(msg(&env, "", &hello()).await.status, 200);
    *env.gsrc.doc.lock().unwrap() =
        json!({"llm": {"endpoints": {"anthropic": {"allowed": false}}}});
    assert_eq!(msg(&env, "", &hello()).await.status, 200, "cached");
    let ev = Invalidate {
        scope: Scope::Org(Uuid::nil()),
    };
    env.gov.invalidate(&Invalidate { scope: Scope::All });
    env.resolver.cache().invalidate(&ev);
    env.resolver.cache().flush_all();
    let r = msg(&env, "", &hello()).await;
    assert!(has(&r, "llm_provider_not_permitted"), "{}", r.body);
}

// ---- egress posture ----

#[tokio::test]
async fn egress_posture_blocks_uncredentialed_with_allow_list() {
    let blocked = |r: &Reply| r.status == 403 && has(r, "blocked_by_egress_policy");
    // Open: allowed.
    let env = start(Opts::default()).await;
    assert_eq!(
        request(&env, env.plain, "GET", "/x", "", b"").await.status,
        200
    );
    // credentialed_only: blocked, but credentialed LLM traffic is unaffected.
    let mut o = Opts::default();
    o.doc["egress"] = json!({"posture": "credentialed_only"});
    let env = start(o).await;
    let r = request(&env, env.plain, "GET", "/x", "", b"").await;
    assert!(blocked(&r), "{} {}", r.status, r.body);
    assert_eq!(msg(&env, "", &hello()).await.status, 200);
    // Org allow-list entry.
    let mut o = Opts::default();
    o.doc["egress"] = json!({"posture": "credentialed_only", "allowlist": [{"host": "127.0.0.1"}]});
    let env = start(o).await;
    assert_eq!(
        request(&env, env.plain, "GET", "/x", "", b"").await.status,
        200
    );
    // Instance minimum forces the posture on an org that says open.
    let env = start(Opts {
        egress_min: aurora_config::EgressPosture::CredentialedOnly,
        ..Opts::default()
    })
    .await;
    assert!(blocked(
        &request(&env, env.plain, "GET", "/x", "", b"").await
    ));
}

#[tokio::test]
async fn open_lane_under_both_postures() {
    // Egress: open reaches anything, credentialed_only only allow-listed hosts.
    let env = start(Opts::default()).await;
    assert_eq!(connect(&env, env.plain).await, 200);
    let mut o = Opts::default();
    o.doc["egress"] = json!({"posture": "credentialed_only"});
    let env = start(o).await;
    assert_eq!(connect(&env, env.plain).await, 403);
    let mut o = Opts::default();
    o.doc["egress"] = json!({"posture": "credentialed_only", "allowlist": [{"host": "127.0.0.1"}]});
    let env = start(o).await;
    assert_eq!(connect(&env, env.plain).await, 200);
    // LLM: governed hosts are refused under sanctioned_only (nothing can be injected).
    let env = start(Opts::default()).await;
    assert_eq!(connect(&env, env.llm).await, 200);
    let env = start(Opts {
        doc: json!({"llm": {"posture": "sanctioned_only", "endpoints": {"anthropic": {"allowed": true}}}}),
        ..Opts::default()
    })
    .await;
    assert_eq!(connect(&env, env.llm).await, 403);
    assert_eq!(connect(&env, env.plain).await, 200);
}

// ---- origin preflight and backstop ----

#[tokio::test]
async fn preflight_refuses_foreign_key_and_writes_back() {
    // Stored origin says verified, but the value is actually another account's key.
    let env = start(Opts {
        secrets: vec![("sk-foreign", true, Some(verified("org_ok")))],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_key_wrong_organization"), "{}", r.body);
    assert_eq!(
        env.counters.messages.load(Ordering::SeqCst),
        0,
        "never sent"
    );
    let w = wait_writes(&env, 1).await;
    assert_eq!(w[0].0, env.secret_ids[0]);
    assert_eq!(w[0].2, "mismatch");
    assert_eq!(w[0].1["status"], "mismatch");
    assert_eq!(w[0].1["organization_id"], "org_bad");
}

#[tokio::test]
async fn reference_secret_value_change_is_caught() {
    let env = start(Opts::default()).await;
    assert_eq!(msg(&env, "", &hello()).await.status, 200);
    // The external value changes to a foreign key outside Aurora.
    env.mem.preload("k0", None, b"sk-foreign");
    tokio::time::sleep(Duration::from_millis(10)).await; // plaintext cache TTL
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_key_wrong_organization"), "{}", r.body);
    assert_eq!(env.counters.messages.load(Ordering::SeqCst), 1);
    wait_writes(&env, 1).await;
}

#[tokio::test]
async fn preflight_fails_closed_when_provider_unreachable() {
    let env = start(Opts {
        secrets: vec![("sk-down", true, Some(verified("org_ok")))],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 503);
    assert!(has(&r, "llm_origin_check_unavailable"), "{}", r.body);
    assert!(r.body.contains("anthropic"));
    assert_eq!(env.counters.messages.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn backstop_replaces_response_from_foreign_account() {
    // Passes preflight (the models call reports a good account) but the response reports a
    // foreign one.
    let env = start(Opts {
        secrets: vec![("sk-sneaky", true, Some(verified("org_ok")))],
        ..Opts::default()
    })
    .await;
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 403);
    assert!(has(&r, "llm_key_wrong_organization"), "{}", r.body);
    assert_eq!(env.counters.messages.load(Ordering::SeqCst), 1);
    let w = wait_writes(&env, 1).await;
    assert_eq!(w[0].2, "mismatch");
}

#[tokio::test]
async fn backstop_counts_missing_headers_without_refusing() {
    let env = start(Opts {
        secrets: vec![("sk-nohdr", true, Some(verified("org_ok")))],
        ..Opts::default()
    })
    .await;
    assert_eq!(env.gov.header_missing_total(), 0);
    let r = msg(&env, "", &hello()).await;
    assert_eq!(r.status, 200);
    assert_eq!(env.gov.header_missing_total(), 1);
}
