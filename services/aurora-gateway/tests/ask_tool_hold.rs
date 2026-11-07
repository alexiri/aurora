#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! GW-011 follow-up (FUP-M2): a tool rule with `ask` (derived `require_approval`) creates a
//! hold against a real database; the decision is applied the way the API applies it
//! (`approvals` row + `aurora_approval` NOTIFY) and the request then proceeds or is refused.

mod common;
use async_trait::async_trait;
use aurora_config::{LlmSection, SecretsSection};
use aurora_db::repo::gateway::{AppConnectionRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca;
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::guard::Guard;
use aurora_gateway::hold::Holds;
use aurora_gateway::invalidation::Hub;
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::reqlog::{LogWriter, RequestLog};
use aurora_gateway::resolve::{PolicySource, Resolver};
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
use std::sync::Arc;
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
            principals: vec![w.agent],
        }))
    }
}

struct Src {
    ws: ScopePolicyRows,
    conns: Vec<AppConnectionRow>,
}

#[async_trait]
impl PolicySource for Src {
    async fn scope_policy(&self, _o: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        Ok(if ws.is_some() {
            self.ws.clone()
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

async fn upstream(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
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
    Ok(Response::new(Full::new(Bytes::from(out))))
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
                {"id": "write_item", "name": "w", "host": "127.0.0.1", "path": "/items", "methods": ["POST"]}
            ]}},
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
    };
    let mut ws = ScopePolicyRows {
        default_action: "block".into(),
        ..ScopePolicyRows::default()
    };
    // Derived-rule shapes (BE-044): allow read tools, ask for write_item.
    ws.derived.push((
        Uuid::now_v7(),
        json!({"name":"d1","enabled":true,"identities":[],"action":"allow",
               "targets":[{"type":"connection","id":conn.id,"tools":["read_item"]}]}),
    ));
    ws.derived.push((
        Uuid::now_v7(),
        json!({"name":"d2","enabled":true,"identities":[],"action":"allow","require_approval":true,
               "targets":[{"type":"connection","id":conn.id,"tools":["write_item"]}]}),
    ));
    let src = Arc::new(Src {
        ws,
        conns: vec![conn],
    });
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
    let store = SnapshotStore::new();
    store.install(parse("h1", snapshot()).unwrap());
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&LlmSection {
            enabled_providers: vec![],
            provider_preference: vec![],
            endpoints: vec![],
        })
        .unwrap(),
    );
    let resolver = Arc::new(Resolver::new(src, llm).with_snapshots(store));
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
    let handler = Arc::new(PolicyHandler::new(PolicyHandlerParts {
        resolver,
        plaintext,
        log: RequestLog::new(Arc::new(Null)),
        external_url: "https://aurora.test/".into(),
        holds: Some(holds),
        touch_pool: None,
    }));
    let gw = ca::generate("gw").unwrap();
    let pl = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = pl.local_addr().unwrap();
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy]).unwrap());
    let (gw_ca, gw_tls) = common::gateway_tls(&gw, proxy);
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
    Env { w, proxy, up, pool }
}

struct Reply {
    status: u16,
    body: String,
}

async fn send(env: &Env, method: &str, path: &str, body: &str) -> Reply {
    let mut s = common::connect(env.proxy).await;
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

async fn pending(pool: &sqlx::PgPool, ws: Uuid) -> Uuid {
    for _ in 0..100 {
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
async fn decide(pool: &sqlx::PgPool, id: Uuid, status: &str, ws: Uuid) {
    sqlx::query(
        "UPDATE approvals SET status=$2, decided_at=now() WHERE id=$1 AND status='pending'",
    )
    .bind(id)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("SELECT pg_notify('aurora_approval', $1)")
        .bind(json!({"id": id, "workspace_id": ws, "event": "decided"}).to_string())
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn ask_tool_holds_then_approval_proceeds_and_denial_refuses() {
    let db = TestDb::new().await;
    let env = Arc::new(start(db.pool().clone()).await);

    // Allowed read tool needs no approval.
    assert_eq!(send(&env, "GET", "/items", "").await.status, 200);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);

    // `ask` tool: held until decided; approve => the request proceeds with the injection.
    let e2 = env.clone();
    let t = tokio::spawn(async move { send(&e2, "POST", "/items", "{\"a\":1}").await });
    let id = pending(&env.pool, env.w.ws).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!t.is_finished(), "request must stay held while pending");
    decide(&env.pool, id, "approved", env.w.ws).await;
    let r = t.await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("authorization: Bearer T0K"), "{}", r.body);

    // Second write: denied => refused, nothing reaches the upstream.
    let e2 = env.clone();
    let t = tokio::spawn(async move { send(&e2, "POST", "/items", "{}").await });
    let id2 = pending(&env.pool, env.w.ws).await;
    assert_ne!(id, id2);
    decide(&env.pool, id2, "denied", env.w.ws).await;
    let r = t.await.unwrap();
    assert_eq!(r.status, 403, "{}", r.body);
    assert!(r.body.contains("manual_approval_denied"), "{}", r.body);
    assert!(r.body.contains(&id2.to_string()), "{}", r.body);
}
