#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! GW-011: token freshness: refresh with advisory lock + CAS, reauth, token.mint.

use aurora_config::SecretsSection;
use aurora_core::Secret;
use aurora_db::repo::gateway::AppConnectionRow;
use aurora_gateway::apps::ConnCand;
use aurora_gateway::mint::{HttpMinter, MintError, MintInput, MintOutput, Minter};
use aurora_gateway::refresh::{RefreshError, TokenManager};
use aurora_gateway::snapshot::{Loaded, parse};
use aurora_secrets::{Purpose, SealContext, SecretVault, StoreRegistry};
use aurora_testkit::TestDb;
use axum::Router;
use axum::extract::State;
use axum::routing::post;
use base64::Engine;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

fn vault() -> Arc<SecretVault> {
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{key}"), "k1").unwrap());
    Arc::new(SecretVault::new(
        StoreRegistry::from_config(&SecretsSection::default(), keyring).unwrap(),
    ))
}

/// Single-use refresh tokens: reusing a spent one revokes (invalid_grant).
#[derive(Default)]
struct Provider {
    calls: usize,
    current: String,
}

async fn token_endpoint(
    State(p): State<Arc<Mutex<Provider>>>,
    body: String,
) -> (http::StatusCode, String) {
    tokio::time::sleep(Duration::from_millis(150)).await;
    let form: HashMap<String, String> = body
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let mut p = p.lock().unwrap();
    p.calls += 1;
    if form.get("refresh_token") != Some(&p.current) || !form.contains_key("client_id") {
        return (
            http::StatusCode::BAD_REQUEST,
            json!({"error": "invalid_grant"}).to_string(),
        );
    }
    p.current = format!("r{}", p.calls);
    (
        http::StatusCode::OK,
        json!({"access_token": format!("a{}", p.calls), "refresh_token": p.current, "expires_in": 3600})
            .to_string(),
    )
}

async fn serve(app: Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{a}")
}

struct Fx {
    db: TestDb,
    vault: Arc<SecretVault>,
    loaded: Loaded,
    cand: ConnCand,
    org: Uuid,
}

fn snapshot(kind: &str, token_url: &str) -> Loaded {
    parse(
        "h",
        json!({
            "format": 1, "contract": 1,
            "packages": [{"package_id": "pkg", "version": "1", "digest": "dg", "publisher": "p",
                          "hooks_digest": null, "egress": {}, "context": []}],
            "oauth_clients": [{"package_id": "pkg", "client_key": "g", "authorize_url": "https://x/a",
                               "token_url": token_url, "pkce": false, "client_auth": "body",
                               "body_format": "form", "fields": []}],
            "apps": [{"app_id": "demo", "package_id": "pkg", "about": {"id": "demo", "name": "demo"},
                      "credentials": [], "metadata": [], "connect": [],
                      "token": {"kind": kind, "client": "g", "refresh_field": "refresh_token",
                                "rotate_refresh_token": true},
                      "tools": null, "approvals": {}, "agent": null, "database": null}],
            "hosts": [], "reserved_hosts": []
        }),
    )
    .unwrap()
}

async fn fixture(kind: &str, token_url: &str, refresh: &str) -> Fx {
    let db = TestDb::new().await;
    let p = db.pool();
    let vault = vault();
    let (org, ws, client, conn) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o','O','ext')",
    )
    .bind(org)
    .execute(p)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'w','W')")
        .bind(ws)
        .bind(org)
        .execute(p)
        .await
        .unwrap();
    let cctx = SealContext {
        purpose: Purpose::OauthClientSecret,
        owner_id: client,
        org_id: org,
        workspace_id: None,
    };
    let cref = vault
        .seal(
            &cctx,
            Secret::from(json!({"client_secret": "shh"}).to_string().into_bytes()),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_oauth_clients (id, org_id, package_id, client_key, fields, secret_ref) VALUES ($1,$2,'pkg','g',$3,$4)")
        .bind(client).bind(org).bind(json!({"client_id": "cid"})).bind(cref.to_column())
        .execute(p).await.unwrap();
    let ctx = SealContext {
        purpose: Purpose::ConnectionCredentials,
        owner_id: conn,
        org_id: org,
        workspace_id: Some(ws),
    };
    let r = vault
        .seal(
            &ctx,
            Secret::from(
                json!({"access_token": "a0", "refresh_token": refresh})
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, credentials_expires_at, oauth_client_id) VALUES ($1,$2,$3,'demo','pkg','1','L','active',$4, now() - interval '10 seconds', $5)")
        .bind(conn).bind(org).bind(ws).bind(r.to_column()).bind(client)
        .execute(p).await.unwrap();
    let row = AppConnectionRow {
        id: conn,
        workspace_id: Some(ws),
        app_id: "demo".into(),
        label: "L".into(),
        status: "active".into(),
        credentials_ref: r.to_column(),
        credentials_version: 1,
        credentials_expires_at: Some(chrono::Utc::now() - chrono::Duration::seconds(10)),
        metadata: json!({}),
        tenant_host: None,
    };
    Fx {
        db,
        vault,
        loaded: snapshot(kind, token_url),
        cand: ConnCand::from_row(row, org).unwrap(),
        org,
    }
}

async fn opened(fx: &Fx, c: &ConnCand) -> Value {
    let ctx = SealContext {
        purpose: Purpose::ConnectionCredentials,
        owner_id: c.id,
        org_id: fx.org,
        workspace_id: c.workspace_id,
    };
    serde_json::from_slice(fx.vault.open(&ctx, &c.vref).await.unwrap().expose()).unwrap()
}

async fn status(fx: &Fx) -> (String, i64) {
    sqlx::query_as("SELECT status, credentials_version FROM app_connections WHERE id=$1")
        .bind(fx.cand.id)
        .fetch_one(fx.db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn two_gateways_one_provider_call() {
    let prov = Arc::new(Mutex::new(Provider {
        calls: 0,
        current: "r0".into(),
    }));
    let base = serve(
        Router::new()
            .route("/token", post(token_endpoint))
            .with_state(prov.clone()),
    )
    .await;
    let fx = fixture("oauth2_refresh", &format!("{base}/token"), "r0").await;
    let m1 = Arc::new(TokenManager::new(
        fx.db.pool().clone(),
        fx.vault.clone(),
        None,
    ));
    let m2 = Arc::new(TokenManager::new(
        fx.db.pool().clone(),
        fx.vault.clone(),
        None,
    ));
    let ctx = HashMap::new();
    let (a, b) = tokio::join!(
        m1.ensure_fresh(&fx.loaded, &fx.cand, &ctx, false),
        m2.ensure_fresh(&fx.loaded, &fx.cand, &ctx, false)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(prov.lock().unwrap().calls, 1, "exactly one provider call");
    assert_eq!((a.version, b.version), (2, 2));
    let creds = opened(&fx, &a).await;
    assert_eq!(creds["access_token"], "a1");
    assert_eq!(creds["refresh_token"], "r1", "rotated refresh token stored");
    assert_eq!(status(&fx).await, ("active".into(), 2));
    // Fresh now: no further call.
    let again = m1.ensure_fresh(&fx.loaded, &a, &ctx, false).await.unwrap();
    assert_eq!(again.version, 2);
    assert_eq!(prov.lock().unwrap().calls, 1);
    // A stale-code refresh from a holder of the old version adopts the new one.
    let forced = m2
        .ensure_fresh(&fx.loaded, &fx.cand, &ctx, true)
        .await
        .unwrap();
    assert_eq!(forced.version, 2);
    assert_eq!(prov.lock().unwrap().calls, 1);
    // A forced refresh on the current version does exchange.
    let forced = m2.ensure_fresh(&fx.loaded, &a, &ctx, true).await.unwrap();
    assert_eq!(forced.version, 3);
    assert_eq!(prov.lock().unwrap().calls, 2);
}

#[tokio::test]
async fn invalid_grant_marks_reauth_required() {
    let prov = Arc::new(Mutex::new(Provider {
        calls: 0,
        current: "other".into(),
    }));
    let base = serve(
        Router::new()
            .route("/token", post(token_endpoint))
            .with_state(prov.clone()),
    )
    .await;
    let fx = fixture("oauth2_refresh", &format!("{base}/token"), "r0").await;
    let m = Arc::new(TokenManager::new(
        fx.db.pool().clone(),
        fx.vault.clone(),
        None,
    ));
    let e = m
        .ensure_fresh(&fx.loaded, &fx.cand, &HashMap::new(), false)
        .await
        .unwrap_err();
    assert_eq!(e, RefreshError::Reauth);
    assert_eq!(status(&fx).await, ("reauth_required".into(), 1));
    // Subsequent attempts stop at the re-read without calling the provider.
    let e = m
        .ensure_fresh(&fx.loaded, &fx.cand, &HashMap::new(), false)
        .await
        .unwrap_err();
    assert_eq!(e, RefreshError::Reauth);
    assert_eq!(prov.lock().unwrap().calls, 1);
}

#[tokio::test]
async fn provider_5xx_is_transient() {
    let app = Router::new().route(
        "/token",
        post(|| async { (http::StatusCode::BAD_GATEWAY, "x") }),
    );
    let base = serve(app).await;
    let fx = fixture("oauth2_refresh", &format!("{base}/token"), "r0").await;
    let m = Arc::new(TokenManager::new(
        fx.db.pool().clone(),
        fx.vault.clone(),
        None,
    ));
    let e = m
        .ensure_fresh(&fx.loaded, &fx.cand, &HashMap::new(), false)
        .await
        .unwrap_err();
    assert_eq!(e, RefreshError::Unavailable);
    assert_eq!(status(&fx).await, ("active".into(), 1));
}

struct Stub(Mutex<usize>);

#[async_trait::async_trait]
impl Minter for Stub {
    async fn mint(&self, i: MintInput) -> Result<MintOutput, MintError> {
        *self.0.lock().unwrap() += 1;
        assert_eq!(i.context.get("org_id").map(String::as_str), Some("o1"));
        assert_eq!(i.client_secret["client_secret"], "shh");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut c = serde_json::Map::new();
        c.insert("access_token".into(), json!("minted"));
        Ok(MintOutput {
            credentials: c,
            expires_at: chrono::Utc::now().timestamp() + 600,
        })
    }
}

#[tokio::test]
async fn mint_under_the_same_lock() {
    let fx = fixture("hook", "http://unused/token", "r0").await;
    let stub = Arc::new(Stub(Mutex::new(0)));
    let mk = || {
        Arc::new(TokenManager::new(
            fx.db.pool().clone(),
            fx.vault.clone(),
            Some(stub.clone()),
        ))
    };
    let (m1, m2) = (mk(), mk());
    let ctx = HashMap::from([("org_id".to_owned(), "o1".to_owned())]);
    let (a, b) = tokio::join!(
        m1.ensure_fresh(&fx.loaded, &fx.cand, &ctx, false),
        m2.ensure_fresh(&fx.loaded, &fx.cand, &ctx, false)
    );
    assert_eq!((a.unwrap().version, b.unwrap().version), (2, 2));
    assert_eq!(*stub.0.lock().unwrap(), 1);
}

#[tokio::test]
async fn http_minter_follows_w9() {
    let seen = Arc::new(Mutex::new(Value::Null));
    let s = seen.clone();
    let app = Router::new().route(
        "/internal/v1/hooks/token.mint",
        post(move |axum::Json(b): axum::Json<Value>| {
            let s = s.clone();
            async move {
                *s.lock().unwrap() = b;
                axum::Json(json!({"ok": {"credentials": [{"key": "access_token", "value": "t"}], "expires_at": 4102444800i64}}))
            }
        }),
    );
    let base = serve(app).await;
    let m = HttpMinter::new(&base, "sec");
    let out = m
        .mint(MintInput {
            package_id: "pkg".into(),
            package_digest: "dg".into(),
            app_id: "demo".into(),
            credentials: serde_json::Map::new(),
            metadata: json!({"a": "b"}),
            tenant_host: None,
            client: serde_json::Map::new(),
            client_secret: serde_json::Map::new(),
            context: Default::default(),
        })
        .await
        .unwrap();
    assert_eq!(out.credentials["access_token"], "t");
    let b = seen.lock().unwrap().clone();
    assert_eq!(b["package_digest"], "dg");
    assert_eq!(b["input"]["metadata"][0]["key"], "a");
}

// ---- RFE-007: internal-secret ring on the gateway ----

#[tokio::test]
async fn minter_presents_active_key_id_and_status_checks_the_ring() {
    use aurora_crypto::{RingKind, SecretRing};
    let seen = Arc::new(std::sync::Mutex::new(
        Vec::<(Option<String>, Option<String>)>::new(),
    ));
    let s = seen.clone();
    let app = axum::Router::new().route(
        "/internal/v1/hooks/token.mint",
        axum::routing::post(move |h: axum::http::HeaderMap| {
            let s = s.clone();
            async move {
                let get = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).map(str::to_owned);
                s.lock().unwrap().push((get("authorization"), get("x-aurora-key-id")));
                axum::Json(json!({"ok": {"credentials": [{"key": "access_token", "value": "t"}], "expires_at": 4102444800i64}}))
            }
        }),
    );
    let base = serve(app).await;
    let b = |n: u8| base64::engine::general_purpose::STANDARD.encode([n; 32]);
    let ring = Arc::new(
        SecretRing::parse(
            &format!("a1:{},b2:{}", b(1), b(2)),
            Some("b2"),
            RingKind::Internal,
        )
        .unwrap(),
    );
    let m = HttpMinter::new(&base, &ring.active_bearer()).with_key_id(ring.active_id());
    m.mint(MintInput {
        package_id: "pkg".into(),
        package_digest: "dg".into(),
        app_id: "demo".into(),
        credentials: serde_json::Map::new(),
        metadata: json!({}),
        tenant_host: None,
        client: serde_json::Map::new(),
        client_secret: serde_json::Map::new(),
        context: Default::default(),
    })
    .await
    .unwrap();
    let got = seen.lock().unwrap().clone();
    assert_eq!(
        got,
        vec![(Some(format!("Bearer {}", b(2))), Some("b2".to_owned()))]
    );

    // Status endpoint: any ring key, ids/fingerprints only, retired/unknown refused.
    use tower::ServiceExt;
    let st = aurora_gateway::status::router(ring.clone());
    let call = |n: Option<u8>, id: Option<&str>| {
        let mut r = axum::http::Request::builder().uri("/internal/v1/status");
        if let Some(n) = n {
            r = r.header("authorization", format!("Bearer {}", b(n)));
        }
        if let Some(id) = id {
            r = r.header("x-aurora-key-id", id);
        }
        r.body(axum::body::Body::empty()).unwrap()
    };
    for (n, id, want) in [
        (None, None, 401),
        (Some(1), Some("a1"), 200),
        (Some(2), Some("b2"), 200),
        (Some(1), None, 200),
        (Some(1), Some("b2"), 401),
        (Some(3), None, 401),
        (Some(2), Some("zz"), 401),
    ] {
        let res = st.clone().oneshot(call(n, id)).await.unwrap();
        assert_eq!(res.status().as_u16(), want, "{n:?} {id:?}");
        if want == 200 {
            let body = http_body_util::BodyExt::collect(res.into_body())
                .await
                .unwrap()
                .to_bytes();
            let text = String::from_utf8(body.to_vec()).unwrap();
            assert!(text.contains("\"gateway\"") && text.contains("\"b2\""));
            assert!(
                !text.contains(&b(1)) && !text.contains(&b(2)),
                "leaked key material"
            );
        }
    }
}
