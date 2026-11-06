#![allow(clippy::unwrap_used, clippy::expect_used)]
//! End-to-end tests: real wasmtime runtime, guest components built with
//! `cargo build --target wasm32-wasip2`, mock API internal endpoint and mock upstream.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use aurora_hookhost::fetch::FetchPolicy;
use aurora_hookhost::runtime::Runtime;
use aurora_hookhost::server::{AppState, router};
use aurora_hookhost::snapshot::Catalog;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SECRET: &[u8] = b"test-internal-secret-32-bytes!!!";

fn target_installed() -> bool {
    let Ok(out) = Command::new("rustc").args(["--print", "sysroot"]).output() else {
        return false;
    };
    let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Path::new(&sysroot)
        .join("lib/rustlib/wasm32-wasip2")
        .is_dir()
}

fn guest() -> Option<&'static Vec<u8>> {
    static G: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    G.get_or_init(|| {
        if !target_installed() {
            eprintln!("SKIP: wasm32-wasip2 target not installed");
            return None;
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let target_dir = root.join("../../target/hook-guests");
        let st = Command::new(env!("CARGO"))
            .args(["build", "--release", "--target", "wasm32-wasip2"])
            .current_dir(root.join("tests/guests/behave"))
            .env("CARGO_TARGET_DIR", &target_dir)
            .env_remove("RUSTFLAGS")
            .status()
            .expect("spawn cargo");
        assert!(st.success(), "guest build failed");
        Some(std::fs::read(target_dir.join("wasm32-wasip2/release/guest_behave.wasm")).unwrap())
    })
    .as_ref()
}

fn bearer() -> String {
    base64::engine::general_purpose::STANDARD.encode(SECRET)
}

struct Env {
    app: axum::Router,
    catalog: Arc<Catalog>,
    upstream: MockServer,
    _api: MockServer,
}

fn snapshot(wasm: &[u8], upstream_host: &str) -> (Vec<u8>, String) {
    let digest = hex::encode(Sha256::digest(wasm));
    let snap = json!({
        "format": 1, "contract": 1,
        "packages": [{
            "package_id": "acme", "version": "1.0.0", "digest": "pkgdigest", "publisher": "p",
            "hooks_digest": digest,
            "egress": {
                "connect.exchange": [upstream_host, "auth.example.com"],
                "token.mint": [upstream_host],
                "approval.summarize": [upstream_host]
            },
            "context": ["mode", "url"]
        }],
        "oauth_clients": [],
        "apps": [{
            "app_id": "acme-app", "package_id": "acme",
            "about": {"id": "acme-app", "name": "Acme"},
            "credentials": [{"name": "token", "secret": true}],
            "metadata": [{"name": "region"}],
            "connect": [], "token": null, "tools": null, "approvals": {}, "agent": null, "database": null
        }],
        "hosts": [{
            "host": "*.example.com", "path_prefix": null, "app_id": "acme-app", "claim_status": "active",
            "tenant": {"field": "sub", "zone": "*.example.com"}
        }],
        "reserved_hosts": []
    });
    (serde_json::to_vec(&snap).unwrap(), digest)
}

async fn setup() -> Option<Env> {
    let wasm = guest()?;
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/data"))
        .respond_with(ResponseTemplate::new(200).set_body_string("tok-from-upstream"))
        .mount(&upstream)
        .await;
    let (snap, digest) = snapshot(wasm, "127.0.0.1");
    let api = MockServer::start().await;
    let auth = format!("Bearer {}", bearer());
    Mock::given(method("GET"))
        .and(path("/internal/v1/registry/active"))
        .and(header("authorization", auth.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(snap))
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/internal/v1/registry/blobs/{digest}")))
        .and(header("authorization", auth.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(wasm.clone()))
        .mount(&api)
        .await;
    let rt = Runtime::new(FetchPolicy {
        insecure_local: true,
    })
    .unwrap();
    let catalog = Catalog::new(&api.uri(), bearer(), rt);
    let app = router(AppState {
        catalog: catalog.clone(),
        bearer: bearer().into(),
    });
    Some(Env {
        app,
        catalog,
        upstream,
        _api: api,
    })
}

fn kvs(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(k, v)| json!({"key": k, "value": v}))
            .collect(),
    )
}

async fn call(env: &Env, hook: &str, input: Value, auth: Option<String>) -> (StatusCode, Value) {
    call_pkg(env, hook, "acme", "pkgdigest", input, auth).await
}

async fn call_pkg(
    env: &Env,
    hook: &str,
    pkg: &str,
    digest: &str,
    input: Value,
    auth: Option<String>,
) -> (StatusCode, Value) {
    let body =
        json!({"package_id": pkg, "package_digest": digest, "app_id": "acme-app", "input": input});
    let mut req = Request::post(format!("/internal/v1/hooks/{hook}"))
        .header("content-type", "application/json");
    if let Some(a) = auth {
        req = req.header("authorization", a);
    }
    let resp = env
        .app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn ok_auth() -> Option<String> {
    Some(format!("Bearer {}", bearer()))
}

fn authorize(mode: &str) -> Value {
    json!({"app_id":"x","method_id":"m","client":[],"redirect_uri":"https://a/cb","state":"st1","scopes":[],
           "access_level":null,"fields":[],"context":kvs(&[("mode",mode)])})
}
fn exchange(mode: &str, url: &str) -> Value {
    json!({"app_id":"x","method_id":"m","source":{"form":kvs(&[("password","hunter2hunter2")])},"client":[],
           "client_secret":[],"redirect_uri":"https://a/cb","pkce_verifier":null,"continuation":null,
           "context":kvs(&[("mode",mode),("url",url),("bogus","dropped")])})
}
fn mint(mode: &str, url: &str) -> Value {
    json!({"app_id":"x","credentials":kvs(&[("token","old")]),"metadata":[],"tenant_host":null,
           "client":[],"client_secret":[],"context":kvs(&[("mode",mode),("url",url)])})
}
fn summarize(mode: &str, url: &str) -> Value {
    json!({"app_id":"x","tool_id":null,"method":"POST","host":"api.example.com","path":"/v1/x",
           "query":kvs(&[("mode",mode),("url",url)]),"headers":[],"body_prefix":[],"truncated":false})
}
fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("<none>")
}

macro_rules! env {
    () => {
        match setup().await {
            Some(e) => e,
            None => return,
        }
    };
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_pulled_and_auth_required() {
    let env = env!();
    assert!(!env.catalog.is_ready().await);
    let (st, _) = call(&env, "token.mint", mint("ok", ""), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = call(
        &env,
        "token.mint",
        mint("ok", ""),
        Some("Bearer nope".into()),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // First authorised call pulls the snapshot on miss.
    let (st, v) = call(&env, "token.mint", mint("ok", ""), ok_auth()).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(env.catalog.is_ready().await);
    assert!(
        !env.catalog.refresh().await.unwrap(),
        "unchanged hash is a no-op"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn happy_path_each_hook() {
    let env = env!();
    let (_, v) = call(&env, "connect.authorize", authorize("ok"), ok_auth()).await;
    assert_eq!(
        v["ok"]["url"], "https://login.example.com/a?state=st1",
        "{v}"
    );
    let (_, v) = call(&env, "connect.exchange", exchange("ok", ""), ok_auth()).await;
    assert_eq!(v["ok"]["connected"]["credentials"][0]["value"], "t1", "{v}");
    let (_, v) = call(&env, "connect.exchange", exchange("tenant", ""), ok_auth()).await;
    assert_eq!(
        v["ok"]["connected"]["tenant_host"], "acme.example.com",
        "{v}"
    );
    let (_, v) = call(
        &env,
        "connect.exchange",
        exchange("continue", ""),
        ok_auth(),
    )
    .await;
    assert_eq!(v["ok"]["continue"]["continuation"], "c1", "{v}");
    let (_, v) = call(&env, "token.mint", mint("ok", ""), ok_auth()).await;
    assert_eq!(v["ok"]["credentials"][0]["value"], "minted", "{v}");
    let (_, v) = call(&env, "approval.summarize", summarize("ok", ""), ok_auth()).await;
    assert_eq!(v["ok"]["title"], "POST /v1/x", "{v}");
    assert_eq!(v["ok"]["fields"][0]["key"], "host");
    // stdout is captured, not an error.
    let (st, _) = call(&env, "connect.authorize", authorize("stdout"), ok_auth()).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_declared_ok_undeclared_denied_summarize_never() {
    let env = env!();
    let good = format!("{}/data", env.upstream.uri());
    let (_, v) = call(&env, "token.mint", mint("fetch", &good), ok_auth()).await;
    assert_eq!(
        v["ok"]["credentials"][0]["value"], "tok-from-upstream",
        "{v}"
    );
    // `localhost` resolves to the same server but is not in the egress list.
    let port = env.upstream.address().port();
    let bad = format!("http://localhost:{port}/data");
    let (_, v) = call(&env, "token.mint", mint("fetch", &bad), ok_auth()).await;
    assert_eq!(code(&v), "egress_denied", "{v}");
    let (_, v) = call(&env, "connect.exchange", exchange("fetch", &bad), ok_auth()).await;
    assert_eq!(code(&v), "egress_denied", "{v}");
    // approval.summarize has no egress even if the snapshot lists a host.
    let (_, v) = call(
        &env,
        "approval.summarize",
        summarize("fetch", &good),
        ok_auth(),
    )
    .await;
    assert_eq!(code(&v), "egress_denied", "{v}");
    // connect.authorize has none either (guest has no fetch path; list is empty by hook).
}

#[tokio::test(flavor = "multi_thread")]
async fn production_policy_blocks_private_and_http() {
    let Some(wasm) = guest() else { return };
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&upstream)
        .await;
    let (snap, digest) = snapshot(wasm, "127.0.0.1");
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/internal/v1/registry/active"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(snap))
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/internal/v1/registry/blobs/{digest}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(wasm.clone()))
        .mount(&api)
        .await;
    let catalog = Catalog::new(
        &api.uri(),
        bearer(),
        Runtime::new(FetchPolicy::default()).unwrap(),
    );
    let env = Env {
        app: router(AppState {
            catalog: catalog.clone(),
            bearer: bearer().into(),
        }),
        catalog,
        upstream,
        _api: api,
    };
    let url = format!("{}/x", env.upstream.uri());
    let (_, v) = call(&env, "token.mint", mint("fetch", &url), ok_auth()).await;
    assert_eq!(code(&v), "egress_denied", "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn infinite_loop_is_killed() {
    let env = env!();
    // Warm the snapshot and module caches so only the kill is timed.
    call(&env, "token.mint", mint("ok", ""), ok_auth()).await;
    let t = Instant::now();
    let (st, v) = call(&env, "approval.summarize", summarize("loop", ""), ok_auth()).await;
    assert_eq!(code(&v), "timeout", "{v}");
    assert_eq!(st, StatusCode::GATEWAY_TIMEOUT);
    assert!(
        t.elapsed() < Duration::from_secs(4),
        "took {:?}",
        t.elapsed()
    );
    // The service stays responsive afterwards.
    let (st, _) = call(&env, "token.mint", mint("ok", ""), ok_auth()).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn allocation_bomb_and_output_flood() {
    let env = env!();
    let (_, v) = call(&env, "connect.exchange", exchange("alloc", ""), ok_auth()).await;
    assert_eq!(code(&v), "memory", "{v}");
    let (_, v) = call(&env, "connect.exchange", exchange("flood", ""), ok_auth()).await;
    assert_eq!(code(&v), "output_too_large", "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_outputs_rejected() {
    let env = env!();
    for (hook, input) in [
        ("connect.exchange", exchange("bad_cred", "")),
        ("connect.exchange", exchange("bad_tenant", "")),
        ("connect.exchange", exchange("continue_bad", "")),
        ("connect.authorize", authorize("badurl")),
    ] {
        let (st, v) = call(&env, hook, input, ok_auth()).await;
        assert_eq!(code(&v), "internal", "{hook}: {v}");
        assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("invalid hook output")
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_and_log_flood() {
    let env = env!();
    let (_, v) = call(&env, "connect.exchange", exchange("reauth", ""), ok_auth()).await;
    assert_eq!(code(&v), "reauth_required");
    let (st, v) = call(&env, "connect.exchange", exchange("log", ""), ok_auth()).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, v) = call_pkg(
        &env,
        "token.mint",
        "acme",
        "stale",
        mint("ok", ""),
        ok_auth(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(code(&v), "invalid_input");
    let (st, _) = call(&env, "nope.nope", json!({}), ok_auth()).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, v) = call(&env, "connect.authorize", json!({"bad": 1}), ok_auth()).await;
    assert_eq!((st, code(&v)), (StatusCode::BAD_REQUEST, "invalid_input"));
}
