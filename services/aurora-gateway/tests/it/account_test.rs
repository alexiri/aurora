//! (phase 5, P5-UX-14, D-335, C-943, SEC-129) `POST /internal/v1/account-test` for a database app: a login through
//! the gateway's own guarded dialler up to `ReadyForQuery`, then close — **no statement** — answered as a code
//! with at most a host, never the server's own text. The target is a real PostgreSQL (the testkit database) and a
//! scripted fake server that records everything a client sends after the startup.
//!
//! A child module of `database` (it reuses that file's environment).
use super::*;
use aurora_core::Secret;
use aurora_crypto::{RingKind, SecretRing};
use aurora_gateway::account_test::{self, AccountTest};
use aurora_secrets::{Purpose, SealContext};
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

struct Fx {
    env: Env,
    test: Arc<AccountTest>,
    ring: Arc<SecretRing>,
}

async fn start_test(fresh_role: bool, guard_allows_loopback: bool) -> Fx {
    let env = start_with(|_, _| {}, false, false, fresh_role).await;
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o','O','ext')",
    )
    .bind(env.w.org)
    .execute(env.db.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'w','W')")
        .bind(env.w.ws)
        .bind(env.w.org)
        .execute(env.db.pool())
        .await
        .unwrap();
    let b = |n: u8| base64::engine::general_purpose::STANDARD.encode([n; 32]);
    let ring = Arc::new(
        SecretRing::parse(&format!("a1:{}", b(1)), Some("a1"), RingKind::Internal).unwrap(),
    );
    let allowed: Vec<String> = if guard_allows_loopback {
        vec!["127.0.0.0/8".to_owned()]
    } else {
        vec![]
    };
    let test = Arc::new(AccountTest {
        ring: ring.clone(),
        snapshots: env.snapshots.clone(),
        pool: env.db.pool().clone(),
        tokens: Arc::new(aurora_gateway::refresh::TokenManager::new(
            env.db.pool().clone(),
            env.vault.clone(),
            None,
        )),
        plaintext: env.plaintext.clone(),
        guard: Arc::new(Guard::new(&allowed, vec![]).unwrap()),
        egress: None,
        external_url: "https://aurora.test".into(),
        limiter: aurora_gateway::library_fetch::Limiter::with_rates(100_000.0, 100_000.0),
    });
    Fx { env, test, ring }
}

impl Fx {
    /// A database connection of the organization with these credentials.
    async fn connection(&self, creds: Value) -> Uuid {
        let id = Uuid::now_v7();
        let ctx = SealContext {
            purpose: Purpose::ConnectionCredentials,
            owner_id: id,
            org_id: self.env.w.org,
            workspace_id: Some(self.env.w.ws),
        };
        let r = self
            .env
            .vault
            .seal(&ctx, Secret::from(creds.to_string().into_bytes()))
            .await
            .unwrap();
        sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'pg','pkg','1','L','active',$4)")
            .bind(id).bind(self.env.w.org).bind(self.env.w.ws).bind(r.to_column())
            .execute(self.env.db.pool()).await.unwrap();
        id
    }

    async fn call(&self, auth: bool, body: Value) -> (u16, Value) {
        let mut b = Request::builder()
            .method(Method::POST)
            .uri("/internal/v1/account-test")
            .header("content-type", "application/json");
        if auth {
            b = b
                .header(
                    "authorization",
                    format!("Bearer {}", self.ring.active_bearer()),
                )
                .header("x-aurora-key-id", self.ring.active_id());
        }
        let res = account_test::router(self.test.clone())
            .oneshot(b.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn database(&self, conn: Uuid) -> Value {
        let (s, v) = self
            .call(
                true,
                json!({"org_id": self.env.w.org, "connection_id": conn, "check": "database"}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }
}

/// A scripted PostgreSQL server: answers the startup with `AuthenticationOk` and `ReadyForQuery`, then records
/// every byte the client sends until it closes the socket.
async fn fake_pg() -> (u16, tokio::sync::oneshot::Receiver<Vec<u8>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut len = [0u8; 4];
        s.read_exact(&mut len).await.unwrap();
        let mut startup = vec![0u8; u32::from_be_bytes(len) as usize - 4];
        s.read_exact(&mut startup).await.unwrap();
        // AuthenticationOk, then ReadyForQuery (idle).
        s.write_all(&[b'R', 0, 0, 0, 8, 0, 0, 0, 0, b'Z', 0, 0, 0, 5, b'I'])
            .await
            .unwrap();
        let mut after = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut after)).await;
        let _ = tx.send(after);
    });
    (port, rx)
}

#[tokio::test]
async fn a_login_that_succeeds_is_ok_and_no_statement_follows_ready_for_query() {
    let fx = start_test(false, true).await;
    let (port, after) = fake_pg().await;
    let conn = fx
        .connection(json!({"host": "127.0.0.1", "port": port, "dbname": "d", "user": "u", "password": "p", "sslmode": "disable"}))
        .await;
    let v = fx.database(conn).await;
    assert_eq!(v["result"], "ok", "{v}");
    // The gateway closed the session without sending anything after the startup (no `Query`, `Parse`, ...).
    let after = tokio::time::timeout(Duration::from_secs(10), after)
        .await
        .expect("the gateway left the session open")
        .unwrap();
    assert!(after.is_empty(), "bytes after ReadyForQuery: {after:?}");
}

#[tokio::test]
async fn the_wrong_password_is_rejected_with_a_host_and_none_of_the_servers_text() {
    let fx = start_test(true, true).await;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "ALTER ROLE {} PASSWORD 'rotated-behind-the-back'",
        fx.env.role
    )))
    .execute(fx.env.db.pool())
    .await
    .unwrap();
    let (host, port, ..) = parts(fx.env.db.url());
    let conn = fx
        .connection(
            json!({"host": host, "port": port, "dbname": parts(fx.env.db.url()).4,
                           "user": fx.env.role, "password": fx.env.role_pw, "sslmode": "disable"}),
        )
        .await;
    let v = fx.database(conn).await;
    assert_eq!(v["result"], "rejected", "{v}");
    let text = v.to_string();
    for leaked in [
        fx.env.role.as_str(),
        "authentication failed",
        fx.env.role_pw.as_str(),
    ] {
        assert!(!text.contains(leaked), "{leaked:?} in the answer: {text}");
    }
    let (s, role) = (fx.env.db.pool(), fx.env.role.clone());
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP OWNED BY {role}")))
        .execute(s)
        .await
        .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP ROLE {role}")))
        .execute(s)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_correct_login_to_the_real_server_is_ok() {
    let fx = start_test(true, true).await;
    let (host, port, _, _, dbname) = parts(fx.env.db.url());
    let conn = fx
        .connection(json!({"host": host, "port": port, "dbname": dbname,
                           "user": fx.env.role, "password": fx.env.role_pw, "sslmode": "disable"}))
        .await;
    assert_eq!(fx.database(conn).await["result"], "ok");
    let (s, role) = (fx.env.db.pool(), fx.env.role.clone());
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP OWNED BY {role}")))
        .execute(s)
        .await
        .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP ROLE {role}")))
        .execute(s)
        .await
        .unwrap();
}

#[tokio::test]
async fn nothing_listening_is_unreachable_with_the_host() {
    let fx = start_test(false, true).await;
    // A port that was just closed.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let conn = fx
        .connection(json!({"host": "127.0.0.1", "port": port, "dbname": "d", "user": "u", "password": "p", "sslmode": "disable"}))
        .await;
    let v = fx.database(conn).await;
    assert_eq!(v["result"], "unreachable", "{v}");
    assert_eq!(v["detail"]["host"], "127.0.0.1");
}

#[tokio::test]
async fn a_private_address_is_refused_like_agent_traffic() {
    // The gateway's destination guard allows no private range here: the same guard that judges agent traffic.
    let fx = start_test(false, false).await;
    let (port, after) = fake_pg().await;
    let conn = fx
        .connection(json!({"host": "127.0.0.1", "port": port, "dbname": "d", "user": "u", "password": "p", "sslmode": "disable"}))
        .await;
    let v = fx.database(conn).await;
    assert_eq!(v["result"], "unreachable", "{v}");
    // The fake server never saw a connection.
    drop(after);
}

#[tokio::test]
async fn the_route_needs_the_internal_secret_and_a_connection_of_the_organization() {
    let fx = start_test(false, true).await;
    let conn = fx.connection(json!({"host": "127.0.0.1"})).await;
    let body = json!({"org_id": fx.env.w.org, "connection_id": conn, "check": "database"});
    assert_eq!(fx.call(false, body).await.0, 401);
    let other = json!({"org_id": Uuid::now_v7(), "connection_id": conn, "check": "database"});
    assert_eq!(fx.call(true, other).await.0, 404);
    // A malformed or unknown request is refused, never run.
    let bad = json!({"org_id": fx.env.w.org, "connection_id": conn, "check": "query"});
    assert_eq!(fx.call(true, bad).await.0, 400);
}
