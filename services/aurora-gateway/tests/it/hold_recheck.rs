#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-084 (REVIEW-5 GRANT-1): an approval authorizes the held request only under the policy in
//! force when it is released. The gateway re-resolves the credential and re-evaluates policy after
//! `approved`: a request that would now be blocked (grant revoked, block rule added, a snapshot
//! under which no tool matches) is refused as on arrival and never reaches the upstream, while a
//! request that would still need approval is released once (the approval stands).

use async_trait::async_trait;
use aurora_config::{LlmSection, SecretsSection};
use aurora_db::repo::gateway::{AppConnectionRow, PolicyRuleRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca;
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::guard::Guard;
use aurora_gateway::hold::Holds;
use aurora_gateway::invalidation::{Hub, Invalidatable};
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::reqlog::{LogWriter, RequestLog};
use aurora_gateway::resolve::{PolicySource, ResolveCache, Resolver};
use aurora_gateway::snapshot::{SnapshotStore, parse};
use aurora_secrets::{MemoryStore, SecretRef, SecretVault, StoreRegistry};
use aurora_testkit::TestDb;
use base64::Engine;
use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, Full};
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

/// The policy the gateway reads; the tests edit it while a request is held, the way the API
/// edits the database, and then drop the resolver cache (the invalidation event).
struct Src {
    ws: Mutex<ScopePolicyRows>,
    conns: Mutex<Vec<AppConnectionRow>>,
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

struct Null;

#[async_trait]
impl LogWriter for Null {
    async fn write(&self, _rows: &[aurora_db::repo::gateway::RequestLogRow]) -> Result<(), String> {
        Ok(())
    }
}

fn snapshot(write_tool: bool, block_untooled: bool) -> Value {
    let mut write = json!([]);
    if write_tool {
        write = json!([
            {"id": "write_item", "name": "w", "host": "127.0.0.1", "path": "/items", "methods": ["POST"]}
        ]);
    }
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
            ]}, "write": {"items": write}},
            "grants": {"custom_only": false, "block_untooled": block_untooled},
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

struct Env {
    w: World,
    proxy: SocketAddr,
    up: SocketAddr,
    pool: sqlx::PgPool,
    src: Arc<Src>,
    cache: Arc<ResolveCache>,
    store: Arc<SnapshotStore>,
    /// Requests that reached the upstream.
    hits: Arc<AtomicUsize>,
    conn_id: Uuid,
}

impl Env {
    /// Applies an edit and delivers the invalidation, as the API's NOTIFY would.
    fn edit(&self, f: impl FnOnce(&mut ScopePolicyRows, &mut Vec<AppConnectionRow>)) {
        f(
            &mut self.src.ws.lock().unwrap(),
            &mut self.src.conns.lock().unwrap(),
        );
        self.cache.flush_all();
    }
}

fn derived(name: &str, conn: Uuid, tool: &str, ask: bool) -> (Uuid, Value) {
    let mut r = json!({"name": name, "enabled": true, "identities": [], "action": "allow",
                       "targets": [{"type": "connection", "id": conn, "tools": [tool]}]});
    if ask {
        r["require_approval"] = json!(true);
    }
    (Uuid::now_v7(), r)
}

async fn start(pool: sqlx::PgPool) -> Env {
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
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
    };
    let conn_id = conn.id;
    let mut ws = ScopePolicyRows {
        default_action: "block".into(),
        ..ScopePolicyRows::default()
    };
    ws.derived.push(derived("d1", conn.id, "read_item", false));
    ws.derived.push(derived("d2", conn.id, "write_item", true));
    let src = Arc::new(Src {
        ws: Mutex::new(ws),
        conns: Mutex::new(vec![conn]),
    });
    let hits = Arc::new(AtomicUsize::new(0));
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    let counted = hits.clone();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let counted = counted.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let counted = counted.clone();
                    async move {
                        counted.fetch_add(1, Ordering::SeqCst);
                        let (parts, body) = req.into_parts();
                        let body = body
                            .collect()
                            .await
                            .map(|b| b.to_bytes())
                            .unwrap_or_default();
                        let mut out = format!("path={}\n", parts.uri.path());
                        for (k, v) in &parts.headers {
                            out.push_str(&format!("{}: {}\n", k, v.to_str().unwrap_or("?")));
                        }
                        out.push_str(&format!("body={}\n", String::from_utf8_lossy(&body)));
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(out))))
                    }
                });
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    let store = SnapshotStore::new();
    store.install(parse("h1", snapshot(true, false)).unwrap());
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&LlmSection {
            enabled_providers: vec![],
            provider_preference: vec![],
            endpoints: vec![],
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
    let holds = Arc::new(Holds::new(
        pool.clone(),
        hub,
        Uuid::now_v7(),
        Duration::from_secs(30),
        Some(rx),
    ));
    let handler = Arc::new(
        PolicyHandler::new(PolicyHandlerParts {
            resolver,
            plaintext,
            log: RequestLog::new(Arc::new(Null)),
            external_url: "https://aurora.test/".into(),
            holds: Some(holds),
            touch_pool: None,
        })
        .with_insecure_loopback_injection(true),
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
        w,
        proxy,
        up,
        pool,
        src,
        cache,
        store,
        hits,
        conn_id,
    }
}

struct Reply {
    status: u16,
    body: String,
}

async fn send(env: &Env, method: &str, path: &str, body: &str) -> Reply {
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"));
    let req = format!(
        "{method} http://{up}{path} HTTP/1.1\r\nHost: {up}\r\nProxy-Authorization: Basic {auth}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
        up = env.up
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Reply {
        status,
        body: body.to_owned(),
    }
}

/// The id of the pending approval of `ws`, polled with a deadline.
async fn pending(pool: &sqlx::PgPool, ws: Uuid) -> Uuid {
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

/// Applies a decision the way the API does: first decision wins, then NOTIFY.
async fn approve(pool: &sqlx::PgPool, id: Uuid, ws: Uuid) {
    sqlx::query(
        "UPDATE approvals SET status='approved', decided_at=now() WHERE id=$1 AND status='pending'",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("SELECT pg_notify('aurora_approval', $1)")
        .bind(json!({"id": id, "workspace_id": ws, "event": "decided"}).to_string())
        .execute(pool)
        .await
        .unwrap();
}

/// Holds a `POST /items`, runs `edit` while it waits, approves it and returns the client's reply.
async fn hold_edit_approve(env: &Arc<Env>, edit: impl FnOnce(&Env)) -> Reply {
    let e2 = env.clone();
    let t = tokio::spawn(async move { send(&e2, "POST", "/items", "{\"a\":1}").await });
    let id = pending(&env.pool, env.w.ws).await;
    edit(env);
    approve(&env.pool, id, env.w.ws).await;
    t.await.unwrap()
}

#[tokio::test]
async fn approval_after_the_grant_was_revoked_is_refused_and_never_forwarded() {
    let db = TestDb::new().await;
    let env = Arc::new(start(db.pool().clone()).await);
    let r = hold_edit_approve(&env, |e| e.edit(|ws, _| ws.derived.clear())).await;
    // No candidate any more: the ordinary `credential_unavailable`, not an uncredentialed send.
    assert_eq!(r.status, 502, "{}", r.body);
    assert!(r.body.contains("credential_unavailable"), "{}", r.body);
    assert!(!r.body.contains("T0K"), "credential leaked: {}", r.body);
    assert_eq!(
        env.hits.load(Ordering::SeqCst),
        0,
        "no request may reach the upstream"
    );
}

#[tokio::test]
async fn approval_after_the_connection_was_disabled_is_refused() {
    let db = TestDb::new().await;
    let env = Arc::new(start(db.pool().clone()).await);
    let r = hold_edit_approve(&env, |e| e.edit(|_, conns| conns.clear())).await;
    assert_eq!(r.status, 502, "{}", r.body);
    assert!(r.body.contains("credential_unavailable"), "{}", r.body);
    assert_eq!(env.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn approval_after_a_block_rule_was_added_is_refused() {
    let db = TestDb::new().await;
    let env = Arc::new(start(db.pool().clone()).await);
    let r = hold_edit_approve(&env, |e| {
        e.edit(|ws, _| {
            ws.rules.push(PolicyRuleRow {
                logical_id: Uuid::now_v7(),
                name: "stop-items".into(),
                enabled: true,
                identities: json!([]),
                targets: json!([{"type":"network","host":"127.0.0.1","path":"/items"}]),
                action: "block".into(),
                require_approval: false,
                approval_timeout_secs: None,
                rate_limit: None,
                conditions: json!([]),
            });
        })
    })
    .await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert!(r.body.contains("blocked_by_policy"), "{}", r.body);
    assert_eq!(env.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn approval_after_a_restricting_snapshot_was_activated_is_refused() {
    let db = TestDb::new().await;
    let env = Arc::new(start(db.pool().clone()).await);
    // The new version drops the write tool and says only catalogued tools may use the credential.
    let r = hold_edit_approve(&env, |e| {
        e.store.install(parse("h2", snapshot(false, true)).unwrap());
        e.cache.flush_all();
    })
    .await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(env.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn approval_after_an_unrelated_policy_edit_is_released_once_with_the_same_bytes() {
    let db = TestDb::new().await;
    let env = Arc::new(start(db.pool().clone()).await);
    let r = hold_edit_approve(&env, |e| {
        e.edit(|ws, _| {
            ws.rules.push(PolicyRuleRow {
                logical_id: Uuid::now_v7(),
                name: "unrelated".into(),
                enabled: true,
                identities: json!([]),
                targets: json!([{"type":"network","host":"elsewhere.test"}]),
                action: "block".into(),
                require_approval: false,
                approval_timeout_secs: None,
                rate_limit: None,
                conditions: json!([]),
            });
        })
    })
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("authorization: Bearer T0K"), "{}", r.body);
    assert!(r.body.contains("body={\"a\":1}"), "{}", r.body);
    assert_eq!(env.hits.load(Ordering::SeqCst), 1, "forwarded exactly once");
    // The rule still needs approval, yet the approval stands: no second row.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals WHERE workspace_id = $1")
        .bind(env.w.ws)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "the release must not create a second hold");
}

#[tokio::test]
async fn the_held_approval_records_the_connection_it_would_use() {
    let db = TestDb::new().await;
    let env = Arc::new(start(db.pool().clone()).await);
    let e2 = env.clone();
    let t = tokio::spawn(async move { send(&e2, "POST", "/items", "{}").await });
    let id = pending(&env.pool, env.w.ws).await;
    let conn: Option<Uuid> = sqlx::query_scalar("SELECT connection_id FROM approvals WHERE id=$1")
        .bind(id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(conn, Some(env.conn_id));
    approve(&env.pool, id, env.w.ws).await;
    assert_eq!(t.await.unwrap().status, 200);
}
