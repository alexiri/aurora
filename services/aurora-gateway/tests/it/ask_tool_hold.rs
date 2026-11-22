#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! GW-011 follow-up (FUP-M2): a tool rule with `ask` (derived `require_approval`) creates a
//! hold against a real database; the decision is applied the way the API applies it
//! (`approvals` row + `aurora_approval` NOTIFY) and the request then proceeds or is refused.

use async_trait::async_trait;
use aurora_config::{LlmSection, SecretsSection};
use aurora_db::repo::gateway::{AppConnectionRow, InstanceHostRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca;
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::hold::Holds;
use aurora_gateway::invalidation::Hub;
use aurora_gateway::plaintext::PlaintextCache;

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
        Ok(Some(crate::common::identity(
            CredentialKind::Agent,
            w.agent,
            w.ws,
            w.org,
        )))
    }
}

struct Src {
    org: ScopePolicyRows,
    ws: ScopePolicyRows,
    conns: Vec<AppConnectionRow>,
    instances: Vec<InstanceHostRow>,
}

#[async_trait]
impl PolicySource for Src {
    async fn instances(&self, _org: Uuid) -> Result<Vec<InstanceHostRow>, String> {
        Ok(self.instances.clone())
    }
    async fn scope_policy(&self, _o: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        Ok(if ws.is_some() {
            self.ws.clone()
        } else {
            self.org.clone()
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

/// The policy rows a test world serves: the workspace's rules, its connections and the registered instances.
struct Rows {
    /// The organization's rules (empty unless a test adds some).
    org: ScopePolicyRows,
    ws: ScopePolicyRows,
    conns: Vec<AppConnectionRow>,
    instances: Vec<InstanceHostRow>,
}

async fn start(pool: sqlx::PgPool) -> Env {
    start_with(pool, snapshot(), demo_rows).await
}

fn demo_rows(w: &World) -> Rows {
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
    Rows {
        org: ScopePolicyRows::default(),
        ws,
        conns: vec![conn],
        instances: vec![],
    }
}

async fn start_with(pool: sqlx::PgPool, snap: Value, mk: impl FnOnce(&World) -> Rows) -> Env {
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
    let rows = mk(&w);
    let src = Arc::new(Src {
        org: rows.org,
        ws: rows.ws,
        conns: rows.conns,
        instances: rows.instances,
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
    store.install(parse("h1", snap).unwrap());
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&LlmSection {
            enabled_providers: vec![],
            provider_preference: vec![],
            dev_upstream_overrides: Default::default(),
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
    let (proxy, _) = crate::common::spawn_proxy(&gw, Arc::new(Static(w)), handler, None).await;
    Env { w, proxy, up, pool }
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
    // (P5-UX-16, C-982) The held row records the app, the tool and the account it would use.
    let (app, tool, conn): (Option<String>, Option<String>, Option<Uuid>) =
        sqlx::query_as("SELECT app_id, tool_id, connection_id FROM approvals WHERE id=$1")
            .bind(id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(app.as_deref(), Some("demo"));
    assert_eq!(tool.as_deref(), Some("write_item"));
    assert!(conn.is_some());
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

/// (FIX-145 follow-up, D-231) For an instance served under a base path (`localhost` + `/jira`), the
/// approval summary's path fields (and `{path.N}` in its link) count from the **app path** (after the base path),
/// so the manifest's segment numbers mean the same under `/jira` as on a host of their own; the
/// approval's own display keeps the full request path.
#[tokio::test]
async fn approval_summary_path_segments_count_from_the_app_path_under_a_base_path() {
    let db = TestDb::new().await;
    let inst = Uuid::now_v7();
    let mut app = json!({
        "app_id": "jira", "package_id": "pkg", "about": {"id": "jira", "name": "jira"},
        "credentials": [{"name": "tok", "secret": true, "optional": false}],
        "metadata": [], "connect": [], "token": null,
        "tools": {"write": {"items": [
            {"id": "comment", "name": "w", "host": "{instance}", "path": "/rest/api/2/issue/*/comment", "methods": ["POST"]}
        ]}},
        // App-path segments: 1 rest, 2 api, 3 2, 4 issue, 5 <key>. Under `/jira` the full path has one more.
        "approvals": {"comment": {
            "title": "Comment on an issue",
            "fields": [{"label": "Issue", "source": "path", "segment": 5}],
            // (D-467) What Jira DC declares: the request's host, which carries the base path.
            "link": "https://{request.host}/browse/{path.5}"
        }},
        "agent": null, "database": null
    });
    app["instances"] = json!({"label": "jira"});
    let mut snap = json!({
        "format": 1, "contract": 1,
        "packages": [{"package_id": "pkg", "version": "1.0.0", "digest": "d", "publisher": "p",
                      "hooks_digest": null, "egress": {}, "context": ["org_id"]}],
        "oauth_clients": [], "apps": [app], "hosts": [], "reserved_hosts": []
    });
    snap["instance_hosts"] = json!([{
        "host": "{instance}", "path_prefix": null, "app_id": "jira", "claim_status": "active",
        "tenant": null, "rewrite_host": null, "inject": [{"bearer": "{credentials.tok}"}],
        "finalize": null, "transform": null, "intercept": null, "passthrough": []
    }]);
    let env = Arc::new(
        start_with(db.pool().clone(), snap, move |w| {
            let mut r = demo_rows(w);
            r.conns[0].app_id = "jira".into();
            r.conns[0].instance_id = Some(inst);
            let cid = r.conns[0].id;
            r.ws.derived = vec![(
                Uuid::now_v7(),
                json!({"name":"d","enabled":true,"identities":[],"action":"allow","require_approval":true,
                       "targets":[{"type":"connection","id":cid,"tools":["comment"]}]}),
            )];
            r.instances = vec![InstanceHostRow {
                id: inst,
                app_id: "jira".into(),
                host: "localhost".into(),
                base_path: Some("/jira".into()),
            }];
            r
        })
        .await,
    );
    let e2 = env.clone();
    let t = tokio::spawn(async move {
        let mut s = crate::common::connect(e2.proxy).await;
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"));
        let req = format!(
            "POST http://localhost:{port}/jira/rest/api/2/issue/KEY-7/comment HTTP/1.1\r\nHost: localhost:{port}\r\nProxy-Authorization: Basic {auth}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}",
            port = e2.up.port()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await;
        String::from_utf8_lossy(&out).into_owned()
    });
    let (id, summary, path): (Uuid, Value, String) = {
        let id = pending(&env.pool, env.w.ws).await;
        sqlx::query_as("SELECT id, summary, path FROM approvals WHERE id = $1")
            .bind(id)
            .fetch_one(&env.pool)
            .await
            .unwrap()
    };
    // `{path.5}` is the issue key (app path), not `issue` (full path, where the base path is segment 1).
    assert_eq!(summary["fields"][0]["value"], "KEY-7", "{summary}");
    // (D-467) `{request.host}` carries the registration's base path, and the link is kept although
    // the instance host is in no `hosts` entry of the snapshot (the request's own host is accepted).
    // `{request.host}` is the host without the port.
    assert_eq!(
        summary["link"], "https://localhost/jira/browse/KEY-7",
        "{summary}"
    );
    // The approval's own display keeps the full path.
    assert_eq!(path, "/jira/rest/api/2/issue/KEY-7/comment");
    decide(&env.pool, id, "denied", env.w.ws).await;
    let out = t.await.unwrap();
    assert!(out.contains("manual_approval_denied"), "{out}");
}

/// The rows of a world where the agent's access has been moved by **Always allow**: the asked
/// tool's derived rule now allows without asking (`grants::always_allow_tool_in_tx` rewrites the
/// grant, and the compiler then emits this shape). `org_block` adds an organization rule that
/// blocks the same request.
fn always_allowed_rows(w: &World, org_block: bool) -> Rows {
    let mut r = demo_rows(w);
    let cid = r.conns[0].id;
    r.ws.derived = vec![(
        Uuid::now_v7(),
        json!({"name":"d1","enabled":true,"identities":[],"action":"allow",
               "targets":[{"type":"connection","id":cid,"tools":["read_item","write_item"]}]}),
    )];
    if org_block {
        r.org.rules.push(aurora_db::repo::gateway::PolicyRuleRow {
            logical_id: Uuid::now_v7(),
            name: "no-writes-anywhere".into(),
            enabled: true,
            identities: json!([]),
            targets: json!([{"type":"network","host":"127.0.0.1","path":"/items"}]),
            action: "block".into(),
            require_approval: false,
            approval_timeout_secs: None,
            rate_limit: None,
            conditions: json!([]),
        });
    }
    r
}

/// (P5-UX-16, SEC-131, D-348) Always allow writes the agent's own change of access and never
/// outranks a block from above: once the tool is allowed without asking, a request an organization
/// rule blocks is still refused (`blocked_by_policy`) and nothing reaches the upstream, while the
/// same world without that rule lets the request through without a hold.
#[tokio::test]
async fn an_organization_block_rule_still_refuses_after_always_allow() {
    // Control: the access after Always allow lets the write through, no approval is created.
    let db = TestDb::new().await;
    let env = start_with(db.pool().clone(), snapshot(), |w| {
        always_allowed_rows(w, false)
    })
    .await;
    let r = send(&env, "POST", "/items", "{\"a\":1}").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("authorization: Bearer T0K"), "{}", r.body);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "an always-allowed tool asks no more");

    // The same access under an organization block rule: refused, no hold, nothing sent upstream.
    let db = TestDb::new().await;
    let env = start_with(db.pool().clone(), snapshot(), |w| {
        always_allowed_rows(w, true)
    })
    .await;
    let r = send(&env, "POST", "/items", "{\"a\":1}").await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert!(r.body.contains("blocked_by_policy"), "{}", r.body);
    assert!(!r.body.contains("T0K"), "{}", r.body);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "a blocked request never creates an approval");
}

/// The demo catalogue with `write_item` marked `approval: ask` by its package (P5-TOOLASK, C-1042).
fn snapshot_package_ask() -> Value {
    let mut s = snapshot();
    s["apps"][0]["tools"]["write"]["items"][0]["approval"] = json!("ask");
    s
}

/// The workspace has no rule about `write_item`: the reads are allowed and the Default Rule
/// allows the rest (so only the package default can hold the write).
fn package_default_rows(w: &World) -> Rows {
    let mut r = demo_rows(w);
    let cid = r.conns[0].id;
    r.ws.default_action = "allow".into();
    r.ws.derived = vec![(
        Uuid::now_v7(),
        json!({"name":"d1","enabled":true,"identities":[],"action":"allow",
               "targets":[{"type":"connection","id":cid,"tools":["read_item"]}]}),
    )];
    r
}

async fn approvals(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM approvals")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// (P5-TOOLASK, D-397, C-1043) A tool whose package default is Ask is held when no rule speaks:
/// the card's reason is the app, the request proceeds after approval and is refused after a denial;
/// a read tool of the same app is untouched.
#[tokio::test]
async fn a_package_default_ask_holds_the_write_and_names_the_app_as_the_reason() {
    let db = TestDb::new().await;
    let env = Arc::new(
        start_with(
            db.pool().clone(),
            snapshot_package_ask(),
            package_default_rows,
        )
        .await,
    );
    assert_eq!(send(&env, "GET", "/items", "").await.status, 200);
    assert_eq!(approvals(&env.pool).await, 0);

    let e2 = env.clone();
    let t = tokio::spawn(async move { send(&e2, "POST", "/items", "{\"a\":1}").await });
    let id = pending(&env.pool, env.w.ws).await;
    let (tool, rule_name, rule_id): (Option<String>, Option<String>, Option<Uuid>) =
        sqlx::query_as("SELECT tool_id, rule_name, rule_logical_id FROM approvals WHERE id=$1")
            .bind(id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(tool.as_deref(), Some("write_item"));
    assert_eq!(
        rule_name.as_deref(),
        Some(aurora_policy::eval::PACKAGE_ASK_REASON)
    );
    assert!(rule_id.is_none(), "no rule decided");
    decide(&env.pool, id, "approved", env.w.ws).await;
    let r = t.await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("authorization: Bearer T0K"), "{}", r.body);

    let e2 = env.clone();
    let t = tokio::spawn(async move { send(&e2, "POST", "/items", "{}").await });
    let id2 = pending(&env.pool, env.w.ws).await;
    assert_ne!(id, id2);
    decide(&env.pool, id2, "denied", env.w.ws).await;
    let r = t.await.unwrap();
    assert_eq!(r.status, 403, "{}", r.body);
    assert!(r.body.contains("manual_approval_denied"), "{}", r.body);
}

/// (C-1043) An explicit Allow of the tool by the workspace wins over the package default; an
/// explicit Block of an organization rule wins over everything; a tool without a package default
/// is unchanged in the same world.
#[tokio::test]
async fn an_explicit_allow_lifts_the_package_default_and_a_block_wins_over_it() {
    // Explicit workspace allow (the shape Always allow and Act grants compile to): no hold.
    let db = TestDb::new().await;
    let env = start_with(db.pool().clone(), snapshot_package_ask(), |w| {
        always_allowed_rows(w, false)
    })
    .await;
    let r = send(&env, "POST", "/items", "{\"a\":1}").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("authorization: Bearer T0K"), "{}", r.body);
    assert_eq!(
        approvals(&env.pool).await,
        0,
        "an explicit allow asks no one"
    );

    // The same allow under an organization block: refused, no hold, nothing sent upstream.
    let db = TestDb::new().await;
    let env = start_with(db.pool().clone(), snapshot_package_ask(), |w| {
        always_allowed_rows(w, true)
    })
    .await;
    let r = send(&env, "POST", "/items", "{\"a\":1}").await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert!(r.body.contains("blocked_by_policy"), "{}", r.body);
    assert!(!r.body.contains("T0K"), "{}", r.body);
    assert_eq!(approvals(&env.pool).await, 0);

    // No package default: the same rule-less world lets the write through, as before.
    let db = TestDb::new().await;
    let env = start_with(db.pool().clone(), snapshot(), package_default_rows).await;
    let r = send(&env, "POST", "/items", "{\"a\":1}").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(approvals(&env.pool).await, 0, "no package default, no hold");
}
