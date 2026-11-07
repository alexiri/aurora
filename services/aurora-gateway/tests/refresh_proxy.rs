#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! GW-011: proxy-level token freshness: refresh before use, stale retry, reauth, context.
#![allow(dead_code)]

mod common;
use async_trait::async_trait;
use aurora_config::SecretsSection;
use aurora_db::repo::gateway::{AppConnectionRow, PolicyRuleRow, ScopePolicyRows, SecretMetaRow};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca;
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::guard::Guard;
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::refresh::TokenManager;
use aurora_gateway::reqlog::{LogWriter, RequestLog};
use aurora_gateway::resolve::{PolicySource, Resolver};
use aurora_gateway::snapshot::{SnapshotStore, parse};
use aurora_secrets::{Purpose, SealContext};
use aurora_secrets::{SecretRef, SecretVault, StoreRegistry};
use aurora_testkit::TestDb;
use axum::Router;
use axum::extract::State;
use axum::routing::post;
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
            principals: vec![w.agent],
        }))
    }
}

#[derive(Default)]
struct Rows {
    ws: ScopePolicyRows,
    secrets: Vec<SecretMetaRow>,
    conns: Vec<AppConnectionRow>,
    /// `(restricted, available)`
    availability: (bool, Vec<String>),
}

struct Src(Mutex<Rows>);

#[async_trait]
impl PolicySource for Src {
    async fn scope_policy(&self, _o: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        Ok(if ws.is_some() {
            self.0.lock().unwrap().ws.clone()
        } else {
            ScopePolicyRows::default()
        })
    }
    async fn secrets(&self, _o: Uuid, _w: Uuid) -> Result<Vec<SecretMetaRow>, String> {
        Ok(self.0.lock().unwrap().secrets.clone())
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
            .0
            .lock()
            .unwrap()
            .conns
            .iter()
            .filter(|c| app_ids.contains(&c.app_id))
            .cloned()
            .collect())
    }
    async fn app_availability(&self, _o: Uuid, _w: Uuid) -> Result<(bool, Vec<String>), String> {
        Ok(self.0.lock().unwrap().availability.clone())
    }
}

struct Null;

#[async_trait]
impl LogWriter for Null {
    async fn write(&self, _rows: &[aurora_db::repo::gateway::RequestLogRow]) -> Result<(), String> {
        Ok(())
    }
}

struct Env {
    proxy: SocketAddr,
    up: SocketAddr,
}

async fn upstream(
    hits: Arc<AtomicUsize>,
    req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    hits.fetch_add(1, Ordering::SeqCst);
    let auth = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let ext = req
        .headers()
        .get("x-ext")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let _ = req.into_body().collect().await;
    let ok = auth == "Bearer a1";
    let mut r = Response::new(Full::new(Bytes::from(if ok {
        format!("auth={auth} ext={ext}")
    } else {
        format!("INVALID_SESSION_ID auth={auth}")
    })));
    if !ok {
        *r.status_mut() = http::StatusCode::UNAUTHORIZED;
    }
    Ok(r)
}
fn app_json(id: &str, tools: Value, optional_tok: bool) -> Value {
    json!({
        "app_id": id, "package_id": "pkg", "about": {"id": id, "name": id},
        "credentials": [{"name": "tok", "secret": true, "optional": optional_tok},
                        {"name": "access_token", "secret": true, "optional": true}],
        "metadata": [{"name": "sub"}], "connect": [], "token": null,
        "tools": tools, "approvals": {}, "agent": null, "database": null
    })
}

fn host_json(app: &str, prefix: Option<&str>, extra: Value) -> Value {
    let mut h = json!({
        "host": "127.0.0.1", "path_prefix": prefix, "app_id": app, "claim_status": "active",
        "tenant": null, "rewrite_host": null, "inject": [], "finalize": null,
        "transform": null, "intercept": null, "passthrough": []
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        h[k] = v;
    }
    h
}

fn snapshot(apps: Vec<Value>, hosts: Vec<Value>) -> Value {
    json!({
        "format": 1, "contract": 1,
        "packages": [{"package_id": "pkg", "version": "1.0.0", "digest": "d", "publisher": "p",
                      "hooks_digest": null, "egress": {}, "context": ["org_id"]}],
        "oauth_clients": [], "apps": apps, "hosts": hosts, "reserved_hosts": []
    })
}

fn conn(w: &World, app: &str, path: &str, tenant: Option<&str>) -> AppConnectionRow {
    AppConnectionRow {
        id: Uuid::now_v7(),
        workspace_id: Some(w.ws),
        app_id: app.into(),
        label: format!("{app}-{path}"),
        status: "active".into(),
        credentials_ref: SecretRef::ext("mem", path, None, "1").unwrap().to_column(),
        credentials_version: 1,
        credentials_expires_at: None,
        metadata: json!({"sub": "tenant1", "region": "eu-west-1"}),
        tenant_host: tenant.map(str::to_owned),
    }
}

fn rule(name: &str, action: &str, targets: Value, approval: bool) -> PolicyRuleRow {
    PolicyRuleRow {
        logical_id: Uuid::now_v7(),
        name: name.into(),
        enabled: true,
        identities: json!([]),
        targets,
        action: action.into(),
        require_approval: approval,
        rate_limit: None,
        conditions: json!([]),
    }
}

#[derive(Default)]
struct Provider {
    calls: usize,
    current: String,
}

async fn token_endpoint(
    State(p): State<Arc<Mutex<Provider>>>,
    body: String,
) -> (http::StatusCode, String) {
    let mut p = p.lock().unwrap();
    p.calls += 1;
    if !body.contains(&format!("refresh_token={}", p.current)) {
        return (
            http::StatusCode::BAD_REQUEST,
            json!({"error": "invalid_grant"}).to_string(),
        );
    }
    p.current = format!("r{}", p.calls);
    (
        http::StatusCode::OK,
        json!({"access_token": format!("a{}", p.calls), "refresh_token": p.current, "expires_in": 3600}).to_string(),
    )
}

struct Rig {
    env: Env,
    db: TestDb,
    prov: Arc<Mutex<Provider>>,
    hits: Arc<AtomicUsize>,
}

/// `status`, `expires_in_secs` (negative = already expired).
async fn start(status: &str, expires_in_secs: i64) -> Rig {
    let db = TestDb::new().await;
    let p = db.pool();
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
    let (client, cid) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o','O','ext-42')",
    )
    .bind(w.org)
    .execute(p)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'w','W')")
        .bind(w.ws)
        .bind(w.org)
        .execute(p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'a','A','byo')").bind(w.agent).bind(w.org).bind(w.ws).execute(p).await.unwrap();
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{key}"), "k1").unwrap());
    let reg = StoreRegistry::from_config(&SecretsSection::default(), keyring).unwrap();
    let vault = Arc::new(SecretVault::new(reg));
    let cref = vault
        .seal(
            &SealContext {
                purpose: Purpose::OauthClientSecret,
                owner_id: client,
                org_id: w.org,
                workspace_id: None,
            },
            aurora_core::Secret::from(json!({"client_secret": "shh"}).to_string().into_bytes()),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_oauth_clients (id, org_id, package_id, client_key, fields, secret_ref) VALUES ($1,$2,'pkg','g',$3,$4)")
        .bind(client).bind(w.org).bind(json!({"client_id": "cid"})).bind(cref.to_column()).execute(p).await.unwrap();
    let r = vault
        .seal(
            &SealContext {
                purpose: Purpose::ConnectionCredentials,
                owner_id: cid,
                org_id: w.org,
                workspace_id: Some(w.ws),
            },
            aurora_core::Secret::from(
                json!({"access_token": "a0", "refresh_token": "r0"})
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, credentials_expires_at, oauth_client_id) VALUES ($1,$2,$3,'demo','pkg','1','L',$4,$5, now() + make_interval(secs => $6), $7)")
        .bind(cid).bind(w.org).bind(w.ws).bind(status).bind(r.to_column()).bind(expires_in_secs as f64).bind(client).execute(p).await.unwrap();
    let row = AppConnectionRow {
        id: cid,
        workspace_id: Some(w.ws),
        app_id: "demo".into(),
        label: "L".into(),
        status: status.into(),
        credentials_ref: r.to_column(),
        credentials_version: 1,
        credentials_expires_at: Some(
            chrono::Utc::now() + chrono::Duration::seconds(expires_in_secs),
        ),
        metadata: json!({}),
        tenant_host: None,
    };
    let prov = Arc::new(Mutex::new(Provider {
        calls: 0,
        current: "r0".into(),
    }));
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let paddr = l.local_addr().unwrap();
    let app = Router::new()
        .route("/token", post(token_endpoint))
        .with_state(prov.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let snap = json!({
        "format": 1, "contract": 1,
        "packages": [{"package_id": "pkg", "version": "1", "digest": "dg", "publisher": "p",
                      "hooks_digest": null, "egress": {}, "context": ["org_external_id"]}],
        "oauth_clients": [{"package_id": "pkg", "client_key": "g", "authorize_url": "https://x/a",
            "token_url": format!("http://{paddr}/token"), "pkce": false, "client_auth": "body",
            "body_format": "form", "fields": []}],
        "apps": [{"app_id": "demo", "package_id": "pkg", "about": {"id": "demo", "name": "demo"},
            "credentials": [], "metadata": [], "connect": [],
            "token": {"kind": "oauth2_refresh", "client": "g", "refresh_field": "refresh_token",
                      "rotate_refresh_token": true,
                      "stale_codes": [{"status": 401, "body_contains": "INVALID_SESSION_ID"}]},
            "tools": null, "approvals": {}, "agent": null, "database": null}],
        "hosts": [host_json("demo", None, json!({"inject": [
            {"bearer": "{credentials.access_token}"},
            {"header": {"name": "x-ext", "value": "{context.org_external_id}"}}]}))],
        "reserved_hosts": []
    });
    let hits = Arc::new(AtomicUsize::new(0));
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap();
    let h2 = hits.clone();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let h = h2.clone();
            tokio::spawn(async move {
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(s), service_fn(move |r| upstream(h.clone(), r)))
                    .await;
            });
        }
    });
    let store = SnapshotStore::new();
    store.install(parse("h1", snap).unwrap());
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&aurora_config::LlmSection {
            enabled_providers: vec![],
            provider_preference: vec![],
            endpoints: vec![],
        })
        .unwrap(),
    );
    let mut rows = Rows::default();
    rows.ws.rules.push(allow_conn(&row, None));
    rows.conns.push(row);
    let src = Arc::new(Src(Mutex::new(rows)));
    let resolver = Arc::new(Resolver::new(src, llm).with_snapshots(store));
    let tokens = Arc::new(TokenManager::new(p.clone(), vault.clone(), None));
    let plaintext = Arc::new(PlaintextCache::new(vault));
    let handler = Arc::new(
        PolicyHandler::new(PolicyHandlerParts {
            resolver,
            plaintext,
            log: RequestLog::new(Arc::new(Null)),
            external_url: "https://aurora.test/".into(),
            holds: None,
            touch_pool: Some(p.clone()),
        })
        .with_tokens(tokens),
    );
    let gw = ca::generate("gw").unwrap();
    let pl = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = pl.local_addr().unwrap();
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![proxy]).unwrap());
    let (gw_ca, gw_tls) = common::gateway_tls(&gw, proxy);
    let px = Proxy::new(ProxyParts {
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
    tokio::spawn(px.run(pl, std::future::pending()));
    Rig {
        env: Env { proxy, up },
        db,
        prov,
        hits,
    }
}
struct Reply {
    status: u16,
    head: String,
    body: String,
}

impl Reply {
    fn code(&self) -> String {
        serde_json::from_str::<Value>(&self.body).map_or(String::new(), |v| {
            v["error"]["code"].as_str().unwrap_or("").to_owned()
        })
    }
}

async fn send(env: &Env, method: &str, path: &str, extra: &str, body: &str) -> Reply {
    let mut s = common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"));
    let req = format!(
        "{method} http://{up}{path} HTTP/1.1\r\nHost: {up}\r\nProxy-Authorization: Basic {auth}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
        up = env.up
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

async fn get(env: &Env, path: &str, extra: &str) -> Reply {
    send(env, "GET", path, extra, "").await
}

fn allow_conn(c: &AppConnectionRow, tools: Option<Vec<&str>>) -> PolicyRuleRow {
    rule(
        "grant",
        "allow",
        json!([{"type": "connection", "id": c.id, "tools": tools}]),
        false,
    )
}

fn one_app(inject: Value) -> Value {
    snapshot(
        vec![app_json("demo", json!(null), false)],
        vec![host_json("demo", None, json!({"inject": inject}))],
    )
}

fn granted(app_conn: AppConnectionRow) -> impl FnOnce(&World) -> Rows {
    move |_| {
        let mut r = Rows::default();
        r.ws.rules.push(allow_conn(&app_conn, None));
        r.conns.push(app_conn);
        r
    }
}

#[tokio::test]
async fn refreshes_before_use_and_substitutes_context() {
    let r = start("active", -5).await;
    let a = get(&r.env, "/x", "").await;
    assert_eq!(a.status, 200, "{}", a.body);
    assert!(a.body.contains("auth=Bearer a1 ext=ext-42"), "{}", a.body);
    assert_eq!(r.prov.lock().unwrap().calls, 1);
    assert_eq!(r.hits.load(Ordering::SeqCst), 1);
    let _keep = &r.db;
}

#[tokio::test]
async fn stale_code_refreshes_and_resends_bodyless_once() {
    let r = start("active", 3600).await;
    let a = get(&r.env, "/x", "").await;
    assert_eq!(a.status, 200, "{}", a.body);
    assert!(a.body.contains("auth=Bearer a1"), "{}", a.body);
    assert_eq!(r.prov.lock().unwrap().calls, 1);
    assert_eq!(r.hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stale_code_with_body_is_not_resent() {
    let r = start("active", 3600).await;
    let a = send(&r.env, "POST", "/x", "", "payload").await;
    assert_eq!(a.status, 401);
    assert!(a.body.contains("INVALID_SESSION_ID"), "{}", a.body);
    assert_eq!(r.prov.lock().unwrap().calls, 0);
    assert_eq!(r.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reauth_required_connection_is_refused_with_guidance() {
    let r = start("reauth_required", 3600).await;
    let a = get(&r.env, "/x", "").await;
    assert_eq!(a.status, 403);
    assert_eq!(a.code(), "connection_reauth_required");
    assert!(a.body.contains("connect_url"), "{}", a.body);
    assert_eq!(r.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_grant_during_refresh_answers_403_reauth() {
    let r = start("active", -5).await;
    r.prov.lock().unwrap().current = "zzz".into();
    let a = get(&r.env, "/x", "").await;
    assert_eq!(a.status, 403);
    assert_eq!(a.code(), "connection_reauth_required");
    assert_eq!(r.hits.load(Ordering::SeqCst), 0);
}
