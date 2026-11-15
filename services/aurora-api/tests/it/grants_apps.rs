#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-154 (D-306(c), C-895, C-896): credential-free apps and `PUT /agents/{agent}/grants/apps/{app}`.

use crate::common::*;
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

/// `free_app`: nothing to enter. `free_custom`: the same, `custom_only`. `needs_key`: a field.
const YAML: &str = r#"package: { id: acme, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
apps:
  - about: { id: free_app, name: Free Directory, description: D, category: dev, label_hint: Free directory }
    connect:
      - { id: anonymous, kind: fields, fields: [] }
    tools:
      read:
        items:
          - { id: look, name: Look, host: free.example, path: /look, methods: [GET] }
    hosts:
      - match: { host: free.example }
        inject: []
  - about: { id: free_custom, name: Free Custom, description: D, category: dev }
    connect:
      - { id: anonymous, kind: fields, fields: [] }
    grants: { custom_only: true, block_untooled: true }
    tools:
      read:
        items:
          - { id: look, name: Look, host: custom.example, path: /look, methods: [GET] }
    hosts:
      - match: { host: custom.example }
        inject: []
  - about: { id: needs_key, name: Needs Key, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    tools:
      read:
        items:
          - { id: look, name: Look, host: key.example, path: /look, methods: [GET] }
    hosts:
      - match: { host: key.example }
        inject:
          - bearer: "{credentials.api_token}"
"#;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    owner: String,
    member: String,
    agent: Uuid,
}

async fn world() -> W {
    let h = Harness::new().await;
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
    assert_eq!(res.status(), 201);
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
    let w = W {
        agent: Uuid::now_v7(),
        h,
        org,
        ws,
        owner: cookie(&ot),
        member: cookie(&mt),
    };
    w.insert_agent(w.agent, "bot").await;
    w
}

impl W {
    async fn insert_agent(&self, id: Uuid, handle: &str) {
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,$4,'Bot','byo','medium','')")
            .bind(id).bind(self.org).bind(self.ws).bind(handle).execute(self.h.pool()).await.unwrap();
    }

    async fn call(&self, who: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", who), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    fn url(&self, agent: Uuid, app: &str) -> String {
        format!("/v1/agents/{agent}/grants/apps/{app}")
    }

    async fn give(&self, agent: Uuid, app: &str) -> (u16, Value) {
        self.call(
            &self.owner,
            Method::PUT,
            &self.url(agent, app),
            Some(json!({"mode": "full"})),
        )
        .await
    }

    async fn connections(&self, app: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM app_connections WHERE workspace_id = $1 AND app_id = $2",
        )
        .bind(self.ws)
        .bind(app)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn credential_free_is_reported_on_the_catalogue() {
    let w = world().await;
    let (s, v) = w.call(&w.owner, Method::GET, "/v1/apps", None).await;
    assert_eq!(s, 200, "{v}");
    let free = |id: &str| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["app_id"] == id)
            .unwrap_or_else(|| panic!("{id}: {v}"))["credential_free"]
            .clone()
    };
    assert_eq!(free("free_app"), true);
    assert_eq!(free("free_custom"), true);
    assert_eq!(free("needs_key"), false);
    let (s, v) = w
        .call(&w.owner, Method::GET, "/v1/apps/free_app", None)
        .await;
    assert_eq!((s, v["credential_free"].clone()), (200, json!(true)), "{v}");
}

#[tokio::test]
async fn creates_the_connection_once_then_reuses_it_for_the_same_and_another_agent() {
    let w = world().await;
    let (s, v) = w.give(w.agent, "free_app").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["created"], true);
    assert_eq!(v["connection"]["app_id"], "free_app");
    assert_eq!(v["connection"]["label"], "Free directory");
    assert_eq!(v["grant"]["mode"], "full");
    let conn = v["connection"]["id"].as_str().unwrap().to_owned();
    assert_eq!(v["grant"]["connection_id"], conn);
    // the connection was created for the person: connected_by the caller, audited with the way
    let (via, agent_id): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT metadata->>'via', metadata->>'agent_id' FROM audit_log WHERE action = 'connection.create' AND target_id = $1",
    )
    .bind(&conn)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (via.as_deref(), agent_id.as_deref()),
        (Some("agent_grant"), Some(w.agent.to_string().as_str()))
    );

    // the same agent again: the grant is replaced, no second connection
    let (s, v) = w.give(w.agent, "free_app").await;
    assert_eq!((s, v["created"].clone()), (200, json!(false)), "{v}");
    assert_eq!(v["connection"]["id"], conn);
    // another agent reuses the connection
    let other = Uuid::now_v7();
    w.insert_agent(other, "other").await;
    let (s, v) = w.give(other, "free_app").await;
    assert_eq!((s, v["created"].clone()), (200, json!(false)), "{v}");
    assert_eq!(v["connection"]["id"], conn);
    assert_eq!(w.connections("free_app").await, 1);

    // removing the grant leaves the connection
    let (s, _) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/agents/{}/grants/connections/{conn}", w.agent),
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_eq!(w.connections("free_app").await, 1);
}

#[tokio::test]
async fn parallel_calls_create_exactly_one_connection() {
    let w = world().await;
    let agents: Vec<Uuid> = (0..5).map(|_| Uuid::now_v7()).collect();
    for (i, a) in agents.iter().enumerate() {
        w.insert_agent(*a, &format!("p{i}")).await;
    }
    let results = futures::future::join_all(agents.iter().map(|a| w.give(*a, "free_app"))).await;
    let mut ids = Vec::new();
    let mut created = 0;
    for (s, v) in &results {
        assert_eq!(*s, 200, "{v}");
        ids.push(v["connection"]["id"].as_str().unwrap().to_owned());
        created += usize::from(v["created"] == true);
    }
    ids.dedup();
    assert_eq!((ids.len(), created), (1, 1), "{results:?}");
    assert_eq!(w.connections("free_app").await, 1);
}

#[tokio::test]
async fn refusals_are_returned_with_their_codes_before_any_write() {
    let w = world().await;
    // unknown app
    let (s, v) = w.give(w.agent, "nope").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (404, Some("app_not_found")),
        "{v}"
    );
    // an app that needs a sign-in
    let (s, v) = w.give(w.agent, "needs_key").await;
    assert_eq!(
        (
            s,
            v["error"]["code"].as_str(),
            v["error"]["details"]["app_id"].as_str()
        ),
        (422, Some("app_not_credential_free"), Some("needs_key")),
        "{v}"
    );
    // not available under a restricted posture
    let p = format!("/v1/orgs/{}/app-availability", w.org);
    let (s, v) = w
        .call(
            &w.owner,
            Method::PUT,
            &p,
            Some(json!({"posture": "restricted", "allow": []})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w.give(w.agent, "free_app").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("app_unavailable")),
        "{v}"
    );
    // a member cannot own the agent
    let (s, _) = w
        .call(
            &w.member,
            Method::PUT,
            &w.url(w.agent, "free_app"),
            Some(json!({"mode": "full"})),
        )
        .await;
    assert_eq!(s, 403);
    // an unknown agent
    let (s, _) = w.give(Uuid::now_v7(), "free_app").await;
    assert_eq!(s, 404);
    for app in ["free_app", "needs_key", "nope"] {
        assert_eq!(w.connections(app).await, 0, "{app}");
    }
}

#[tokio::test]
async fn a_custom_only_app_refuses_full_and_leaves_no_connection_behind() {
    let w = world().await;
    let (s, v) = w.give(w.agent, "free_custom").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("full_grant_forbidden")),
        "{v}"
    );
    // the whole transaction rolled back: no connection (and so no sealed credentials) remains
    assert_eq!(w.connections("free_custom").await, 0);
    // a custom grant works
    let (s, v) = w
        .call(
            &w.owner,
            Method::PUT,
            &w.url(w.agent, "free_custom"),
            Some(json!({"mode": "custom", "allow_tools": ["look"]})),
        )
        .await;
    assert_eq!((s, v["created"].clone()), (200, json!(true)), "{v}");
    assert_eq!(w.connections("free_custom").await, 1);
}
