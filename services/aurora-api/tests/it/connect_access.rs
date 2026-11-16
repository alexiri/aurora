#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-UX-2 (D-322, C-957, C-988, SEC-130): connect with access, sealed through OAuth, and a
//! website or an AI key with access.

use crate::common::*;
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use aurora_testkit::oauth::MockOAuth;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

/// `acc_app` has two read tools and one write tool; `acc_custom` is the same but `custom_only`;
/// `acc_writes` has no read tool (the `read` level cannot expand to anything).
const YAML: &str = r#"package: { id: accpkg, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
permissions:
  token_hosts: [accounts.example.com]
oauth_clients:
  - id: goog
    authorize_url: https://accounts.example.com/auth
    token_url: https://accounts.example.com/token
    pkce: true
    fields:
      - { name: client_id, label: Client ID }
      - { name: client_secret, label: Secret, secret: true }
apps:
  - about: { id: acc_app, name: Acc, description: D, category: dev, web_hosts: [app.acc.example] }
    credentials:
      - { name: api_token, secret: true, optional: true }
      - { name: access_token, secret: true, optional: true }
      - { name: refresh_token, secret: true, optional: true }
    connect:
      - id: api_key
        kind: fields
        fields:
          - { name: api_token, label: T, type: secret, required: true }
      - id: oauth
        kind: oauth2
        client: goog
        scopes:
          - { scope: read, name: Read, access: read }
          - { scope: write, name: Write, access: write }
        default_scopes: [read, write]
        callback: query
    hosts:
      - match: { host: "api.acc.example" }
        inject:
          - bearer: "{credentials.api_token}"
    tools:
      read:
        items:
          - { id: list_things, name: List, host: api.acc.example, path: /things, methods: [GET] }
          - { id: get_thing, name: Get, host: api.acc.example, path: "/things/*", methods: [GET] }
      write:
        items:
          - { id: create_thing, name: Create, host: api.acc.example, path: /things, methods: [POST] }
  - about: { id: acc_custom, name: Acc Custom, description: D, category: dev, web_hosts: [app.cus.example] }
    credentials:
      - { name: api_token, secret: true, optional: true }
    connect:
      - id: api_key
        kind: fields
        fields:
          - { name: api_token, label: T, type: secret, required: true }
    grants: { custom_only: true, block_untooled: true }
    hosts:
      - match: { host: "api.cus.example" }
        inject:
          - bearer: "{credentials.api_token}"
    tools:
      read:
        items:
          - { id: look, name: Look, host: api.cus.example, path: /look, methods: [GET] }
  - about: { id: acc_writes, name: Acc Writes, description: D, category: dev, web_hosts: [app.wr.example] }
    credentials:
      - { name: api_token, secret: true, optional: true }
    connect:
      - id: api_key
        kind: fields
        fields:
          - { name: api_token, label: T, type: secret, required: true }
    hosts:
      - match: { host: "api.wr.example" }
        inject:
          - bearer: "{credentials.api_token}"
    tools:
      write:
        items:
          - { id: push, name: Push, host: api.wr.example, path: /push, methods: [POST] }
"#;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    owner: String,
    member: String,
    owner_id: Uuid,
    agent: Uuid,
    oauth: MockOAuth,
}

async fn world() -> W {
    let oauth = MockOAuth::start().await;
    let mut h = Harness::new().await;
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
    h.member(org, o, "member", "active").await;
    h.bind_ws(ws, o, "owner").await;
    let m = h.user("mem@x.io", false).await;
    h.member(org, m, "member", "active").await;
    h.bind_ws(ws, m, "member").await;
    let (_, ot) = h.good_session(o).await;
    let (_, mt) = h.good_session(m).await;
    let w = W {
        h,
        org,
        ws,
        owner: cookie(&ot),
        member: cookie(&mt),
        owner_id: o,
        agent: Uuid::now_v7(),
        oauth,
    };
    w.insert_agent(w.agent, w.ws, "bot").await;
    w
}

impl W {
    async fn insert_agent(&self, id: Uuid, ws: Uuid, handle: &str) {
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,$4,'Bot','byo','medium','')")
            .bind(id).bind(self.org).bind(ws).bind(handle).execute(self.h.pool()).await.unwrap();
    }

    async fn call(&self, who: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", who), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    fn body(&self, method: &str, grant: Option<Value>) -> Value {
        let mut b = json!({"scope": "workspace", "workspace_id": self.ws, "method_id": method,
            "fields": {"api_token": "tok"}});
        if let Some(g) = grant {
            b["grant"] = g;
        }
        b
    }

    async fn connect(&self, who: &str, app: &str, b: Value) -> (u16, Value) {
        self.call(
            who,
            Method::POST,
            &format!("/v1/apps/{app}/connect"),
            Some(b),
        )
        .await
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn page(&self, who: &str, cookie_pair: Option<&str>, uri: &str) -> String {
        let ck = match cookie_pair {
            Some(c) => format!("{who}; {c}"),
            None => who.to_owned(),
        };
        let req = Request::builder()
            .uri(uri)
            .header("cookie", ck)
            .body(Body::empty())
            .unwrap();
        let res = self.h.app.clone().oneshot(req).await.unwrap();
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
    }

    async fn oauth_client(&self) {
        let (s, v) = self
            .call(&self.owner, Method::POST, &format!("/v1/workspaces/{}/oauth-clients", self.ws),
                Some(json!({"package_id": "accpkg", "client_key": "goog", "fields": {"client_id": "cid", "client_secret": "csec"}})))
            .await;
        assert_eq!(s, 201, "{v}");
    }

    /// Start an OAuth connect with a grant: (provider URL, cookie pair, whole response body).
    async fn start_oauth(&self, grant: Value) -> (String, String, Value) {
        let mut b = self.body("oauth", Some(grant));
        b["fields"] = json!({});
        let (s, sc, v) = self
            .h
            .send(
                Method::POST,
                "/v1/apps/acc_app/connect",
                &[("cookie", self.owner.as_str()), ("origin", ORIGIN)],
                Some(b),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["kind"], "redirect");
        let c = sc
            .get_all("set-cookie")
            .iter()
            .next()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        (
            v["url"].as_str().unwrap().to_owned(),
            c.split(';').next().unwrap().to_owned(),
            v,
        )
    }
}

fn local(loc: &str) -> String {
    let i = loc.find("/v1/oauth/callback").unwrap();
    loc[i..].to_owned()
}

#[tokio::test]
async fn fields_connect_writes_the_account_and_each_level_expands_from_the_snapshot() {
    let w = world().await;
    for (level, mode, allow, ask, access) in [
        (
            "read",
            "custom",
            vec!["get_thing", "list_things"],
            vec![],
            "read",
        ),
        (
            "ask",
            "custom",
            vec!["get_thing", "list_things"],
            vec!["create_thing"],
            "write",
        ),
        ("act", "full", vec![], vec![], "write"),
    ] {
        let (s, v) = w
            .connect(
                &w.owner,
                "acc_app",
                w.body(
                    "api_key",
                    Some(json!({"agent_ids": [w.agent], "level": level})),
                ),
            )
            .await;
        assert_eq!(s, 200, "{level}: {v}");
        assert_eq!(v["kind"], "connected");
        assert_eq!(v["connection"]["access"], access, "{level}: {v}");
        let g = &v["grants"][0];
        assert_eq!(g["agent_id"], json!(w.agent), "{v}");
        assert_eq!(g["mode"], mode, "{level}: {v}");
        let sorted = |x: &Value| {
            let mut t: Vec<String> = x
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_owned())
                .collect();
            t.sort();
            t
        };
        assert_eq!(sorted(&g["allow_tools"]), allow, "{level}: {v}");
        assert_eq!(sorted(&g["ask_tools"]), ask, "{level}: {v}");
        // The audit row says the access came with the account.
        let via: Option<String> = sqlx::query_scalar(
            "SELECT metadata->>'via' FROM audit_log WHERE action = 'connection.create' \
             ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(w.h.pool())
        .await
        .unwrap();
        assert_eq!(via.as_deref(), Some("connect_journey"));
    }
    assert_eq!(w.count("app_connections").await, 3);
    assert_eq!(w.count("grants").await, 3);
    // Without `grant` the answer carries no grants and nothing else changes.
    let (s, v) = w
        .connect(&w.owner, "acc_app", w.body("api_key", None))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["grants"], json!([]));
    assert_eq!(w.count("grants").await, 3);
}

#[tokio::test]
async fn a_forced_failure_of_the_access_part_rolls_back_the_account_too() {
    let w = world().await;
    // The `read` level of an app with no read tool expands to nothing: the grant service refuses
    // it (`grant_invalid`) after the connection row was inserted, in the same transaction.
    let (s, v) = w
        .connect(
            &w.owner,
            "acc_writes",
            w.body(
                "api_key",
                Some(json!({"agent_ids": [w.agent], "level": "read"})),
            ),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "grant_invalid", "{v}");
    assert_eq!(v["error"]["details"]["agent_id"], json!(w.agent));
    assert_eq!(
        w.count("app_connections").await,
        0,
        "the account rolled back"
    );
    assert_eq!(w.count("grants").await, 0);
    // A database failure while writing the grant (here: a trigger) does the same.
    sqlx::query(
        "CREATE FUNCTION fail_grant() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'forced'; END $$",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_grant BEFORE INSERT ON grants FOR EACH ROW EXECUTE FUNCTION fail_grant()")
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .connect(
            &w.owner,
            "acc_app",
            w.body(
                "api_key",
                Some(json!({"agent_ids": [w.agent], "level": "ask"})),
            ),
        )
        .await;
    assert!(s >= 400, "{v}");
    assert_eq!(w.count("app_connections").await, 0, "{v}");
    let audited: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'connection.create'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(audited, 0, "no audit row for a rolled-back account");
}

#[tokio::test]
async fn refused_before_any_write() {
    let w = world().await;
    // act on a custom_only app: 422, nothing written.
    let (s, v) = w
        .connect(
            &w.owner,
            "acc_custom",
            w.body(
                "api_key",
                Some(json!({"agent_ids": [w.agent], "level": "act"})),
            ),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "full_grant_forbidden");
    // The other levels are fine on it.
    let (s, v) = w
        .connect(
            &w.owner,
            "acc_custom",
            w.body(
                "api_key",
                Some(json!({"agent_ids": [w.agent], "level": "read"})),
            ),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let before = (w.count("app_connections").await, w.count("grants").await);
    // An agent of another workspace, an unknown agent, a non-owner, duplicates, none, too many.
    let other_ws = w.h.workspace(w.org).await;
    let foreign = Uuid::now_v7();
    w.insert_agent(foreign, other_ws, "foreign").await;
    let cases = [
        (
            "agent of another workspace",
            w.owner.as_str(),
            json!({"agent_ids": [foreign], "level": "read"}),
            404,
        ),
        (
            "unknown agent",
            w.owner.as_str(),
            json!({"agent_ids": [Uuid::now_v7()], "level": "read"}),
            404,
        ),
        (
            "duplicates",
            w.owner.as_str(),
            json!({"agent_ids": [w.agent, w.agent], "level": "read"}),
            400,
        ),
        (
            "none",
            w.owner.as_str(),
            json!({"agent_ids": [], "level": "read"}),
            400,
        ),
        (
            "too many",
            w.owner.as_str(),
            json!({"agent_ids": (0..51).map(|_| Uuid::now_v7()).collect::<Vec<_>>(), "level": "read"}),
            400,
        ),
    ];
    for (what, who, grant, want) in cases {
        let (s, v) = w
            .connect(who, "acc_app", w.body("api_key", Some(grant)))
            .await;
        assert_eq!(s, want, "{what}: {v}");
    }
    // A member cannot connect at all (can_own), with or without a grant.
    let (s, v) = w
        .connect(
            &w.member,
            "acc_app",
            w.body(
                "api_key",
                Some(json!({"agent_ids": [w.agent], "level": "read"})),
            ),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("forbidden")),
        "{v}"
    );
    assert_eq!(
        before,
        (w.count("app_connections").await, w.count("grants").await),
        "nothing was written by any refusal"
    );
    // A grant for an organization account is not a thing; a level that is not one is a 400.
    let adam = w.h.user("adam@x.io", false).await;
    w.h.member(w.org, adam, "admin", "active").await;
    let (_, at) = w.h.good_session(adam).await;
    let (s, _) = w
        .connect(
            &cookie(&at),
            "acc_app",
            json!({"scope": "organization", "org_id": w.org, "method_id": "api_key",
                   "fields": {"api_token": "t"}, "grant": {"agent_ids": [w.agent], "level": "read"}}),
        )
        .await;
    assert_eq!(s, 400);
    let (s, _) = w
        .connect(
            &w.owner,
            "acc_app",
            w.body(
                "api_key",
                Some(json!({"agent_ids": [w.agent], "level": "root"})),
            ),
        )
        .await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn a_reconnect_with_a_grant_is_refused() {
    let w = world().await;
    let (s, v) = w
        .connect(&w.owner, "acc_app", w.body("api_key", None))
        .await;
    assert_eq!(s, 200, "{v}");
    let id = v["connection"]["id"].as_str().unwrap().to_owned();
    let mut b = w.body(
        "api_key",
        Some(json!({"agent_ids": [w.agent], "level": "read"})),
    );
    b["reconnect_connection_id"] = json!(id);
    let (s, v) = w.connect(&w.owner, "acc_app", b).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "grant_not_allowed_on_reconnect");
    assert_eq!(w.count("grants").await, 0);
    // Without a grant a reconnect is unchanged.
    let mut b = w.body("api_key", None);
    b["reconnect_connection_id"] = json!(id);
    let (s, v) = w.connect(&w.owner, "acc_app", b).await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn oauth_applies_the_access_at_the_callback_and_the_state_holds_no_agent_id() {
    let w = world().await;
    w.oauth_client().await;
    w.oauth.set_extra(json!({"scope": "read write"}));
    let (url, ck, start) = w
        .start_oauth(json!({"agent_ids": [w.agent], "level": "ask"}))
        .await;
    // Nothing was written before the redirect, and nothing readable names the agent.
    assert_eq!(w.count("app_connections").await, 0);
    assert_eq!(w.count("grants").await, 0);
    let agent = w.agent.to_string();
    for (what, hay) in [
        ("url", url.as_str()),
        ("cookie", ck.as_str()),
        ("body", &start.to_string()),
    ] {
        assert!(!hay.contains(&agent), "the {what} names the agent: {hay}");
        assert!(!hay.contains(&agent.replace('-', "")), "{what}");
    }
    let loc = w.oauth.approve(&url).await;
    let cb = local(&loc);
    // A tampered state or another user's session: refused, nothing written.
    let bad = w
        .page(&w.owner, Some(&ck), &cb.replace("state=", "state=A"))
        .await;
    assert!(bad.contains("error=oauth_state_invalid"), "{bad}");
    let other = w.page(&w.member, Some(&ck), &cb).await;
    assert!(other.contains("error=oauth_state_invalid"), "{other}");
    assert_eq!(w.count("app_connections").await, 0);
    assert_eq!(w.count("grants").await, 0);
    // The callback writes the account and the access, and tells the dialog which agents.
    let pg = w.page(&w.owner, Some(&ck), &cb).await;
    assert!(pg.contains("type:\"aurora:connected\""), "{pg}");
    assert!(
        pg.contains(&format!("granted_agent_ids:[\"{agent}\"]")),
        "{pg}"
    );
    assert!(!pg.contains("grant_failed"), "{pg}");
    assert_eq!(w.count("app_connections").await, 1);
    let (mode, allow, ask): (String, Vec<String>, Vec<String>) =
        sqlx::query_as("SELECT mode, allow_tools, ask_tools FROM grants WHERE agent_id = $1")
            .bind(w.agent)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(mode, "custom");
    assert_eq!(allow.len(), 2);
    assert_eq!(ask, ["create_thing"]);
    let access: String = sqlx::query_scalar("SELECT access FROM app_connections")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(access, "write");
    // The state cannot be replayed.
    let again = w.page(&w.owner, Some(&ck), &cb).await;
    assert!(again.contains("error=oauth_"), "{again}");
    assert_eq!(w.count("app_connections").await, 1);
}

#[tokio::test]
async fn oauth_is_authorized_before_the_redirect() {
    let w = world().await;
    w.oauth_client().await;
    let foreign = Uuid::now_v7();
    let other_ws = w.h.workspace(w.org).await;
    w.insert_agent(foreign, other_ws, "foreign").await;
    let mut b = w.body(
        "oauth",
        Some(json!({"agent_ids": [foreign], "level": "read"})),
    );
    b["fields"] = json!({});
    let (s, v) = w.connect(&w.owner, "acc_app", b).await;
    assert_eq!(s, 404, "{v}");
    assert!(v.get("url").is_none());
    let mut b = w.body(
        "oauth",
        Some(json!({"agent_ids": [w.agent], "level": "read"})),
    );
    b["fields"] = json!({});
    let (s, v) = w.connect(&w.member, "acc_app", b).await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(w.count("app_connections").await, 0);
}

#[tokio::test]
async fn an_agent_deleted_between_start_and_callback_keeps_the_account_without_access() {
    let w = world().await;
    w.oauth_client().await;
    w.oauth.set_extra(json!({"scope": "read write"}));
    let keep = Uuid::now_v7();
    w.insert_agent(keep, w.ws, "keeper").await;
    let (url, ck, _) = w
        .start_oauth(json!({"agent_ids": [keep, w.agent], "level": "read"}))
        .await;
    let loc = w.oauth.approve(&url).await;
    sqlx::query("DELETE FROM agents WHERE id = $1")
        .bind(w.agent)
        .execute(w.h.pool())
        .await
        .unwrap();
    let pg = w.page(&w.owner, Some(&ck), &local(&loc)).await;
    assert!(pg.contains("type:\"aurora:connected\""), "{pg}");
    assert!(pg.contains("granted_agent_ids:[]"), "{pg}");
    assert!(pg.contains("grant_failed:{code:\"not_found\"}"), "{pg}");
    // The account is kept; the access is all or nothing, so even the surviving agent has none.
    assert_eq!(w.count("app_connections").await, 1);
    assert_eq!(w.count("grants").await, 0);
}

#[tokio::test]
async fn a_caller_demoted_between_start_and_callback_writes_nothing() {
    let w = world().await;
    w.oauth_client().await;
    let (url, ck, _) = w
        .start_oauth(json!({"agent_ids": [w.agent], "level": "read"}))
        .await;
    let loc = w.oauth.approve(&url).await;
    sqlx::query("UPDATE workspace_access SET role = 'member' WHERE user_id = $1")
        .bind(w.owner_id)
        .execute(w.h.pool())
        .await
        .unwrap();
    // A caller who is no longer a workspace owner can neither connect nor give access: the
    // callback is refused as it always was (authorize_scope) and nothing is written.
    let pg = w.page(&w.owner, Some(&ck), &local(&loc)).await;
    assert!(pg.contains("error=oauth_exchange_failed"), "{pg}");
    assert_eq!(w.count("app_connections").await, 0);
    assert_eq!(w.count("grants").await, 0);
}

#[tokio::test]
async fn a_website_or_an_ai_key_is_made_with_its_access_in_one_transaction() {
    let w = world().await;
    let url = format!("/v1/workspaces/{}/secrets", w.ws);
    let body = |name: &str, grant: Option<Value>| {
        let mut b = json!({"name": name, "kind": "generic", "host_pattern": "api.example.test",
            "value": "sekret"});
        if let Some(g) = grant {
            b["grant"] = g;
        }
        b
    };
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &url,
            Some(body("site", Some(json!({"agent_ids": [w.agent]})))),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let sid: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM grants WHERE secret_id = $1 AND agent_id = $2")
            .bind(sid)
            .bind(w.agent)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(n, 1);
    // Refusals as C-504, before anything is written.
    let foreign = Uuid::now_v7();
    let other_ws = w.h.workspace(w.org).await;
    w.insert_agent(foreign, other_ws, "foreign").await;
    let before = w.count("secrets").await;
    for (what, who, grant, want) in [
        (
            "other workspace",
            &w.owner,
            json!({"agent_ids": [foreign]}),
            404,
        ),
        (
            "unknown",
            &w.owner,
            json!({"agent_ids": [Uuid::now_v7()]}),
            404,
        ),
        (
            "duplicates",
            &w.owner,
            json!({"agent_ids": [w.agent, w.agent]}),
            400,
        ),
        ("none", &w.owner, json!({"agent_ids": []}), 400),
        (
            "not an owner",
            &w.member,
            json!({"agent_ids": [w.agent]}),
            403,
        ),
    ] {
        let (s, v) = w
            .call(who, Method::POST, &url, Some(body("refused", Some(grant))))
            .await;
        assert_eq!(s, want, "{what}: {v}");
    }
    assert_eq!(w.count("secrets").await, before, "nothing was written");
    // A failure of the access part rolls the secret back with it.
    sqlx::query(
        "CREATE FUNCTION fail_grant() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'forced'; END $$",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_grant BEFORE INSERT ON grants FOR EACH ROW EXECUTE FUNCTION fail_grant()")
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &url,
            Some(body("rolled-back", Some(json!({"agent_ids": [w.agent]})))),
        )
        .await;
    assert!(s >= 400, "{v}");
    assert_eq!(w.count("secrets").await, before, "{v}");
    // An organization secret takes no grant.
    let (s, _) = w
        .call(
            &w.owner,
            Method::POST,
            &format!("/v1/orgs/{}/secrets", w.org),
            Some(body("org", Some(json!({"agent_ids": [w.agent]})))),
        )
        .await;
    assert!(matches!(s, 400 | 403), "{s}");
}
