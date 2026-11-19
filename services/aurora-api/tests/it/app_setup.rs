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
  - about: { id: ghlike, name: GH Like, description: D, category: dev, summary: Repositories and issues on GH Like., first_prompt: "Which issues are open?" }
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
        access_levels:
          - { id: read, label: Read only, permission_hints: ["Contents · Read", "Issues · Read"] }
          - { id: write, label: Read and write, permission_hints: ["Contents · Read and write", "Issues · Read and write"] }
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

/// The AWS-like package's guide: every placeholder, and one that must stay literal.
const GUIDE: &str = "# Set up\n\nCallback: {{aurora.callback_url}}\n\nExternal id: {{aurora.external_id}}\n\nSetup: {{aurora.app_setup}}\n\nClients: {{aurora.page:oauth_clients}}\n\nHosted: {{aurora.page:self_hosted}}\n\nPackages: {{aurora.page:packages}}\n";

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
    install_with_guide(h, admin, key, yaml, None).await;
}

async fn install_with_guide(
    h: &Harness,
    admin: &str,
    key: &SigningKey,
    yaml: &str,
    guide: Option<&[u8]>,
) {
    let hd = [("cookie", admin), ("origin", ORIGIN)];
    let (_, v) = upload(h, admin, key, yaml, guide).await;
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

/// Upload a signed package; the status and body of the answer.
async fn upload(
    h: &Harness,
    admin: &str,
    key: &SigningKey,
    yaml: &str,
    guide: Option<&[u8]>,
) -> (u16, Value) {
    let mut b = PackageBuilder::new(yaml);
    if let Some(g) = guide {
        b.set_file("docs/setup.md", g);
    }
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
    let status = res.status().as_u16();
    let v: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, v)
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
    world_with(|_| {}).await
}

async fn world_with(mutate: impl FnOnce(&mut aurora_config::ApiConfig)) -> W {
    let h = Harness::with_config(mutate).await;
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
    install_with_guide(&h, &root_ck, &key, AWS, Some(GUIDE.as_bytes())).await;
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

/// C-924: the instance setup status lists the apps people cannot sign in to for lack of an OAuth
/// client, and says so when e-mail is not set up.
#[tokio::test]
async fn setup_status_lists_apps_people_cannot_sign_in_to_and_missing_email() {
    let w = world().await;
    let (s, v) = w
        .call(
            &w.root,
            Method::PUT,
            "/v1/admin/placement",
            Some(json!({"mode": "shared_org", "default_org_id": w.org})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let optional = |v: &Value| -> Vec<Value> {
        v["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == "optional")
            .unwrap()["checks"]
            .as_array()
            .unwrap()
            .clone()
    };
    let by_id = |checks: &[Value], id: &str| checks.iter().find(|c| c["id"] == id).cloned();

    let (s, v) = w.call(&w.root, Method::GET, "/v1/admin/setup", None).await;
    assert_eq!(s, 200, "{v}");
    let checks = optional(&v);
    // Linear-like: its only way to connect is an OAuth client that does not exist.
    let lin = by_id(&checks, "app:lin").unwrap();
    assert_eq!(lin["reason"], "app_needs_oauth_client", "{lin}");
    assert_eq!(lin["status"], "warning");
    assert_eq!(
        (
            lin["app_id"].as_str(),
            lin["package"].as_str(),
            lin["client_key"].as_str()
        ),
        (Some("lin"), Some("multi"), Some("linear"))
    );
    assert_eq!(lin["value"], "Linear Like");
    // GitHub-like: a token works, so it is only informational.
    let gh = by_id(&checks, "app:ghlike").unwrap();
    assert_eq!(gh["reason"], "app_token_only", "{gh}");
    assert_eq!(gh["status"], "ok");
    // Apps blocked by something else, and apps with no OAuth method, are not listed.
    assert!(by_id(&checks, "app:ghes").is_none());
    assert!(by_id(&checks, "app:awsk").is_none());
    // The test instance has no SMTP server.
    let mail = by_id(&checks, "email").unwrap();
    assert_eq!(mail["reason"], "email_not_configured");
    assert_eq!(
        mail["message"],
        "E-mail is not set up. Aurora cannot send approval or invitation e-mails."
    );

    // An organization client for Linear takes Linear off the list; the other stays.
    w.client(
        &w.org_admin,
        &format!("/v1/orgs/{}/oauth-clients", w.org),
        "linear",
    )
    .await;
    let (_, v) = w.call(&w.root, Method::GET, "/v1/admin/setup", None).await;
    let checks = optional(&v);
    assert!(by_id(&checks, "app:lin").is_none(), "{checks:?}");
    assert!(by_id(&checks, "app:ghlike").is_some());
    // No secret-shaped text, no client id.
    assert!(!v.to_string().contains("SECRET"));
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

// ---- P5-UX-13 (D-334, C-940): the guide, filled in for the viewer ----

async fn guide(w: &W, who: &str, app: &str, q: &str) -> (u16, String) {
    let req = Request::builder()
        .uri(format!("/v1/apps/{app}/guide?{q}"))
        .header("cookie", who)
        .header("origin", ORIGIN)
        .body(Body::empty())
        .unwrap();
    let res = w.h.app.clone().oneshot(req).await.unwrap();
    let s = res.status().as_u16();
    let b = res.into_body().collect().await.unwrap().to_bytes();
    (s, String::from_utf8_lossy(&b).into_owned())
}

async fn ws_slug(w: &W) -> String {
    sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1")
        .bind(w.ws)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn the_guide_is_filled_in_for_a_workspace_owner() {
    let w = world().await;
    let (s, g) = guide(&w, &w.ws_owner, "awsk", &w.ws_q()).await;
    assert_eq!(s, 200, "{g}");
    let (o, ws) = (org_slug(&w).await, ws_slug(&w).await);
    let b = external(&g);
    assert!(!g.contains("{{aurora."), "{g}");
    assert!(
        g.contains("Callback: `") && g.contains("/v1/oauth/callback`"),
        "{g}"
    );
    // A member of the organization sees its external id (the test org's equals its slug).
    assert!(g.contains(&format!("External id: `{o}`")), "{g}");
    // The workspace owner's setup page; a workspace has no OAuth clients tab, so it is the
    // same page. The tabs they cannot open name who can.
    let setup = format!("[AWS Keys setup]({b}/o/{o}/w/{ws}/apps/awsk)");
    assert!(g.contains(&format!("Setup: {setup}")), "{g}");
    assert!(g.contains(&format!("Clients: {setup}")), "{g}");
    assert!(
        g.contains("Hosted: Self-hosted apps (ask an organization admin: Adam Admin)"),
        "{g}"
    );
    assert!(
        g.contains("Packages: Packages (ask an administrator of this Aurora instance)"),
        "{g}"
    );
}

/// The deployment's external URL, read back from the callback URL the guide printed.
fn external(g: &str) -> String {
    let i = g.find("Callback: `").unwrap() + "Callback: `".len();
    let j = g.find("/v1/oauth/callback`").unwrap();
    g[i..j].to_owned()
}

#[tokio::test]
async fn the_guide_links_an_org_admin_to_the_organizations_tabs() {
    let w = world().await;
    let o = org_slug(&w).await;
    for q in [w.org_q(), w.ws_q()] {
        let (s, g) = guide(&w, &w.org_admin, "awsk", &q).await;
        assert_eq!(s, 200, "{g}");
        let b = external(&g);
        assert!(
            g.contains(&format!(
                "Clients: [OAuth clients]({b}/o/{o}/apps?tab=oauth-clients)"
            )),
            "{g}"
        );
        assert!(
            g.contains(&format!(
                "Hosted: [Self-hosted apps]({b}/o/{o}/apps?tab=self-hosted)"
            )),
            "{g}"
        );
        assert!(
            g.contains("Packages: Packages (ask an administrator of this Aurora instance)"),
            "{g}"
        );
    }
    let (_, g) = guide(&w, &w.org_admin, "awsk", &w.org_q()).await;
    assert!(
        g.contains(&format!(
            "Setup: [AWS Keys setup]({}/o/{o}/apps/awsk)",
            external(&g)
        )),
        "{g}"
    );
}

#[tokio::test]
async fn the_guide_sends_an_instance_admin_to_administration_and_hides_the_external_id() {
    let w = world().await;
    // No level named (and not a member of any organization): the callback URL is real, the
    // organization's id is not shown, every tab is the Administration one.
    let (s, g) = guide(&w, &w.root, "awsk", "").await;
    assert_eq!(s, 200, "{g}");
    let b = external(&g);
    assert!(
        g.contains("External id: (shown to members of your organization)"),
        "{g}"
    );
    assert!(
        g.contains(&format!(
            "Clients: [OAuth clients]({b}/admin/apps?tab=oauth-clients)"
        )),
        "{g}"
    );
    assert!(
        g.contains(&format!(
            "Hosted: [Self-hosted apps]({b}/admin/apps?tab=self-hosted)"
        )),
        "{g}"
    );
    assert!(
        g.contains(&format!(
            "Packages: [Packages]({b}/admin/apps?tab=packages)"
        )),
        "{g}"
    );
    assert!(
        g.contains("Setup: the app's setup page (ask an organization admin)"),
        "{g}"
    );
}

#[tokio::test]
async fn a_plain_member_sees_the_external_id_and_who_to_ask() {
    let w = world().await;
    let o = org_slug(&w).await;
    let (s, g) = guide(&w, &w.member, "awsk", &w.ws_q()).await;
    assert_eq!(s, 200, "{g}");
    assert!(g.contains(&format!("External id: `{o}`")), "{g}");
    assert!(
        g.contains("Setup: the app's setup page (ask a workspace owner: Adam Admin, Wendy Owner)"),
        "{g}"
    );
    assert!(
        g.contains("Clients: OAuth clients (ask an organization admin: Adam Admin)"),
        "{g}"
    );
    // Someone outside the organization learns nothing about it: not found, no id.
    let stranger = named(&w.h, "out@x.io", "Out Sider", false).await;
    let (_, t) = w.h.good_session(stranger).await;
    for q in [w.ws_q(), w.org_q()] {
        let (s, g) = guide(&w, &cookie(&t), "awsk", &q).await;
        assert_eq!(s, 404, "{g}");
        assert!(!g.contains(&o), "{g}");
    }
}

#[tokio::test]
async fn guide_values_are_escaped_and_never_substituted_twice() {
    let w = world().await;
    sqlx::query("UPDATE organizations SET external_id = $2 WHERE id = $1")
        .bind(w.org)
        .bind("a`*_[x](y){{aurora.page:packages}}")
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, g) = guide(&w, &w.ws_owner, "awsk", &w.ws_q()).await;
    assert_eq!(s, 200, "{g}");
    // A code span with a longer fence than any backtick run in the value, text kept as is.
    assert!(
        g.contains("External id: ``a`*_[x](y){{aurora.page:packages}}``"),
        "{g}"
    );
    // The value's placeholder was not filled in a second pass.
    assert_eq!(g.matches("{{aurora.").count(), 1, "{g}");
}

#[tokio::test]
async fn an_unknown_placeholder_is_refused_when_the_package_is_uploaded() {
    let w = world().await;
    let key = generate_signing_key().unwrap();
    let (s, v) = w
        .call(
            &w.root,
            Method::POST,
            "/v1/admin/trusted-keys",
            Some(json!({"publisher": "acme", "name": "k2", "public_key": PackageBuilder::key_id(&key)})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (status, v) = upload(
        &w.h,
        &w.root,
        &key,
        &AWS.replace("id: awspkg", "id: awspkg2")
            .replace("id: awsk", "id: awsk2"),
        Some(b"Open {{aurora.page:admin_apps}}.\n"),
    )
    .await;
    assert!(
        v.to_string().contains("guide_unknown_placeholder"),
        "{status} {v}"
    );
    assert_ne!(v["status"], "pending_approval", "{v}");
}

#[path = "app_setup_requests.rs"]
mod requests;

// ---- P5-POLISH-3 (D-388, C-1029): readiness in the catalogue ----

impl W {
    async fn catalogue(&self, who: &str) -> (u16, Value) {
        self.call(
            who,
            Method::GET,
            &format!("/v1/apps?workspace_id={}", self.ws),
            None,
        )
        .await
    }
}

fn item<'a>(list: &'a Value, app: &str) -> &'a Value {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["app_id"] == app)
        .unwrap_or_else(|| panic!("{app} not listed: {list}"))
}

/// The catalogue's readiness is the connect plan's, for every app of the fixture: `ready` exactly when the plan
/// suggests a method; otherwise `needs` is the union of what the people methods need, in a fixed order, and
/// `who_can` the people who can fix the first blocked one.
async fn assert_catalogue_agrees_with_plans(w: &W, who: &str) {
    let (s, list) = w.catalogue(who).await;
    assert_eq!(s, 200, "{list}");
    for app in ["ghlike", "lin", "ghes", "awsk"] {
        let it = item(&list, app);
        let (s, plan) = w
            .call(
                who,
                Method::GET,
                &format!("/v1/apps/{app}/connect-plan?{}", w.ws_q()),
                None,
            )
            .await;
        assert_eq!(s, 200, "{plan}");
        let r = &it["readiness"];
        let ready = !plan["suggested_method"].is_null();
        assert_eq!(
            r["state"],
            if ready { "ready" } else { "needs_admin" },
            "{app}: {r} vs {plan}"
        );
        if ready {
            assert!(
                r.get("needs").is_none() && r.get("who_can").is_none(),
                "{app}: {r}"
            );
            continue;
        }
        let blocked: Vec<&Value> = plan["methods"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["audience"] == "person" && m["ready"] == false)
            .collect();
        let mut want: Vec<&str> = Vec::new();
        for n in ["oauth_client", "instance", "admin_material"] {
            if blocked
                .iter()
                .any(|m| m["needs"].as_array().unwrap().iter().any(|x| x == n))
            {
                want.push(n);
            }
        }
        assert_eq!(r["needs"], json!(want), "{app}: {r}");
        assert_eq!(r["who_can"], blocked[0]["who_can"], "{app}: {r}");
    }
}

#[tokio::test]
async fn catalogue_readiness_follows_the_connect_plan_as_clients_and_servers_appear() {
    let w = world().await;
    // Nothing set up: the token apps are ready, the sign-in-only and server-bound ones need an admin.
    let (_, list) = w.catalogue(&w.ws_owner).await;
    assert_eq!(
        item(&list, "ghlike")["readiness"],
        json!({"state": "ready"})
    );
    assert_eq!(item(&list, "awsk")["readiness"], json!({"state": "ready"}));
    let lin = &item(&list, "lin")["readiness"];
    assert_eq!(lin["state"], "needs_admin", "{lin}");
    assert_eq!(lin["needs"], json!(["oauth_client"]));
    assert_eq!(lin["who_can"]["role"], "org_admin");
    assert_eq!(people(&lin["who_can"]), ["Adam Admin"]);
    assert_eq!(
        item(&list, "ghes")["readiness"]["needs"],
        json!(["instance"])
    );
    assert_catalogue_agrees_with_plans(&w, &w.ws_owner).await;
    assert_catalogue_agrees_with_plans(&w, &w.org_admin).await;

    // An instance client makes the sign-in app ready, at once and for everyone.
    w.client(&w.root, "/v1/admin/oauth-clients", "linear").await;
    let (_, list) = w.catalogue(&w.ws_owner).await;
    assert_eq!(item(&list, "lin")["readiness"], json!({"state": "ready"}));
    assert_catalogue_agrees_with_plans(&w, &w.ws_owner).await;

    // A member reads the same readiness (they may not connect, but the catalogue says what is missing).
    let (s, list) = w.catalogue(&w.member).await;
    assert_eq!(s, 200, "{list}");
    assert_eq!(item(&list, "ghes")["readiness"]["state"], "needs_admin");
}

#[tokio::test]
async fn a_switched_off_app_is_hidden_from_a_member_and_shown_to_an_admin_as_unavailable() {
    let w = world().await;
    sqlx::query("UPDATE organizations SET app_posture = 'restricted' WHERE id = $1")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO org_app_availability (org_id, app_id) VALUES ($1, 'ghlike')")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    // A workspace owner and a member see only what is switched on.
    for who in [&w.ws_owner, &w.member] {
        let (s, list) = w.catalogue(who).await;
        assert_eq!(s, 200, "{list}");
        let ids: Vec<&str> = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["app_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["ghlike"], "{list}");
        assert_eq!(item(&list, "ghlike")["available"], true);
    }
    // An organization admin sees all of them; a switched-off one is `available: false` and says who allows it.
    let (_, list) = w.catalogue(&w.org_admin).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 4, "{list}");
    let lin = item(&list, "lin");
    assert_eq!(lin["available"], false);
    assert_eq!(lin["readiness"]["state"], "needs_admin");
    assert_eq!(lin["readiness"]["who_can"]["action"], "allow_app");
    assert_eq!(lin["readiness"]["who_can"]["you_can"], true);
    assert_eq!(item(&list, "ghlike")["readiness"]["state"], "ready");
}

/// (P5-POLISH-3, D-387, C-1031) The package's text for people reaches the clients: `summary` on the catalogue,
/// `summary`, `first_prompt` and the token permissions per level on the app, and the same hints on the connect
/// plan's methods. A token method's levels are not account choices (`access_levels` stays an OAuth thing).
#[tokio::test]
async fn package_text_summary_first_prompt_and_permission_hints_are_served() {
    let w = world().await;
    let (_, list) = w.catalogue(&w.member).await;
    assert_eq!(
        item(&list, "ghlike")["summary"],
        "Repositories and issues on GH Like."
    );
    assert!(item(&list, "lin")["summary"].is_null());

    let (s, d) = w
        .call(&w.member, Method::GET, "/v1/apps/ghlike", None)
        .await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(d["summary"], "Repositories and issues on GH Like.");
    assert_eq!(d["first_prompt"], "Which issues are open?");
    let token = method_of(&d["connect"], "token");
    assert_eq!(
        token["permission_hints"],
        json!([
            {"level": "read", "label": "Read only", "hints": ["Contents · Read", "Issues · Read"]},
            {"level": "write", "label": "Read and write", "hints": ["Contents · Read and write", "Issues · Read and write"]},
        ]),
        "{token}"
    );
    assert_eq!(token["access_levels"], json!([]), "not an account choice");
    assert_eq!(
        method_of(&d["connect"], "oauth")["permission_hints"],
        json!([])
    );

    let (_, plan) = w
        .call(
            &w.ws_owner,
            Method::GET,
            &format!("/v1/apps/ghlike/connect-plan?{}", w.ws_q()),
            None,
        )
        .await;
    assert_eq!(
        method(&plan, "token")["permission_hints"],
        token["permission_hints"]
    );
}

fn method_of<'a>(methods: &'a Value, id: &str) -> &'a Value {
    methods
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap_or_else(|| panic!("{id}: {methods}"))
}

#[tokio::test]
async fn the_catalogue_lists_the_accounts_a_caller_may_give_and_the_agents_using_them() {
    let w = world().await;
    let conn = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) \
         VALUES ($1,$2,$3,'ghlike','multi','1.0.0','Work GitHub','active','x')",
    )
    .bind(conn)
    .bind(w.org)
    .bind(w.ws)
    .execute(w.h.pool())
    .await
    .unwrap();
    let (_, list) = w.catalogue(&w.ws_owner).await;
    assert_eq!(
        item(&list, "ghlike")["accounts"],
        json!([{"id": conn, "name": "Work GitHub", "agent_ids": []}]),
        "{list}"
    );
    assert_eq!(item(&list, "lin")["accounts"], json!([]));
    assert_eq!(item(&list, "ghlike")["connection_count"], 1);
    // The catalogue without a workspace has no readiness and no accounts.
    let (s, plain) = w.call(&w.ws_owner, Method::GET, "/v1/apps", None).await;
    assert_eq!(s, 200, "{plain}");
    assert!(item(&plain, "ghlike")["readiness"].is_null());
}
