#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-GSL (D-149, SEC-84): `POST /internal/v1/library-fetch` — the request is built from the
//! package's `library` templates, only for an org-scoped active connection of the named org,
//! with the credential injected by the host entry, no redirects, size cap, internal secret.
//! Real Postgres (connection rows), the fake git host, a mock OAuth token endpoint.

use aurora_config::SecretsSection;
use aurora_core::Secret;
use aurora_crypto::{RingKind, SecretRing};
use aurora_gateway::library_fetch::{self, LibraryFetch};
use aurora_gateway::pipeline::Scheme;
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::refresh::TokenManager;
use aurora_gateway::snapshot::{SnapshotStore, parse};
use aurora_gateway::upstream::Upstream;
use aurora_secrets::{Purpose, SealContext, SecretVault, StoreRegistry};
use aurora_testkit::TestDb;
use aurora_testkit::git_host::{Change, FakeGitHost};
use axum::body::Body;
use base64::Engine;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

const TOKEN: &str = "tok-secret";

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn vault() -> Arc<SecretVault> {
    let key = b64(&[7u8; 32]);
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&format!("k1:{key}"), "k1").unwrap());
    Arc::new(SecretVault::new(
        StoreRegistry::from_config(&SecretsSection::default(), keyring).unwrap(),
    ))
}

fn library_block(host: &str) -> Value {
    json!({
        "repo_pattern": "^[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}$",
        "request_host": host,
        "resolve_ref": {"path": "/repos/{repo}/commits/{ref}", "default_path": "/repos/{repo}",
                         "sha": "$.sha", "default_branch": "$.default_branch"},
        "list_tree": {"path": "/repos/{repo}/git/trees/{sha}?recursive=1", "entries": "$.tree[*]",
                      "fields": {"path": "$.path", "type": "$.type", "mode": "$.mode", "blob": "$.sha", "size": "$.size"},
                      "file_type": "blob", "truncated": "$.truncated", "paginate": "none"},
        "get_blob": {"path": "/repos/{repo}/git/blobs/{blob}", "content": "$.content", "encoding": "$.encoding"},
        "required_access": "read"
    })
}

fn app(id: &str, library: Option<Value>, token: Value, instances: Option<Value>) -> Value {
    let mut a = json!({
        "app_id": id, "package_id": "pkg", "about": {"id": id, "name": id},
        "credentials": [{"name": "access_token", "secret": true, "optional": false},
                        {"name": "refresh_token", "secret": true, "optional": true}],
        "metadata": [], "connect": [], "token": token,
        "tools": null, "approvals": {}, "agent": null, "database": null
    });
    if let Some(l) = library {
        a["library"] = l;
    }
    if let Some(i) = instances {
        a["instances"] = i;
    }
    a
}

fn host(app: &str, host: &str) -> Value {
    json!({
        "host": host, "path_prefix": null, "app_id": app, "claim_status": "active",
        "tenant": null, "rewrite_host": null,
        "inject": [{"bearer": "{credentials.access_token}"}], "finalize": null,
        "transform": null, "intercept": null, "passthrough": []
    })
}

fn snapshot(
    apps: Vec<Value>,
    hosts: Vec<Value>,
    instance_hosts: Vec<Value>,
    token_url: &str,
) -> Value {
    json!({
        "format": 1, "contract": 1,
        "packages": [{"package_id": "pkg", "version": "1.0.0", "digest": "d", "publisher": "p",
                      "hooks_digest": null, "egress": {}, "context": ["org_id"]}],
        "oauth_clients": [{"package_id": "pkg", "client_key": "g", "authorize_url": "https://x/a",
                           "token_url": token_url, "pkce": false, "client_auth": "body",
                           "body_format": "form", "fields": []}],
        "apps": apps, "hosts": hosts, "instance_hosts": instance_hosts, "reserved_hosts": []
    })
}

struct Fx {
    db: TestDb,
    git: FakeGitHost,
    fetch: Arc<LibraryFetch>,
    ring: Arc<SecretRing>,
    org: Uuid,
    other_org: Uuid,
    /// Org-scoped active connection of `forge`.
    conn: Uuid,
    vault: Arc<SecretVault>,
}

async fn insert_conn(
    fx_db: &TestDb,
    vault: &SecretVault,
    org: Uuid,
    ws: Option<Uuid>,
    app: &str,
    status: &str,
    creds: Value,
) -> Uuid {
    let id = Uuid::now_v7();
    let ctx = SealContext {
        purpose: Purpose::ConnectionCredentials,
        owner_id: id,
        org_id: org,
        workspace_id: ws,
    };
    let r = vault
        .seal(&ctx, Secret::from(creds.to_string().into_bytes()))
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,$4,'pkg','1','L',$5,$6)")
        .bind(id).bind(org).bind(ws).bind(app).bind(status).bind(r.to_column())
        .execute(fx_db.pool()).await.unwrap();
    id
}

async fn start_with(snap: impl FnOnce(&str) -> Value) -> Fx {
    let db = TestDb::new().await;
    let git = FakeGitHost::start().await;
    git.add_repo("acme/skills");
    git.commit(
        "acme/skills",
        "main",
        &[
            Change::Put(
                "alpha/SKILL.md".into(),
                b"---\ndescription: d\n---\nbody\n".to_vec(),
            ),
            Change::Put("alpha/notes.txt".into(), b"notes".to_vec()),
        ],
    );
    let vault = vault();
    let org = Uuid::now_v7();
    let other_org = Uuid::now_v7();
    for (o, slug) in [(org, "o1"), (other_org, "o2")] {
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,$2,'O',$2)",
        )
        .bind(o)
        .bind(slug)
        .execute(db.pool())
        .await
        .unwrap();
    }
    let conn = insert_conn(
        &db,
        &vault,
        org,
        None,
        "forge",
        "active",
        json!({"access_token": TOKEN}),
    )
    .await;
    let b = |n: u8| b64(&[n; 32]);
    let ring = Arc::new(
        SecretRing::parse(&format!("a1:{}", b(1)), Some("a1"), RingKind::Internal).unwrap(),
    );
    let store = SnapshotStore::new();
    store.install(parse("h1", snap(&format!("{}/token", git.base()))).unwrap());
    let guard =
        Arc::new(aurora_gateway::guard::Guard::new(&["127.0.0.0/8".to_owned()], vec![]).unwrap());
    let upstream = Arc::new(Upstream::new(guard, None, None, Duration::from_secs(5)).unwrap());
    let fetch = Arc::new(LibraryFetch {
        ring: ring.clone(),
        snapshots: store,
        pool: db.pool().clone(),
        tokens: Arc::new(TokenManager::new(db.pool().clone(), vault.clone(), None)),
        plaintext: Arc::new(PlaintextCache::new(vault.clone())),
        upstream,
        external_url: "https://aurora.test".into(),
        scheme: Scheme::Http,
        port: git.addr().port(),
        // generous: the rate-limit test builds its own default limiter
        limiter: library_fetch::Limiter::with_rates(100_000.0, 100_000.0),
    });
    Fx {
        db,
        git,
        fetch,
        ring,
        org,
        other_org,
        conn,
        vault,
    }
}

async fn start() -> Fx {
    start_with(|token_url| {
        snapshot(
            vec![
                app("forge", Some(library_block("127.0.0.1")), Value::Null, None),
                app("nolib", None, Value::Null, None),
            ],
            vec![host("forge", "127.0.0.1")],
            vec![],
            token_url,
        )
    })
    .await
}

impl Fx {
    async fn post(&self, auth: bool, body: Value) -> (u16, Value) {
        self.post_via(self.fetch.clone(), auth, body).await
    }

    async fn post_via(&self, fetch: Arc<LibraryFetch>, auth: bool, body: Value) -> (u16, Value) {
        let mut b = Request::builder()
            .method(Method::POST)
            .uri("/internal/v1/library-fetch")
            .header("content-type", "application/json");
        if auth {
            b = b
                .header(
                    "authorization",
                    format!("Bearer {}", self.ring.active_bearer()),
                )
                .header("x-aurora-key-id", self.ring.active_id());
        }
        let res = library_fetch::router(fetch)
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

    fn req(&self, step: &str, params: Value) -> Value {
        json!({"org_id": self.org, "connection_id": self.conn, "step": step, "params": params, "max_bytes": 1_000_000})
    }

    async fn ok(&self, step: &str, params: Value) -> Value {
        let (s, v) = self.post(true, self.req(step, params)).await;
        assert_eq!(s, 200, "{v}");
        v
    }
}

fn body_of(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(v["body"].as_str().unwrap())
        .unwrap()
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

#[tokio::test]
async fn each_step_is_one_get_built_from_the_template_with_the_injected_credential() {
    let fx = start().await;
    let head = fx.git.head("acme/skills", "main");
    // resolve_ref: no ref reads the default branch, a ref (or HEAD) resolves it
    let v = fx.ok("resolve_ref", json!({"repo": "acme/skills"})).await;
    assert_eq!(v["status"], 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&body_of(&v)).unwrap()["default_branch"],
        "main"
    );
    let v = fx
        .ok("resolve_ref", json!({"repo": "acme/skills", "ref": "main"}))
        .await;
    assert_eq!(
        serde_json::from_slice::<Value>(&body_of(&v)).unwrap()["sha"],
        head
    );
    let v = fx
        .ok("list_tree", json!({"repo": "acme/skills", "sha": head}))
        .await;
    let tree: Value = serde_json::from_slice(&body_of(&v)).unwrap();
    let blob = tree["tree"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "alpha/notes.txt")
        .unwrap()["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let v = fx
        .ok("get_blob", json!({"repo": "acme/skills", "blob": blob}))
        .await;
    let b: Value = serde_json::from_slice(&body_of(&v)).unwrap();
    assert_eq!(b["encoding"], "base64");
    // the fake host saw exactly one GET per step, with the credential, and nothing of the agents'
    let reqs = fx.git.requests();
    assert_eq!(reqs.len(), 4, "{reqs:?}");
    assert_eq!(reqs[0].target, "/repos/acme/skills");
    assert_eq!(reqs[1].target, "/repos/acme/skills/commits/main");
    assert_eq!(
        reqs[2].target,
        format!("/repos/acme/skills/git/trees/{head}?recursive=1")
    );
    assert_eq!(
        reqs[3].target,
        format!("/repos/acme/skills/git/blobs/{blob}")
    );
    for r in &reqs {
        assert_eq!(r.method, "GET");
        assert_eq!(
            r.authorization.as_deref(),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert!(
            r.header_names.iter().all(|h| matches!(
                h.as_str(),
                "host" | "authorization" | "accept" | "user-agent"
            )),
            "no agent headers: {:?}",
            r.header_names
        );
    }
    // a page parameter is appended to the template's query, as an integer
    let v = fx
        .ok(
            "list_tree",
            json!({"repo": "acme/skills", "sha": head, "page": 2}),
        )
        .await;
    assert_eq!(v["status"], 200);
    assert!(
        fx.git
            .requests()
            .last()
            .unwrap()
            .target
            .ends_with("?recursive=1&page=2")
    );
}

#[tokio::test]
async fn sec_84_parameter_fuzzing_is_refused_before_anything_is_sent() {
    let fx = start().await;
    let head = fx.git.head("acme/skills", "main");
    let long = "a".repeat(300);
    let cases: Vec<(&str, Value)> = vec![
        ("resolve_ref", json!({"repo": "../x"})),
        ("resolve_ref", json!({"repo": "acme/.."})),
        ("resolve_ref", json!({"repo": "acme/skills/extra"})),
        ("resolve_ref", json!({"repo": "acme/skills?x=1"})),
        ("resolve_ref", json!({"repo": "acme/skills#f"})),
        ("resolve_ref", json!({"repo": "acme/%2e%2e"})),
        ("resolve_ref", json!({"repo": "acme/sk\r\nHost: evil"})),
        ("resolve_ref", json!({"repo": "ac\u{43c}e/skills"})),
        (
            "resolve_ref",
            json!({"repo": "acme/skills", "ref": "../../etc"}),
        ),
        (
            "resolve_ref",
            json!({"repo": "acme/skills", "ref": "a/../b"}),
        ),
        (
            "resolve_ref",
            json!({"repo": "acme/skills", "ref": "%2e%2e/x"}),
        ),
        (
            "resolve_ref",
            json!({"repo": "acme/skills", "ref": "x\r\nX-Evil: 1"}),
        ),
        ("resolve_ref", json!({"repo": "acme/skills", "ref": "x?y"})),
        ("resolve_ref", json!({"repo": "acme/skills", "ref": "-x"})),
        (
            "resolve_ref",
            json!({"repo": "acme/skills", "ref": "\u{ff0e}\u{ff0e}"}),
        ),
        ("resolve_ref", json!({"repo": "acme/skills", "ref": long})),
        ("list_tree", json!({"repo": "acme/skills", "sha": "../etc"})),
        (
            "list_tree",
            json!({"repo": "acme/skills", "sha": "ABCDEF".repeat(7)}),
        ),
        (
            "list_tree",
            json!({"repo": "acme/skills", "sha": "a".repeat(65)}),
        ),
        ("list_tree", json!({"repo": "acme/skills"})),
        (
            "list_tree",
            json!({"repo": "acme/skills", "sha": head, "page": 1}),
        ),
        (
            "list_tree",
            json!({"repo": "acme/skills", "sha": head, "page": 99999}),
        ),
        ("get_blob", json!({"repo": "acme/skills", "blob": "x/../y"})),
        (
            "get_blob",
            json!({"repo": "acme/skills", "blob": format!("{head}?x")}),
        ),
        ("get_blob", json!({"repo": "acme/skills"})),
    ];
    for (step, params) in cases {
        let (s, v) = fx.post(true, fx.req(step, params.clone())).await;
        assert_eq!(
            (s, code(&v)),
            (400, "library_param"),
            "{step} {params}: {v}"
        );
    }
    assert!(fx.git.requests().is_empty(), "nothing reached the git host");
    // unknown request keys and an unknown step are refused outright
    let mut r = fx.req("resolve_ref", json!({"repo": "acme/skills"}));
    r["url"] = json!("http://evil.example/");
    assert_eq!(fx.post(true, r).await.0, 400);
    let mut r = fx.req(
        "resolve_ref",
        json!({"repo": "acme/skills", "host": "evil.example"}),
    );
    r["max_bytes"] = json!(10);
    assert_eq!(fx.post(true, r).await.0, 400);
    assert_eq!(
        fx.post(true, fx.req("clone", json!({"repo": "acme/skills"})))
            .await
            .0,
        400
    );
    assert!(fx.git.requests().is_empty());
}

#[tokio::test]
async fn sec_84_connection_checks_other_org_workspace_scoped_disabled_other_app() {
    let fx = start().await;
    let ws = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'w','W')")
        .bind(ws)
        .bind(fx.org)
        .execute(fx.db.pool())
        .await
        .unwrap();
    let creds = json!({"access_token": TOKEN});
    let ws_conn = insert_conn(
        &fx.db,
        &fx.vault,
        fx.org,
        Some(ws),
        "forge",
        "active",
        creds.clone(),
    )
    .await;
    let disabled = insert_conn(
        &fx.db,
        &fx.vault,
        fx.org,
        None,
        "forge",
        "disabled",
        creds.clone(),
    )
    .await;
    let reauth = insert_conn(
        &fx.db,
        &fx.vault,
        fx.org,
        None,
        "forge",
        "reauth_required",
        creds.clone(),
    )
    .await;
    let nolib = insert_conn(
        &fx.db,
        &fx.vault,
        fx.org,
        None,
        "nolib",
        "active",
        creds.clone(),
    )
    .await;
    let gone_app = insert_conn(
        &fx.db,
        &fx.vault,
        fx.org,
        None,
        "removed-app",
        "active",
        creds,
    )
    .await;
    let p = json!({"repo": "acme/skills", "ref": "main"});
    let mk = |org: Uuid, conn: Uuid| json!({"org_id": org, "connection_id": conn, "step": "resolve_ref", "params": p, "max_bytes": 1000});
    // another organization's connection, an unknown one
    let (s, v) = fx.post(true, mk(fx.other_org, fx.conn)).await;
    assert_eq!((s, code(&v)), (404, "connection_not_found"), "{v}");
    let (s, v) = fx.post(true, mk(fx.org, Uuid::now_v7())).await;
    assert_eq!((s, code(&v)), (404, "connection_not_found"), "{v}");
    // workspace-scoped, disabled, an app without a library block, an app missing from the snapshot
    for (c, what) in [
        (ws_conn, "workspace"),
        (disabled, "disabled"),
        (nolib, "nolib"),
        (gone_app, "gone"),
    ] {
        let (s, v) = fx.post(true, mk(fx.org, c)).await;
        assert_eq!((s, code(&v)), (409, "library_unavailable"), "{what}: {v}");
    }
    let (s, v) = fx.post(true, mk(fx.org, reauth)).await;
    assert_eq!((s, code(&v)), (409, "reauth_required"), "{v}");
    assert!(
        fx.git.requests().is_empty(),
        "no refused connection reached the git host"
    );
}

#[tokio::test]
async fn sec_84_the_route_needs_the_internal_secret() {
    let fx = start().await;
    let body = fx.req("resolve_ref", json!({"repo": "acme/skills"}));
    assert_eq!(fx.post(false, body.clone()).await.0, 401);
    // a wrong secret
    let res = library_fetch::router(fx.fetch.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/internal/v1/library-fetch")
                .header("authorization", format!("Bearer {}", b64(&[9u8; 32])))
                .header("x-aurora-key-id", "a1")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    // GET is not the route
    let res = library_fetch::router(fx.fetch.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/internal/v1/library-fetch")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 405);
    assert!(fx.git.requests().is_empty());
}

#[tokio::test]
async fn sec_84_redirects_are_not_followed_and_the_location_is_not_returned() {
    let fx = start().await;
    fx.git.redirect("/commits/", "http://evil.example/steal");
    let v = fx
        .ok("resolve_ref", json!({"repo": "acme/skills", "ref": "main"}))
        .await;
    assert_eq!(v["status"], 302);
    assert!(v["headers"].get("location").is_none());
    assert_eq!(
        fx.git.requests().len(),
        1,
        "the redirect target was never requested"
    );
    assert!(!v.to_string().contains("evil.example"));
}

#[tokio::test]
async fn sec_84_oversize_responses_are_cut_and_the_credential_never_appears() {
    let fx = start().await;
    fx.git.oversize("/commits/", 3000);
    let mut r = fx.req("resolve_ref", json!({"repo": "acme/skills", "ref": "main"}));
    r["max_bytes"] = json!(1000);
    let (s, v) = fx.post(true, r).await;
    assert_eq!((s, code(&v)), (413, "library_response_too_large"), "{v}");
    // the hard cap holds whatever max_bytes asks (4 MiB)
    fx.git.clear_overrides();
    fx.git.oversize("/commits/", 5 * 1024 * 1024);
    let mut r = fx.req("resolve_ref", json!({"repo": "acme/skills", "ref": "main"}));
    r["max_bytes"] = json!(100 * 1024 * 1024);
    let (s, v) = fx.post(true, r).await;
    assert_eq!((s, code(&v)), (413, "library_response_too_large"), "{v}");
    fx.git.clear_overrides();
    // the git host's error answers are passed through as data; no response carries the credential
    for status in [401, 403, 404, 429, 500] {
        fx.git.override_status("/commits/", status);
        let (s, v) = fx
            .post(
                true,
                fx.req("resolve_ref", json!({"repo": "acme/skills", "ref": "main"})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["status"], status);
        let text = v.to_string().to_lowercase();
        assert!(
            !text.contains(TOKEN) && !text.contains("authorization") && !text.contains("bearer"),
            "{text}"
        );
        fx.git.clear_overrides();
    }
    // the git host unreachable
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let to_dead = Arc::new(LibraryFetch {
        port: dead,
        ..clone_fetch(&fx.fetch)
    });
    let (s, v) = fx
        .post_via(
            to_dead,
            true,
            fx.req("resolve_ref", json!({"repo": "acme/skills", "ref": "main"})),
        )
        .await;
    assert_eq!((s, code(&v)), (502, "library_upstream"), "{v}");
    assert!(!v.to_string().contains("127.0.0.1"));
}

/// FIX-065: headers and part of the body arrive, then the connection is reset — never a 200
/// carrying a truncated file (the API would hash and ingest it).
#[tokio::test]
async fn a_body_cut_short_by_the_upstream_is_502_library_upstream_not_a_partial_success() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let fx = start().await;
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf).await;
            let _ = s
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000\r\n\r\n{\"sha\":\"abc",
                )
                .await;
            let _ = s.flush().await;
            // Close with data announced but not sent: the body ends in an error.
            drop(s);
        }
    });
    let cut = Arc::new(LibraryFetch {
        port,
        ..clone_fetch(&fx.fetch)
    });
    let (s, v) = fx
        .post_via(
            cut,
            true,
            fx.req("resolve_ref", json!({"repo": "acme/skills", "ref": "main"})),
        )
        .await;
    assert_eq!((s, code(&v)), (502, "library_upstream"), "{v}");
}

fn clone_fetch(f: &Arc<LibraryFetch>) -> LibraryFetch {
    LibraryFetch {
        ring: f.ring.clone(),
        snapshots: f.snapshots.clone(),
        pool: f.pool.clone(),
        tokens: f.tokens.clone(),
        plaintext: f.plaintext.clone(),
        upstream: f.upstream.clone(),
        external_url: f.external_url.clone(),
        scheme: f.scheme,
        port: f.port,
        limiter: library_fetch::Limiter::default(),
    }
}

#[tokio::test]
async fn the_route_is_rate_limited_per_organization() {
    use std::time::Instant;
    // The production limiter (20/s per gateway, 5/s per organization) on an injected clock, so the
    // result does not depend on how loaded the machine is.
    let t0 = Instant::now();
    let l = library_fetch::Limiter::default();
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    assert_eq!(
        (0..8).filter(|_| l.allow_at(a, t0)).count(),
        5,
        "per-org burst of 5"
    );
    assert!(l.allow_at(b, t0), "another organization has its own bucket");
    assert!(!l.allow_at(a, t0));
    let later = t0 + Duration::from_secs(1);
    assert_eq!(
        (0..8).filter(|_| l.allow_at(a, later)).count(),
        5,
        "refilled at 5 per second"
    );
    // a refused request consumes nothing from the other bucket: 25 orgs share the global 20
    let g = library_fetch::Limiter::default();
    let allowed = (0..25).filter(|_| g.allow_at(Uuid::now_v7(), t0)).count();
    assert_eq!(allowed, 20, "per-gateway burst of 20");

    // the refusal on the wire: the 429 envelope (a limiter that never has a whole token)
    let fx = start().await;
    let never = Arc::new(LibraryFetch {
        limiter: library_fetch::Limiter::with_rates(0.5, 0.5),
        ..clone_fetch(&fx.fetch)
    });
    let (s, v) = fx
        .post_via(
            never,
            true,
            fx.req("resolve_ref", json!({"repo": "acme/skills", "ref": "main"})),
        )
        .await;
    assert_eq!((s, code(&v)), (429, "rate_limited"), "{v}");
    assert!(
        fx.git.requests().is_empty(),
        "a limited request never reached the git host"
    );
}

#[tokio::test]
async fn an_instance_bound_app_reads_the_apps_default_host() {
    let fx = start_with(|token_url| {
        snapshot(
            vec![app(
                "forge",
                Some(library_block("{instance}")),
                Value::Null,
                Some(json!({"label": "Forge host", "default_host": "127.0.0.1", "probe": null})),
            )],
            vec![],
            vec![json!({
                "host": "{instance}", "path_prefix": null, "app_id": "forge", "claim_status": "active",
                "tenant": null, "rewrite_host": null,
                "inject": [{"bearer": "{credentials.access_token}"}], "finalize": null,
                "transform": null, "intercept": null, "passthrough": []
            })],
            token_url,
        )
    })
    .await;
    let v = fx
        .ok("resolve_ref", json!({"repo": "acme/skills", "ref": "main"}))
        .await;
    assert_eq!(v["status"], 200);
    assert_eq!(
        fx.git.requests()[0].authorization.as_deref(),
        Some(format!("Bearer {TOKEN}").as_str())
    );
}

#[tokio::test]
async fn a_host_rule_with_rewrite_or_a_foreign_claim_cannot_serve_a_library() {
    // the request host is claimed by another app: refused, not sent
    let fx = start_with(|token_url| {
        snapshot(
            vec![
                app("forge", Some(library_block("127.0.0.1")), Value::Null, None),
                app("other", None, Value::Null, None),
            ],
            vec![host("other", "127.0.0.1")],
            vec![],
            token_url,
        )
    })
    .await;
    let (s, v) = fx
        .post(
            true,
            fx.req("resolve_ref", json!({"repo": "acme/skills", "ref": "main"})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "library_unavailable"), "{v}");
    assert!(fx.git.requests().is_empty());
}

// ---- OAuth refresh ---------------------------------------------------------------------------

async fn token_endpoint(fail: bool) -> String {
    let app = axum::Router::new().route(
        "/token",
        axum::routing::post(move || async move {
            if fail {
                (http::StatusCode::BAD_REQUEST, json!({"error": "invalid_grant"}).to_string())
            } else {
                (
                    http::StatusCode::OK,
                    json!({"access_token": "fresh-token", "refresh_token": "r2", "expires_in": 3600}).to_string(),
                )
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{a}")
}

async fn oauth_fx(fail: bool) -> Fx {
    let token_url = token_endpoint(fail).await;
    let fx = start_with(move |_| {
        snapshot(
            vec![app(
                "forge",
                Some(library_block("127.0.0.1")),
                json!({"kind": "oauth2_refresh", "client": "g", "refresh_field": "refresh_token", "rotate_refresh_token": true}),
                None,
            )],
            vec![host("forge", "127.0.0.1")],
            vec![],
            &format!("{token_url}/token"),
        )
    })
    .await;
    // an OAuth client for the package and an expired org-level connection
    let client = Uuid::now_v7();
    let cctx = SealContext {
        purpose: Purpose::OauthClientSecret,
        owner_id: client,
        org_id: fx.org,
        workspace_id: None,
    };
    let cref = fx
        .vault
        .seal(
            &cctx,
            Secret::from(json!({"client_secret": "shh"}).to_string().into_bytes()),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_oauth_clients (id, org_id, package_id, client_key, fields, secret_ref) VALUES ($1,$2,'pkg','g',$3,$4)")
        .bind(client).bind(fx.org).bind(json!({"client_id": "cid"})).bind(cref.to_column())
        .execute(fx.db.pool()).await.unwrap();
    sqlx::query("UPDATE app_connections SET oauth_client_id = $2, credentials_expires_at = now() - interval '10 seconds' WHERE id = $1")
        .bind(fx.conn).bind(client).execute(fx.db.pool()).await.unwrap();
    // the stored credentials carry a refresh token
    let ctx = SealContext {
        purpose: Purpose::ConnectionCredentials,
        owner_id: fx.conn,
        org_id: fx.org,
        workspace_id: None,
    };
    let r = fx
        .vault
        .seal(
            &ctx,
            Secret::from(
                json!({"access_token": "stale", "refresh_token": "r1"})
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE app_connections SET credentials_ref = $2 WHERE id = $1")
        .bind(fx.conn)
        .bind(r.to_column())
        .execute(fx.db.pool())
        .await
        .unwrap();
    fx
}

#[tokio::test]
async fn an_expired_oauth_token_is_refreshed_before_the_read() {
    let fx = oauth_fx(false).await;
    let v = fx
        .ok("resolve_ref", json!({"repo": "acme/skills", "ref": "main"}))
        .await;
    assert_eq!(v["status"], 200);
    assert_eq!(
        fx.git.requests()[0].authorization.as_deref(),
        Some("Bearer fresh-token")
    );
}

#[tokio::test]
async fn a_revoked_oauth_grant_is_409_reauth_required_and_sets_the_flag() {
    let fx = oauth_fx(true).await;
    let (s, v) = fx
        .post(
            true,
            fx.req("resolve_ref", json!({"repo": "acme/skills", "ref": "main"})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "reauth_required"), "{v}");
    let st: String = sqlx::query_scalar("SELECT status FROM app_connections WHERE id = $1")
        .bind(fx.conn)
        .fetch_one(fx.db.pool())
        .await
        .unwrap();
    assert_eq!(st, "reauth_required");
    assert!(fx.git.requests().is_empty());
}
