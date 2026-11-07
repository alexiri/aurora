#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-002: granted apps' `agent.env` and `agent.stubs` reach the sandbox spec and boot document.
mod common;

use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use axum::body::Body;
use base64::Engine;
use common::*;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const YAML: &str = r#"package: { id: acme, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
apps:
  - about: { id: app_a, name: A, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    agent:
      env:
        - { name: APP_A_BASE, value: "https://a.example/api" }
      stubs:
        - { path: ".config/app-a/config.json", mode: "0600", content: '{"token":"aurora-managed"}' }
    tools:
      read:
        items:
          - { id: get_a, name: Get, host: a.example, path: /x, methods: [GET] }
    hosts:
      - match: { host: "a.example" }
        inject:
          - bearer: "{credentials.api_token}"
  - about: { id: app_b, name: B, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    agent:
      env:
        - { name: APP_B_BASE, value: "https://b.example/api" }
      stubs:
        - { path: ".config/app-b/config.json", content: "b" }
    tools:
      read:
        items:
          - { id: get_b, name: Get, host: b.example, path: /x, methods: [GET] }
    hosts:
      - match: { host: "b.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    owner: String,
    tok: String,
    rep: Uuid,
}

async fn world() -> W {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| {
        c.gateway_ca_dir = d;
        c.llm.endpoints.push(aurora_config::LlmEndpoint {
            id: "anthropic".into(),
            provider: "anthropic".into(),
            default_model: Some("claude-test".into()),
            ..Default::default()
        });
    })
    .await;
    let root = h.user("root@x.io", true).await;
    let (_, admin_tok) = h.good_session(root).await;
    let admin = cookie(&admin_tok);
    let hd = [("cookie", admin.as_str()), ("origin", ORIGIN)];
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/runners",
            &hd,
            Some(json!({"name": "r1"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let tok = v["token"].as_str().unwrap().to_owned();
    let key: SigningKey = generate_signing_key().unwrap();
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
    let (_, ot) = h.good_session(o).await;
    let mut w = W {
        h,
        org,
        ws,
        owner: cookie(&ot),
        tok,
        rep: Uuid::nil(),
    };
    let caps = json!({
        "backend": "fake", "home_durability": "resident",
        "resources": {"cpu": {"min": 250, "max": 4000}, "memory_mib": {"min": 256, "max": 8192},
                      "pids": {"min": 64, "max": 4096}, "home_gib": {"min": 1, "max": 100}},
        "max_sandboxes": 4, "harnesses": ["fake"], "max_attachment_bytes": 1048576
    });
    let (s, v) = w
        .runner(
            None,
            "/v1/runner/register",
            json!({"version": "0.1", "backend": "fake", "capabilities": caps, "advertised_host": "runner-a"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    w.rep = v["replica_id"].as_str().unwrap().parse().unwrap();
    w
}

impl W {
    async fn runner(&self, replica: Option<Uuid>, uri: &str, body: Value) -> (u16, Value) {
        let b = bearer(&self.tok);
        let rid = replica.map(|r| r.to_string());
        let mut hd = vec![("authorization", b.as_str())];
        if let Some(r) = &rid {
            hd.push(("x-aurora-replica", r.as_str()));
        }
        let (s, _, v) = self.h.send(Method::POST, uri, &hd, Some(body)).await;
        (s.as_u16(), v)
    }

    async fn call(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", &self.owner), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    async fn agent(&self, handle: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) VALUES ($1,$2,$3,$4,$4,'hosted','fake')")
            .bind(id).bind(self.org).bind(self.ws).bind(handle).execute(self.h.pool()).await.unwrap();
        let sec = Uuid::now_v7();
        sqlx::query("INSERT INTO secrets (id, org_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,'llm','anthropic','api_key','anthropic','api.anthropic.com','{}','managed','x')")
            .bind(sec).bind(self.org).bind(format!("k-{handle}")).execute(self.h.pool()).await.unwrap();
        sqlx::query(
            "INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(Uuid::now_v7())
        .bind(self.ws)
        .bind(id)
        .bind(sec)
        .execute(self.h.pool())
        .await
        .unwrap();
        id
    }

    /// An active workspace connection of `app` (both apps exist; nothing is granted yet).
    async fn conn(&self, app: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,$4,'acme','1.0.0','c','active','x')")
            .bind(id).bind(self.org).bind(self.ws).bind(app).execute(self.h.pool()).await.unwrap();
        id
    }

    async fn grant(&self, agent: Uuid, conn: Uuid) {
        let (s, v) = self
            .call(
                Method::PUT,
                &format!("/v1/agents/{agent}/grants/connections/{conn}"),
                Some(json!({"mode": "full"})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }

    async fn sandbox(&self, agent: Uuid) -> Value {
        let (s, v) = self
            .call(Method::GET, &format!("/v1/agents/{agent}/sandbox"), None)
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    /// Message the agent, fetch the `sandbox.start` item, report the sandbox running.
    async fn start(&self, agent: Uuid) -> (Value, Value) {
        let (s, v) = self
            .call(
                Method::POST,
                &format!("/v1/agents/{agent}/conversations/direct"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        let conv = v["id"].as_str().unwrap().to_owned();
        let (s, v) = self
            .call(
                Method::POST,
                &format!("/v1/conversations/{conv}/messages"),
                Some(json!({"text": "hello"})),
            )
            .await;
        assert_eq!(s, 202, "{v}");
        let (s, v) = self
            .runner(
                Some(self.rep),
                "/v1/runner/work",
                json!({"wait_secs": 0, "capacity": {"running": 0, "max": 4}}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        let start = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["kind"] == "sandbox.start")
            .expect("sandbox.start")
            .clone();
        let raw = start["spec"]["boot_secret"].as_str().unwrap();
        let doc: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(raw)
                .unwrap(),
        )
        .unwrap();
        let (s, v) = self
            .runner(
                Some(self.rep),
                "/v1/runner/reports",
                json!({"reports": [{
                    "report_id": Uuid::now_v7(), "sandbox_id": start["sandbox_id"],
                    "boot_id": start["boot_id"], "kind": "sandbox.status",
                    "state": "running", "container_ref": "c"}]}),
            )
            .await;
        assert_eq!(
            (s, v["results"][0]["status"].as_str()),
            (200, Some("applied")),
            "{v}"
        );
        (start, doc)
    }
}

fn env_of<'a>(start: &'a Value, k: &str) -> Option<&'a str> {
    start["spec"]["env"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p[0] == k)
        .and_then(|p| p[1].as_str())
}

#[tokio::test]
async fn granted_app_env_and_stubs_reach_the_boot_and_restart_to_update_follows_grants() {
    let w = world().await;
    let agent = w.agent("daneel").await;
    let (ca, cb) = (w.conn("app_a").await, w.conn("app_b").await);
    w.grant(agent, ca).await;

    let (start, doc) = w.start(agent).await;
    // env: handle + the granted app's entries; the ungranted app contributes nothing
    assert_eq!(env_of(&start, "AURORA_AGENT_HANDLE"), Some("daneel"));
    assert_eq!(env_of(&start, "APP_A_BASE"), Some("https://a.example/api"));
    assert_eq!(env_of(&start, "APP_B_BASE"), None);
    assert!(env_of(&start, "AURORA_BOOT").is_none());
    // files: the granted app's stub only, in the boot document
    let files = doc["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0]["path"], "/workspace/.config/app-a/config.json");
    assert_eq!(files[0]["mode"], "0600");
    assert_eq!(files[0]["only_if_absent_or_sentinel"], true);
    let content = base64::engine::general_purpose::STANDARD
        .decode(files[0]["content_b64"].as_str().unwrap())
        .unwrap();
    assert_eq!(content, br#"{"token":"aurora-managed"}"#);
    assert!(!doc.to_string().contains("b.example"));

    // unchanged grants: nothing to update
    assert_eq!(w.sandbox(agent).await["restart_to_update"], false);
    // granting the second app changes what a boot would get
    w.grant(agent, cb).await;
    assert_eq!(w.sandbox(agent).await["restart_to_update"], true);
}

impl W {
    /// Re-register the runner (same token) advertising `image`.
    async fn reregister(&mut self, image: &str) {
        let caps = json!({
            "backend": "fake", "home_durability": "resident", "image": image,
            "resources": {"cpu": {"min": 250, "max": 4000}, "memory_mib": {"min": 256, "max": 8192},
                          "pids": {"min": 64, "max": 4096}, "home_gib": {"min": 1, "max": 100}},
            "max_sandboxes": 4, "harnesses": ["fake"], "max_attachment_bytes": 1048576
        });
        let (s, v) = self
            .runner(
                None,
                "/v1/runner/register",
                json!({"version": "0.1", "backend": "fake", "capabilities": caps, "advertised_host": "runner-a"}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        self.rep = v["replica_id"].as_str().unwrap().parse().unwrap();
    }
}

#[tokio::test]
async fn a_new_runner_image_flags_restart_to_update() {
    let mut w = world().await;
    w.reregister("sandbox:A").await;
    let agent = w.agent("imgbot").await;
    let (start, _) = w.start(agent).await;
    assert_eq!(start["spec"]["image"], "sandbox:A");
    assert_eq!(w.sandbox(agent).await["restart_to_update"], false);
    // same image again: still nothing to update
    w.reregister("sandbox:A").await;
    assert_eq!(w.sandbox(agent).await["restart_to_update"], false);
    // image-only upgrade
    w.reregister("sandbox:B").await;
    let v = w.sandbox(agent).await;
    assert_eq!(v["image"], "sandbox:B", "{v}");
    assert_eq!(v["restart_to_update"], true);
}

#[tokio::test]
async fn an_unavailable_app_contributes_no_env_or_stubs_and_flags_restart() {
    let w = world().await;
    let agent = w.agent("avail").await;
    let ca = w.conn("app_a").await;
    w.grant(agent, ca).await;
    let (start, _) = w.start(agent).await;
    assert_eq!(env_of(&start, "APP_A_BASE"), Some("https://a.example/api"));
    assert_eq!(w.sandbox(agent).await["restart_to_update"], false);
    // The org restricts to app_b only: app_a's env/stubs drop out of what a boot would get.
    sqlx::query("UPDATE organizations SET app_posture = 'restricted' WHERE id = $1")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO org_app_availability (org_id, app_id, workspace_id) VALUES ($1, 'app_b', NULL)")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(w.sandbox(agent).await["restart_to_update"], true);
    // a fresh boot of another agent granted app_a gets nothing from it
    let other = w.agent("avail2").await;
    w.grant(other, ca).await;
    let (start2, doc2) = w.start(other).await;
    assert_eq!(env_of(&start2, "APP_A_BASE"), None);
    assert!(doc2["files"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_boot_recorded_without_an_image_flags_only_when_the_image_is_known_to_differ() {
    let mut w = world().await;
    w.reregister("sandbox:A").await;
    let agent = w.agent("legacy").await;
    w.start(agent).await;
    // A boot recorded before the image was stored.
    sqlx::query("UPDATE sandboxes SET resource_profile = resource_profile - 'boot_image' WHERE agent_id = $1")
        .bind(agent)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(w.sandbox(agent).await["restart_to_update"], false);
    w.reregister("sandbox:A").await;
    assert_eq!(w.sandbox(agent).await["restart_to_update"], false);
    w.reregister("sandbox:B").await;
    assert_eq!(w.sandbox(agent).await["restart_to_update"], true);
}
