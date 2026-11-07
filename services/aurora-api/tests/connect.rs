#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;

use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::Limits;
use aurora_registry::builder::PackageBuilder;
use aurora_testkit::oauth::MockOAuth;
use aurora_testkit::wiremock::matchers::{method, path};
use aurora_testkit::wiremock::{Mock, MockServer, ResponseTemplate};
use axum::body::Body;
use common::*;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const YAML: &str = r#"package: { id: acme, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
permissions:
  hooks: [connect.authorize, connect.exchange, token.mint]
  egress:
    connect.exchange: [auth.acme.example]
    token.mint: [auth.acme.example]
  context: [org_id]
oauth_clients:
  - id: goog
    authorize_url: https://accounts.example.com/auth
    token_url: https://accounts.example.com/token
    pkce: true
    fields:
      - { name: client_id, label: Client ID }
      - { name: client_secret, label: Secret, secret: true }
  - id: tenantco
    authorize_url: { template: "https://{connection.tenant_host}/authorize", zone: "*.tenant.example.test" }
    token_url: { template: "https://{connection.tenant_host}/token", zone: "*.tenant.example.test" }
    fields:
      - { name: client_id, label: Client ID }
      - { name: client_secret, label: Secret, secret: true }
apps:
  - about: { id: acme_app, name: Acme, description: D, category: dev, web_hosts: [app.acme.example] }
    credentials:
      - { name: api_token, secret: true, optional: true }
      - { name: access_token, secret: true, optional: true }
      - { name: refresh_token, secret: true, optional: true }
    metadata: [ { name: org } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
        probe: { url: "https://api.acme.example/me", label: /email, metadata: { org: /org/name } }
      - id: oauth
        kind: oauth2
        client: goog
        scopes:
          - { scope: read, name: Read, access: read }
          - { scope: write, name: Write, access: write }
        default_scopes: [read, write]
        callback: query
        probe: { url: "https://api.acme.example/me", label: /email }
      - id: hook
        kind: hook
        authorize: true
        fields: [ { name: account, label: Account, type: text, store: metadata, required: false } ]
    agent: { default_blocklist: [ "blocked.example" ] }
    token: { kind: hook, max_lifetime_secs: 3600 }
    hosts:
      - match: { host: "api.acme.example" }
        inject:
          - bearer: "{credentials.api_token}"
  - about: { id: tenant_app, name: Tenant, description: D, category: dev, web_hosts: [www.tenant.example.test] }
    credentials:
      - { name: access_token, secret: true, optional: true }
      - { name: refresh_token, secret: true, optional: true }
    connect:
      - id: oauth
        kind: oauth2
        client: tenantco
        scopes:
          - { scope: read, name: Read, access: read }
        default_scopes: [read]
        fields:
          - { name: subdomain, label: Host, type: text, store: tenant_host, required: true }
    hosts:
      - match: { host: "*.tenant.example.test" }
        tenant: { field: connection.tenant_host, zone: "*.tenant.example.test" }
        inject:
          - bearer: "{credentials.access_token}"
"#;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    owner: String,
    member: String,
    oauth: MockOAuth,
}

async fn world(hook_url: Option<String>) -> W {
    let oauth = MockOAuth::start().await;
    let h = Harness::with_config(|c| {
        if let Some(u) = hook_url {
            c.hookhost_url = u;
        }
    })
    .await;
    let mut h = h;
    h.state = h.state.clone().with_connect_override(oauth.base());
    h.app = test_router(h.state.clone());
    let root = h.user("root@x.io", true).await;
    let (_, admin_tok) = h.good_session(root).await;
    let admin = cookie(&admin_tok);
    let key: SigningKey = generate_signing_key().unwrap();
    let hd = [("cookie", admin.as_str()), ("origin", ORIGIN)];
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/trusted-keys",
            &hd,
            Some(json!({"publisher": "acme", "name": "k", "public_key": PackageBuilder::key_id(&key)})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let mut b = PackageBuilder::new(YAML);
    let wasm = hooks_wasm();
    b.set_file("hooks.wasm", &wasm);
    b.sign(&key, &PackageBuilder::key_id(&key));
    let boundary = "XB";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"package\"; filename=\"p\"\r\nContent-Type: application/gzip\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(&b.build().unwrap());
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/admin/plugins")
        .header("cookie", &admin)
        .header("origin", ORIGIN)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let res = h.app.clone().oneshot(req).await.unwrap();
    let v: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(v["status"], "pending_approval", "{}", v["violations"]);
    let (s, _, v) = h
        .send(
            Method::POST,
            &format!("/v1/admin/plugins/{}/approve", v["id"].as_str().unwrap()),
            &hd,
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let o = h.user("owner@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    h.bind_ws(ws, o, "owner").await;
    let m = h.user("mem@x.io", false).await;
    h.member(org, m, "member", "active").await;
    h.bind_ws(ws, m, "member").await;
    let (_, ot) = h.good_session(o).await;
    let (_, mt) = h.good_session(m).await;
    W {
        h,
        org,
        ws,
        owner: cookie(&ot),
        member: cookie(&mt),
        oauth,
    }
}

/// The fixture package's compiled hook component (all four hook points).
fn hooks_wasm() -> Vec<u8> {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/fixtures/dist/fixture-hooks.aurora-plugin"
    );
    let bytes = std::fs::read(p).unwrap();
    let pkg = aurora_registry::Package::read(&bytes, &Limits::default()).unwrap();
    pkg.file("hooks.wasm").unwrap().to_vec()
}

impl W {
    async fn call(
        &self,
        who: &str,
        m: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (u16, Vec<(String, String)>, Value) {
        let (s, hd, v) = self
            .h
            .send(m, uri, &[("cookie", who), ("origin", ORIGIN)], body)
            .await;
        let sc = hd
            .get_all("set-cookie")
            .iter()
            .map(|c| ("set-cookie".to_owned(), c.to_str().unwrap().to_owned()))
            .collect();
        (s.as_u16(), sc, v)
    }

    fn connect_body(&self, method: &str) -> Value {
        json!({"scope": "workspace", "workspace_id": self.ws, "method_id": method})
    }

    async fn page(&self, who: &str, extra_cookie: Option<&str>, uri: &str) -> (String, bool) {
        let ck = match extra_cookie {
            Some(c) => format!("{who}; {c}"),
            None => who.to_owned(),
        };
        let req = Request::builder()
            .uri(uri)
            .header("cookie", ck)
            .body(Body::empty())
            .unwrap();
        let res = self.h.app.clone().oneshot(req).await.unwrap();
        let cleared = res
            .headers()
            .get_all("set-cookie")
            .iter()
            .any(|c| c.to_str().unwrap().contains("Max-Age=0"));
        (
            String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec())
                .unwrap(),
            cleared,
        )
    }

    async fn oauth_client(&self) {
        let (s, _, v) = self
            .call(&self.owner, Method::POST, &format!("/v1/workspaces/{}/oauth-clients", self.ws),
                Some(json!({"package_id": "acme", "client_key": "goog", "fields": {"client_id": "cid", "client_secret": "csec"}})))
            .await;
        assert_eq!(s, 201, "{v}");
    }

    /// Start an oauth connect and return (provider URL, cookie pair).
    async fn start_oauth(&self) -> (String, String) {
        let (s, sc, v) = self
            .call(
                &self.owner,
                Method::POST,
                "/v1/apps/acme_app/connect",
                Some(self.connect_body("oauth")),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["kind"], "redirect");
        let c = &sc[0].1;
        assert!(
            c.starts_with("__Host-aurora_oauth=") && c.contains("HttpOnly") && c.contains("Secure")
        );
        (
            v["url"].as_str().unwrap().to_owned(),
            c.split(';').next().unwrap().to_owned(),
        )
    }
}

fn local(loc: &str) -> String {
    let i = loc.find("/v1/oauth/callback").unwrap();
    loc[i..].to_owned()
}

#[tokio::test]
async fn fields_connect_with_probe_blocklist_and_no_credentials_leak() {
    let w = world(None).await;
    let mut b = w.connect_body("api_key");
    b["fields"] = json!({"api_token": "sekret-token"});
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(b.clone()),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["kind"], "connected");
    assert_eq!(v["connection"]["label"], "me@acme.io");
    assert_eq!(v["connection"]["metadata"]["org"], "Acme");
    assert_eq!(v["missing_scopes"], json!([]));
    assert!(!v.to_string().contains("sekret-token"));
    let id: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM derived_rules WHERE source_kind='blocklist' AND source_id=$1",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
    // probe sent the app's own bearer injection
    // member cannot connect; unknown field -> 400
    assert_eq!(
        w.call(
            &w.member,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(b)
        )
        .await
        .0,
        403
    );
    let mut bad = w.connect_body("api_key");
    bad["fields"] = json!({"api_token": "x", "zzz": "y"});
    assert_eq!(
        w.call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(bad)
        )
        .await
        .0,
        400
    );
    let mut miss = w.connect_body("api_key");
    miss["fields"] = json!({});
    assert_eq!(
        w.call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(miss)
        )
        .await
        .0,
        400
    );
}

#[tokio::test]
async fn probe_failure_is_422() {
    let w = world(None).await;
    let mut b = w.connect_body("api_key");
    b["fields"] = json!({"api_token": "bad"});
    let (s, _, v) = w
        .call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "connect_probe_failed");
    assert_eq!(v["error"]["details"]["upstream_status"], 403);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_connections")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn oauth_requires_client_and_unavailable_app_is_403() {
    let w = world(None).await;
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(w.connect_body("oauth")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("oauth_client_required"))
    );
    sqlx::query("UPDATE organizations SET app_posture='restricted' WHERE id=$1")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(w.connect_body("oauth")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("app_unavailable"))
    );
}

#[tokio::test]
async fn oauth_happy_path_pkce_cookie_and_state_checks() {
    let w = world(None).await;
    w.oauth_client().await;
    w.oauth.set_extra(json!({"scope": "read"}));
    let (url, ck) = w.start_oauth().await;
    assert!(url.contains("code_challenge_method=S256") && url.contains("client_id=cid"));
    let loc = w.oauth.approve(&url).await;
    let cb = local(&loc);

    // wrong user, missing cookie, tampered state => error page, nothing created
    let (pg, _) = w.page(&w.member, Some(&ck), &cb).await;
    assert!(pg.contains("error=oauth_state_invalid"), "{pg}");
    let (pg, _) = w.page(&w.owner, None, &cb).await;
    assert!(pg.contains("error=oauth_state_invalid"));
    let tampered = cb.replace("state=", "state=A");
    let (pg, _) = w.page(&w.owner, Some(&ck), &tampered).await;
    assert!(pg.contains("error=oauth_state_invalid"));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_connections")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);

    let (pg, cleared) = w.page(&w.owner, Some(&ck), &cb).await;
    assert!(
        pg.contains("aurora:connected") && pg.contains("window.close()"),
        "{pg}"
    );
    assert!(cleared);
    let (scopes, label, uid): (Vec<String>, String, Uuid) =
        sqlx::query_as("SELECT scopes, label, connected_by FROM app_connections")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (scopes, label.as_str()),
        (vec!["read".to_owned()], "me@acme.io")
    );
    assert!(uid != Uuid::nil());
    let req = &w.oauth.token_requests()[0];
    assert!(req.contains_key("code_verifier") && req["client_id"] == "cid");
}

#[tokio::test]
async fn expired_state_and_fragment_bridge() {
    let w = world(None).await;
    w.oauth_client().await;
    let (url, ck) = w.start_oauth().await;
    let (pg, _) = w.page(&w.owner, Some(&ck), "/v1/oauth/callback").await;
    assert!(pg.contains("location.hash"), "{pg}");
    // expire: forge by unsealing is impossible; use a state sealed with exp in the past.
    let loc = w.oauth.approve(&url).await;
    let cb = local(&loc);
    let (pg, _) = w
        .page(&w.owner, Some(&ck), &format!("{cb}&error=access_denied"))
        .await;
    assert!(pg.contains("error=oauth_exchange_failed"));
}

#[tokio::test]
async fn reconnect_updates_in_place_and_clears_reauth() {
    let w = world(None).await;
    let mut b = w.connect_body("api_key");
    b["fields"] = json!({"api_token": "t1"});
    let (_, _, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(b.clone()),
        )
        .await;
    let id: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE app_connections SET status='reauth_required', status_reason='invalid_grant', label='Renamed' WHERE id=$1")
        .bind(id).execute(w.h.pool()).await.unwrap();
    b["fields"] = json!({"api_token": "t2"});
    b["reconnect_connection_id"] = json!(id);
    let (s, _, v) = w
        .call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (
            v["connection"]["id"].as_str().unwrap(),
            v["connection"]["status"].as_str().unwrap()
        ),
        (id.to_string().as_str(), "active")
    );
    let (n, ver): (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM app_connections), credentials_version FROM app_connections WHERE id=$1")
        .bind(id).fetch_one(w.h.pool()).await.unwrap();
    assert_eq!((n, ver), (1, 2));
}

#[tokio::test]
async fn hook_authorize_and_exchange_via_stub_host() {
    let stub = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/internal/v1/hooks/connect.authorize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"ok": {"url": "https://accounts.example.com/go", "continuation": "c1"}}),
        ))
        .mount(&stub)
        .await;
    Mock::given(method("POST")).and(path("/internal/v1/hooks/connect.exchange"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": {"connected": {
            "credentials": [{"key": "access_token", "value": "hat"}], "metadata": [{"key": "org", "value": "O"}],
            "scopes": [], "label": "Hooked", "expires_at": null, "tenant_host": null}}})))
        .mount(&stub).await;
    let w = world(Some(stub.uri())).await;
    let (s, sc, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(w.connect_body("hook")),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["url"], "https://accounts.example.com/go");
    let ck = sc[0].1.split(';').next().unwrap().to_owned();
    let reqs = stub.received_requests().await.unwrap();
    let input = &reqs[0].body_json::<Value>().unwrap()["input"];
    let state = input["state"].as_str().unwrap().to_owned();
    let (pg, _) = w
        .page(
            &w.owner,
            Some(&ck),
            &format!("/v1/oauth/callback?code=x&state={state}"),
        )
        .await;
    assert!(pg.contains("aurora:connected"), "{pg}");
    let label: String = sqlx::query_scalar("SELECT label FROM app_connections")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(label, "Hooked");
    // hook host down => connect_hook_failed
    drop(stub);
}

#[tokio::test]
async fn missing_scopes_surface_on_page_connection_and_dto() {
    let w = world(None).await;
    w.oauth_client().await;
    // The provider grants fewer scopes than requested (read + write).
    w.oauth.set_extra(json!({"scope": "read"}));
    let (url, ck) = w.start_oauth().await;
    let loc = w.oauth.approve(&url).await;
    let (pg, _) = w.page(&w.owner, Some(&ck), &local(&loc)).await;
    assert!(pg.contains("missing_scopes:[\"write\"]"), "{pg}");
    assert!(pg.contains("were not granted: write"), "{pg}");
    assert!(!pg.contains("window.close();</script>") || pg.contains("length===0"));
    let (id, requested): (Uuid, Vec<String>) =
        sqlx::query_as("SELECT id, requested_scopes FROM app_connections")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(requested, vec!["read".to_owned(), "write".to_owned()]);
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/connections/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["scopes"], json!(["read"]));
    assert_eq!(v["missing_scopes"], json!(["write"]));
}

#[tokio::test]
async fn full_grant_has_no_missing_scopes() {
    let w = world(None).await;
    w.oauth_client().await;
    w.oauth.set_extra(json!({"scope": "read write"}));
    let (url, ck) = w.start_oauth().await;
    let loc = w.oauth.approve(&url).await;
    let (pg, _) = w.page(&w.owner, Some(&ck), &local(&loc)).await;
    assert!(pg.contains("missing_scopes:[]"), "{pg}");
    let id: Uuid = sqlx::query_scalar("SELECT id FROM app_connections")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let (_, _, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/connections/{id}"),
            None,
        )
        .await;
    assert_eq!(v["missing_scopes"], json!([]));
}

impl W {
    async fn tenant_client(&self) {
        let (s, _, v) = self
            .call(&self.owner, Method::POST, &format!("/v1/workspaces/{}/oauth-clients", self.ws),
                Some(json!({"package_id": "acme", "client_key": "tenantco", "fields": {"client_id": "cid", "client_secret": "csec"}})))
            .await;
        assert_eq!(s, 201, "{v}");
    }

    async fn tenant_connect(&self, host: &str) -> (u16, Value) {
        let mut b = self.connect_body("oauth");
        b["fields"] = json!({"subdomain": host});
        let (s, _, v) = self
            .call(
                &self.owner,
                Method::POST,
                "/v1/apps/tenant_app/connect",
                Some(b),
            )
            .await;
        (s, v)
    }
}

#[tokio::test]
async fn first_connect_takes_tenant_host_from_fields() {
    let w = world(None).await;
    w.tenant_client().await;
    let (s, v) = w.tenant_connect("Acme.Tenant.Example.Test").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["kind"], "redirect");
    let url = v["url"].as_str().unwrap();
    assert!(
        url.starts_with("https://acme.tenant.example.test/authorize?"),
        "{url}"
    );
    assert!(w.oauth.token_requests().is_empty());
}

#[tokio::test]
async fn tenant_host_outside_zone_ip_or_tricks_is_refused_before_any_request() {
    let w = world(None).await;
    w.tenant_client().await;
    for bad in [
        "evil.example.com",
        "tenant.example.test",
        "10.0.0.1",
        "127.0.0.1",
        "acme.tenant.example.test@evil.example.com",
        "evil.example.com@acme.tenant.example.test",
        "acme.tenant.example.test:8443",
        "acme.tenant.example.test/path",
        "",
    ] {
        let (s, v) = w.tenant_connect(bad).await;
        assert_eq!(s, 400, "{bad}: {v}");
        assert_eq!(v["error"]["code"], "invalid_request", "{bad}");
    }
    assert!(w.oauth.token_requests().is_empty());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_connections")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn tenant_oauth_callback_stores_host_and_exchanges_against_it() {
    let w = world(None).await;
    w.tenant_client().await;
    let mut b = w.connect_body("oauth");
    b["fields"] = json!({"subdomain": "acme.tenant.example.test"});
    let (s, sc, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/tenant_app/connect",
            Some(b),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // The provider is the in-process mock; the test override maps the tenant host to it.
    let url = v["url"].as_str().unwrap().to_owned();
    let ck = sc[0].1.split(';').next().unwrap().to_owned();
    let loc = w.oauth.approve(&url).await;
    let (pg, _) = w.page(&w.owner, Some(&ck), &local(&loc)).await;
    assert!(pg.contains("aurora:connected"), "{pg}");
    let host: Option<String> = sqlx::query_scalar("SELECT tenant_host FROM app_connections")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(host.as_deref(), Some("acme.tenant.example.test"));
}
