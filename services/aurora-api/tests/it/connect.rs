#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_registry::Limits;
use aurora_testkit::oauth::MockOAuth;
use aurora_testkit::wiremock::matchers::{method, path};
use aurora_testkit::wiremock::{Mock, MockServer, ResponseTemplate};
use axum::body::Body;
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
  token_hosts: [accounts.example.com]
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
    metadata: [ { name: org }, { name: tls_verification }, { name: token_scopes } ]
    connect:
      - id: api_key
        kind: fields
        fields:
          - { name: api_token, label: T, type: secret, required: true }
          - { name: tls_verification, label: TLS, type: select, store: metadata, options: [ { value: verify, label: V }, { value: "off", label: Off } ] }
        scopes:
          - { scope: "read:a", name: Read A, access: read }
          - { scope: "read:b", name: Read B, access: read }
          - { scope: "write:a", name: Write A, access: write }
        probe_scopes:
          "read:a": { url: "https://api.acme.example/me" }
        probe: { url: "https://api.acme.example/me", label: /email, metadata: { org: /org/name, token_scopes: "header:x-oauth-scopes" } }
      - id: oauth
        kind: oauth2
        client: goog
        scopes:
          - { scope: read, name: Read, access: read }
          - { scope: write, name: Write, access: write }
        default_scopes: [read, write]
        callback: query
        probe: { url: "https://api.acme.example/me", label: /email }
      - id: oauth_app
        kind: oauth2
        client: goog
        scopes: []
        default_scopes: []
        callback: query
        probe: { url: "https://api.acme.example/me", label: /email }
      - id: hook
        kind: hook
        authorize: true
        fields: [ { name: account, label: Account, type: text, store: metadata, required: false } ]
      - id: hook_form
        kind: hook
        authorize: false
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

#[path = "account_test.rs"]
mod account_test;

// (P5-ICS, D-273) The secret-link app's connect.
#[path = "link_connect.rs"]
mod link_connect;

// (P5-WEBHOOK-2, D-320) The outgoing webhook's connect, edit, preview, presets and send test.
#[path = "outgoing_webhooks.rs"]
mod outgoing_webhooks;

// (FIX-194, D-420) `store: label`: a form field names the connection.
#[path = "label_field.rs"]
mod label_field;

// (P5-CALDAV, D-275) The own-server app's connect (discovery).
#[path = "caldav_connect.rs"]
mod caldav_connect;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    owner: String,
    member: String,
    oauth: MockOAuth,
}

async fn world(hook_url: Option<String>) -> W {
    world_with(hook_url, YAML, true).await
}

/// [`world`] with another package manifest (`hooks`: ship the fixture hook component with it).
async fn world_with(hook_url: Option<String>, yaml: &str, hooks: bool) -> W {
    world_with_files(hook_url, yaml, hooks, &[]).await
}

/// [`world_with`] plus archive files (the presets of an outgoing webhook, P5-WEBHOOK-2).
async fn world_with_files(
    hook_url: Option<String>,
    yaml: &str,
    hooks: bool,
    files: &[(String, Vec<u8>)],
) -> W {
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
    // (P5-DEDUP-1 T1 helper, with P5-WEBHOOK-2's archive files) Install and approve the package.
    let _admin = {
        let a = crate::plugins_lifecycle::setup(&h).await;
        let wasm = if hooks { Some(hooks_wasm()) } else { None };
        let mut extra: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(p, d)| (p.as_str(), d.as_slice()))
            .collect();
        if let Some(w) = &wasm {
            extra.push(("hooks.wasm", w.as_slice()));
        }
        a.install_with(yaml, &extra).await;
        a.cookie
    };
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
    // (FIX-133) A probe metadata pointer `header:<name>` stores a response header.
    assert_eq!(
        v["connection"]["metadata"]["token_scopes"],
        "repo, read:org"
    );
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

/// (FIX-106, D-178 Q-R5-7) The opt-out of server certificate verification is a named, audited,
/// visible per-connection choice: it lands in the connection's metadata (which the dashboard
/// shows) and in the `connection.create` audit row; a connection without it records neither.
#[tokio::test]
async fn a_tls_verification_opt_out_is_audited_and_visible_on_the_connection() {
    let w = world(None).await;
    let audit = |id: Uuid| {
        let pool = w.h.pool().clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT metadata->>'tls_verification' FROM audit_log \
                 WHERE action = 'connection.create' AND target_id = $1",
            )
            .bind(id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let mut b = w.connect_body("api_key");
    b["fields"] = json!({"api_token": "t1", "tls_verification": "off"});
    let (s, _, v) = w
        .call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["metadata"]["tls_verification"], "off");
    let id: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(audit(id).await.as_deref(), Some("off"));
    let mut b = w.connect_body("api_key");
    b["fields"] = json!({"api_token": "t2", "tls_verification": "verify"});
    let (s, _, v) = w
        .call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
        .await;
    assert_eq!(s, 200, "{v}");
    let id: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(audit(id).await, None);
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
    assert_eq!(v["error"]["details"]["cause"], "rejected");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_connections")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
}

/// (P5-UX-2, C-958) The probe says why: 401 and 403 are `rejected`, a provider 500 is
/// `unexpected` (the status is kept), and a host nothing answers is `unreachable` with the host.
#[tokio::test]
async fn probe_failure_says_why() {
    let mut w = world(None).await;
    for (token, cause, status) in [
        ("nope", "rejected", json!(401)),
        ("boom", "unexpected", json!(500)),
    ] {
        let mut b = w.connect_body("api_key");
        b["fields"] = json!({ "api_token": token });
        let (s, _, v) = w
            .call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
            .await;
        assert_eq!(s, 422, "{v}");
        assert_eq!(v["error"]["code"], "connect_probe_failed");
        assert_eq!(v["error"]["details"]["cause"], cause, "{v}");
        assert_eq!(v["error"]["details"]["upstream_status"], status, "{v}");
    }
    // Nothing listens on the provider's address: `unreachable`, with the probed host.
    w.h.state =
        w.h.state
            .clone()
            .with_connect_override("http://127.0.0.1:1");
    w.h.app = test_router(w.h.state.clone());
    let mut b = w.connect_body("api_key");
    b["fields"] = json!({"api_token": "fine"});
    let (s, _, v) = w
        .call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
        .await;
    assert_eq!(s, 422, "{v}");
    let d = &v["error"]["details"];
    assert_eq!(d["cause"], "unreachable", "{v}");
    assert_eq!(d["host"], "api.acme.example", "{v}");
    assert!(d["upstream_status"].is_null());
}

/// (P5-UX-2, C-958) Both client refusals name who can fix it.
#[tokio::test]
async fn client_required_refusals_carry_who_can() {
    let w = world(None).await;
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(w.connect_body("oauth")),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "oauth_client_required");
    let who = &v["error"]["details"]["who_can"];
    assert_eq!(who["role"], "org_admin", "{v}");
    assert!(
        who["people"].is_array() && who.get("scope").is_none(),
        "{v}"
    );
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
    // (FIX-149, C-842) The result goes over the same-origin channel with the app id; nothing refers
    // to `window.opener`, and the window closes only on the dialog's ack.
    assert!(pg.contains("BroadcastChannel(\"aurora-connect\")"), "{pg}");
    assert!(
        pg.contains("type:\"aurora:connected\"")
            && pg.contains("app_id:\"acme_app\"")
            && pg.contains("connection_id:\""),
        "{pg}"
    );
    assert!(!pg.contains("opener"), "{pg}");
    let (before_ack, after_ack) = pg.split_once("aurora:connect-ack").expect("ack handler");
    assert!(!before_ack.contains("window.close"), "{pg}");
    assert!(after_ack.contains("window.close()"), "{pg}");
    assert!(
        pg.contains("tab=connections"),
        "link to the Connections tab: {pg}"
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

/// (FIX-136, D-241) A pasted token's scopes: prefilled by the method's read probes, or declared by
/// the person (each one of the method's scopes), stored as the connection's `scopes`, editable with
/// `PATCH` (only a declared-scope token connection; each one of the method's scopes).
#[tokio::test]
async fn a_token_connections_scopes_are_probed_declared_and_editable() {
    let w = world(None).await;
    let connect = |scopes: Option<Value>, token: &str| {
        let mut b = w.connect_body("api_key");
        b["fields"] = json!({"api_token": token});
        if let Some(s) = scopes {
            b["scopes"] = s;
        }
        b
    };
    let post = |b: Value| {
        let w = &w;
        async move {
            w.call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
                .await
        }
    };
    // No declaration: the read probe (`read:a`) passes for a good token, so only it is ticked; the
    // write scope is never probed.
    let (s, _, v) = post(connect(None, "t1")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["scopes"], json!(["read:a"]));
    // A token the probe refuses has none ticked.
    let (s, _, v) = post(connect(None, "bad")).await;
    assert_eq!(s, 422, "the main probe fails for a bad token: {v}");
    // Declared by the person.
    let (s, _, v) = post(connect(Some(json!(["read:b", "write:a"])), "t2")).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["scopes"], json!(["read:b", "write:a"]));
    let id = v["connection"]["id"].as_str().unwrap().to_owned();
    // A scope the method does not declare is a 400.
    let (s, _, v) = post(connect(Some(json!(["nope"])), "t3")).await;
    assert_eq!(s, 400, "{v}");
    // Editable afterwards, within the method's scopes.
    let path = format!("/v1/connections/{id}");
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &path,
            Some(json!({"scopes": ["read:a"]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["scopes"], json!(["read:a"]));
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &path,
            Some(json!({"scopes": ["nope"]})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    // An OAuth connection's scopes are what the provider granted: not editable.
    w.oauth_client().await;
    w.oauth.set_extra(json!({"scope": "read"}));
    let (url, ck) = w.start_oauth().await;
    let loc = w.oauth.approve(&url).await;
    let (pg, _) = w.page(&w.owner, Some(&ck), &local(&loc)).await;
    assert!(pg.contains("aurora:connected"), "{pg}");
    let oid: Uuid =
        sqlx::query_scalar("SELECT id FROM app_connections WHERE connect_method = 'oauth'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &format!("/v1/connections/{oid}"),
            Some(json!({"scopes": ["read"]})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
}

/// (FIX-133) A scope-less OAuth method (GitHub App user authorization: permissions live on the app,
/// not in the request): the authorization URL carries no `scope`, the code exchange still uses
/// PKCE, and the expiring token with its rotating refresh token is stored with its expiry.
#[tokio::test]
async fn a_scopeless_oauth_method_connects_with_an_expiring_token() {
    let w = world(None).await;
    w.oauth_client().await;
    w.oauth.set_extra(json!({
        "expires_in": 28800, "refresh_token": "ghr_fixture", "refresh_token_expires_in": 15_897_600,
        "scope": ""
    }));
    let (s, sc, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(w.connect_body("oauth_app")),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let url = v["url"].as_str().unwrap().to_owned();
    let ck = sc[0].1.split(';').next().unwrap().to_owned();
    assert!(
        url.contains("code_challenge_method=S256") && !url.contains("scope="),
        "{url}"
    );
    let loc = w.oauth.approve(&url).await;
    let (pg, _) = w.page(&w.owner, Some(&ck), &local(&loc)).await;
    assert!(pg.contains("aurora:connected"), "{pg}");
    let (scopes, expires, keys): (
        Vec<String>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<Vec<String>>,
    ) = sqlx::query_as(
        "SELECT scopes, credentials_expires_at, credential_keys FROM app_connections",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(scopes.is_empty());
    let left = expires.unwrap() - chrono::Utc::now();
    assert!(
        left.num_seconds() > 28_000 && left.num_seconds() <= 28_800,
        "{left}"
    );
    let mut keys = keys.unwrap();
    keys.sort();
    assert_eq!(keys, ["access_token", "refresh_token"]);
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
    // (FIX-149, C-842) The failure page reports on the channel too, for the flow's app.
    assert!(pg.contains("type:\"aurora:connect-failed\""), "{pg}");
    assert!(pg.contains("app_id:\"acme_app\""), "{pg}");
    assert!(pg.contains("error:\"oauth_exchange_failed\""), "{pg}");
    assert!(!pg.contains("opener"), "{pg}");
    // A flow that is not this user's reveals no app id.
    let (pg, _) = w.page(&w.member, Some(&ck), &cb).await;
    assert!(pg.contains("error=oauth_state_invalid"), "{pg}");
    assert!(!pg.contains("acme_app"), "{pg}");
}

/// (FIX-149, C-842) Both callback scripts carry the response's CSP nonce.
#[tokio::test]
async fn callback_scripts_carry_the_response_nonce() {
    let w = world(None).await;
    w.oauth_client().await;
    let (url, ck) = w.start_oauth().await;
    let loc = w.oauth.approve(&url).await;
    let cb = local(&loc);
    for uri in [cb.clone(), format!("{cb}&error=access_denied")] {
        let req = Request::builder()
            .uri(&uri)
            .header("cookie", format!("{}; {ck}", w.owner))
            .body(Body::empty())
            .unwrap();
        let res = w.h.app.clone().oneshot(req).await.unwrap();
        let csp = res
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let pg = String::from_utf8(body.to_vec()).unwrap();
        let nonce = csp
            .split("'nonce-")
            .nth(1)
            .and_then(|r| r.split('\'').next())
            .expect("nonce in the CSP");
        assert!(
            pg.contains(&format!("<script nonce=\"{nonce}\">")),
            "{csp} {pg}"
        );
        assert_eq!(pg.matches("<script").count(), 1, "{pg}");
    }
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

/// (FIX-123, D-195(a), D-214) A connection records the connect method that created it, the DTO
/// shows it, and a reconnect writes the method of that reconnect (a row of unknown method, as
/// before the column, gets it recorded).
#[tokio::test]
async fn a_connection_records_its_connect_method_and_a_reconnect_rewrites_it() {
    let w = world(None).await;
    let mut b = w.connect_body("api_key");
    b["fields"] = json!({"api_token": "t1"});
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(b.clone()),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["connect_method"], "api_key");
    let id: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    let stored: Option<String> =
        sqlx::query_scalar("SELECT connect_method FROM app_connections WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some("api_key"));
    // A row from before the column (unknown method) is filled in by the next reconnect.
    sqlx::query("UPDATE app_connections SET connect_method = NULL WHERE id = $1")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    b["fields"] = json!({"api_token": "t2"});
    b["reconnect_connection_id"] = json!(id);
    let (s, _, v) = w
        .call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["connect_method"], "api_key");
}

/// (FIX-133, D-240) A connection is `write` unless connected read-only; a reconnect without `access`
/// keeps it; the connection can be changed afterwards with `PATCH`; anything else is a 400.
#[tokio::test]
async fn a_connection_can_be_read_only_from_connect_time_and_changed_later() {
    let w = world(None).await;
    let mut b = w.connect_body("api_key");
    b["fields"] = json!({"api_token": "t1"});
    // Default: write.
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(b.clone()),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["access"], "write");
    // An unknown value is refused before anything is stored.
    b["access"] = json!("admin");
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(b.clone()),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    // Read-only from the start.
    b["access"] = json!("read");
    b["fields"] = json!({"api_token": "t2"});
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(b.clone()),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["access"], "read");
    let id: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    let stored: String = sqlx::query_scalar("SELECT access FROM app_connections WHERE id = $1")
        .bind(id)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(stored, "read");
    // A reconnect that does not say keeps it.
    b.as_object_mut().unwrap().remove("access");
    b["fields"] = json!({"api_token": "t3"});
    b["reconnect_connection_id"] = json!(id);
    let (s, _, v) = w
        .call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["access"], "read");
    // PATCH changes it; a bad value is refused.
    let path = format!("/v1/connections/{id}");
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &path,
            Some(json!({"access": "write"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["access"], "write");
    let (s, _, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &path,
            Some(json!({"access": "everything"})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn connect_records_credential_key_names_and_a_reconnect_clears_a_major_upgrade_flip() {
    let w = world(None).await;
    let mut b = w.connect_body("api_key");
    b["fields"] = json!({"api_token": "super-secret-value"});
    let (_, _, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/acme_app/connect",
            Some(b.clone()),
        )
        .await;
    let id: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    let keys: Option<Vec<String>> =
        sqlx::query_scalar("SELECT credential_keys FROM app_connections WHERE id=$1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    // Names only, never values (SEC-57).
    assert_eq!(keys.unwrap(), ["api_token"]);
    // A plugin major upgrade flipped it (C-342) and it recorded the old version.
    let why = "plugin_major_upgrade: acme 0.9.0 -> 1.0.0; missing: api_token";
    sqlx::query("UPDATE app_connections SET status='reauth_required', status_reason=$2, reauth_required_at=now(), app_version='0.9.0', credential_keys=ARRAY['legacy_name'] WHERE id=$1")
        .bind(id).bind(why).execute(w.h.pool()).await.unwrap();
    let (s, _, item) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/connections/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{item}");
    assert_eq!(item["status"], "reauth_required");
    assert_eq!(
        item["status_reason"], why,
        "visible on the DTO, not cut at 64"
    );
    b["fields"] = json!({"api_token": "t2"});
    b["reconnect_connection_id"] = json!(id);
    let (s, _, v) = w
        .call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
        .await;
    assert_eq!(s, 200, "{v}");
    let (status, why, ver, keys): (String, Option<String>, String, Option<Vec<String>>) =
        sqlx::query_as("SELECT status, status_reason, app_version, credential_keys FROM app_connections WHERE id=$1")
            .bind(id).fetch_one(w.h.pool()).await.unwrap();
    assert_eq!(
        (status.as_str(), why, ver.as_str()),
        ("active", None, "1.0.0")
    );
    assert_eq!(keys.unwrap(), ["api_token"]);
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

/// (FIX-133, D-240) A form-only hook method gets the connection's access in the exchange form
/// under the reserved `aurora_access` key (so the hook can narrow its token); a reconnect without
/// `access` passes the connection's current one; a user-supplied `aurora_access` is refused.
#[tokio::test]
async fn hook_exchange_form_carries_the_connection_access() {
    let stub = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/internal/v1/hooks/connect.exchange"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"ok": {"connected": {
            "credentials": [{"key": "access_token", "value": "hat"}], "metadata": [],
            "scopes": [], "label": "Hooked", "expires_at": null, "tenant_host": null}}})),
        )
        .mount(&stub)
        .await;
    let w = world(Some(stub.uri())).await;
    let mut b = w.connect_body("hook_form");
    let go = |b: Value| {
        let w = &w;
        async move {
            w.call(&w.owner, Method::POST, "/v1/apps/acme_app/connect", Some(b))
                .await
        }
    };
    // Default and explicit access.
    let (s, _, v) = go(b.clone()).await;
    assert_eq!(s, 200, "{v}");
    b["access"] = json!("read");
    let (s, _, v) = go(b.clone()).await;
    assert_eq!(s, 200, "{v}");
    let id = v["connection"]["id"].clone();
    // A reconnect that does not say passes the current access.
    b.as_object_mut().unwrap().remove("access");
    b["reconnect_connection_id"] = id;
    let (s, _, v) = go(b.clone()).await;
    assert_eq!(s, 200, "{v}");
    let forms: Vec<Option<String>> = stub
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| {
            let input = &r.body_json::<Value>().unwrap()["input"];
            input["source"]["form"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["key"] == "aurora_access")
                .map(|e| e["value"].as_str().unwrap().to_owned())
        })
        .collect();
    assert_eq!(
        forms,
        [
            Some("write".into()),
            Some("read".into()),
            Some("read".into())
        ]
    );
    // The reserved key is not a form field: a user cannot set it.
    b.as_object_mut().unwrap().remove("reconnect_connection_id");
    b["fields"] = json!({"aurora_access": "write"});
    let (s, _, v) = go(b).await;
    assert_eq!(s, 400, "{v}");
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

/// (FIX-109) Reconnecting to a package version whose blocklist is empty removes the derived
/// `blocklist` rule an earlier version left (the app has no `default_blocklist`, as after an
/// upgrade that dropped it); a first connect writes none.
#[tokio::test]
async fn reconnect_with_an_empty_blocklist_leaves_no_blocklist_rule() {
    let w = world(None).await;
    w.tenant_client().await;
    let mut b = w.connect_body("oauth");
    b["fields"] = json!({"subdomain": "acme.tenant.example.test"});
    let (s, sc, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/tenant_app/connect",
            Some(b.clone()),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let url = v["url"].as_str().unwrap().to_owned();
    let ck = sc[0].1.split(';').next().unwrap().to_owned();
    let loc = w.oauth.approve(&url).await;
    let (pg, _) = w.page(&w.owner, Some(&ck), &local(&loc)).await;
    assert!(pg.contains("aurora:connected"), "{pg}");
    let id: Uuid = sqlx::query_scalar("SELECT id FROM app_connections")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let rules = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM derived_rules WHERE source_kind = 'blocklist' AND source_id = $1",
        )
        .bind(id)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
    };
    assert_eq!(
        rules().await,
        0,
        "a first connect of a blocklist-free app writes none"
    );
    // The previous package version had a blocklist: its rule is still in the table.
    sqlx::query("INSERT INTO derived_rules (id, org_id, workspace_id, source_kind, source_id, ordinal, logical_id, rule) VALUES ($1,$2,$3,'blocklist',$4,0,$5,'{}')")
        .bind(Uuid::now_v7()).bind(w.org).bind(w.ws).bind(id).bind(Uuid::now_v7())
        .execute(w.h.pool()).await.unwrap();
    assert_eq!(rules().await, 1);
    b["reconnect_connection_id"] = json!(id);
    let (s, sc, v) = w
        .call(
            &w.owner,
            Method::POST,
            "/v1/apps/tenant_app/connect",
            Some(b),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let url = v["url"].as_str().unwrap().to_owned();
    let ck = sc[0].1.split(';').next().unwrap().to_owned();
    let loc = w.oauth.approve(&url).await;
    let (pg, _) = w.page(&w.owner, Some(&ck), &local(&loc)).await;
    assert!(pg.contains("aurora:connected"), "{pg}");
    assert_eq!(
        rules().await,
        0,
        "the old blocklist rule is gone after the reconnect"
    );
}
