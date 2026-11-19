#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-011B-1 (RFE-011b, D-264, SEC-74, C-820, C-827): a request that needs approval and whose
//! deadline is long is **parked**: refused at once with 403 `approval_parked`, never sent, the
//! approval kept `pending` in Postgres with a salted fingerprint. Every eligibility condition of
//! `07 §Park and resume` has a positive and a negative case here; the grant side is
//! `approval_grants.rs`, which reuses this harness.

use crate::common::Capture;
use async_trait::async_trait;
use aurora_config::{LlmEndpoint, LlmSection, SecretsSection};
use aurora_db::repo::gateway::{AppConnectionRow, RequestLogRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca;
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::guard::Guard;
use aurora_gateway::hold::Holds;
use aurora_gateway::invalidation::{Hub, Invalidatable};
use aurora_gateway::park::ParkConfig;
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::reqlog::RequestLog;
use aurora_gateway::resolve::{PolicySource, ResolveCache, Resolver};
use aurora_gateway::snapshot::{SnapshotStore, parse};
use aurora_secrets::{MemoryStore, SecretRef, SecretVault, StoreRegistry};
use base64::Engine;
use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use uuid::Uuid;

pub(crate) const TOKEN: &str = "aug_good";
/// A string that must never appear in a table, a log row or a `details` object.
pub(crate) const CANARY: &str = "CANARY-7f3a9c";

#[derive(Clone, Copy)]
pub(crate) struct World {
    pub org: Uuid,
    pub ws: Uuid,
    pub agent: Uuid,
}

/// Who the next request authenticates as (the tests switch it).
#[derive(Clone)]
pub(crate) struct Who {
    pub kind: CredentialKind,
    pub conversation: Option<Uuid>,
    pub turn: Option<Uuid>,
    pub speaker: Option<Uuid>,
    /// A BYO admission (the agent brings its own LLM key).
    pub byo: bool,
}

pub(crate) struct Static {
    pub w: World,
    pub who: Mutex<Who>,
}

#[async_trait]
impl CredentialResolver for Static {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        if token != TOKEN {
            return Ok(None);
        }
        let (w, who) = (self.w, self.who.lock().unwrap().clone());
        Ok(Some(RequestIdentity {
            credential_id: Uuid::nil(),
            kind: who.kind,
            agent_id: w.agent,
            workspace_id: w.ws,
            org_id: w.org,
            conversation_id: who.conversation,
            turn_id: who.turn,
            trace_context: None,
            context_label: None,
            speaker_user_id: who.speaker,
            speaker_group_ids: Vec::new(),
            capabilities: vec![],
            byo: who.byo.then(|| aurora_gateway::auth::ByoAdmission {
                effective: aurora_llm::governance::EffectiveByo {
                    posture: aurora_llm::governance::ByoPosture::Enabled,
                    posture_level: aurora_llm::governance::ByoLevel::Instance,
                    source_cidrs: None,
                    source_level: None,
                    require_context: false,
                    require_context_level: None,
                },
                org_slug: "acme".into(),
                ws_slug: "main".into(),
            }),
        }))
    }
}

pub(crate) struct Src {
    pub ws: Mutex<ScopePolicyRows>,
    pub conns: Mutex<Vec<AppConnectionRow>>,
}

#[async_trait]
impl PolicySource for Src {
    async fn scope_policy(&self, _o: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        Ok(if ws.is_some() {
            self.ws.lock().unwrap().clone()
        } else {
            ScopePolicyRows::default()
        })
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
            .lock()
            .unwrap()
            .iter()
            .filter(|c| app_ids.contains(&c.app_id))
            .cloned()
            .collect())
    }
    async fn app_availability(&self, _o: Uuid, _w: Uuid) -> Result<(bool, Vec<String>), String> {
        Ok((false, vec![]))
    }
}

fn snapshot() -> Value {
    json!({
        "format": 1, "contract": 1,
        "packages": [{"package_id": "pkg", "version": "1.0.0", "digest": "d", "publisher": "p",
                      "hooks_digest": null, "egress": {}, "context": ["org_id"]}],
        "oauth_clients": [],
        "apps": [{
            "app_id": "demo", "package_id": "pkg", "about": {"id": "demo", "name": "demo"},
            "credentials": [{"name": "tok", "secret": true, "optional": false}],
            "metadata": [], "connect": [], "token": null,
            "tools": {"read": {"items": [
                {"id": "read_item", "name": "r", "host": "127.0.0.1", "path": "/items", "methods": ["GET"]}
            ]}, "write": {"items": [
                {"id": "write_item", "name": "w", "host": "127.0.0.1", "path": "/items", "methods": ["POST", "PUT"]}
            ]}},
            "grants": {"custom_only": false, "block_untooled": false},
            "approvals": {}, "agent": null, "database": null
        }],
        "hosts": [{
            "host": "127.0.0.1", "path_prefix": null, "app_id": "demo", "claim_status": "active",
            "tenant": null, "rewrite_host": null, "inject": [{"bearer": "{credentials.tok}"}],
            "finalize": null, "transform": null, "intercept": null, "passthrough": []
        }],
        "reserved_hosts": []
    })
}

pub(crate) struct Opts {
    pub park: bool,
    /// `approvals.timeout_secs`: the timeout of requests whose rules set none.
    pub default_timeout_secs: u64,
    pub park_cfg: Option<ParkConfig>,
    pub llm_host: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            park: true,
            default_timeout_secs: 7200,
            park_cfg: None,
            llm_host: false,
        }
    }
}

pub(crate) struct Env {
    pub w: World,
    pub proxy: SocketAddr,
    pub up: SocketAddr,
    pub pool: sqlx::PgPool,
    pub src: Arc<Src>,
    pub cache: Arc<ResolveCache>,
    pub holds: Arc<Holds>,
    pub log: Arc<RequestLog>,
    pub captured: Arc<Capture>,
    pub who: Arc<Static>,
    /// Requests that reached the upstream.
    pub hits: Arc<AtomicUsize>,
    /// Makes the upstream drop connections (a failed upstream).
    pub upstream_down: Arc<AtomicBool>,
    pub conv: Uuid,
}

impl Env {
    pub fn edit(&self, f: impl FnOnce(&mut ScopePolicyRows, &mut Vec<AppConnectionRow>)) {
        f(
            &mut self.src.ws.lock().unwrap(),
            &mut self.src.conns.lock().unwrap(),
        );
        self.cache.flush_all();
    }

    pub fn set_who(&self, f: impl FnOnce(&mut Who)) {
        f(&mut self.who.who.lock().unwrap());
    }
}

pub(crate) fn derived(
    name: &str,
    conn: Uuid,
    tool: &str,
    ask: bool,
    identities: Value,
) -> (Uuid, Value) {
    let mut r = json!({"name": name, "enabled": true, "identities": identities,
                       "action": "allow",
                       "targets": [{"type": "connection", "id": conn, "tools": [tool]}]});
    if ask {
        r["require_approval"] = json!(true);
    }
    (Uuid::now_v7(), r)
}

pub(crate) async fn start(pool: sqlx::PgPool, o: Opts) -> Env {
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
    let conv = Uuid::now_v7();
    let mem = Arc::new(MemoryStore::new("mem"));
    mem.preload("c1", None, br#"{"tok":"T0K"}"#);
    mem.allow_reference(w.org, "*");
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{key}"), "k1").unwrap());
    let mut reg = StoreRegistry::from_config(&SecretsSection::default(), keyring).unwrap();
    reg.register(mem);
    let conn = AppConnectionRow {
        id: Uuid::now_v7(),
        workspace_id: Some(w.ws),
        app_id: "demo".into(),
        label: "demo".into(),
        status: "active".into(),
        credentials_ref: SecretRef::ext("mem", "c1", None, "1").unwrap().to_column(),
        credentials_version: 1,
        credentials_expires_at: None,
        metadata: json!({}),
        tenant_host: None,
        instance_id: None,
        connect_method: None,
        access: "write".into(),
        scopes: Vec::new(),
    };
    let mut ws = ScopePolicyRows {
        default_action: "block".into(),
        ..ScopePolicyRows::default()
    };
    ws.derived
        .push(derived("d1", conn.id, "read_item", false, json!([])));
    ws.derived
        .push(derived("d2", conn.id, "write_item", true, json!([])));
    let src = Arc::new(Src {
        ws: Mutex::new(ws),
        conns: Mutex::new(vec![conn]),
    });
    let hits = Arc::new(AtomicUsize::new(0));
    let upstream_down = Arc::new(AtomicBool::new(false));
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    let (counted, down) = (hits.clone(), upstream_down.clone());
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let (counted, down) = (counted.clone(), down.clone());
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let (counted, down) = (counted.clone(), down.clone());
                    async move {
                        counted.fetch_add(1, Ordering::SeqCst);
                        if down.load(Ordering::SeqCst) {
                            return Err("upstream down");
                        }
                        let (parts, body) = req.into_parts();
                        let body = body
                            .collect()
                            .await
                            .map(|b| b.to_bytes())
                            .unwrap_or_default();
                        let mut out = format!("path={}\n", parts.uri.path());
                        out.push_str(&format!("body={}\n", String::from_utf8_lossy(&body)));
                        Ok::<_, &str>(Response::new(Full::new(Bytes::from(out))))
                    }
                });
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    let store = SnapshotStore::new();
    store.install(parse("h1", snapshot()).unwrap());
    // `llm_host`: the mock upstream is a governed LLM endpoint (its host is an endpoint host).
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&LlmSection {
            enabled_providers: if o.llm_host {
                vec!["openai_compatible".into()]
            } else {
                vec![]
            },
            provider_preference: vec![],
            dev_upstream_overrides: Default::default(),
            endpoints: if o.llm_host {
                vec![LlmEndpoint {
                    id: "mock".into(),
                    provider: "openai_compatible".into(),
                    base_url: Some(format!("http://{up}")),
                    ..LlmEndpoint::default()
                }]
            } else {
                vec![]
            },
        })
        .unwrap(),
    );
    let resolver = Arc::new(Resolver::new(src.clone(), llm).with_snapshots(store.clone()));
    let cache = resolver.cache();
    let plaintext = Arc::new(PlaintextCache::new(Arc::new(SecretVault::new(reg))));
    let (shutdown, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(shutdown);
    let hub = Hub::new();
    tokio::spawn(hub.clone().run(pool.clone(), rx.clone()));
    while !hub.is_connected() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let cfg = o.park_cfg.unwrap_or(ParkConfig {
        enabled: o.park,
        threshold_secs: 900,
        max_timeout_secs: 86_400,
        resume_window_secs: 1800,
        deny_cooldown_secs: 900,
        max_body_bytes: 262_144,
        max_pending_per_agent: 20,
    });
    let holds = Arc::new(
        Holds::new(
            pool.clone(),
            hub,
            Uuid::now_v7(),
            Duration::from_secs(o.default_timeout_secs),
            Some(rx),
        )
        .with_park(cfg),
    );
    let captured = Arc::new(Capture::default());
    let log = RequestLog::new(captured.clone());
    let handler = Arc::new(
        PolicyHandler::new(PolicyHandlerParts {
            resolver,
            plaintext,
            log: log.clone(),
            external_url: "https://aurora.test/".into(),
            holds: Some(holds.clone()),
            touch_pool: None,
        })
        .with_insecure_loopback_injection(true),
    );
    let gw = ca::generate("gw").unwrap();
    let pl = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = pl.local_addr().unwrap();
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy]).unwrap());
    let (gw_ca, gw_tls) = crate::common::gateway_tls(&gw, proxy);
    let who = Arc::new(Static {
        w,
        who: Mutex::new(Who {
            kind: CredentialKind::Context,
            conversation: Some(conv),
            turn: Some(Uuid::now_v7()),
            speaker: None,
            byo: false,
        }),
    });
    let p = Proxy::new(ProxyParts {
        ca: gw_ca,
        tls: gw_tls,
        resolver: who.clone(),
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
        pool,
        src,
        cache,
        holds,
        log,
        captured,
        who,
        hits,
        upstream_down,
        conv,
    }
}

pub(crate) struct Reply {
    pub status: u16,
    pub head: String,
    pub body: String,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
    pub fn details(&self) -> Value {
        self.json()["error"]["details"].clone()
    }
    pub fn code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    }
}

/// One request through the proxy. `chunked` sends the body with `Transfer-Encoding: chunked`.
pub(crate) async fn send_ex(
    env: &Env,
    method: &str,
    path_and_query: &str,
    body: &str,
    chunked: bool,
) -> Reply {
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"));
    let (framing, payload) = if chunked {
        (
            "Transfer-Encoding: chunked".to_owned(),
            format!("{:x}\r\n{body}\r\n0\r\n\r\n", body.len()),
        )
    } else {
        (format!("Content-Length: {}", body.len()), body.to_owned())
    };
    let req = format!(
        "{method} http://{up}{path_and_query} HTTP/1.1\r\nHost: {up}\r\nProxy-Authorization: Basic {auth}\r\n{framing}\r\nConnection: close\r\n\r\n{payload}",
        up = env.up
    );
    crate::common::write_flushed(&mut s, req.as_bytes()).await;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(15), s.read_to_end(&mut out)).await;
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
        body: body.to_owned(),
    }
}

pub(crate) async fn send(env: &Env, method: &str, path: &str, body: &str) -> Reply {
    send_ex(env, method, path, body, false).await
}

/// The approval rows of the workspace, newest first: `(id, status, parked)`.
pub(crate) async fn rows(pool: &sqlx::PgPool, ws: Uuid) -> Vec<(Uuid, String, bool)> {
    sqlx::query_as(
        "SELECT id, status, parked FROM approvals WHERE workspace_id=$1 ORDER BY id DESC",
    )
    .bind(ws)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The id of the pending approval of `ws`, polled with a deadline.
pub(crate) async fn pending(pool: &sqlx::PgPool, ws: Uuid) -> Uuid {
    for _ in 0..200 {
        if let Some(id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM approvals WHERE workspace_id=$1 AND status='pending'",
        )
        .bind(ws)
        .fetch_optional(pool)
        .await
        .unwrap()
        {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no pending approval");
}

/// Applies an approval the way the API does for a parked row (C-823): `approved` and the grant
/// `ready` for `window_secs`.
pub(crate) async fn approve_ready(pool: &sqlx::PgPool, id: Uuid, window_secs: i64) {
    sqlx::query(
        "UPDATE approvals SET status='approved', decided_at=now(), resolved_at=now(), \
         resume_state='ready', resume_expires_at=now() + make_interval(secs => $2) \
         WHERE id=$1 AND status='pending' AND parked",
    )
    .bind(id)
    .bind(window_secs as f64)
    .execute(pool)
    .await
    .unwrap();
}

pub(crate) async fn log_rows(env: &Env) -> Vec<RequestLogRow> {
    env.log.flush().await;
    env.captured.0.lock().unwrap().clone()
}

/// Every approval row, as text, for the body-canary scan.
async fn tables_text(pool: &sqlx::PgPool) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT coalesce(string_agg(a::text, E'\\n'), '') FROM approvals a",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_long_approval_is_parked_refused_at_once_and_never_sent() {
    let db = aurora_testkit::TestDb::new().await;
    let env = start(db.pool().clone(), Opts::default()).await;
    let body = format!("{{\"note\":\"{CANARY}\"}}");
    let r = send(&env, "POST", &format!("/items?token={CANARY}"), &body).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "approval_parked", "{}", r.body);
    assert!(
        r.head
            .to_ascii_lowercase()
            .contains("x-should-retry: false"),
        "{}",
        r.head
    );
    let d = r.details();
    let id: Uuid = d["approval_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(d["resume_window_secs"], 1800);
    assert!(d["expires_at"].as_str().is_some());
    assert_eq!(
        d["manage_url"],
        format!("https://aurora.test/o/acme/w/main/approvals?focus={id}")
    );
    assert!(r.body.contains("30 minutes"), "{}", r.body);
    assert_eq!(
        env.hits.load(Ordering::SeqCst),
        0,
        "a parked request is never sent"
    );
    // The row: pending, parked, salted fingerprint, a deadline of the effective timeout.
    let (status, parked, salt, fp, secs): (String, bool, Vec<u8>, Vec<u8>, f64) = sqlx::query_as(
        "SELECT status, parked, fp_salt, fingerprint, extract(epoch FROM deadline - created_at)::float8 \
         FROM approvals WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&db.pool().clone())
    .await
    .unwrap();
    assert_eq!((status.as_str(), parked), ("pending", true));
    assert_eq!((salt.len(), fp.len()), (16, 32));
    assert!((secs - 7200.0).abs() < 5.0, "deadline {secs}");
    // Identical repeat: the same answer and the same id, nothing inserted.
    let again = send(&env, "POST", &format!("/items?token={CANARY}"), &body).await;
    assert_eq!(again.code(), "approval_parked");
    assert_eq!(again.details()["approval_id"], d["approval_id"]);
    assert_eq!(rows(db.pool(), env.w.ws).await.len(), 1);
    // Another body is another request: a new approval.
    let other = send(&env, "POST", "/items", "{\"x\":2}").await;
    assert_ne!(other.details()["approval_id"], d["approval_id"]);
    assert_eq!(rows(db.pool(), env.w.ws).await.len(), 2);
    assert_eq!(env.hits.load(Ordering::SeqCst), 0);
    // No body or query value anywhere: tables, details, request log.
    assert!(!tables_text(db.pool()).await.contains(CANARY));
    assert!(!r.body.contains(CANARY) && !again.body.contains(CANARY));
    let log = log_rows(&env).await;
    assert!(!format!("{log:?}").contains(CANARY), "{log:?}");
    // C-827: the refusal and its repeat are `approval_parked` rows carrying the approval id.
    let parked: Vec<_> = log
        .iter()
        .filter(|r| r.decision == "approval_parked")
        .collect();
    assert_eq!(parked.len(), 3);
    assert_eq!(parked[0].approval_id, Some(id));
    assert_eq!(parked[0].status, 403);
    assert_eq!(parked[0].error_code.as_deref(), Some("approval_parked"));
    assert!(parked.iter().all(|r| !r.resumed));
}

#[tokio::test]
async fn with_parking_off_the_request_is_held_as_before() {
    let db = aurora_testkit::TestDb::new().await;
    let env = Arc::new(
        start(
            db.pool().clone(),
            Opts {
                park: false,
                ..Opts::default()
            },
        )
        .await,
    );
    let e2 = env.clone();
    let t = tokio::spawn(async move { send(&e2, "POST", "/items", "{\"a\":1}").await });
    let id = pending(db.pool(), env.w.ws).await;
    let parked: bool = sqlx::query_scalar("SELECT parked FROM approvals WHERE id=$1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(!parked);
    // Decide it the way the API does for a hold.
    sqlx::query("UPDATE approvals SET status='approved', decided_at=now() WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("SELECT pg_notify('aurora_approval', $1)")
        .bind(json!({"id": id, "workspace_id": env.w.ws, "event": "decided"}).to_string())
        .execute(db.pool())
        .await
        .unwrap();
    let r = t.await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(env.hits.load(Ordering::SeqCst), 1);
    // The hold's request-log row names its approval and is not `resumed`.
    let log = log_rows(&env).await;
    assert!(
        log.iter()
            .any(|r| r.decision == "approved" && r.approval_id == Some(id) && !r.resumed),
        "{log:?}"
    );
}

/// Sends a request that must be **held** (not parked): returns the pending row once it appears,
/// then abandons it so the request task ends.
async fn assert_held(
    env: &Arc<Env>,
    method: &'static str,
    q: &'static str,
    chunked: bool,
    body: String,
) {
    let e2 = env.clone();
    let t = tokio::spawn(async move { send_ex(&e2, method, q, &body, chunked).await });
    // The held row (a parked one from an earlier step of the test may exist and stay pending).
    let mut held = None;
    for _ in 0..200 {
        held = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM approvals WHERE workspace_id=$1 AND status='pending' AND NOT parked",
        )
        .bind(env.w.ws)
        .fetch_optional(&env.pool)
        .await
        .unwrap();
        if held.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let id = held.expect("the request must be held (a pending, unparked row)");
    sqlx::query("UPDATE approvals SET status='denied', decided_at=now() WHERE id=$1")
        .bind(id)
        .execute(&env.pool)
        .await
        .unwrap();
    sqlx::query("SELECT pg_notify('aurora_approval', $1)")
        .bind(json!({"id": id, "workspace_id": env.w.ws, "event": "decided"}).to_string())
        .execute(&env.pool)
        .await
        .unwrap();
    let r = t.await.unwrap();
    assert_eq!(r.code(), "manual_approval_denied", "{}", r.body);
    assert_eq!(env.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn timeout_at_the_threshold_is_held_one_above_it_parks() {
    let db = aurora_testkit::TestDb::new().await;
    let at = Arc::new(
        start(
            db.pool().clone(),
            Opts {
                default_timeout_secs: 900,
                ..Opts::default()
            },
        )
        .await,
    );
    assert_held(&at, "POST", "/items", false, "{}".into()).await;
    let db2 = aurora_testkit::TestDb::new().await;
    let above = start(
        db2.pool().clone(),
        Opts {
            default_timeout_secs: 901,
            ..Opts::default()
        },
    )
    .await;
    assert_eq!(
        send(&above, "POST", "/items", "{}").await.code(),
        "approval_parked"
    );
}

#[tokio::test]
async fn boot_and_conversation_less_traffic_is_held() {
    let db = aurora_testkit::TestDb::new().await;
    let env = Arc::new(start(db.pool().clone(), Opts::default()).await);
    env.set_who(|w| w.kind = CredentialKind::Boot);
    assert_held(&env, "POST", "/items", false, "{}".into()).await;
    env.set_who(|w| {
        w.kind = CredentialKind::Context;
        w.conversation = None;
        w.turn = None;
    });
    assert_held(&env, "POST", "/items", false, "{}".into()).await;
    // A context credential with a conversation but no turn is not a turn of a hosted agent.
    env.set_who(|w| {
        w.conversation = Some(Uuid::now_v7());
        w.turn = None;
    });
    assert_held(&env, "POST", "/items", false, "{}".into()).await;
}

#[tokio::test]
async fn body_at_the_cap_parks_over_it_chunked_or_unbounded_is_held() {
    let db = aurora_testkit::TestDb::new().await;
    let cfg = ParkConfig {
        enabled: true,
        threshold_secs: 900,
        max_timeout_secs: 86_400,
        resume_window_secs: 1800,
        deny_cooldown_secs: 900,
        max_body_bytes: 4096,
        max_pending_per_agent: 20,
    };
    let env = Arc::new(
        start(
            db.pool().clone(),
            Opts {
                park_cfg: Some(cfg),
                ..Opts::default()
            },
        )
        .await,
    );
    let at = "a".repeat(4096);
    assert_eq!(
        send(&env, "POST", "/items", &at).await.code(),
        "approval_parked"
    );
    assert_held(&env, "POST", "/items", false, "a".repeat(4097)).await;
    assert_held(&env, "POST", "/items", true, "{}".into()).await;
}

#[tokio::test]
async fn a_verdict_that_depends_on_the_speaker_is_held() {
    let db = aurora_testkit::TestDb::new().await;
    let env = Arc::new(start(db.pool().clone(), Opts::default()).await);
    let user = Uuid::now_v7();
    // Only this person may use the write tool (and asks first): the resume turn has no speaker.
    env.edit(|ws, conns| {
        let conn = conns[0].id;
        ws.derived.clear();
        ws.derived.push(derived(
            "d-user",
            conn,
            "write_item",
            true,
            json!([{"kind": "user", "id": user}]),
        ));
    });
    env.set_who(|w| w.speaker = Some(user));
    assert_held(&env, "POST", "/items", false, "{}".into()).await;
    // The same rule for everyone does not depend on the speaker: parked even with a speaker.
    env.edit(|ws, conns| {
        let conn = conns[0].id;
        ws.derived.clear();
        ws.derived
            .push(derived("d-all", conn, "write_item", true, json!([])));
    });
    assert_eq!(
        send(&env, "POST", "/items", "{}").await.code(),
        "approval_parked"
    );
}

// D-264(c): LLM traffic never parks — a governed LLM endpoint's request that needs approval is
// held, even with a long deadline and every other condition met.
#[tokio::test]
async fn a_governed_llm_endpoint_request_is_held_never_parked() {
    let db = aurora_testkit::TestDb::new().await;
    let env = Arc::new(
        start(
            db.pool().clone(),
            Opts {
                llm_host: true,
                ..Opts::default()
            },
        )
        .await,
    );
    env.edit(|ws, _| {
        ws.derived.clear();
        ws.default_action = "allow".into();
        ws.rules.push(aurora_db::repo::gateway::PolicyRuleRow {
            logical_id: Uuid::now_v7(),
            name: "ask-llm".into(),
            enabled: true,
            identities: json!([]),
            targets: json!([{"type":"network","host":"127.0.0.1","path":"/items"}]),
            action: "allow".into(),
            require_approval: true,
            approval_timeout_secs: None,
            rate_limit: None,
            conditions: json!([]),
        });
    });
    assert_held(&env, "POST", "/items", false, "{}".into()).await;
    assert!(rows(&env.pool, env.w.ws).await.iter().all(|r| !r.2));
}

// D-264 (eligibility 1): a BYO-admitted agent's request is held, never parked.
#[tokio::test]
async fn a_byo_request_is_held_never_parked() {
    let db = aurora_testkit::TestDb::new().await;
    let env = Arc::new(start(db.pool().clone(), Opts::default()).await);
    env.set_who(|w| w.byo = true);
    assert_held(&env, "POST", "/items", false, "{}".into()).await;
    assert!(rows(&env.pool, env.w.ws).await.iter().all(|r| !r.2));
}

#[tokio::test]
async fn caps_answer_approval_limit_reached() {
    let db = aurora_testkit::TestDb::new().await;
    let cfg = ParkConfig {
        enabled: true,
        threshold_secs: 900,
        max_timeout_secs: 86_400,
        resume_window_secs: 1800,
        deny_cooldown_secs: 900,
        max_body_bytes: 262_144,
        max_pending_per_agent: 2,
    };
    let env = start(
        db.pool().clone(),
        Opts {
            park_cfg: Some(cfg),
            ..Opts::default()
        },
    )
    .await;
    for n in 0..2 {
        assert_eq!(
            send(&env, "POST", "/items", &format!("{{\"n\":{n}}}"))
                .await
                .code(),
            "approval_parked"
        );
    }
    let r = send(&env, "POST", "/items", "{\"n\":9}").await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (403, "approval_limit_reached")
    );
    assert_eq!(r.details(), json!({"limit": 2, "per": "pending"}));
    assert!(
        r.head
            .to_ascii_lowercase()
            .contains("x-should-retry: false")
    );
    assert_eq!(rows(db.pool(), env.w.ws).await.len(), 2, "nothing inserted");
    // An identical repeat of a pending request is not a new approval: not counted against the cap.
    assert_eq!(
        send(&env, "POST", "/items", "{\"n\":0}").await.code(),
        "approval_parked"
    );
    let log = log_rows(&env).await;
    assert!(log.iter().any(|r| r.decision == "rate_limited"
        && r.error_code.as_deref() == Some("approval_limit_reached")));
    // The hourly cap: 60 parked approvals created in the last hour.
    sqlx::query("UPDATE approvals SET status='abandoned' WHERE workspace_id=$1")
        .bind(env.w.ws)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, conversation_id, \
         gateway_instance_id, method, host, path, summary, status, deadline, parked, fp_salt, fingerprint) \
         SELECT gen_random_uuid(), $1, $2, $3, $4, gen_random_uuid(), 'POST', 'h', '/x', '{}'::jsonb, \
                'expired', now(), true, '\\x00'::bytea, '\\x00'::bytea FROM generate_series(1, 58)",
    )
    .bind(env.w.org)
    .bind(env.w.ws)
    .bind(env.w.agent)
    .bind(env.conv)
    .execute(db.pool())
    .await
    .unwrap();
    let r = send(&env, "POST", "/items", "{\"n\":100}").await;
    assert_eq!(r.code(), "approval_limit_reached", "{}", r.body);
    assert_eq!(r.details(), json!({"limit": 60, "per": "hour"}));
}

#[tokio::test]
async fn a_denied_request_is_answered_without_a_new_approval_inside_the_cooldown() {
    let db = aurora_testkit::TestDb::new().await;
    let env = start(db.pool().clone(), Opts::default()).await;
    let first = send(&env, "POST", "/items", "{\"a\":1}").await;
    let id: Uuid = first.details()["approval_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    sqlx::query("UPDATE approvals SET status='denied', decided_at=now() WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    let r = send(&env, "POST", "/items", "{\"a\":1}").await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (403, "manual_approval_denied"),
        "{}",
        r.body
    );
    assert_eq!(r.details()["approval_id"], json!(id));
    assert!(r.details()["cooldown_until"].as_str().is_some());
    assert_eq!(rows(db.pool(), env.w.ws).await.len(), 1, "no new approval");
    assert_eq!(env.hits.load(Ordering::SeqCst), 0);
    // A different request is not covered by the cooldown.
    assert_eq!(
        send(&env, "POST", "/items", "{\"a\":2}").await.code(),
        "approval_parked"
    );
    // After the cooldown the same request parks again.
    sqlx::query("UPDATE approvals SET decided_at = now() - interval '1 hour' WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        send(&env, "POST", "/items", "{\"a\":1}").await.code(),
        "approval_parked"
    );
    let log = log_rows(&env).await;
    assert!(
        log.iter()
            .any(|r| r.decision == "approval_denied" && r.approval_id == Some(id))
    );
}
