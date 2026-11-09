#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P2-DBL-1: the database lane against a real PostgreSQL (the testkit database is both the
//! policy/approval store and the target of the stored connection).

use async_trait::async_trait;
use aurora_config::{LlmSection, SecretsSection};
use aurora_db::repo::gateway::{
    AppConnectionRow, PolicyRuleRow, RequestLogRow, ScopePolicyRows, SecretMetaRow,
};
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
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use uuid::Uuid;

const TOKEN_A: &str = "aug_agent_a";
const TOKEN_B: &str = "aug_agent_b";
const TOKEN_FROZEN: &str = "aug_frozen";
const TOKEN_EXPIRED: &str = "aug_expired";

#[derive(Clone, Copy)]
struct World {
    org: Uuid,
    ws: Uuid,
    agent_a: Uuid,
    agent_b: Uuid,
}

struct Static(World);

#[async_trait]
impl CredentialResolver for Static {
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        let w = self.0;
        let agent = match token {
            TOKEN_A => w.agent_a,
            TOKEN_B => w.agent_b,
            TOKEN_FROZEN => return Err(ResolveError::Frozen { level: "agent" }),
            TOKEN_EXPIRED => return Err(ResolveError::Expired),
            _ => return Ok(None),
        };
        Ok(Some(RequestIdentity {
            credential_id: Uuid::now_v7(),
            kind: CredentialKind::Agent,
            agent_id: agent,
            workspace_id: w.ws,
            org_id: w.org,
            conversation_id: None,
            turn_id: None,
            context_label: None,
            speaker_user_id: None,
            capabilities: vec![],
            byo: None,
        }))
    }
}

struct Rows {
    ws: ScopePolicyRows,
    conn: AppConnectionRow,
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
        Ok(vec![])
    }
    async fn slugs(&self, _w: Uuid) -> Result<Option<(String, String)>, String> {
        Ok(Some(("acme".into(), "main".into())))
    }
    async fn connection(
        &self,
        _o: Uuid,
        _w: Uuid,
        id: Uuid,
    ) -> Result<Option<AppConnectionRow>, String> {
        let r = self.0.lock().unwrap();
        Ok((r.conn.id == id).then(|| r.conn.clone()))
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
    w: World,
    proxy: SocketAddr,
    conn_id: Uuid,
    gw_ca: String,
    db: TestDb,
    captured: Arc<Capture>,
    log: Arc<RequestLog>,
    _src: Arc<Src>,
    role: String,
    role_pw: String,
    _shutdown: tokio::sync::watch::Sender<bool>,
}

fn rule(name: &str, tools: &[&str], conn: Uuid, agent: Uuid, approval: bool) -> PolicyRuleRow {
    PolicyRuleRow {
        logical_id: Uuid::now_v7(),
        name: name.into(),
        enabled: true,
        identities: json!([{"kind": "agent", "id": agent}]),
        targets: json!([{"type": "connection", "id": conn, "tools": tools}]),
        action: "allow".into(),
        require_approval: approval,
        rate_limit: None,
        conditions: json!([]),
        approval_timeout_secs: None,
    }
}

fn snapshot() -> Value {
    let tool = |id: &str, path: &str| json!({"id": id, "name": id, "host": "*.db.aurora.internal", "path": path, "methods": ["POST"]});
    json!({
        "format": 1, "contract": 1,
        "packages": [{"package_id": "pkg", "version": "1.0.0", "digest": "d", "publisher": "p",
                      "hooks_digest": null, "egress": {}, "context": []}],
        "oauth_clients": [],
        "apps": [{
            "app_id": "pg", "package_id": "pkg", "about": {"id": "pg", "name": "pg"},
            "credentials": [], "metadata": [], "connect": [], "token": null,
            "tools": {"read": {"items": [tool("pg_read", "/read")]},
                      "write": {"items": [tool("pg_write", "/write"), tool("pg_ddl", "/ddl"),
                                          tool("pg_admin", "/admin")]}},
            "approvals": {}, "agent": null, "database": {"engine": "postgres"}
        }],
        "hosts": [], "reserved_hosts": []
    })
}

/// `(host, port, user, password, dbname)` of a `postgres://` URL.
fn parts(url: &str) -> (String, u16, String, String, String) {
    let rest = url.strip_prefix("postgres://").unwrap();
    let (auth, tail) = rest.split_once('@').unwrap();
    let (user, pw) = auth.split_once(':').unwrap();
    let (hostport, db) = tail.split_once('/').unwrap();
    let (host, port) = hostport.split_once(':').unwrap();
    (
        host.into(),
        port.parse().unwrap(),
        user.into(),
        pw.into(),
        db.split('?').next().unwrap().into(),
    )
}

async fn start() -> Env {
    start_with(|_, _| {}, true, true, false).await
}

/// `tweak(rules, conn)` may adjust the rows; `holds`: wire approvals.
async fn start_with(
    tweak: impl FnOnce(&mut ScopePolicyRows, &mut AppConnectionRow),
    holds: bool,
    default_rules: bool,
    fresh_role: bool,
) -> Env {
    let db = TestDb::new().await;
    sqlx::query("CREATE TABLE dbl_t (id int PRIMARY KEY, v text)")
        .execute(db.pool())
        .await
        .unwrap();
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent_a: Uuid::now_v7(),
        agent_b: Uuid::now_v7(),
    };
    let (host, port, mut user, mut pw, dbname) = parts(db.url());
    if fresh_role {
        // A role whose name and password appear nowhere else, so that a scan of everything the
        // agent receives can prove the database password never reaches it.
        let u = format!("dbl_{}", Uuid::now_v7().simple());
        pw = format!("pw-{}", Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE ROLE {u} LOGIN PASSWORD '{pw}'"))
            .execute(db.pool())
            .await
            .unwrap();
        for g in [
            format!("GRANT CONNECT ON DATABASE \"{dbname}\" TO {u}"),
            format!("GRANT USAGE ON SCHEMA public TO {u}"),
            format!("GRANT ALL ON ALL TABLES IN SCHEMA public TO {u}"),
        ] {
            sqlx::query(&g).execute(db.pool()).await.unwrap();
        }
        user = u;
    }
    let mem = Arc::new(MemoryStore::new("mem"));
    mem.preload(
        "pgconn",
        None,
        json!({"host": host, "port": port, "dbname": dbname, "user": user, "password": pw,
               "sslmode": "disable"})
        .to_string()
        .as_bytes(),
    );
    mem.allow_reference(w.org, "*");
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{key}"), "k1").unwrap());
    let mut reg = StoreRegistry::from_config(&SecretsSection::default(), keyring).unwrap();
    reg.register(mem);
    let conn_id = Uuid::now_v7();
    let mut conn = AppConnectionRow {
        id: conn_id,
        workspace_id: Some(w.ws),
        app_id: "pg".into(),
        label: "analytics".into(),
        status: "active".into(),
        credentials_ref: SecretRef::ext("mem", "pgconn", None, "1")
            .unwrap()
            .to_column(),
        credentials_version: 1,
        credentials_expires_at: None,
        metadata: json!({}),
        tenant_host: None,
        instance_id: None,
    };
    // A database app is default-deny: only the classes a rule allows run.
    let mut ws = ScopePolicyRows {
        default_action: "block".into(),
        ..ScopePolicyRows::default()
    };
    if default_rules {
        ws.rules
            .push(rule("read", &["pg_read"], conn_id, w.agent_a, false));
        ws.rules
            .push(rule("write", &["pg_write"], conn_id, w.agent_a, true));
    }
    tweak(&mut ws, &mut conn);
    let src = Arc::new(Src(Mutex::new(Rows { ws, conn })));
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
    let resolver = Arc::new(Resolver::new(src.clone(), llm).with_snapshots(store));
    let plaintext = Arc::new(PlaintextCache::new(Arc::new(SecretVault::new(reg))));
    let captured = Arc::new(Capture::default());
    let log = RequestLog::new(captured.clone());
    let hub = Hub::new();
    let (shutdown, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(hub.clone().run(db.pool().clone(), rx.clone()));
    while !hub.is_connected() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let holds = holds.then(|| {
        Arc::new(Holds::new(
            db.pool().clone(),
            hub,
            Uuid::now_v7(),
            Duration::from_secs(30),
            Some(rx),
        ))
    });
    let handler = Arc::new(PolicyHandler::new(PolicyHandlerParts {
        resolver,
        plaintext,
        log: log.clone(),
        external_url: "https://aurora.test/".into(),
        holds,
        touch_pool: None,
    }));
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
        conn_id,
        gw_ca: gw.cert_pem,
        db,
        captured,
        log,
        _src: src,
        role: user,
        role_pw: pw,
        _shutdown: shutdown,
    }
}

// ---- a minimal PostgreSQL client ------------------------------------------------------------

type Msg = (u8, Vec<u8>);

struct Pg {
    io: TlsStream<TcpStream>,
    /// Everything the server sent, for secret scans.
    seen: Vec<u8>,
    pid: u32,
    key: u32,
}

fn cstr(v: &mut Vec<u8>, s: &str) {
    v.extend_from_slice(s.as_bytes());
    v.push(0);
}

fn framed(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![tag];
    v.extend_from_slice(&(body.len() as u32 + 4).to_be_bytes());
    v.extend_from_slice(body);
    v
}

fn startup(user: &str) -> Vec<u8> {
    let mut b = 196_608u32.to_be_bytes().to_vec();
    for (k, v) in [("user", user), ("database", "ignored")] {
        cstr(&mut b, k);
        cstr(&mut b, v);
    }
    b.push(0);
    let mut out = (b.len() as u32 + 4).to_be_bytes().to_vec();
    out.extend_from_slice(&b);
    out
}

async fn tls_session(env: &Env, name: &str) -> TlsStream<TcpStream> {
    let mut tcp = TcpStream::connect(env.proxy).await.unwrap();
    let mut req = 8u32.to_be_bytes().to_vec();
    req.extend_from_slice(&80_877_103u32.to_be_bytes());
    tcp.write_all(&req).await.unwrap();
    let mut a = [0u8; 1];
    tcp.read_exact(&mut a).await.unwrap();
    assert_eq!(a[0], b'S');
    TlsConnector::from(crate::common::client_config(&env.gw_ca, &[]))
        .connect(
            rustls::pki_types::ServerName::try_from(name.to_owned()).unwrap(),
            tcp,
        )
        .await
        .unwrap()
}

async fn read_msg<R: AsyncReadExt + Unpin>(io: &mut R) -> Msg {
    let mut h = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(20), io.read_exact(&mut h))
        .await
        .expect("server silent")
        .expect("read");
    let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize - 4;
    let mut b = vec![0u8; len];
    io.read_exact(&mut b).await.unwrap();
    (h[0], b)
}

fn err_fields(b: &[u8]) -> Vec<(u8, String)> {
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() && b[i] != 0 {
        let c = b[i];
        let end = b[i + 1..].iter().position(|x| *x == 0).unwrap() + i + 1;
        out.push((c, String::from_utf8_lossy(&b[i + 1..end]).into_owned()));
        i = end + 1;
    }
    out
}

fn sqlstate(b: &[u8]) -> String {
    err_fields(b)
        .into_iter()
        .find(|(c, _)| *c == b'C')
        .map(|(_, v)| v)
        .unwrap_or_default()
}

fn message(b: &[u8]) -> String {
    err_fields(b)
        .into_iter()
        .find(|(c, _)| *c == b'M')
        .map(|(_, v)| v)
        .unwrap_or_default()
}

impl Pg {
    /// Login as `user`/`password`; `Err((sqlstate, message))` when refused.
    async fn login(env: &Env, user: &str, password: &str) -> Result<Pg, (String, String)> {
        let name = format!("{}.db.aurora.internal", env.conn_id);
        let mut io = tls_session(env, &name).await;
        io.write_all(&startup(user)).await.unwrap();
        let (t, b) = read_msg(&mut io).await;
        if t == b'E' {
            return Err((sqlstate(&b), message(&b)));
        }
        assert_eq!((t, &b[..]), (b'R', &3u32.to_be_bytes()[..]));
        let mut p = Vec::new();
        cstr(&mut p, password);
        io.write_all(&framed(b'p', &p)).await.unwrap();
        let mut pg = Pg {
            io,
            seen: vec![],
            pid: 0,
            key: 0,
        };
        loop {
            let (t, b) = read_msg(&mut pg.io).await;
            pg.seen.extend_from_slice(&b);
            match t {
                b'E' => return Err((sqlstate(&b), message(&b))),
                b'K' => {
                    pg.pid = u32::from_be_bytes(b[0..4].try_into().unwrap());
                    pg.key = u32::from_be_bytes(b[4..8].try_into().unwrap());
                }
                b'Z' => return Ok(pg),
                _ => {}
            }
        }
    }

    async fn ok(env: &Env) -> Pg {
        Pg::login(env, &env.conn_id.to_string(), TOKEN_A)
            .await
            .unwrap()
    }

    async fn send(&mut self, tag: u8, body: &[u8]) {
        self.io.write_all(&framed(tag, body)).await.unwrap();
    }

    async fn until_ready(&mut self) -> Vec<Msg> {
        let mut out = vec![];
        loop {
            let m = read_msg(&mut self.io).await;
            self.seen.extend_from_slice(&m.1);
            let done = m.0 == b'Z';
            out.push(m);
            if done {
                return out;
            }
        }
    }

    async fn query(&mut self, sql: &str) -> Vec<Msg> {
        let mut b = vec![];
        cstr(&mut b, sql);
        self.send(b'Q', &b).await;
        self.until_ready().await
    }

    async fn parse(&mut self, name: &str, sql: &str) {
        let mut b = vec![];
        cstr(&mut b, name);
        cstr(&mut b, sql);
        b.extend_from_slice(&0u16.to_be_bytes());
        self.send(b'P', &b).await;
    }

    async fn bind(&mut self, portal: &str, stmt: &str) {
        let mut b = vec![];
        cstr(&mut b, portal);
        cstr(&mut b, stmt);
        b.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        self.send(b'B', &b).await;
    }

    async fn execute(&mut self, portal: &str) {
        let mut b = vec![];
        cstr(&mut b, portal);
        b.extend_from_slice(&0u32.to_be_bytes());
        self.send(b'E', &b).await;
    }

    async fn sync(&mut self) -> Vec<Msg> {
        self.send(b'S', &[]).await;
        self.until_ready().await
    }
}

fn status(msgs: &[Msg]) -> u8 {
    msgs.last().unwrap().1[0]
}

fn error(msgs: &[Msg]) -> Option<(String, String)> {
    msgs.iter()
        .find(|(t, _)| *t == b'E')
        .map(|(_, b)| (sqlstate(b), message(b)))
}

fn first_col(msgs: &[Msg]) -> Vec<String> {
    msgs.iter()
        .filter(|(t, _)| *t == b'D')
        .map(|(_, b)| {
            let len = i32::from_be_bytes(b[2..6].try_into().unwrap());
            String::from_utf8_lossy(&b[6..6 + len as usize]).into_owned()
        })
        .collect()
}

fn tags(msgs: &[Msg]) -> String {
    msgs.iter().map(|(t, _)| *t as char).collect()
}

async fn rows_in(env: &Env) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM dbl_t")
        .fetch_one(env.db.pool())
        .await
        .unwrap()
}

async fn decide(env: &Env, status: &str) -> Uuid {
    for _ in 0..200 {
        if let Some(id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM approvals WHERE workspace_id=$1 AND status='pending'",
        )
        .bind(env.w.ws)
        .fetch_optional(env.db.pool())
        .await
        .unwrap()
        {
            sqlx::query("UPDATE approvals SET status=$2, decided_at=now() WHERE id=$1")
                .bind(id)
                .bind(status)
                .execute(env.db.pool())
                .await
                .unwrap();
            sqlx::query("SELECT pg_notify('aurora_approval', $1)")
                .bind(json!({"id": id, "workspace_id": env.w.ws, "event": "decided"}).to_string())
                .execute(env.db.pool())
                .await
                .unwrap();
            return id;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no pending approval");
}

// ---- tests ------------------------------------------------------------------------------------

#[tokio::test]
async fn login_read_allowed_and_no_password_leaks() {
    let env = start_with(|_, _| {}, true, true, true).await;
    let mut pg = Pg::ok(&env).await;
    let r = pg.query("select 1 + 1, 'x'").await;
    assert_eq!(first_col(&r), ["2"]);
    assert_eq!(status(&r), b'I');
    // The server authenticated the gateway as the dedicated role...
    let r = pg.query("select current_user").await;
    assert_eq!(first_col(&r), std::slice::from_ref(&env.role));
    // ...and nothing the agent ever received contains that role's password.
    let seen = String::from_utf8_lossy(&pg.seen).into_owned();
    assert!(
        !seen.contains(&env.role_pw),
        "database password reached the agent"
    );
    assert!(
        !seen.contains("SCRAM"),
        "server authentication leaked to the agent"
    );
    // BackendKeyData is the gateway's, not the server's.
    let r = pg.query("select pg_backend_pid()").await;
    assert_ne!(first_col(&r)[0], pg.pid.to_string());
    // Transaction statements are always allowed.
    let r = pg.query("begin; select 1; commit").await;
    assert!(error(&r).is_none(), "{r:?}");
    // the request log carries the synthetic request
    let mut rows = vec![];
    for _ in 0..50 {
        env.log.flush().await;
        rows = env.captured.0.lock().unwrap().clone();
        if rows.iter().any(|r| r.path == "/read") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let row = rows.iter().find(|r| r.path == "/read").expect("log row");
    assert_eq!(row.lane, "db");
    assert_eq!(row.method, "POST");
    assert_eq!(row.host, format!("{}.db.aurora.internal", env.conn_id));
    assert_eq!(row.tool_id.as_deref(), Some("pg_read"));
    assert_eq!(row.connection_id, Some(env.conn_id));
    // remove the test role from the shared cluster
    drop(pg);
    tokio::time::sleep(Duration::from_millis(300)).await;
    sqlx::query(&format!("DROP OWNED BY {}", env.role))
        .execute(env.db.pool())
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE {}", env.role))
        .execute(env.db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn login_refusals() {
    let env = start().await;
    let id = env.conn_id.to_string();
    // wrong credential
    let e = Pg::login(&env, &id, "aug_nope").await.err().unwrap();
    assert_eq!(e.0, "28P01");
    // (RFE-013, C-312) an expired agent token is refused like a revoked one: the same FATAL
    // `28P01` and the same words, so the lane discloses nothing new.
    let ex = Pg::login(&env, &id, TOKEN_EXPIRED).await.err().unwrap();
    assert_eq!(
        (ex.0.clone(), ex.1.clone()),
        (e.0.clone(), e.1.clone()),
        "{ex:?}"
    );
    // user is not a connection id
    let e = Pg::login(&env, "postgres", TOKEN_A).await.err().unwrap();
    assert_eq!(e.0, "28000");
    // an agent without a grant (and an unknown connection id) get the same answer
    let e = Pg::login(&env, &id, TOKEN_B).await.err().unwrap();
    assert_eq!(e.0, "42501", "{e:?}");
    let e2 = Pg::login(&env, &Uuid::now_v7().to_string(), TOKEN_A)
        .await
        .err()
        .unwrap();
    assert_eq!((e.0, e.1), (e2.0, e2.1));
    // frozen agent: same admission check as HTTP
    let e = Pg::login(&env, &id, TOKEN_FROZEN).await.err().unwrap();
    assert_eq!(e.0, "28000");
    assert!(e.1.contains("paused"), "{e:?}");
}

#[tokio::test]
async fn startup_without_tls_is_refused_before_any_password() {
    let env = start().await;
    let mut tcp = TcpStream::connect(env.proxy).await.unwrap();
    tcp.write_all(&startup(&env.conn_id.to_string()))
        .await
        .unwrap();
    let (t, b) = read_msg(&mut tcp).await;
    assert_eq!(t, b'E');
    assert_eq!(sqlstate(&b), "28000");
    assert!(message(&b).contains("TLS"));
    let mut rest = vec![];
    let _ = tcp.read_to_end(&mut rest).await;
    assert!(rest.is_empty(), "no password request was sent");
}

#[tokio::test]
async fn disabled_and_non_database_connections_are_refused() {
    let env = start_with(|_, c| c.status = "disabled".into(), true, true, false).await;
    let e = Pg::login(&env, &env.conn_id.to_string(), TOKEN_A)
        .await
        .err()
        .unwrap();
    assert_eq!(e.0, "42501");
    assert!(e.1.contains("disabled"), "{e:?}");
    let env = start_with(|_, c| c.app_id = "other".into(), true, true, false).await;
    let e = Pg::login(&env, &env.conn_id.to_string(), TOKEN_A)
        .await
        .err()
        .unwrap();
    assert!(e.1.contains("not a PostgreSQL"), "{e:?}");
}

#[tokio::test]
async fn write_needs_approval_then_runs() {
    let env = start().await;
    let mut pg = Pg::ok(&env).await;
    let mut b = vec![];
    cstr(&mut b, "insert into dbl_t values (1, 'secret-literal')");
    pg.send(b'Q', &b).await;
    let id = decide(&env, "approved").await;
    let r = pg.until_ready().await;
    assert!(error(&r).is_none(), "{r:?}");
    assert_eq!(rows_in(&env).await, 1);
    // the approval shows the statement with literals redacted
    let summary: Value = sqlx::query_scalar("SELECT summary FROM approvals WHERE id=$1")
        .bind(id)
        .fetch_one(env.db.pool())
        .await
        .unwrap();
    let s = summary.to_string();
    assert!(s.contains("insert into dbl_t values (?, ?)"), "{s}");
    assert!(!s.contains("secret-literal"), "{s}");
    // denied
    let mut b = vec![];
    cstr(&mut b, "insert into dbl_t values (2, 'b')");
    pg.send(b'Q', &b).await;
    decide(&env, "denied").await;
    let r = pg.until_ready().await;
    let e = error(&r).unwrap();
    assert_eq!(e.0, "42501");
    assert_eq!(rows_in(&env).await, 1);
    // the session is still usable
    assert_eq!(first_col(&pg.query("select 5").await), ["5"]);
}

#[tokio::test]
async fn ddl_admin_and_always_refused_never_reach_the_server() {
    let env = start().await;
    let mut pg = Pg::ok(&env).await;
    for sql in [
        "drop table dbl_t",
        "vacuum",
        "alter system set work_mem = '1GB'",
        "set role postgres",
        "select pg_read_file('/etc/passwd')",
        "copy dbl_t from program 'id'",
        "this is not sql",
        "select 1; drop table dbl_t",
    ] {
        let r = pg.query(sql).await;
        let e = error(&r).unwrap_or_else(|| panic!("{sql} was not refused: {r:?}"));
        assert_eq!(e.0, "42501", "{sql}: {e:?}");
        assert_eq!(status(&r), b'I');
    }
    assert_eq!(rows_in(&env).await, 0);
    sqlx::query("SELECT 1 FROM dbl_t")
        .execute(env.db.pool())
        .await
        .unwrap();
    assert_eq!(first_col(&pg.query("select 1").await), ["1"]);
}

#[tokio::test]
async fn refusal_inside_a_transaction_aborts_it_like_a_server_error() {
    let env = start().await;
    let mut pg = Pg::ok(&env).await;
    assert_eq!(status(&pg.query("begin").await), b'T');
    let r = pg.query("drop table dbl_t").await;
    assert_eq!(error(&r).unwrap().0, "42501");
    assert_eq!(status(&r), b'E');
    // the server really is in the aborted state
    let r = pg.query("select 1").await;
    assert_eq!(error(&r).unwrap().0, "25P02");
    assert_eq!(status(&pg.query("rollback").await), b'I');
    assert_eq!(first_col(&pg.query("select 2").await), ["2"]);
}

#[tokio::test]
async fn prepared_statement_state_stays_consistent_after_a_refusal() {
    let env = start().await;
    let mut pg = Pg::ok(&env).await;
    // Parse of a never-allowed statement: refused at once, nothing reaches the server; the rest
    // of the pipeline is discarded until Sync, as after a server error.
    pg.parse("bad", "alter system set work_mem = '1GB'").await;
    pg.bind("", "bad").await;
    pg.execute("").await;
    let r = pg.sync().await;
    assert_eq!(tags(&r), "EZ");
    assert_eq!(error(&r).unwrap().0, "42501");
    // a later Bind of the refused name fails like on a server that never saw it
    pg.bind("", "bad").await;
    let r = pg.sync().await;
    assert_eq!(error(&r).unwrap().0, "26000");
    // a policy refusal at Execute: Parse/Bind went through, Execute is refused
    pg.parse("ddl", "drop table dbl_t").await;
    pg.bind("", "ddl").await;
    pg.execute("").await;
    let r = pg.sync().await;
    assert_eq!(error(&r).unwrap().0, "42501");
    assert_eq!(tags(&r), "12EZ", "{r:?}");
    // and the connection keeps working, including a good prepared statement
    pg.parse("good", "select 40 + 2").await;
    pg.bind("p1", "good").await;
    pg.execute("p1").await;
    let r = pg.sync().await;
    assert!(error(&r).is_none(), "{r:?}");
    assert_eq!(first_col(&r), ["42"]);
    // re-executing the same prepared statement goes through policy again
    pg.bind("", "good").await;
    pg.execute("").await;
    assert_eq!(first_col(&pg.sync().await), ["42"]);
    // refusal in a transaction leaves the server aborted
    pg.query("begin").await;
    pg.parse("", "drop table dbl_t").await;
    pg.bind("", "").await;
    pg.execute("").await;
    let r = pg.sync().await;
    assert_eq!(status(&r), b'E');
    assert_eq!(status(&pg.query("rollback").await), b'I');
    assert_eq!(rows_in(&env).await, 0);
}

#[tokio::test]
async fn a_name_cannot_be_reused_to_swap_the_statement_under_policy() {
    let env = start().await;
    let mut pg = Pg::ok(&env).await;
    // "s" = a write (needs approval, which no one will give: denied below)
    pg.parse("s", "insert into dbl_t values (9, 'x')").await;
    // Re-parsing the same name as a read would make the server keep the write: refused here.
    pg.parse("s", "select 1").await;
    pg.bind("", "s").await;
    pg.execute("").await;
    let r = pg.sync().await;
    assert_eq!(error(&r).unwrap().0, "42P05", "{r:?}");
    // Executing "s" is judged as the write it is: it waits for approval and is denied.
    pg.bind("", "s").await;
    pg.execute("").await;
    pg.send(b'S', &[]).await;
    decide(&env, "denied").await;
    let r = pg.until_ready().await;
    assert_eq!(error(&r).unwrap().0, "42501", "{r:?}");
    assert_eq!(rows_in(&env).await, 0);
}

#[tokio::test]
async fn cancel_uses_gateway_keys_mapped_to_server_keys() {
    let env = start_with(
        |ws, _| {
            // admin allowed too, for pg_sleep
            let id = ws.rules[0].targets[0]["id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let a = ws.rules[0].identities[0]["id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            ws.rules.push(rule("admin", &["pg_admin"], id, a, false));
        },
        true,
        true,
        false,
    )
    .await;
    let mut pg = Pg::ok(&env).await;
    let mut b = vec![];
    cstr(&mut b, "select pg_sleep(30)");
    pg.send(b'Q', &b).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    // a wrong key does nothing
    let mut tcp = TcpStream::connect(env.proxy).await.unwrap();
    let mut c = 16u32.to_be_bytes().to_vec();
    c.extend_from_slice(&80_877_102u32.to_be_bytes());
    c.extend_from_slice(&pg.pid.to_be_bytes());
    c.extend_from_slice(&(pg.key ^ 1).to_be_bytes());
    tcp.write_all(&c).await.unwrap();
    let mut rest = vec![];
    let _ = tcp.read_to_end(&mut rest).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // the right gateway key cancels the server-side query
    let mut tcp = TcpStream::connect(env.proxy).await.unwrap();
    let mut c = 16u32.to_be_bytes().to_vec();
    c.extend_from_slice(&80_877_102u32.to_be_bytes());
    c.extend_from_slice(&pg.pid.to_be_bytes());
    c.extend_from_slice(&pg.key.to_be_bytes());
    tcp.write_all(&c).await.unwrap();
    let r = pg.until_ready().await;
    assert_eq!(error(&r).unwrap().0, "57014", "{r:?}");
    assert_eq!(first_col(&pg.query("select 1").await), ["1"]);
}

#[tokio::test]
async fn http_proxy_traffic_is_not_captured_by_the_sniffer() {
    let env = start().await;
    // A CONNECT without credentials still gets the HTTP answer (407) over TLS.
    let mut s = crate::common::connect(env.proxy).await;
    s.write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
        .await
        .unwrap();
    let mut out = vec![0u8; 64];
    let n = s.read(&mut out).await.unwrap();
    assert!(String::from_utf8_lossy(&out[..n]).starts_with("HTTP/1.1 407"));
    // Plain HTTP bytes are still dropped without an answer.
    let mut t = TcpStream::connect(env.proxy).await.unwrap();
    t.write_all(b"GET http://example.com/ HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    let mut rest = vec![];
    let _ = t.read_to_end(&mut rest).await;
    assert!(rest.is_empty());
}
