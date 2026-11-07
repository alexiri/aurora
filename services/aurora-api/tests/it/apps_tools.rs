#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_api::domains::tools::{ToolCallContext, ToolContext, registry};
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

const YAML: &str = r#"package: { id: acme, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
apps:
  - about: { id: acme_app, name: Acme, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    agent: { default_blocklist: [ "blocked.example" ] }
    hosts:
      - match: { host: "api.acme.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    conv: Uuid,
    turn: Uuid,
}

/// Installs and approves the `acme` package (app `acme_app`), then creates one agent with a
/// running conversation and turn in a fresh workspace.
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
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
         VALUES ($1,$2,$3,'bot','Bot','hosted','fake')",
    )
    .bind(agent)
    .bind(org)
    .bind(ws)
    .execute(h.pool())
    .await
    .unwrap();
    let (conv, turn) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at)
         VALUES ($1,$2,$3,'api',$1::text, now())",
    )
    .bind(conv)
    .bind(agent)
    .bind(ws)
    .execute(h.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text)
         VALUES ($1,$2,$3,'turn','running','{}','system','s','hi')",
    )
    .bind(turn)
    .bind(conv)
    .bind(agent)
    .execute(h.pool())
    .await
    .unwrap();
    W {
        h,
        org,
        ws,
        agent,
        conv,
        turn,
    }
}

impl W {
    fn ctx(&self, agent: Uuid) -> ToolCallContext {
        ToolCallContext {
            agent: ToolContext {
                pool: self.h.pool().clone(),
                agent_id: agent,
                workspace_id: self.ws,
                org_id: self.org,
            },
            conversation_id: self.conv,
            turn_id: Some(self.turn),
            call_id: "c1".into(),
            state: self.h.state.clone(),
        }
    }

    /// Runs a platform tool through the registry; returns the serialized `ToolResult`.
    async fn run(&self, tool: &str, args: Value) -> Value {
        let r = registry()
            .call(&self.ctx(self.agent), tool, args, Duration::from_secs(10))
            .await;
        serde_json::to_value(&r).unwrap()
    }

    async fn notices(&self, code: &str) -> Vec<Value> {
        sqlx::query_scalar(
            "SELECT payload FROM transcript_events WHERE conversation_id = $1 AND type = 'notice' \
             AND payload->>'code' = $2 ORDER BY seq",
        )
        .bind(self.conv)
        .bind(code)
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }

    async fn proposals(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM secrets WHERE proposed_by_agent_id = $1 AND status = 'proposed'",
        )
        .bind(self.agent)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }
}

fn error_code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

fn error_message(v: &Value) -> &str {
    v["error"]["message"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn propose_creates_proposed_secret_without_value_and_notifies() {
    let w = world().await;
    let v = w
        .run(
            "propose_custom_connection",
            json!({"name": "Acme key", "host": "api.acme.example", "path": "/v1/*",
                   "recipe": {"kind": "bearer"}, "reason": "to list the projects"}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    let id: Uuid = v["content"][0]["value"]["secret_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(v["content"][0]["value"]["status"], "proposed");

    let (status, source, vref, ws, by, host, path): (
        String,
        String,
        String,
        Option<Uuid>,
        Option<Uuid>,
        String,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT status, value_source, value_ref, workspace_id, proposed_by_agent_id, host_pattern, path_pattern \
         FROM secrets WHERE id = $1",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(status, "proposed");
    assert_eq!(source, "managed");
    assert_eq!(vref, "pending:");
    assert_eq!(ws, Some(w.ws));
    assert_eq!(by, Some(w.agent));
    assert_eq!(host, "api.acme.example");
    assert_eq!(path.as_deref(), Some("/v1/*"));

    let n = w.notices("secret_proposed").await;
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0]["secret_id"], id.to_string());
    assert!(
        n[0]["text"]
            .as_str()
            .unwrap()
            .contains("never in this chat")
    );
    assert!(n[0].get("value").is_none());
}

#[tokio::test]
async fn propose_refuses_values_bad_recipes_and_governed_hosts() {
    let w = world().await;
    let base = json!({"name": "k", "host": "api.acme.example", "recipe": {"kind": "bearer"},
                      "reason": "r"});
    // A value is never accepted.
    let mut with_value = base.clone();
    with_value["value"] = json!("hunter2");
    let v = w.run("propose_custom_connection", with_value).await;
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(error_code(&v), "invalid_arguments");

    // Unknown recipe kind and an invalid header name.
    let mut bad = base.clone();
    bad["recipe"] = json!({"kind": "nope"});
    let v = w.run("propose_custom_connection", bad).await;
    assert_eq!(error_code(&v), "invalid_arguments", "{v}");
    let mut bad = base.clone();
    bad["recipe"] = json!({"kind": "header", "name": "bad name"});
    let v = w.run("propose_custom_connection", bad).await;
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(error_code(&v), "tool_failed");
    assert!(error_message(&v).contains("/injection"), "{v}");

    // Governed LLM host: create an LLM key instead.
    let mut llm = base.clone();
    llm["host"] = json!("api.openai.com");
    let v = w.run("propose_custom_connection", llm).await;
    assert_eq!(v["ok"], false, "{v}");
    assert!(error_message(&v).contains("LLM key"), "{v}");

    // Empty reason.
    let mut no_reason = base.clone();
    no_reason["reason"] = json!("   ");
    let v = w.run("propose_custom_connection", no_reason).await;
    assert_eq!(error_code(&v), "invalid_arguments", "{v}");

    assert_eq!(w.proposals().await, 0);
    assert!(w.notices("secret_proposed").await.is_empty());
}

#[tokio::test]
async fn sixth_pending_proposal_is_refused() {
    let w = world().await;
    for i in 1..=5 {
        let v = w
            .run(
                "propose_custom_connection",
                json!({"name": format!("key {i}"), "host": format!("h{i}.acme.example"),
                       "recipe": {"kind": "bearer"}, "reason": "r"}),
            )
            .await;
        assert_eq!(v["ok"], true, "{i}: {v}");
    }
    let v = w
        .run(
            "propose_custom_connection",
            json!({"name": "key 6", "host": "h6.acme.example", "recipe": {"kind": "bearer"},
                   "reason": "r"}),
        )
        .await;
    assert_eq!(v["ok"], false, "{v}");
    assert!(error_message(&v).contains("5 pending"), "{v}");
    assert_eq!(w.proposals().await, 5);
    assert_eq!(w.notices("secret_proposed").await.len(), 5);
}

#[tokio::test]
async fn accept_grants_only_the_proposing_agent() {
    let w = world().await;
    let other = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
         VALUES ($1,$2,$3,'other','Other','hosted','fake')",
    )
    .bind(other)
    .bind(w.org)
    .bind(w.ws)
    .execute(w.h.pool())
    .await
    .unwrap();
    let v = w
        .run(
            "propose_custom_connection",
            json!({"name": "Acme key", "host": "api.acme.example", "recipe": {"kind": "bearer"},
                   "reason": "r"}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    let id = v["content"][0]["value"]["secret_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let o = w.h.user("owner@x.io", false).await;
    w.h.member(w.org, o, "owner", "active").await;
    w.h.bind_ws(w.ws, o, "owner").await;
    let (_, tok) = w.h.good_session(o).await;
    let c = cookie(&tok);
    let (s, _, v) =
        w.h.send(
            Method::POST,
            &format!("/v1/secrets/{id}/accept"),
            &[("cookie", &c), ("origin", ORIGIN)],
            Some(json!({"value": "the-real-token"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "active");
    assert!(!v.to_string().contains("the-real-token"));

    let granted: Vec<Uuid> = sqlx::query_scalar("SELECT agent_id FROM grants WHERE secret_id = $1")
        .bind(id.parse::<Uuid>().unwrap())
        .fetch_all(w.h.pool())
        .await
        .unwrap();
    assert_eq!(
        granted,
        vec![w.agent],
        "grant must go to the proposing agent only"
    );
    assert!(!granted.contains(&other));
}

#[tokio::test]
async fn request_app_connection_notifies_with_link_and_refuses_unknown_or_unavailable() {
    let w = world().await;
    let (org_slug, ws_slug): (String, String) = (
        sqlx::query_scalar("SELECT slug FROM organizations WHERE id = $1")
            .bind(w.org)
            .fetch_one(w.h.pool())
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1")
            .bind(w.ws)
            .fetch_one(w.h.pool())
            .await
            .unwrap(),
    );

    let v = w
        .run(
            "request_app_connection",
            json!({"app_id": "acme_app", "reason": "list the projects"}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["content"][0]["value"]["status"], "requested");
    let n = w.notices("app_connection_requested").await;
    assert_eq!(n.len(), 1, "{n:?}");
    let expected = format!(
        "{}/o/{org_slug}/w/{ws_slug}/apps/acme_app/connect",
        w.h.state.config.external_url.trim_end_matches('/')
    );
    assert_eq!(n[0]["connect_url"], expected);
    assert_eq!(n[0]["app_id"], "acme_app");
    assert_eq!(n[0]["app_name"], "Acme");
    assert_eq!(n[0]["reason"], "list the projects");

    // Unknown app: refused, naming list_apps, no notice.
    let v = w
        .run(
            "request_app_connection",
            json!({"app_id": "nope_app", "reason": "r"}),
        )
        .await;
    assert_eq!(v["ok"], false, "{v}");
    assert!(error_message(&v).contains("list_apps"), "{v}");

    // Empty reason is refused.
    let v = w
        .run(
            "request_app_connection",
            json!({"app_id": "acme_app", "reason": ""}),
        )
        .await;
    assert_eq!(error_code(&v), "invalid_arguments", "{v}");

    // Restricted org posture without an allow entry: unavailable, naming list_apps.
    sqlx::query("UPDATE organizations SET app_posture = 'restricted' WHERE id = $1")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    let v = w
        .run(
            "request_app_connection",
            json!({"app_id": "acme_app", "reason": "r"}),
        )
        .await;
    assert_eq!(v["ok"], false, "{v}");
    assert!(error_message(&v).contains("not available"), "{v}");
    assert!(error_message(&v).contains("list_apps"), "{v}");

    // Allowed org-wide: accepted again.
    sqlx::query("INSERT INTO org_app_availability (org_id, app_id, workspace_id) VALUES ($1, 'acme_app', NULL)")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    let v = w
        .run(
            "request_app_connection",
            json!({"app_id": "acme_app", "reason": "r"}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(w.notices("app_connection_requested").await.len(), 2);
}
