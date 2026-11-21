#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! GW-010: registry snapshot, app candidates, identification, mechanisms.

use async_trait::async_trait;
use aurora_config::SecretsSection;
use aurora_db::repo::gateway::{
    AppConnectionRow, InstanceHostRow, PolicyRuleRow, ScopePolicyRows, SecretMetaRow,
};
use aurora_gateway::auth::{CredentialKind, CredentialResolver, RequestIdentity, ResolveError};
use aurora_gateway::ca;
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::mechanisms::finalize::{SignParams, UNSIGNED_PAYLOAD, sign};
use aurora_gateway::plaintext::PlaintextCache;

use aurora_gateway::reqlog::{LogWriter, RequestLog};
use aurora_gateway::resolve::{PolicySource, Resolver};
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
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;

const TOKEN: &str = "aug_good";

// (D-251) The LDAP lane's pipeline tests share this file's harness.
#[path = "ldap_lane.rs"]
mod ldap_lane;
// (P5-ICS, D-273) So do the secret-link lane's.
#[path = "link_lane.rs"]
mod link_lane;

// (P5-WEBHOOK-2, D-320) The outgoing-webhook send path of the same lane.
#[path = "webhook_send.rs"]
mod webhook_send;

#[path = "account_health.rs"]
mod account_health;
// (P5-CALDAV, D-275) The person-pinned server's injection tests share this file's harness.
#[path = "own_server.rs"]
mod own_server;

// (FIX-166, D-368) Request-body framing on the upstream hop, seen by a frame-level oracle.
#[path = "wire_framing.rs"]
mod wire_framing;

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
        if token != TOKEN && token != EVAL_CTX {
            return Ok(None);
        }
        let w = self.0;
        let eval = token == EVAL_CTX;
        Ok(Some(RequestIdentity {
            credential_id: Uuid::nil(),
            kind: if eval {
                CredentialKind::Context
            } else {
                CredentialKind::Agent
            },
            agent_id: w.agent,
            workspace_id: w.ws,
            org_id: w.org,
            conversation_id: None,
            turn_id: None,
            trace_context: None,
            context_label: None,
            speaker_user_id: None,
            speaker_group_ids: Vec::new(),
            capabilities: if eval {
                vec![aurora_gateway::auth::CAP_EVAL_READ_ONLY.into()]
            } else {
                vec![]
            },
            byo: None,
        }))
    }
}

/// (P4-EVAL-2) the `auc_` credential of a turn in an eval conversation.
const EVAL_CTX: &str = "auc_eval";

#[derive(Default)]
struct Rows {
    ws: ScopePolicyRows,
    /// The organization's rules (default: none).
    org: ScopePolicyRows,
    secrets: Vec<SecretMetaRow>,
    conns: Vec<AppConnectionRow>,
    /// `(restricted, available)`
    availability: (bool, Vec<String>),
    /// Active app instances visible to the org (D-74).
    instances: Vec<InstanceHostRow>,
    /// (P5-WEBHOOK-2) Per connection: the stored webhook configuration and the org's
    /// `settings.outgoing_webhooks`.
    wh: std::collections::HashMap<Uuid, (Value, Option<Value>)>,
}

struct Src(Mutex<Rows>);

#[async_trait]
impl PolicySource for Src {
    async fn scope_policy(&self, _o: Uuid, ws: Option<Uuid>) -> Result<ScopePolicyRows, String> {
        Ok(if ws.is_some() {
            self.0.lock().unwrap().ws.clone()
        } else {
            self.0.lock().unwrap().org.clone()
        })
    }
    async fn webhook_config(
        &self,
        _o: Uuid,
        id: Uuid,
    ) -> Result<(Option<Value>, Option<Value>), String> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .wh
            .get(&id)
            .map_or((None, None), |(c, o)| (Some(c.clone()), o.clone())))
    }
    async fn connection(
        &self,
        _o: Uuid,
        _w: Uuid,
        id: Uuid,
    ) -> Result<Option<AppConnectionRow>, String> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .conns
            .iter()
            .find(|c| c.id == id)
            .cloned())
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
    async fn instances(&self, _o: Uuid) -> Result<Vec<InstanceHostRow>, String> {
        Ok(self.0.lock().unwrap().instances.clone())
    }
}

/// Keeps every request-log row (the secret-link tests read them).
struct Capture(Arc<Mutex<Vec<aurora_db::repo::gateway::RequestLogRow>>>);

#[async_trait]
impl LogWriter for Capture {
    async fn write(&self, rows: &[aurora_db::repo::gateway::RequestLogRow]) -> Result<(), String> {
        self.0.lock().unwrap().extend_from_slice(rows);
        Ok(())
    }
}

struct Env {
    proxy: SocketAddr,
    up: SocketAddr,
    /// The request-log rows written so far (after `log.flush()`).
    rows: Arc<Mutex<Vec<aurora_db::repo::gateway::RequestLogRow>>>,
    log: Arc<RequestLog>,
    /// The handler's secret-link body cache.
    cache: Arc<aurora_gateway::seclink::LinkCache>,
    /// The resolver's policy cache: flushed after a test changes the rows.
    resolve_cache: Arc<aurora_gateway::resolve::ResolveCache>,
    /// With a hold store: the shutdown signal its holds listen to (`send(true)` = SIGTERM).
    shutdown: Option<tokio::sync::watch::Sender<bool>>,
}

async fn upstream(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let (parts, body) = req.into_parts();
    let body = body
        .collect()
        .await
        .map(|b| b.to_bytes())
        .unwrap_or_default();
    let mut out = format!(
        "path={}\nquery={}\nhost={}\n",
        parts.uri.path(),
        parts.uri.query().unwrap_or(""),
        parts
            .headers
            .get("host")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
    );
    // (`x-echo-no-headers`: an upstream that names the path but never reflects a header, as a
    // well-behaved DAV server does.)
    if !parts.headers.contains_key("x-echo-no-headers") {
        for (k, v) in &parts.headers {
            out.push_str(&format!("{}: {}\n", k, v.to_str().unwrap_or("?")));
        }
    }
    out.push_str(&format!("body={}\n", String::from_utf8_lossy(&body)));
    // (FIX-104) The gateway withholds a response that echoes a value it put in the URL; a test
    // that wants to see the whole request asks for it hex-encoded (`x-echo-hex`).
    if parts.headers.contains_key("x-echo-hex") {
        out = out.bytes().map(|b| format!("{b:02x}")).collect();
    }
    let mut r = Response::new(Full::new(Bytes::from(out)));
    if parts.uri.path().ends_with("/401") {
        *r.status_mut() = http::StatusCode::UNAUTHORIZED;
    }
    if parts.uri.path().ends_with("/403") {
        *r.status_mut() = http::StatusCode::FORBIDDEN;
    }
    if parts.uri.path().ends_with("/404") {
        *r.status_mut() = http::StatusCode::NOT_FOUND;
    }
    if parts.uri.path().ends_with("/challenge") {
        *r.status_mut() = http::StatusCode::UNAUTHORIZED;
        r.headers_mut().insert(
            "www-authenticate",
            "Bearer realm=\"https://auth.example.com/token\""
                .parse()
                .unwrap(),
        );
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
        instance_id: None,
        connect_method: None,
        access: "write".into(),
        scopes: Vec::new(),
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
        approval_timeout_secs: None,
        rate_limit: None,
        conditions: json!([]),
    }
}

/// `creds`: `(store path, credentials json)`.
async fn start(
    snap: Value,
    creds: &[(&str, &str)],
    mk: impl FnOnce(&World) -> Rows,
) -> (Env, Arc<Src>, Arc<SnapshotStore>) {
    start_with_ca(snap, creds, mk, None).await
}

/// [`start`] with an extra CA bundle the gateway trusts for its own upstream TLS (the LDAP lane's
/// fake directory server presents a certificate of a private CA).
async fn start_with_ca(
    snap: Value,
    creds: &[(&str, &str)],
    mk: impl FnOnce(&World) -> Rows,
    extra_ca_pem: Option<String>,
) -> (Env, Arc<Src>, Arc<SnapshotStore>) {
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
    start_in(w, snap, creds, mk, extra_ca_pem, None, None, false).await
}

/// [`start_with_ca`] in a given world, with the churn-column pool (`touch_pool`) of a real database
/// (P5-UX-14: account health is written there).
#[allow(clippy::too_many_arguments)]
async fn start_in(
    w: World,
    snap: Value,
    creds: &[(&str, &str)],
    mk: impl FnOnce(&World) -> Rows,
    extra_ca_pem: Option<String>,
    touch_pool: Option<aurora_db::PgPool>,
    account_health: Option<Arc<aurora_gateway::account_health::AccountHealth>>,
    // With a `touch_pool`: also give the handler a hold store on that database (approval holds).
    with_holds: bool,
) -> (Env, Arc<Src>, Arc<SnapshotStore>) {
    let mem = Arc::new(MemoryStore::new("mem"));
    for (p, c) in creds {
        mem.preload(p, None, c.as_bytes());
    }
    mem.allow_reference(w.org, "*");
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{key}"), "k1").unwrap());
    let mut reg = StoreRegistry::from_config(&SecretsSection::default(), keyring).unwrap();
    reg.register(mem);
    let src = Arc::new(Src(Mutex::new(mk(&w))));
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
        aurora_llm::ProviderRegistry::from_config(&aurora_config::LlmSection {
            enabled_providers: vec![],
            provider_preference: vec![],
            dev_upstream_overrides: Default::default(),
            endpoints: vec![],
        })
        .unwrap(),
    );
    let resolver = Arc::new(Resolver::new(src.clone(), llm).with_snapshots(store.clone()));
    let resolve_cache = resolver.cache();
    let plaintext = Arc::new(PlaintextCache::new(Arc::new(SecretVault::new(reg))));
    let rows = Arc::new(Mutex::new(Vec::new()));
    let log = RequestLog::new(Arc::new(Capture(rows.clone())));
    let mut shutdown_tx = None;
    let holds = if let (true, Some(pool)) = (with_holds, touch_pool.as_ref()) {
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        shutdown_tx = Some(shutdown);
        let hub = aurora_gateway::invalidation::Hub::new();
        tokio::spawn(hub.clone().run(pool.clone(), rx.clone()));
        while !hub.is_connected() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Some(Arc::new(aurora_gateway::hold::Holds::new(
            pool.clone(),
            hub,
            Uuid::now_v7(),
            Duration::from_secs(30),
            Some(rx),
        )))
    } else {
        None
    };
    let mut handler = PolicyHandler::new(PolicyHandlerParts {
        resolver,
        plaintext,
        log: log.clone(),
        external_url: "https://aurora.test/".into(),
        holds,
        touch_pool,
    })
    .with_insecure_loopback_injection(true);
    if let Some(h) = account_health {
        handler = handler.with_account_health(h);
    }
    let cache = handler.link_cache();
    let handler = Arc::new(handler);
    let gw = ca::generate("gw").unwrap();
    let (proxy, _) = crate::common::spawn_proxy(
        &gw,
        Arc::new(Static(w)),
        handler,
        extra_ca_pem.map(String::into_bytes),
    )
    .await;
    (
        Env {
            proxy,
            up,
            rows,
            log,
            cache,
            resolve_cache,
            shutdown: shutdown_tx,
        },
        src,
        store,
    )
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
    send_as(env, TOKEN, method, path, extra, body).await
}

async fn send_as(
    env: &Env,
    token: &str,
    method: &str,
    path: &str,
    extra: &str,
    body: &str,
) -> Reply {
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{token}"));
    let req = format!(
        "{method} http://{up}{path} HTTP/1.1\r\nHost: {up}\r\nProxy-Authorization: Basic {auth}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
        up = env.up
    );
    crate::common::write_flushed(&mut s, req.as_bytes()).await;
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
async fn inject_mechanisms_overwrite_and_escape() {
    let inject = json!([
        {"bearer": "{credentials.tok}"},
        {"header": {"name": "X-Key", "value": "k-{credentials.tok}"}},
        {"query": {"name": "api_key", "value": "{credentials.tok}"}},
        {"remove_header": "x-remove"},
        {"path_template": {"prefix": "/bot", "value": "{credentials.tok}"}},
    ]);
    let w0 = World {
        org: Uuid::nil(),
        ws: Uuid::nil(),
        agent: Uuid::nil(),
    };
    let c = conn(&w0, "demo", "c1", None);
    let mut c2 = c.clone();
    c2.workspace_id = None;
    let (env, ..) = start(one_app(inject), &[("c1", r#"{"tok":"a b/&"}"#)], move |w| {
        let mut c = c2;
        c.workspace_id = Some(w.ws);
        granted(c)(w)
    })
    .await;
    let r = get(
        &env,
        "/botX/send?api_key=agent&z=1",
        "Authorization: Bearer agent\r\nX-Key: agent\r\nX-Remove: yes\r\nX-Echo-Hex: 1\r\n",
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = Reply {
        body: String::from_utf8_lossy(
            &(0..r.body.trim().len() / 2)
                .filter_map(|i| u8::from_str_radix(&r.body.trim()[2 * i..2 * i + 2], 16).ok())
                .collect::<Vec<u8>>(),
        )
        .into_owned(),
        ..r
    };
    assert!(r.body.contains("authorization: Bearer a b/&"), "{}", r.body);
    assert!(r.body.contains("x-key: k-a b/&"), "{}", r.body);
    assert!(
        r.body.contains("query=z=1&api_key=a%20b%2F%26"),
        "{}",
        r.body
    );
    assert!(r.body.contains("path=/bota%20b%2F%26/send"), "{}", r.body);
    assert!(!r.body.contains("x-remove"), "{}", r.body);
    assert!(!r.body.contains("agent\n"), "{}", r.body);
}

#[tokio::test]
async fn crlf_and_empty_optional_fail_closed() {
    let snap = snapshot(
        vec![
            app_json("hdr", json!(null), false),
            app_json("opt", json!(null), true),
        ],
        vec![
            host_json(
                "hdr",
                Some("/hdr"),
                json!({"inject": [{"header": {"name": "X-K", "value": "{credentials.tok}"}}]}),
            ),
            host_json(
                "opt",
                Some("/opt"),
                json!({"inject": [{"bearer": "{credentials.tok}"}]}),
            ),
        ],
    );
    let (env, ..) = start(
        snap,
        &[("bad", "{\"tok\":\"a\\r\\nX-Evil: 1\"}"), ("none", "{}")],
        |w| {
            let (a, b) = (conn(w, "hdr", "bad", None), conn(w, "opt", "none", None));
            let mut r = Rows::default();
            r.ws.rules.push(allow_conn(&a, None));
            r.ws.rules.push(allow_conn(&b, None));
            r.conns = vec![a, b];
            r
        },
    )
    .await;
    for p in ["/hdr/x", "/opt/x"] {
        let r = get(&env, p, "").await;
        assert_eq!(r.status, 502, "{}", r.body);
        assert_eq!(r.code(), "credential_unavailable");
    }
}

fn tools_json() -> Value {
    json!({"read": {"items": [
        {"id": "read_item", "name": "r", "host": "127.0.0.1", "path": "/items", "methods": ["GET"]},
        {"id": "gql_q", "name": "q", "host": "127.0.0.1", "path": "/graphql", "methods": ["POST"], "graphql": "query"}
    ]}, "write": {"items": [
        {"id": "write_item", "name": "w", "host": "127.0.0.1", "path": "/items", "methods": ["POST"]},
        {"id": "gql_m", "name": "m", "host": "127.0.0.1", "path": "/graphql", "methods": ["POST"], "graphql": "mutation"}
    ]}})
}

#[tokio::test]
async fn tool_grants_and_graphql() {
    let snap = snapshot(
        vec![app_json("demo", tools_json(), false)],
        vec![host_json(
            "demo",
            None,
            json!({"inject": [{"bearer": "{credentials.tok}"}]}),
        )],
    );
    let (env, ..) = start(snap, &[("c1", r#"{"tok":"T"}"#)], |w| {
        let c = conn(w, "demo", "c1", None);
        let mut r = Rows::default();
        r.ws.default_action = "block".into();
        // Derived-rule shapes: allow read tools, ask for write_item.
        r.ws.derived.push((
            Uuid::now_v7(),
            json!({"name":"d1","enabled":true,"identities":[],"action":"allow",
                   "targets":[{"type":"connection","id":c.id,"tools":["read_item","gql_q"]}]}),
        ));
        r.ws.derived.push((
            Uuid::now_v7(),
            json!({"name":"d2","enabled":true,"identities":[],"action":"allow","require_approval":true,
                   "targets":[{"type":"connection","id":c.id,"tools":["write_item"]}]}),
        ));
        r.conns.push(c);
        r
    })
    .await;
    assert_eq!(get(&env, "/items", "").await.status, 200);
    // ask tool: held (no hold store here => fails closed through the approval path)
    let r = send(&env, "POST", "/items", "", "{}").await;
    assert_eq!(r.code(), "approval_store_unavailable", "{}", r.body);
    // Unlisted path => no tool => default block.
    let r = get(&env, "/other", "").await;
    assert_eq!(r.code(), "blocked_by_default_policy", "{}", r.body);
    // GraphQL query allowed, mutation blocked.
    let q = r#"{"query":"query { a }"}"#;
    let m = r#"{"query":"mutation { a }"}"#;
    assert_eq!(send(&env, "POST", "/graphql", "", q).await.status, 200);
    let r = send(&env, "POST", "/graphql", "", m).await;
    assert_eq!(r.code(), "blocked_by_default_policy", "{}", r.body);
}

#[tokio::test]
async fn precedence_ambiguity_and_choice() {
    let snap = one_app(json!([{"bearer": "{credentials.tok}"}]));
    let (env, src, _) = start(
        snap,
        &[("c1", r#"{"tok":"one"}"#), ("c2", r#"{"tok":"two"}"#)],
        |w| {
            let (a, b) = (conn(w, "demo", "c1", None), conn(w, "demo", "c2", None));
            let mut r = Rows::default();
            r.ws.rules.push(allow_conn(&a, None));
            r.conns = vec![a, b];
            // A generic secret on the same host, also granted.
            let s = SecretMetaRow {
                id: Uuid::now_v7(),
                workspace_id: Some(w.ws),
                name: "gen".into(),
                kind: "generic".into(),
                llm_provider: None,
                llm_auth_mode: None,
                llm_endpoint_id: None,
                host_pattern: "127.0.0.1".into(),
                path_pattern: None,
                injection: json!({"kind": "bearer"}),
                value_ref: SecretRef::ext("mem", "c1", None, "1").unwrap().to_column(),
                value_version: 1,
                origin: None,
            };
            r.ws.rules.push(rule(
                "g",
                "allow",
                json!([{"type":"secret","id":s.id}]),
                false,
            ));
            r.secrets.push(s);
            r
        },
    )
    .await;
    // App connection beats the generic secret on the same host.
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("authorization: Bearer one"), "{}", r.body);
    let id1 = src.0.lock().unwrap().conns[0].id;
    let r = get(&env, "/x", &format!("x-aurora-connection-id: {id1}\r\n")).await;
    assert!(r.body.contains("Bearer one"), "{}", r.body);
    let r = get(
        &env,
        "/x",
        &format!("x-aurora-connection-id: {}\r\n", Uuid::now_v7()),
    )
    .await;
    assert_eq!(r.code(), "connection_not_found");
}

#[tokio::test]
async fn two_granted_connections_conflict() {
    let snap = one_app(json!([{"bearer": "{credentials.tok}"}]));
    let (env, ..) = start(
        snap,
        &[("c1", r#"{"tok":"one"}"#), ("c2", r#"{"tok":"two"}"#)],
        |w| {
            let (a, b) = (conn(w, "demo", "c1", None), conn(w, "demo", "c2", None));
            let mut r = Rows::default();
            r.ws.rules.push(allow_conn(&a, None));
            r.ws.rules.push(allow_conn(&b, None));
            r.conns = vec![a, b];
            r
        },
    )
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 409);
    assert_eq!(r.code(), "multiple_connections");
    let v: Value = serde_json::from_str(&r.body).unwrap();
    let ch = v["error"]["details"]["choices"].as_array().unwrap();
    assert_eq!(ch.len(), 2);
    let pick = ch[1]["id"].as_str().unwrap().to_owned();
    let r = get(&env, "/x", &format!("x-aurora-connection-id: {pick}\r\n")).await;
    assert_eq!(r.status, 200);
    assert!(
        r.head.to_ascii_lowercase().contains("x-aurora-connections"),
        "{}",
        r.head
    );
}

#[tokio::test]
async fn lost_claim_and_unavailable_and_guidance() {
    // Lost claim never injects.
    let mut h = host_json(
        "demo",
        None,
        json!({"inject": [{"bearer": "{credentials.tok}"}]}),
    );
    h["claim_status"] = json!("lost");
    let snap = snapshot(vec![app_json("demo", json!(null), false)], vec![h]);
    let (env, ..) = start(snap, &[("c1", r#"{"tok":"T"}"#)], |w| {
        granted(conn(w, "demo", "c1", None))(w)
    })
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 200);
    assert!(!r.body.contains("authorization"), "{}", r.body);

    // Restricted posture without availability => app_unavailable.
    let (env, ..) = start(one_app(json!([])), &[("c1", r#"{"tok":"T"}"#)], |w| {
        let mut r = granted(conn(w, "demo", "c1", None))(w);
        r.availability = (true, vec![]);
        r
    })
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 403);
    assert_eq!(r.code(), "app_unavailable");

    // (FIX-154, C-897) A credential-free app leads with the agent's Access tab.
    let mut free = app_json("demo", json!(null), false);
    free["credentials"] = json!([]);
    free["metadata"] = json!([]);
    free["connect"] = json!([{"id": "anonymous", "kind": "fields", "fields": []}]);
    let snap = snapshot(vec![free], vec![host_json("demo", Some("/api"), json!({}))]);
    let (env, ..) = start(snap, &[], |_| Rows::default()).await;
    let r = get(&env, "/api/401", "").await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (401, "app_not_connected"),
        "{}",
        r.body
    );
    assert!(
        r.body
            .contains("give this agent access on its Access tab: ")
            && r.body.contains("/agents/")
            && r.body.contains("?tab=access"),
        "{}",
        r.body
    );

    // Not connected / path unserved.
    let snap = snapshot(
        vec![app_json("demo", json!(null), false)],
        vec![host_json("demo", Some("/api"), json!({}))],
    );
    let (env, ..) = start(snap, &[], |_| Rows::default()).await;
    let r = get(&env, "/api/401", "").await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (401, "app_not_connected"),
        "{}",
        r.body
    );
    // (FIX-153, C-893) an app with a sign-in leads with connecting, then giving the agent access.
    // (C-959, P5-UX-17) A refusal that knows the agent carries the journey's deep link as
    // `connect_url` (the agent's Access tab with the journey open) and keeps the workspace link.
    let v: Value = serde_json::from_str(&r.body).unwrap();
    let d = &v["error"]["details"];
    let agent_link = d["connect_url"].as_str().unwrap();
    assert!(
        agent_link.contains("/agents/") && agent_link.ends_with("?tab=access&connect=demo"),
        "{}",
        r.body
    );
    assert!(
        d["connect_url_workspace"]
            .as_str()
            .unwrap()
            .ends_with("/apps?tab=apps&connect=demo"),
        "{}",
        r.body
    );
    assert!(
        r.body
            .contains("?tab=access&connect=demo, then give this agent access"),
        "{}",
        r.body
    );
    assert!(r.body.contains("access_url"), "{}", r.body);
    let r = get(&env, "/other/401", "").await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (401, "app_path_unserved"),
        "{}",
        r.body
    );

    // Disabled account.
    let (env, ..) = start(
        snapshot(
            vec![app_json("demo", json!(null), false)],
            vec![host_json("demo", None, json!({}))],
        ),
        &[],
        |w| {
            let mut c = conn(w, "demo", "c1", None);
            c.status = "disabled".into();
            let mut r = Rows::default();
            r.conns.push(c);
            r
        },
    )
    .await;
    let r = get(&env, "/401", "").await;
    assert_eq!(r.code(), "access_restricted");
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["error"]["details"]["reason"], "disabled");
}

#[tokio::test]
async fn tenant_guard_421_and_rewrite_zone() {
    let tenant = json!({"field": "connection.tenant_host", "zone": "*.example.com"});
    let snap = snapshot(
        vec![app_json("demo", json!(null), false)],
        vec![host_json(
            "demo",
            None,
            json!({"tenant": tenant, "inject": [{"bearer": "{credentials.tok}"}]}),
        )],
    );
    let (env, ..) = start(snap, &[("c1", r#"{"tok":"T"}"#)], |w| {
        granted(conn(w, "demo", "c1", Some("acme.example.com")))(w)
    })
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(r.status, 421, "{}", r.body);
    assert_eq!(r.code(), "host_mismatch");

    // rewrite_host outside the zone (metadata under agent/admin control) is refused.
    let rw = json!({"template": "{metadata.sub}.evil.org", "zone": "*.example.com"});
    let snap = snapshot(
        vec![app_json("demo", json!(null), false)],
        vec![host_json("demo", None, json!({"rewrite_host": rw}))],
    );
    let (env, ..) = start(snap, &[("c1", r#"{"tok":"T"}"#)], |w| {
        granted(conn(w, "demo", "c1", None))(w)
    })
    .await;
    let r = get(&env, "/x", "").await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (502, "credential_unavailable"),
        "{}",
        r.body
    );
}

#[tokio::test]
async fn sigv4_trailer_intercept_and_challenge() {
    let finalize = json!({"aws_sigv4": {
        "region": "{metadata.region}", "service": "svc",
        "access_key": "AKIDEXAMPLE", "secret_key": "{credentials.tok}"}});
    let snap = snapshot(
        vec![
            app_json("demo", json!(null), false),
            app_json("tr", json!(null), false),
            app_json("ic", json!(null), false),
        ],
        vec![
            host_json("demo", Some("/aws"), json!({"finalize": finalize})),
            host_json(
                "tr",
                Some("/git"),
                json!({"transform": {"git_commit_trailer": {"trailer": "Co-Authored-By: Agent"}}}),
            ),
            host_json(
                "ic",
                Some("/oauth"),
                json!({
                "intercept": {"oauth_token_endpoint": {"path": "/oauth/token", "method": "POST"}},
                "passthrough": ["registry_token_challenge"]}),
            ),
        ],
    );
    let (env, ..) = start(
        snap,
        &[
            ("a", r#"{"tok":"SECRETKEY"}"#),
            ("t", r#"{"tok":"x"}"#),
            ("i", r#"{"access_token":"ACCESS"}"#),
        ],
        |w| {
            let cs = [
                conn(w, "demo", "a", None),
                conn(w, "tr", "t", None),
                conn(w, "ic", "i", None),
            ];
            let mut r = Rows::default();
            for c in &cs {
                r.ws.rules.push(allow_conn(c, None));
            }
            r.conns = cs.to_vec();
            r
        },
    )
    .await;
    // SigV4: recompute from the headers the upstream saw.
    let r = get(&env, "/aws/x?b=2&a=1", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let hdr = |n: &str| {
        r.body
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{n}: ")))
            .unwrap()
            .to_owned()
    };
    assert_eq!(hdr("x-amz-content-sha256"), UNSIGNED_PAYLOAD);
    let host = r
        .body
        .lines()
        .find_map(|l| l.strip_prefix("host="))
        .unwrap();
    let expect = sign(&SignParams {
        method: &http::Method::GET,
        host_header: host,
        path: "/aws/x",
        query: Some("b=2&a=1"),
        region: "eu-west-1",
        service: "svc",
        access_key: "AKIDEXAMPLE",
        secret_key: "SECRETKEY",
        session_token: None,
        amz_date: &hdr("x-amz-date"),
        payload_hash: UNSIGNED_PAYLOAD,
        sign_content_sha256: true,
    });
    assert_eq!(hdr("authorization"), expect.authorization);

    // git_commit_trailer appends once.
    let body = r#"{"message":"fix","tree":"t"}"#;
    let r = send(
        &env,
        "POST",
        "/git/commits",
        "Content-Type: application/json\r\n",
        body,
    )
    .await;
    assert!(
        r.body
            .contains(r#""message":"fix\n\nCo-Authored-By: Agent""#),
        "{}",
        r.body
    );
    let body2 = r#"{"message":"fix\n\nCo-Authored-By: Agent"}"#;
    let r = send(
        &env,
        "POST",
        "/git/commits",
        "Content-Type: application/json\r\n",
        body2,
    )
    .await;
    assert_eq!(r.body.matches("Co-Authored-By").count(), 1, "{}", r.body);

    // oauth_token_endpoint answered locally with the stored token.
    let r = send(&env, "POST", "/oauth/token", "", "grant_type=refresh_token").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(
        (v["access_token"].as_str(), v["token_type"].as_str()),
        (Some("ACCESS"), Some("Bearer"))
    );

    // registry_token_challenge: upstream 401 Bearer challenge passes through.
    let r = get(&env, "/oauth/challenge", "").await;
    assert_eq!(r.status, 401);
    // (injected => untouched anyway); anonymous flow checked below
    assert!(r.body.contains("path=/oauth/challenge"), "{}", r.body);
}

#[tokio::test]
async fn registry_token_challenge_anonymous() {
    let snap = snapshot(
        vec![app_json("ic", json!(null), false)],
        vec![host_json(
            "ic",
            None,
            json!({"passthrough": ["registry_token_challenge"]}),
        )],
    );
    let (env, ..) = start(snap, &[], |_| Rows::default()).await;
    let r = get(&env, "/challenge", "").await;
    assert_eq!(r.status, 401);
    assert!(
        r.head
            .to_ascii_lowercase()
            .contains("www-authenticate: bearer"),
        "{}",
        r.head
    );
    assert_eq!(r.code(), "");
}

// ---- snapshot store (database) ----

async fn put_snapshot(db: &aurora_db::PgPool, hash: &str, at: &str, body: &Value) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO registry_snapshots (id, contract, content_hash, snapshot, compiled_by, activated_at) \
         VALUES ($1, 1, $2, $3, 'test', {at})"
    )))
    .bind(Uuid::now_v7())
    .bind(hash)
    .bind(body)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn snapshot_loads_refuses_unknown_and_swaps_on_notify() {
    let db = aurora_testkit::TestDb::new().await;
    let pool = db.pool();
    let store = SnapshotStore::new();
    assert!(!store.is_loaded());
    assert!(!store.refresh(pool).await.unwrap());
    assert!(!store.is_loaded());
    put_snapshot(
        pool,
        "h-a",
        "now() - interval '2 minutes'",
        &one_app(json!([])),
    )
    .await;
    assert!(store.refresh(pool).await.unwrap());
    assert_eq!(&*store.current().unwrap().hash, "h-a");

    // Newer snapshot naming an unknown mechanism: refused, previous kept.
    let bad = one_app(json!([{"frobnicate": "x"}]));
    put_snapshot(pool, "h-bad", "now() - interval '1 minute'", &bad).await;
    assert!(store.refresh(pool).await.is_err());
    assert_eq!(&*store.current().unwrap().hash, "h-a");

    // A later valid snapshot swaps in via NOTIFY without restart.
    let (tx, rx) = tokio::sync::watch::channel(false);
    let s2 = store.clone();
    tokio::spawn(s2.run(pool.clone(), rx));
    tokio::time::sleep(Duration::from_millis(300)).await;
    put_snapshot(
        pool,
        "h-b",
        "now()",
        &one_app(json!([{"bearer": "{credentials.tok}"}])),
    )
    .await;
    aurora_db::bus::notify(
        pool,
        aurora_db::bus::Channel::Registry,
        &json!({"hash": "h-b"}),
    )
    .await
    .unwrap();
    for _ in 0..50 {
        if &*store.current().unwrap().hash == "h-b" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(&*store.current().unwrap().hash, "h-b");
    let _ = tx.send(true);
}

/// (FIX-052, D-166, C-633) A snapshot whose `inject` targets `host` is refused
/// at load time, naming the app and host; the previously loaded snapshot stays.
#[tokio::test]
async fn snapshot_load_refuses_host_header_injection() {
    let db = aurora_testkit::TestDb::new().await;
    let pool = db.pool();
    let store = SnapshotStore::new();
    put_snapshot(
        pool,
        "h-ok",
        "now() - interval '2 minutes'",
        &one_app(json!([])),
    )
    .await;
    assert!(store.refresh(pool).await.unwrap());

    // `Host` (any casing) in a manifest inject on the regular host: refused.
    for name in ["Host", "HOST"] {
        let bad = one_app(json!([{"header": {"name": name, "value": "evil"}}]));
        put_snapshot(pool, &format!("h-{name}"), "now()", &bad).await;
        let err = store.refresh(pool).await.unwrap_err().to_string();
        assert!(
            err.contains("host override is not supported")
                && err.contains("app `demo`")
                && err.contains("127.0.0.1"),
            "error must name the app and host: {err}"
        );
        assert_eq!(&*store.current().unwrap().hash, "h-ok", "previous kept");
    }

    // The same refusal for an instance-host inject (`{instance}` entries of an
    // app that declares `instances`).
    let mut bad = one_app(json!([]));
    bad["apps"][0]["instances"] = json!({"label": "Demo", "default_host": "127.0.0.1"});
    let mut ih = bad["hosts"][0].clone();
    ih["host"] = json!("{instance}");
    ih["inject"] = json!([{"header": {"name": "host", "value": "evil"}}]);
    bad["instance_hosts"] = json!([ih]);
    put_snapshot(pool, "h-inst", "now()", &bad).await;
    let err = store.refresh(pool).await.unwrap_err().to_string();
    assert!(
        err.contains("host override is not supported") && err.contains("app `demo`"),
        "instance-host inject refused, naming the app: {err}"
    );
    assert_eq!(&*store.current().unwrap().hash, "h-ok", "previous kept");
}

// ---- D-74 / SEC-04 / SEC-59: self-hosted app instances -------------------------------------
//
// The test upstream listens on 127.0.0.1. The static (default) host of the instance-bound app is
// `127.0.0.1`; the registered instance host is `localhost` (it resolves to the same listener, so
// the same server answers both names and the injected headers show which entry served).

/// GET `http://{host}:{port}{path}` through the proxy.
async fn get_host(env: &Env, host: &str, path: &str) -> Reply {
    send_host(env, "GET", host, path).await
}

/// `{method} http://{host}:{port}{path}` through the proxy, with an empty body.
async fn send_host(env: &Env, method: &str, host: &str, path: &str) -> Reply {
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"));
    let req = format!(
        "{method} http://{host}:{port}{path} HTTP/1.1\r\nHost: {host}:{port}\r\nProxy-Authorization: Basic {auth}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        port = env.up.port()
    );
    s.write_all(req.as_bytes()).await.unwrap();
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
        head: head.to_owned(),
        body: dechunk(body),
    }
}

fn inst_app() -> Value {
    let mut a = app_json("inst", json!(null), false);
    a["instances"] = json!({"label": "Inst", "default_host": "127.0.0.1"});
    a
}

/// `inst` serves its default host `127.0.0.1` (static claim) and any registered instance host.
fn inst_snapshot(extra_apps: Vec<Value>, extra_hosts: Vec<Value>) -> Value {
    let inject = json!({"inject": [{"bearer": "{credentials.tok}"}]});
    let mut apps = vec![inst_app()];
    apps.extend(extra_apps);
    let mut hosts = vec![host_json("inst", None, inject.clone())];
    hosts.extend(extra_hosts);
    let mut s = snapshot(apps, hosts);
    let mut ih = host_json("inst", None, inject);
    ih["host"] = json!("{instance}");
    s["instance_hosts"] = json!([ih]);
    s
}

fn instance_row(id: Uuid, host: &str) -> InstanceHostRow {
    instance_row_at(id, "inst", host, None)
}

/// A registration with a base path (D-231).
fn instance_row_at(id: Uuid, app: &str, host: &str, base: Option<&str>) -> InstanceHostRow {
    InstanceHostRow {
        id,
        app_id: app.into(),
        host: host.into(),
        base_path: base.map(str::to_owned),
    }
}

fn bound(w: &World, path: &str, instance: Option<Uuid>) -> AppConnectionRow {
    let mut c = conn(w, "inst", path, None);
    c.instance_id = instance;
    c
}

fn has_token(r: &Reply, token: &str) -> bool {
    r.body.contains(&format!("authorization: Bearer {token}"))
}

#[tokio::test]
async fn instance_connection_is_injected_on_its_exact_host_only() {
    let inst = Uuid::now_v7();
    let (env, ..) = start(
        inst_snapshot(vec![], vec![]),
        &[("c1", r#"{"tok":"INSTSECRET"}"#)],
        move |w| {
            let mut r = granted(bound(w, "c1", Some(inst)))(w);
            r.instances.push(instance_row(inst, "localhost"));
            r
        },
    )
    .await;
    // The registered instance host: injected.
    let r = get_host(&env, "localhost", "/api/v4/user").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(has_token(&r, "INSTSECRET"), "{}", r.body);
    // The app's static default host: the instance connection is refused like a tenant-bound one,
    // naming the host it is bound to; nothing reaches the upstream.
    let r = get_host(&env, "127.0.0.1", "/api/v4/user").await;
    assert_eq!(r.status, 421, "{}", r.body);
    assert_eq!(r.code(), "host_mismatch");
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["error"]["details"]["expected_host"], "localhost");
    assert!(!r.body.contains("INSTSECRET"));
}

#[tokio::test]
async fn default_host_connection_is_never_injected_on_an_instance_host() {
    let inst = Uuid::now_v7();
    let (env, ..) = start(
        inst_snapshot(vec![], vec![]),
        &[("c1", r#"{"tok":"DEFAULTSECRET"}"#)],
        move |w| {
            let mut r = granted(bound(w, "c1", None))(w);
            r.instances.push(instance_row(inst, "localhost"));
            r
        },
    )
    .await;
    // Served on the default host it was made for ...
    let r = get_host(&env, "127.0.0.1", "/api/v4/user").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(has_token(&r, "DEFAULTSECRET"), "{}", r.body);
    // ... but never sent to a registered instance host.
    let r = get_host(&env, "localhost", "/api/v4/user").await;
    assert!(!r.body.contains("DEFAULTSECRET"), "{}", r.body);
    assert!(!r.head.contains("DEFAULTSECRET"));
}

#[tokio::test]
async fn unregistered_disabled_or_deleted_instance_injects_nothing() {
    let inst = Uuid::now_v7();
    // No instance row is active (never registered, disabled, or deleted): the host is an ordinary
    // unknown host, and the connection that still names the instance is not a candidate anywhere.
    let (env, ..) = start(
        inst_snapshot(vec![], vec![]),
        &[("c1", r#"{"tok":"GONESECRET"}"#)],
        move |w| granted(bound(w, "c1", Some(inst)))(w),
    )
    .await;
    for host in ["localhost", "127.0.0.1"] {
        let r = get_host(&env, host, "/api/v4/user").await;
        assert!(!r.body.contains("GONESECRET"), "{host}: {}", r.body);
        assert_ne!(r.status, 421, "{host}: {}", r.body);
    }
    // The same without any connection: an instance host nobody registered is not served.
    let (env, ..) = start(inst_snapshot(vec![], vec![]), &[], |_| Rows::default()).await;
    let r = get_host(&env, "localhost", "/api/v4/user").await;
    assert!(
        !r.body
            .to_ascii_lowercase()
            .contains("authorization: bearer"),
        "{}",
        r.body
    );
}

#[tokio::test]
async fn another_apps_static_claim_shadows_a_registered_instance_host() {
    let inst = Uuid::now_v7();
    // A package later claims `localhost` statically: the instance never injects there.
    let mut other_host = host_json("other", None, json!({}));
    other_host["host"] = json!("localhost");
    let (env, ..) = start(
        inst_snapshot(
            vec![app_json("other", json!(null), false)],
            vec![other_host],
        ),
        &[("c1", r#"{"tok":"SHADOWSECRET"}"#)],
        move |w| {
            let mut r = granted(bound(w, "c1", Some(inst)))(w);
            r.instances.push(instance_row(inst, "localhost"));
            r
        },
    )
    .await;
    let r = get_host(&env, "localhost", "/api/v4/user").await;
    assert!(!r.body.contains("SHADOWSECRET"), "{}", r.body);
}

#[tokio::test]
async fn instance_connection_of_another_instance_is_refused_with_its_host() {
    let (mine, other) = (Uuid::now_v7(), Uuid::now_v7());
    let (env, ..) = start(
        inst_snapshot(vec![], vec![]),
        &[("c1", r#"{"tok":"OTHERSECRET"}"#)],
        move |w| {
            // The connection belongs to another instance of the same app; this org also has
            // `localhost` registered. A request to `localhost` must not use it.
            let mut r = granted(bound(w, "c1", Some(other)))(w);
            r.instances.push(instance_row(mine, "localhost"));
            r.instances.push(instance_row(other, "other.example.test"));
            r
        },
    )
    .await;
    let r = get_host(&env, "localhost", "/api/v4/user").await;
    assert_eq!(r.status, 421, "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["error"]["details"]["expected_host"], "other.example.test");
    assert!(!r.body.contains("OTHERSECRET"));
}

/// (FIX-009, D-86) A snapshot of an instance-bound OAuth app whose default host is
/// `gitlab.test` and whose token URL is `https://{instance}/oauth/token`.
fn refreshable_instance_snapshot() -> Value {
    let mut a = app_json("inst", json!(null), false);
    a["instances"] = json!({"label": "Inst", "default_host": "gitlab.test"});
    a["token"] = json!({"kind": "oauth2_refresh", "client": "g", "refresh_field": "refresh_token",
                        "rotate_refresh_token": true});
    let inject = json!({"inject": [{"bearer": "{credentials.tok}"}]});
    let mut dh = host_json("inst", None, inject.clone());
    dh["host"] = json!("gitlab.test");
    let mut ih = host_json("inst", None, inject);
    ih["host"] = json!("{instance}");
    let mut s = snapshot(vec![a], vec![dh]);
    s["instance_hosts"] = json!([ih]);
    s["oauth_clients"] = json!([{"package_id": "pkg", "client_key": "g",
        "authorize_url": "https://{instance}/oauth/authorize",
        "token_url": "https://{instance}/oauth/token", "pkce": false, "client_auth": "body",
        "body_format": "form", "fields": []}]);
    s
}

/// What the resolver hands the refresh path for `host`: each connection's `{instance}` host and
/// the token URL a refresh would be sent to.
async fn refresh_targets(
    host: &str,
    rows: impl FnOnce(&World) -> Rows,
) -> Vec<(Option<String>, String)> {
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
    let src = Arc::new(Src(Mutex::new(rows(&w))));
    let store = SnapshotStore::new();
    store.install(parse("h1", refreshable_instance_snapshot()).unwrap());
    let llm = Arc::new(
        aurora_llm::ProviderRegistry::from_config(&aurora_config::LlmSection {
            enabled_providers: vec![],
            provider_preference: vec![],
            dev_upstream_overrides: Default::default(),
            endpoints: vec![],
        })
        .unwrap(),
    );
    let resolver = Resolver::new(src, llm).with_snapshots(store.clone());
    let id = crate::common::identity(CredentialKind::Agent, w.agent, w.ws, w.org);
    let rv = resolver.resolve(&id, host, 443).await.unwrap();
    let loaded = store.current().unwrap();
    let app = loaded.snapshot.app("inst").unwrap();
    let token = app.token.as_ref().unwrap();
    let client = &loaded.snapshot.oauth_clients[0];
    let current = serde_json::Map::from_iter([("refresh_token".to_owned(), json!("r1"))]);
    rv.conns
        .iter()
        .map(|c| {
            let url = aurora_gateway::refresh::build_exchange(
                client,
                token,
                &current,
                &serde_json::Map::new(),
                &serde_json::Map::new(),
                None,
                c.instance_base(),
            )
            .map(|r| r.url)
            .unwrap_or_else(|_| "unavailable".into());
            (c.instance_host.clone(), url)
        })
        .collect()
}

#[tokio::test]
async fn default_host_connection_refreshes_against_the_default_host() {
    let inst = Uuid::now_v7();
    let got = refresh_targets("gitlab.test", move |w| {
        let mut r = granted(bound(w, "c1", None))(w);
        r.instances.push(instance_row(inst, "desk.acme.test"));
        r
    })
    .await;
    assert_eq!(
        got,
        vec![(
            Some("gitlab.test".to_owned()),
            "https://gitlab.test/oauth/token".to_owned()
        )]
    );
}

#[tokio::test]
async fn refresh_never_crosses_between_default_and_instance_hosts() {
    let inst = Uuid::now_v7();
    // The default-host connection resolved on a registered instance's host still targets
    // `gitlab.test` (the guard refuses to inject there; see the proxy tests), never the instance.
    let on_instance = refresh_targets("desk.acme.test", move |w| {
        let mut r = granted(bound(w, "c1", None))(w);
        r.instances.push(instance_row(inst, "desk.acme.test"));
        r
    })
    .await;
    assert!(
        on_instance
            .iter()
            .all(|(_, u)| u == "https://gitlab.test/oauth/token"),
        "{on_instance:?}"
    );
    // The instance connection refreshes at its own instance, never at the default host, from
    // either host it can be resolved on.
    for host in ["gitlab.test", "desk.acme.test"] {
        let got = refresh_targets(host, move |w| {
            let mut r = granted(bound(w, "c2", Some(inst)))(w);
            r.instances.push(instance_row(inst, "desk.acme.test"));
            r
        })
        .await;
        assert!(
            got.iter()
                .all(|(_, u)| u == "https://desk.acme.test/oauth/token"),
            "{host}: {got:?}"
        );
    }
}

// ---- P4-GRANTS (D-123, SEC-67): an Indico-like app with a broad token -------------------------

/// A catalogue of two tools on an app whose token is far broader (`full:everything`).
fn indico_like(custom_only: bool, block_untooled: bool) -> Value {
    let mut app = app_json(
        "demo",
        json!({"read": {"items": [
            {"id": "read_item", "name": "r", "host": "127.0.0.1", "path": "/items", "methods": ["GET"]}
        ]}, "write": {"items": [
            {"id": "write_item", "name": "w", "host": "127.0.0.1", "path": "/items", "methods": ["POST"]}
        ]}}),
        false,
    );
    app["grants"] = json!({"custom_only": custom_only, "block_untooled": block_untooled});
    snapshot(
        vec![app],
        vec![host_json(
            "demo",
            None,
            json!({"inject": [{"bearer": "{credentials.tok}"}]}),
        )],
    )
}

fn derived_custom(c: &AppConnectionRow) -> Vec<(Uuid, Value)> {
    // The rules a custom grant compiles to (07 table): allow, then block everything else.
    vec![
        (
            Uuid::now_v7(),
            json!({"name":"g","enabled":true,"identities":[],"action":"allow",
                   "targets":[{"type":"connection","id":c.id,"tools":["read_item"]}]}),
        ),
        (
            Uuid::now_v7(),
            json!({"name":"g-rest","enabled":true,"identities":[],"action":"block",
                   "targets":[{"type":"connection","id":c.id}]}),
        ),
    ]
}

/// (P4-EVAL-2, C-868, SEC-114) On an app host an eval turn keeps the app's read tools (even a
/// POST one: a GraphQL query) and plain GETs, and is refused `eval_read_only` for a write tool or
/// an untooled non-GET, under a `full` grant that would allow all of them. Nothing is injected on a
/// refusal; the agent credential is unchanged.
#[tokio::test]
async fn eval_turn_keeps_read_tools_and_refuses_write_tools_on_an_app_host() {
    let snap = snapshot(
        vec![app_json("demo", tools_json(), false)],
        vec![host_json(
            "demo",
            None,
            json!({"inject": [{"bearer": "{credentials.tok}"}]}),
        )],
    );
    let (env, ..) = start(snap, &[("c1", r#"{"tok":"T0K"}"#)], |w| {
        let c = conn(w, "demo", "c1", None);
        let mut r = Rows::default();
        r.ws.rules.push(allow_conn(&c, None));
        r.conns.push(c);
        r
    })
    .await;
    let q = r#"{"query":"query { a }"}"#;
    let m = r#"{"query":"mutation { a }"}"#;
    // Reads pass and are injected as usual.
    let r = send_as(&env, EVAL_CTX, "GET", "/items", "", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("authorization: Bearer T0K"), "{}", r.body);
    let r = send_as(&env, EVAL_CTX, "POST", "/graphql", "", q).await;
    assert_eq!(r.status, 200, "read tool by POST: {}", r.body);
    // Writes are refused before injection.
    for (method, path, body) in [
        ("POST", "/items", "{}"),
        ("POST", "/graphql", m),
        ("POST", "/untooled", "{}"),
        ("DELETE", "/items", ""),
    ] {
        let r = send_as(&env, EVAL_CTX, method, path, "", body).await;
        assert_eq!(r.status, 403, "{method} {path}: {}", r.body);
        assert_eq!(r.code(), "eval_read_only", "{method} {path}: {}", r.body);
        assert!(!r.body.contains("T0K"), "{}", r.body);
    }
    // The same write with the agent credential is not refused by this rule.
    let r = send(&env, "POST", "/items", "", "{}").await;
    assert_eq!(r.status, 200, "{}", r.body);
}

/// (P4-EVAL-2 review, C-868) A request "whose effect is a write" is refused whatever its method: an
/// app tool tagged write that is called by GET (a "trigger" endpoint) is refused `eval_read_only`
/// in an eval turn, while a plain GET with no tool match and a GET read tool still pass.
#[tokio::test]
async fn eval_turn_refuses_a_write_tool_called_by_get() {
    let tools = json!({"read": {"items": [
        {"id": "read_item", "name": "r", "host": "127.0.0.1", "path": "/items", "methods": ["GET"]}
    ]}, "write": {"items": [
        {"id": "run_export", "name": "x", "host": "127.0.0.1", "path": "/export/run", "methods": ["GET"]}
    ]}});
    let snap = snapshot(
        vec![app_json("demo", tools, false)],
        vec![host_json(
            "demo",
            None,
            json!({"inject": [{"bearer": "{credentials.tok}"}]}),
        )],
    );
    let (env, ..) = start(snap, &[("c1", r#"{"tok":"T0K"}"#)], |w| {
        let c = conn(w, "demo", "c1", None);
        let mut r = Rows::default();
        r.ws.rules.push(allow_conn(&c, None));
        r.conns.push(c);
        r
    })
    .await;
    let r = send_as(&env, EVAL_CTX, "GET", "/export/run", "", "").await;
    assert_eq!(r.status, 403, "write tool by GET: {}", r.body);
    assert_eq!(r.code(), "eval_read_only", "{}", r.body);
    assert!(!r.body.contains("T0K"), "{}", r.body);
    // Reads still pass: a read tool by GET, and a GET that matches no tool.
    for path in ["/items", "/untooled"] {
        let r = send_as(&env, EVAL_CTX, "GET", path, "", "").await;
        assert_eq!(r.status, 200, "GET {path}: {}", r.body);
    }
    // The agent credential is not refused by this rule.
    let r = send(&env, "GET", "/export/run", "", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
}

/// FIX-133 (D-240): a read-only connection allows the app's read tools and blocks its write tools
/// and untooled requests under a `full` grant; an existing (`write`) connection is unchanged.
#[tokio::test]
async fn read_only_connection_blocks_write_and_untooled_even_under_a_full_grant() {
    let (env, ..) = start(
        indico_like(false, false),
        &[("c1", r#"{"tok":"RO-TOKEN"}"#)],
        |w| {
            let mut c = conn(w, "demo", "c1", None);
            c.access = "read".into();
            let mut r = Rows::default();
            r.ws.rules.push(allow_conn(&c, None));
            r.conns.push(c);
            r
        },
    )
    .await;
    let r = get(&env, "/items", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("authorization: Bearer RO-TOKEN"),
        "{}",
        r.body
    );
    for (method, path) in [("POST", "/items"), ("GET", "/anything/else")] {
        let r = send(&env, method, path, "", "{}").await;
        assert_eq!(r.status, 403, "{method} {path}: {}", r.body);
        assert_eq!(r.code(), "blocked_by_policy");
        let v: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(
            v["error"]["details"]["reason"], "read_only_connection",
            "{method} {path}: {}",
            r.body
        );
        assert!(!r.body.contains("RO-TOKEN"));
    }
}

/// FIX-159 (D-329, C-930): the gateway's side of the simulate == gateway table. The same app shape,
/// grant (reads allowed, writes asked, the rest blocked), read-only connection and requests as
/// `services/aurora-api/tests/it/simulate_apps.rs::simulate_connection_custom_grant_matches_gateway`,
/// with the verdicts asserted there: the simulator and the gateway run the same identification
/// (`aurora_registry::identify`) and the same evaluator, and both tables must say the same.
#[tokio::test]
async fn read_only_custom_grant_decides_the_simulate_table() {
    let mut app = app_json(
        "aws",
        json!({"read": {"items": [
            {"id": "s3_list_buckets", "name": "l", "host": "127.0.0.1", "path": "/", "methods": ["GET"]},
            {"id": "s3_get_object", "name": "g", "host": "127.0.0.1", "path": "/*/*/**", "methods": ["GET", "HEAD"]},
            {"id": "sts_get_caller_identity", "name": "w", "host": "127.0.0.1", "path": "/", "methods": ["POST"],
             "params": [{"source": "form", "name": "Action", "match": {"equals": "GetCallerIdentity"}}]}
        ]}, "write": {"items": [
            {"id": "s3_put_object", "name": "p", "host": "127.0.0.1", "path": "/*/*/**", "methods": ["PUT"]},
            {"id": "s3_delete_object", "name": "d", "host": "127.0.0.1", "path": "/*/*/**", "methods": ["DELETE"]}
        ]}}),
        false,
    );
    app["grants"] = json!({"custom_only": false, "block_untooled": true});
    let snap = snapshot(
        vec![app],
        vec![host_json(
            "aws",
            None,
            json!({"inject": [{"bearer": "{credentials.tok}"}]}),
        )],
    );
    let (env, ..) = start(snap, &[("c1", r#"{"tok":"AWS-KEY"}"#)], |w| {
        let mut c = conn(w, "aws", "c1", None);
        c.access = "read".into();
        let mut r = Rows::default();
        r.ws.rules.push(allow_conn(
            &c,
            Some(vec!["s3_list_buckets", "s3_get_object", "sts_get_caller_identity"]),
        ));
        let mut ask = rule(
            "ask",
            "allow",
            json!([{"type": "connection", "id": c.id, "tools": ["s3_put_object", "s3_delete_object"]}]),
            true,
        );
        ask.approval_timeout_secs = None;
        r.ws.rules.push(ask);
        r.ws.rules.push(rule(
            "rest",
            "block",
            json!([{"type": "connection", "id": c.id}]),
            false,
        ));
        r.conns.push(c);
        r
    })
    .await;
    const FORM: &str = "Content-Type: application/x-www-form-urlencoded\r\n";
    // (what, method, path, headers, body, status, reason of the refusal)
    #[allow(clippy::type_complexity)]
    let cases: [(&str, &str, &str, &str, &str, u16, Option<&str>); 4] = [
        (
            "PUT on a read-only connection",
            "PUT",
            "/bucket/key.txt",
            "",
            "",
            403,
            Some("read_only_connection"),
        ),
        ("list buckets", "GET", "/", "", "", 200, None),
        (
            "bare STS POST",
            "POST",
            "/",
            "",
            "",
            403,
            Some("app_untooled_blocked"),
        ),
        (
            "GetCallerIdentity",
            "POST",
            "/",
            FORM,
            "Action=GetCallerIdentity&Version=2011-06-15",
            200,
            None,
        ),
    ];
    for (what, method, path, extra, body, status, reason) in cases {
        let r = send(&env, method, path, extra, body).await;
        assert_eq!(r.status, status, "{what}: {}", r.body);
        if let Some(reason) = reason {
            let v: Value = serde_json::from_str(&r.body).unwrap();
            assert_eq!(
                v["error"]["details"]["reason"], reason,
                "{what}: {}",
                r.body
            );
        }
    }
}

/// Two read tools (`/items` needs `read:a`, `/other` needs `read:b`) and one with no requirement.
fn scoped(annotated: bool) -> Value {
    let sc = |s: &str| if annotated { json!([s]) } else { json!([]) };
    let mut app = app_json(
        "demo",
        json!({"read": {"items": [
            {"id": "needs_a", "name": "a", "host": "127.0.0.1", "path": "/items", "methods": ["GET"], "scopes": sc("read:a")},
            {"id": "needs_b", "name": "b", "host": "127.0.0.1", "path": "/other", "methods": ["GET"], "scopes": sc("read:b")},
            {"id": "needs_none", "name": "n", "host": "127.0.0.1", "path": "/free", "methods": ["GET"]}
        ]}}),
        false,
    );
    app["grants"] = json!({"custom_only": false, "block_untooled": false});
    snapshot(
        vec![app],
        vec![host_json(
            "demo",
            None,
            json!({"inject": [{"bearer": "{credentials.tok}"}]}),
        )],
    )
}

/// FIX-136 (D-241): a tool whose scopes the connection lacks is refused before injection with
/// `tool_scope_missing` and the token is not sent; a covered tool and a tool with no requirement
/// pass; unknown scopes (empty) and packages without annotations behave exactly as before.
#[tokio::test]
async fn a_tool_needing_a_scope_the_connection_lacks_is_refused_before_injection() {
    for (annotated, scopes, path, status) in [
        (true, vec!["read:a"], "/items", 200),
        (true, vec!["read:a"], "/other", 403),
        (true, vec!["read:a"], "/free", 200),
        (true, vec!["read:b", "read:a"], "/other", 200),
        // Unknown scopes never refuse.
        (true, vec![], "/other", 200),
        // No annotations: unchanged.
        (false, vec!["read:a"], "/other", 200),
    ] {
        let (env, ..) = start(scoped(annotated), &[("c1", r#"{"tok":"SC-TOKEN"}"#)], |w| {
            let mut c = conn(w, "demo", "c1", None);
            c.scopes = scopes.iter().map(|s| (*s).to_owned()).collect();
            let mut r = Rows::default();
            r.ws.rules.push(allow_conn(&c, None));
            r.conns.push(c);
            r
        })
        .await;
        let r = get(&env, path, "").await;
        assert_eq!(
            r.status, status,
            "{annotated} {scopes:?} {path}: {}",
            r.body
        );
        if status == 403 {
            assert_eq!(r.code(), "tool_scope_missing", "{}", r.body);
            let v: Value = serde_json::from_str(&r.body).unwrap();
            assert_eq!(v["error"]["details"]["tool_id"], "needs_b");
            assert_eq!(v["error"]["details"]["needs"], json!(["read:b"]));
            assert!(
                !r.body.contains("SC-TOKEN"),
                "the token is never sent: {}",
                r.body
            );
        }
    }
}

#[tokio::test]
async fn a_write_connection_is_unchanged_by_the_read_only_check() {
    let (env, ..) = start(
        indico_like(false, false),
        &[("c1", r#"{"tok":"RW-TOKEN"}"#)],
        |w| {
            let c = conn(w, "demo", "c1", None); // access = write, as every existing connection
            let mut r = Rows::default();
            r.ws.rules.push(allow_conn(&c, None));
            r.conns.push(c);
            r
        },
    )
    .await;
    let r = send(&env, "POST", "/items", "", "{}").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("authorization: Bearer RW-TOKEN"),
        "{}",
        r.body
    );
}

#[tokio::test]
async fn custom_only_and_block_untooled_never_inject_an_untooled_request() {
    let (env, ..) = start(
        indico_like(true, true),
        &[("c1", r#"{"tok":"BROADTOKEN"}"#)],
        |w| {
            let c = conn(w, "demo", "c1", None);
            let mut r = Rows::default();
            r.ws.derived.extend(derived_custom(&c));
            // A stale full-style rule that something wrote by hand: inert for a custom_only app.
            r.ws.rules.push(allow_conn(&c, None));
            r.conns.push(c);
            r
        },
    )
    .await;
    // A catalogued tool of the custom grant behaves per grant: injected.
    let r = get(&env, "/items", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("authorization: Bearer BROADTOKEN"),
        "{}",
        r.body
    );
    // Untooled request: blocked before any rule, reason app_untooled_blocked, token never sent.
    let r = get(&env, "/anything/else", "").await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "blocked_by_policy");
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(
        v["error"]["details"]["reason"], "app_untooled_blocked",
        "{}",
        r.body
    );
    assert!(!r.body.contains("BROADTOKEN"));
    // A catalogued tool that is not allowed stays blocked by the grant's rule 3 (not by the flag).
    let r = send(&env, "POST", "/items", "", "{}").await;
    assert_eq!(r.status, 403, "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_ne!(
        v["error"]["details"]["reason"], "app_untooled_blocked",
        "{}",
        r.body
    );
    assert!(!r.body.contains("BROADTOKEN"));
}

#[tokio::test]
async fn custom_only_makes_a_full_style_rule_inert_and_block_untooled_alone_covers_full_grants() {
    // custom_only without block_untooled: only a stale full-style allow rule exists. It neither
    // allows nor selects the connection, so the token is never injected on any path.
    let (env, ..) = start(
        indico_like(true, false),
        &[("c1", r#"{"tok":"BROADTOKEN"}"#)],
        |w| {
            let c = conn(w, "demo", "c1", None);
            let mut r = Rows::default();
            r.ws.rules.push(allow_conn(&c, None));
            r.conns.push(c);
            r
        },
    )
    .await;
    for p in ["/items", "/anything/else"] {
        let r = get(&env, p, "").await;
        assert!(!r.body.contains("BROADTOKEN"), "{p}: {}", r.body);
        assert!(
            !r.body.to_lowercase().contains("authorization: bearer"),
            "{p}: {}",
            r.body
        );
    }
    // block_untooled without custom_only: a full grant covers catalogued tools only.
    let (env, ..) = start(
        indico_like(false, true),
        &[("c1", r#"{"tok":"BROADTOKEN"}"#)],
        |w| {
            let c = conn(w, "demo", "c1", None);
            let mut r = Rows::default();
            r.ws.rules.push(allow_conn(&c, None));
            r.conns.push(c);
            r
        },
    )
    .await;
    let r = get(&env, "/items", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("authorization: Bearer BROADTOKEN"),
        "{}",
        r.body
    );
    let r = get(&env, "/anything/else", "").await;
    assert_eq!(r.status, 403, "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(
        v["error"]["details"]["reason"], "app_untooled_blocked",
        "{}",
        r.body
    );
    assert!(!r.body.contains("BROADTOKEN"));
}

#[tokio::test]
async fn an_app_without_flags_is_unaffected_by_the_restrictions() {
    let (env, ..) = start(
        indico_like(false, false),
        &[("c1", r#"{"tok":"T"}"#)],
        |w| {
            let c = conn(w, "demo", "c1", None);
            let mut r = Rows::default();
            r.ws.rules.push(allow_conn(&c, None));
            r.conns.push(c);
            r
        },
    )
    .await;
    let r = get(&env, "/anything/else", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("authorization: Bearer T"), "{}", r.body);
}

/// SEC-01/SEC-67/SEC-68: the shape of the aws-keys / aws-role STS entry. Only a clean
/// `Action=GetCallerIdentity` is classified (read) and injected; every credential-minting action
/// and every doubtful request is blocked as `app_untooled_blocked` and nothing is sent upstream.
#[tokio::test]
async fn sts_credential_minting_actions_are_blocked_and_never_signed() {
    let mut app = app_json(
        "sts",
        json!({"read": {"items": [{"id": "sts_get_caller_identity", "name": "w", "host": "127.0.0.1",
            "path": "/", "methods": ["POST"],
            "params": [{"source": "form", "name": "Action", "match": {"equals": "GetCallerIdentity"}}]}]}}),
        false,
    );
    app["grants"] = json!({"custom_only": false, "block_untooled": true});
    let snap = snapshot(
        vec![app],
        vec![host_json(
            "sts",
            None,
            json!({"inject": [{"bearer": "{credentials.tok}"}]}),
        )],
    );
    let (env, ..) = start(snap, &[("c1", r#"{"tok":"STSKEY"}"#)], |w| {
        let c = conn(w, "sts", "c1", None);
        let mut r = Rows::default();
        // A full grant: the strongest one a user can hold on a block_untooled app.
        r.ws.rules.push(allow_conn(&c, None));
        r.conns.push(c);
        r
    })
    .await;
    const FORM: &str = "Content-Type: application/x-www-form-urlencoded\r\n";
    let big = format!("Action=GetCallerIdentity&pad={}", "a".repeat(300 * 1024));
    let cases: Vec<(&str, String, String, String)> = vec![
        ("AssumeRole", "/".into(), FORM.into(), "Action=AssumeRole&RoleArn=x".into()),
        ("AssumeRoleWithWebIdentity", "/".into(), FORM.into(), "Action=AssumeRoleWithWebIdentity".into()),
        ("AssumeRoleWithSAML", "/".into(), FORM.into(), "Action=AssumeRoleWithSAML".into()),
        ("GetSessionToken", "/".into(), FORM.into(), "Action=GetSessionToken".into()),
        ("GetFederationToken", "/".into(), FORM.into(), "Action=GetFederationToken&Name=x".into()),
        ("GetAccessKeyInfo", "/".into(), FORM.into(), "Action=GetAccessKeyInfo".into()),
        ("repeated", "/".into(), FORM.into(), "Action=GetCallerIdentity&Action=AssumeRole".into()),
        ("repeated, other order", "/".into(), FORM.into(), "Action=AssumeRole&Action=GetCallerIdentity".into()),
        ("query and body differ", "/?Action=AssumeRole".into(), FORM.into(), "Action=GetCallerIdentity".into()),
        ("query GCI, body AssumeRole", "/?Action=GetCallerIdentity".into(), FORM.into(), "Action=AssumeRole".into()),
        ("query only", "/?Action=GetCallerIdentity".into(), FORM.into(), "Version=1".into()),
        ("compressed", "/".into(), format!("{FORM}Content-Encoding: gzip\r\n"), "Action=GetCallerIdentity".into()),
        ("truncated", "/".into(), FORM.into(), big),
        ("lowercase value", "/".into(), FORM.into(), "Action=getcalleridentity".into()),
        ("odd-cased value", "/".into(), FORM.into(), "Action=GetCallerIDentity".into()),
        ("odd-cased name", "/".into(), FORM.into(), "ACTION=GetCallerIdentity".into()),
        ("other content type", "/".into(), "Content-Type: text/plain\r\n".into(), "Action=GetCallerIdentity".into()),
        (
            "x-amz-target",
            "/".into(),
            "X-Amz-Target: AWSSecurityTokenServiceV20110615.GetCallerIdentity\r\nContent-Type: application/x-amz-json-1.1\r\n".into(),
            "{}".into(),
        ),
    ];
    for (name, path, extra, body) in cases {
        let r = send(&env, "POST", &path, &extra, &body).await;
        assert_eq!(r.status, 403, "{name}: {}", r.body);
        let v: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(
            v["error"]["details"]["reason"], "app_untooled_blocked",
            "{name}: {}",
            r.body
        );
        let low = r.body.to_lowercase();
        for h in [
            "authorization",
            "x-amz-security-token",
            "x-amz-date",
            "stskey",
        ] {
            assert!(!low.contains(h), "{name}: {h} leaked: {}", r.body);
        }
    }
    // Only the clean request is classified as the read tool and injected.
    let r = send(
        &env,
        "POST",
        "/",
        FORM,
        "Action=GetCallerIdentity&Version=2011-06-15",
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("authorization: Bearer STSKEY"),
        "{}",
        r.body
    );
}

/// (P5-GRAFANA security review, D-429) The shipped grafana package, compiled as the API does:
/// under a `full` grant its `grants.block_untooled` refuses every untooled request before any rule
/// and never sends the token. Grafana's data source proxy forwards any method and path to the
/// backend, so this is what keeps the backends' admin APIs out of reach. Catalogued reads still run.
#[tokio::test]
async fn grafana_full_grant_blocks_untooled_proxy_requests() {
    let path = format!(
        "{}/../../plugins/builtin/grafana/plugin.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut m = aurora_registry::Manifest::parse(&std::fs::read_to_string(&path).unwrap(), "9.9.9")
        .unwrap();
    assert!(m.apps[0].grants.block_untooled, "grafana blocks untooled");
    // The agent guide's files are digested from the stored archive; the gateway never reads them.
    m.apps[0].agent = None;
    let p = aurora_registry::snapshot::ActivePackage {
        manifest: m,
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
    };
    let compiled = aurora_registry::snapshot::compile(&[p], &[], &[], 1).unwrap();
    let snap: Value = serde_json::from_slice(&compiled.json).unwrap();
    let inst = Uuid::now_v7();
    let (env, ..) = start(
        snap,
        &[("g1", r#"{"access_token":"GLSATOKEN"}"#)],
        move |w| {
            let mut c = conn(w, "grafana", "g1", None);
            c.instance_id = Some(inst);
            // A full grant: the strongest one a person can give.
            let mut r = granted(c)(w);
            r.instances
                .push(instance_row_at(inst, "grafana", "localhost", None));
            r
        },
    )
    .await;
    // Catalogued reads: injected.
    for p in [
        "/api/user",
        "/api/datasources/proxy/uid/prom-1/api/v1/query?query=up",
        "/api/datasources/proxy/uid/loki-1/loki/api/v1/labels",
    ] {
        let r = get_host(&env, "localhost", p).await;
        assert_eq!(r.status, 200, "{p}: {}", r.body);
        assert!(has_token(&r, "GLSATOKEN"), "{p}: {}", r.body);
    }
    // Untooled: data source proxy admin and status paths, and Grafana endpoints no tool offers.
    for (method, p) in [
        (
            "POST",
            "/api/datasources/proxy/uid/prom-1/api/v1/admin/tsdb/delete_series",
        ),
        (
            "PUT",
            "/api/datasources/proxy/uid/prom-1/api/v1/admin/tsdb/clean_tombstones",
        ),
        (
            "GET",
            "/api/datasources/proxy/uid/prom-1/api/v1/status/config",
        ),
        (
            "POST",
            "/api/datasources/proxy/uid/loki-1/loki/api/v1/delete",
        ),
        ("DELETE", "/api/datasources/proxy/uid/prom-1/api/v1/query"),
        ("GET", "/api/datasources/proxy/7/api/v1/query"),
        ("POST", "/api/serviceaccounts/2/tokens"),
        ("DELETE", "/api/dashboards/uid/abc"),
    ] {
        let r = send_host(&env, method, "localhost", p).await;
        assert_eq!(r.status, 403, "{method} {p}: {}", r.body);
        assert_eq!(r.code(), "blocked_by_policy", "{method} {p}");
        let v: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(
            v["error"]["details"]["reason"], "app_untooled_blocked",
            "{method} {p}: {}",
            r.body
        );
        assert!(!r.body.contains("GLSATOKEN"), "{method} {p}: {}", r.body);
    }
}

// ---- D-231 / SEC-59: a registration under a base path -----------------------------------------

/// `GET http://{host}:{port}{path}` through the proxy with extra header lines.
async fn get_host_with(env: &Env, host: &str, path: &str, extra: &str) -> Reply {
    let mut s = crate::common::connect(env.proxy).await;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("agent:{TOKEN}"));
    let req = format!(
        "GET http://{host}:{port}{path} HTTP/1.1\r\nHost: {host}:{port}\r\nProxy-Authorization: Basic {auth}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n",
        port = env.up.port()
    );
    s.write_all(req.as_bytes()).await.unwrap();
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
        head: head.to_owned(),
        body: dechunk(body),
    }
}

/// An instance-bound app with a read tool on `{instance}`; the manifest knows nothing of any base
/// path (`{instance}` is the whole registration).
fn jira_like(id: &str, tool: &str, path: &str) -> Value {
    let mut a = app_json(
        id,
        json!({"read": {"items": [
            {"id": tool, "name": "r", "host": "{instance}", "path": path, "methods": ["GET"]}
        ]}}),
        false,
    );
    a["instances"] = json!({"label": id});
    a
}

fn inst_host_entry(app: &str) -> Value {
    let mut ih = host_json(
        app,
        None,
        json!({"inject": [{"bearer": "{credentials.tok}"}]}),
    );
    ih["host"] = json!("{instance}");
    ih
}

fn token_on(r: &Reply, token: &str) -> bool {
    r.body.contains(&format!("authorization: Bearer {token}"))
}

fn derived_allow(c: &AppConnectionRow, tools: &[&str]) -> (Uuid, Value) {
    (
        Uuid::now_v7(),
        json!({"name":"d","enabled":true,"identities":[],"action":"allow",
               "targets":[{"type":"connection","id":c.id,"tools":tools}]}),
    )
}

/// A connection on `localhost` + `/jira` gets its credential on `/jira/...`, is matched to the
/// manifest tool on the app path, and gets nothing anywhere else on the host.
#[tokio::test]
async fn a_base_path_registration_is_injected_only_under_its_base_path() {
    let inst = Uuid::now_v7();
    let mut snap = snapshot(
        vec![jira_like("jira", "myself", "/rest/api/2/myself")],
        vec![],
    );
    snap["instance_hosts"] = json!([inst_host_entry("jira")]);
    let conn_id = Arc::new(Mutex::new(Uuid::nil()));
    let cid = conn_id.clone();
    let (env, ..) = start(snap, &[("c1", r#"{"tok":"JIRASECRET"}"#)], move |w| {
        let mut c = conn(w, "jira", "c1", None);
        c.instance_id = Some(inst);
        *cid.lock().unwrap() = c.id;
        let mut r = Rows::default();
        r.ws.default_action = "block".into();
        r.ws.derived.push(derived_allow(&c, &["myself"]));
        r.conns.push(c);
        r.instances
            .push(instance_row_at(inst, "jira", "localhost", Some("/jira")));
        r
    })
    .await;
    // Inside the base path: injected; the tool is matched on the app path (`/rest/api/2/myself`),
    // the upstream request keeps the full path.
    let r = get_host(&env, "localhost", "/jira/rest/api/2/myself").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(token_on(&r, "JIRASECRET"), "{}", r.body);
    assert!(
        r.body.contains("path=/jira/rest/api/2/myself\n"),
        "{}",
        r.body
    );
    // A catalogued path that is not granted is still judged as a tool on the app path.
    let r = get_host(&env, "localhost", "/jira/rest/api/2/other").await;
    assert_eq!(r.code(), "blocked_by_default_policy", "{}", r.body);
    assert!(!token_on(&r, "JIRASECRET"));
    // `/%6Aira` is `/jira` for the gateway and the upstream alike: inside, injected.
    let r = get_host(&env, "localhost", "/%6Aira/rest/api/2/myself").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(token_on(&r, "JIRASECRET"), "{}", r.body);
    assert!(
        r.body.contains("path=/jira/rest/api/2/myself\n"),
        "{}",
        r.body
    );
    // Nothing is injected outside the base path, in any spelling that leaves it.
    for p in [
        "/rest/api/2/myself",
        "/jira2/rest/api/2/myself",
        "/JIRA/rest/api/2/myself",
        "//rest/api/2/myself",
        "/jira/../rest/api/2/myself",
        "/jira/%2e%2e/rest/api/2/myself",
        "/./jira/rest/api/2/myself",
        "/",
    ] {
        let r = get_host(&env, "localhost", p).await;
        assert!(!token_on(&r, "JIRASECRET"), "{p}: {}", r.body);
        assert!(!r.head.contains("JIRASECRET"), "{p}");
    }
    // Used by name outside its base path, the connection is refused like one on the wrong host,
    // with where to send the request.
    let id = *conn_id.lock().unwrap();
    let r = get_host_with(
        &env,
        "localhost",
        "/rest/api/2/myself",
        &format!("x-aurora-connection-id: {id}\r\n"),
    )
    .await;
    assert_eq!(r.status, 421, "{}", r.body);
    assert_eq!(r.code(), "host_mismatch");
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["error"]["details"]["expected_host"], "localhost");
    assert_eq!(
        v["error"]["details"]["expected_base_url"],
        "https://localhost/jira"
    );
    assert!(!r.body.contains("JIRASECRET"));
}

/// Two registrations on one host (`/jira` and `/confluence`) each get only their own credential,
/// and a registration without a base path serves the rest of the host.
#[tokio::test]
async fn registrations_on_one_host_get_only_their_own_credentials() {
    let (ji, ci) = (Uuid::now_v7(), Uuid::now_v7());
    let mut snap = snapshot(
        vec![
            jira_like("jira", "j_me", "/rest/api/2/myself"),
            jira_like("conf", "c_me", "/rest/api/user/current"),
        ],
        vec![],
    );
    snap["instance_hosts"] = json!([inst_host_entry("jira"), inst_host_entry("conf")]);
    let (env, ..) = start(
        snap,
        &[
            ("cj", r#"{"tok":"JIRASECRET"}"#),
            ("cc", r#"{"tok":"CONFSECRET"}"#),
        ],
        move |w| {
            let mut j = conn(w, "jira", "cj", None);
            j.instance_id = Some(ji);
            let mut c = conn(w, "conf", "cc", None);
            c.instance_id = Some(ci);
            let mut r = Rows::default();
            r.ws.default_action = "block".into();
            r.ws.derived.push(derived_allow(&j, &["j_me"]));
            r.ws.derived.push(derived_allow(&c, &["c_me"]));
            r.conns = vec![j, c];
            r.instances
                .push(instance_row_at(ji, "jira", "localhost", Some("/jira")));
            r.instances.push(instance_row_at(
                ci,
                "conf",
                "localhost",
                Some("/confluence"),
            ));
            r
        },
    )
    .await;
    let r = get_host(&env, "localhost", "/jira/rest/api/2/myself").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        token_on(&r, "JIRASECRET") && !token_on(&r, "CONFSECRET"),
        "{}",
        r.body
    );
    let r = get_host(&env, "localhost", "/confluence/rest/api/user/current").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        token_on(&r, "CONFSECRET") && !token_on(&r, "JIRASECRET"),
        "{}",
        r.body
    );
    // Each app's tools exist only under its own registration.
    let r = get_host(&env, "localhost", "/confluence/rest/api/2/myself").await;
    assert!(!token_on(&r, "JIRASECRET"), "{}", r.body);
    let r = get_host(&env, "localhost", "/jira/rest/api/user/current").await;
    assert!(!token_on(&r, "CONFSECRET"), "{}", r.body);
}

/// A refresh goes to the registration's own address, base path included (D-231).
#[tokio::test]
async fn refresh_targets_the_registrations_base_path() {
    let inst = Uuid::now_v7();
    let got = refresh_targets("desk.acme.test", move |w| {
        let mut r = granted(bound(w, "c2", Some(inst)))(w);
        r.instances.push(instance_row_at(
            inst,
            "inst",
            "desk.acme.test",
            Some("/jira"),
        ));
        r
    })
    .await;
    assert_eq!(
        got,
        vec![(
            Some("desk.acme.test".to_owned()),
            "https://desk.acme.test/jira/oauth/token".to_owned()
        )]
    );
}
