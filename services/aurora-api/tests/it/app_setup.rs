#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-UX-2 (D-321, D-330, C-931, C-955, C-946): the setup status and the connect plan.

use crate::common::*;
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

/// `ghlike` mirrors GitHub: an admin-only app method, two OAuth methods (two clients) and a token.
/// `lin` has one OAuth method. `ghes` is instance-bound with no hosted default.
const MULTI: &str = r#"package: { id: multi, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: M }
permissions:
  token_hosts: [accounts.example.com]
oauth_clients:
  - id: gh_oauth
    authorize_url: https://accounts.example.com/auth
    token_url: https://accounts.example.com/token
    pkce: true
    fields:
      - { name: client_id, label: Client ID }
      - { name: client_secret, label: Secret, secret: true }
  - id: gh_app_oauth
    authorize_url: https://accounts.example.com/auth
    token_url: https://accounts.example.com/token
    pkce: true
    fields:
      - { name: client_id, label: Client ID }
      - { name: client_secret, label: Secret, secret: true }
  - id: linear
    authorize_url: https://accounts.example.com/auth
    token_url: https://accounts.example.com/token
    pkce: true
    fields:
      - { name: client_id, label: Client ID }
      - { name: client_secret, label: Secret, secret: true }
apps:
  - about: { id: ghlike, name: GH Like, description: D, category: dev }
    credentials:
      - { name: access_token, secret: true, optional: true }
      - { name: refresh_token, secret: true, optional: true }
    connect:
      - id: github_app
        kind: fields
        label: GitHub App
        recommended: true
        audience: admin
        fields:
          - { name: access_token, label: Key, type: secret, required: true }
      - id: oauth
        kind: oauth2
        label: Sign in
        client: gh_oauth
        scopes:
          - { scope: read, name: Read, access: read }
          - { scope: write, name: Write, access: write }
        default_scopes: [read]
        callback: query
      - id: oauth_app
        kind: oauth2
        label: Sign in with your own app
        client: gh_app_oauth
        scopes: []
        callback: query
      - id: token
        kind: fields
        label: Personal token
        fields:
          - { name: access_token, label: Token, type: secret, required: true }
    hosts:
      - match: { host: api.gh.example }
        inject:
          - bearer: "{credentials.access_token}"
  - about: { id: lin, name: Linear Like, description: D, category: dev }
    credentials:
      - { name: access_token, secret: true, optional: true }
    connect:
      - id: oauth
        kind: oauth2
        label: Sign in
        client: linear
        scopes:
          - { scope: read, name: Read, access: read }
          - { scope: write, name: Write, access: write }
        scope_presets: { read: [read], write: [read, write] }
        default_scopes: [read]
        callback: query
    hosts:
      - match: { host: api.lin.example }
        inject:
          - bearer: "{credentials.access_token}"
  - about: { id: ghes, name: GH Server, description: D, category: dev }
    instances: { label: Server }
    credentials:
      - { name: access_token, secret: true, optional: true }
    connect:
      - id: token
        kind: fields
        label: Personal token
        fields:
          - { name: access_token, label: Token, type: secret, required: true }
    hosts:
      - match: { host: "{instance}" }
        inject:
          - bearer: "{credentials.access_token}"
"#;

/// A package whose apps read the organization's external id (the `aws-keys` shape).
const AWS: &str = r#"package: { id: awspkg, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: A }
permissions:
  context: [org_external_id]
apps:
  - about: { id: awsk, name: AWS Keys, description: D, category: dev }
    credentials:
      - { name: access_token, secret: true }
    connect:
      - id: keys
        kind: fields
        fields:
          - { name: access_token, label: Key, type: secret, required: true }
    hosts:
      - match: { host: api.aws.example }
        inject:
          - bearer: "{credentials.access_token}"
"#;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    root: String,
    org_admin: String,
    ws_owner: String,
    member: String,
}

async fn install(h: &Harness, admin: &str, key: &SigningKey, yaml: &str) {
    let hd = [("cookie", admin), ("origin", ORIGIN)];
    let mut b = PackageBuilder::new(yaml);
    b.sign(key, &PackageBuilder::key_id(key));
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
        .header("cookie", admin)
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
}

async fn named(h: &Harness, email: &str, name: &str, admin: bool) -> Uuid {
    let u = h.user(email, admin).await;
    sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
        .bind(u)
        .bind(name)
        .execute(h.pool())
        .await
        .unwrap();
    u
}

async fn world() -> W {
    let h = Harness::new().await;
    let root = named(&h, "root@x.io", "Root Admin", true).await;
    let (_, rt) = h.good_session(root).await;
    let root_ck = cookie(&rt);
    let key: SigningKey = generate_signing_key().unwrap();
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/trusted-keys",
            &[("cookie", root_ck.as_str()), ("origin", ORIGIN)],
            Some(json!({"publisher": "acme", "name": "k", "public_key": PackageBuilder::key_id(&key)})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    install(&h, &root_ck, &key, MULTI).await;
    install(&h, &root_ck, &key, AWS).await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let adam = named(&h, "adam@x.io", "Adam Admin", false).await;
    h.member(org, adam, "admin", "active").await;
    let wo = named(&h, "wo@x.io", "Wendy Owner", false).await;
    h.member(org, wo, "member", "active").await;
    h.bind_ws(ws, wo, "owner").await;
    let m = named(&h, "mem@x.io", "Mia Member", false).await;
    h.member(org, m, "member", "active").await;
    h.bind_ws(ws, m, "member").await;
    let (_, at) = h.good_session(adam).await;
    let (_, wt) = h.good_session(wo).await;
    let (_, mt) = h.good_session(m).await;
    W {
        h,
        org,
        ws,
        root: root_ck,
        org_admin: cookie(&at),
        ws_owner: cookie(&wt),
        member: cookie(&mt),
    }
}

impl W {
    async fn call(&self, who: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", who), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    async fn setup(&self, who: &str, app: &str, q: &str) -> (u16, Value) {
        self.call(who, Method::GET, &format!("/v1/apps/{app}/setup?{q}"), None)
            .await
    }

    fn ws_q(&self) -> String {
        format!("workspace_id={}", self.ws)
    }

    fn org_q(&self) -> String {
        format!("org_id={}", self.org)
    }

    async fn client(&self, who: &str, uri: &str, key: &str) -> Value {
        let (s, v) = self
            .call(
                who,
                Method::POST,
                uri,
                Some(json!({"package_id": "multi", "client_key": key,
                    "fields": {"client_id": "cid-SECRETID", "client_secret": "csec-SECRET"}})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v
    }
}

fn method<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap_or_else(|| panic!("{id}: {v}"))
}

fn people(v: &Value) -> Vec<&str> {
    v["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn github_like_with_no_clients_suggests_the_token_and_names_the_org_admins() {
    let w = world().await;
    let (s, v) = w.setup(&w.ws_owner, "ghlike", &w.ws_q()).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["available"], true);
    assert_eq!(method(&v, "github_app")["audience"], "admin");
    assert_eq!(method(&v, "github_app")["ready"], true);
    for id in ["oauth", "oauth_app"] {
        let m = method(&v, id);
        assert_eq!(m["ready"], false, "{m}");
        assert_eq!(m["needs"], json!(["oauth_client"]), "{m}");
        assert_eq!(m["who_can"]["role"], "org_admin", "{m}");
        assert_eq!(people(&m["who_can"]), ["Adam Admin"], "{m}");
        // The workspace owner is not an org admin: no path, so the sentence names Adam.
        assert_eq!(m["who_can"]["you_can"], false);
    }
    assert_eq!(method(&v, "token")["ready"], true);
    assert_eq!(
        v["suggested_method"], "token",
        "an admin method is never suggested: {v}"
    );
    // The plan is the same function.
    let (s, p) = w
        .call(
            &w.ws_owner,
            Method::GET,
            &format!("/v1/apps/ghlike/connect-plan?{}", w.ws_q()),
            None,
        )
        .await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(
        p["methods"], v["methods"],
        "the plan and the setup status agree"
    );
    assert_eq!(p["suggested_method"], v["suggested_method"]);
    assert_eq!(p["available"], v["available"]);
}

#[tokio::test]
async fn an_org_client_suggests_oauth_a_workspace_client_serves_that_workspace_only() {
    let w = world().await;
    // A workspace client for `oauth`: ready in the workspace view, still missing in the org view.
    w.client(
        &w.ws_owner,
        &format!("/v1/workspaces/{}/oauth-clients", w.ws),
        "gh_oauth",
    )
    .await;
    let (_, v) = w.setup(&w.ws_owner, "ghlike", &w.ws_q()).await;
    assert_eq!(method(&v, "oauth")["ready"], true, "{v}");
    assert_eq!(v["suggested_method"], "oauth");
    assert_eq!(v["clients"]["workspace"].as_array().unwrap().len(), 1);
    let (_, o) = w.setup(&w.org_admin, "ghlike", &w.org_q()).await;
    assert_eq!(method(&o, "oauth")["ready"], false, "{o}");
    assert_eq!(o["suggested_method"], "token");
    assert!(o["clients"]["workspace"].is_null());
    // An organization client makes it ready at both levels; used_by_methods names the method.
    w.client(
        &w.org_admin,
        &format!("/v1/orgs/{}/oauth-clients", w.org),
        "gh_oauth",
    )
    .await;
    let (_, o) = w.setup(&w.org_admin, "ghlike", &w.org_q()).await;
    assert_eq!(method(&o, "oauth")["ready"], true, "{o}");
    assert_eq!(o["suggested_method"], "oauth");
    assert_eq!(
        o["clients"]["organization"][0]["used_by_methods"],
        json!([{"id": "oauth", "label": "Sign in"}])
    );
    // The app detail names the same.
    let (_, d) = w
        .call(&w.org_admin, Method::GET, "/v1/apps/ghlike", None)
        .await;
    let oc = d["oauth_clients"].as_array().unwrap();
    let used = oc.iter().find(|c| c["client_key"] == "gh_oauth").unwrap();
    assert_eq!(
        used["used_by_methods"],
        json!([{"id": "oauth", "label": "Sign in"}])
    );
}

#[tokio::test]
async fn an_instance_client_is_set_up_without_an_id_and_nothing_secret_leaks() {
    let w = world().await;
    let (_, v) = w.setup(&w.ws_owner, "lin", &w.ws_q()).await;
    assert_eq!(v["suggested_method"], Value::Null, "{v}");
    assert_eq!(v["clients"]["instance"]["set_up"], false);
    let m = method(&v, "oauth");
    assert_eq!(m["who_can"]["role"], "org_admin");
    assert_eq!(
        m["scope_presets"],
        json!({"read": ["read"], "write": ["read", "write"]})
    );
    w.client(&w.root, "/v1/admin/oauth-clients", "linear").await;
    let (_, v) = w.setup(&w.ws_owner, "lin", &w.ws_q()).await;
    assert_eq!(v["clients"]["instance"], json!({"set_up": true}), "{v}");
    assert_eq!(v["suggested_method"], "oauth");
    // No client secret, client id, or e-mail, and never the instance admin, in any answer.
    for who in [&w.ws_owner, &w.org_admin] {
        for (app, q) in [("lin", w.ws_q()), ("ghlike", w.org_q())] {
            let (s, v) = w.setup(who, app, &q).await;
            let t = v.to_string();
            if s == 200 {
                assert!(!t.contains("SECRET"), "{t}");
                assert!(!t.contains('@'), "{t}");
                assert!(!t.contains("Root"), "{t}");
            }
        }
    }
}

#[tokio::test]
async fn external_id_only_for_packages_that_read_it_and_the_callback_url_is_given() {
    let w = world().await;
    let (_, a) = w.setup(&w.ws_owner, "awsk", &w.ws_q()).await;
    let ext: String = sqlx::query_scalar("SELECT external_id FROM organizations WHERE id = $1")
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(a["values"]["external_id"], json!(ext), "{a}");
    assert!(
        a["values"]["callback_url"]
            .as_str()
            .unwrap()
            .ends_with("/v1/oauth/callback")
    );
    let (_, l) = w.setup(&w.ws_owner, "lin", &w.ws_q()).await;
    assert!(l["values"].get("external_id").is_none(), "{l}");
}

#[tokio::test]
async fn an_instance_bound_app_without_a_server_needs_an_instance() {
    let w = world().await;
    let (_, v) = w.setup(&w.org_admin, "ghes", &w.org_q()).await;
    let m = method(&v, "token");
    assert_eq!(m["ready"], false, "{v}");
    assert_eq!(m["needs"], json!(["instance"]));
    assert_eq!(m["who_can"]["action"], "register_instance");
    assert_eq!(m["who_can"]["you_can"], true);
    assert_eq!(
        m["who_can"]["path"],
        format!("/o/{}/apps?tab=self-hosted", org_slug(&w).await)
    );
    assert_eq!(v["suggested_method"], Value::Null);
}

async fn org_slug(w: &W) -> String {
    sqlx::query_scalar("SELECT slug FROM organizations WHERE id = $1")
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_restricted_org_without_the_app_is_unavailable_and_says_who_allows_it() {
    let w = world().await;
    sqlx::query("UPDATE organizations SET app_posture = 'restricted' WHERE id = $1")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (_, v) = w.setup(&w.org_admin, "lin", &w.org_q()).await;
    assert_eq!(v["available"], false, "{v}");
    assert_eq!(v["available_who_can"]["role"], "org_admin");
    assert!(method(&v, "oauth")["ready"] == false);
    assert_eq!(v["suggested_method"], Value::Null);
    assert_eq!(v["can"]["connect_org"], false);
}

#[tokio::test]
async fn who_may_ask_for_a_setup_status() {
    let w = world().await;
    // A member cannot see the workspace view; the refusal names the workspace owners.
    let (s, v) = w.setup(&w.member, "lin", &w.ws_q()).await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(v["error"]["details"]["who_can"]["role"], "workspace_owner");
    // A workspace owner who is not an org admin cannot see the org view.
    let (s, v) = w.setup(&w.ws_owner, "lin", &w.org_q()).await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(v["error"]["details"]["who_can"]["role"], "org_admin");
    assert_eq!(people(&v["error"]["details"]["who_can"]), ["Adam Admin"]);
    // No level: 400.
    let (s, _) = w.setup(&w.org_admin, "lin", "").await;
    assert_eq!(s, 400);
    // Unknown app: 404.
    let (s, _) = w.setup(&w.org_admin, "nope", &w.org_q()).await;
    assert_eq!(s, 404);
}
