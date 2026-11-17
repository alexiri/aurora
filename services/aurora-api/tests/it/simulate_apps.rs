//! FIX-159 (D-329, C-930; SEC-67, SEC-68, D-240): "Check what would happen" decides app requests
//! the way the gateway does. The owner's walkthrough: an AWS connection that is read-only, reads
//! allowed and writes set to ask; the simulator said **allow, no credential** for an S3 upload and
//! a bare STS POST because it never identified the app, so no connection rule ever applied.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use crate::plugins_lifecycle::{Admin, setup};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const S3: &str = "s3.eu-west-1.aws.test.example";
const STS: &str = "sts.eu-west-1.aws.test.example";

/// The shape of the builtin `aws-keys` app on test hosts: S3 reads and writes, STS only for
/// `GetCallerIdentity` (a `params` form tool), everything else untooled and blocked.
fn yaml() -> String {
    r#"package: { id: awst, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
apps:
  - about: { id: awst_app, name: AwsT }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: keys
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    grants: { block_untooled: true }
    tools:
      read:
        items:
          - { id: s3_list_buckets, name: List buckets, host: "s3.*.aws.test.example", path: /, methods: [GET] }
          - { id: s3_get_object, name: Get object, host: "s3.*.aws.test.example", path: "/*/*/**", methods: [GET, HEAD] }
          - id: sts_get_caller_identity
            name: Who am I
            host: "sts.*.aws.test.example"
            path: /
            methods: [POST]
            params: [ { source: form, name: Action, match: { equals: GetCallerIdentity } } ]
      write:
        items:
          - { id: s3_put_object, name: Upload, host: "s3.*.aws.test.example", path: "/*/*/**", methods: [PUT] }
          - { id: s3_delete_object, name: Delete, host: "s3.*.aws.test.example", path: "/*/*/**", methods: [DELETE] }
    hosts:
      - match: { host: "s3.*.aws.test.example" }
        inject:
          - bearer: "{credentials.api_token}"
      - match: { host: "sts.*.aws.test.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#
    .to_owned()
}

/// The `params` fixtures the package linter requires for the STS tool (the cases of the builtin
/// `aws-keys` package, on the test hosts).
fn fixtures() -> String {
    let form = "headers: { content-type: application/x-www-form-urlencoded }";
    let url = format!("https://{STS}/");
    let case = |name: &str, extra: &str, body: &str, expect: &str| {
        format!(
            "- name: \"{name}\"\n  connection: {{ credentials: {{ api_token: tok }}, metadata: {{ region: eu-west-1 }} }}\n  request: {{ method: POST, url: \"{url}{extra}\", {form}, body: \"{body}\" }}\n  expect: {expect}\n"
        )
    };
    let none = "{ app: awst_app, no_tool: true }";
    let mut s = case(
        "sts_get_caller_identity: positive",
        "",
        "Action=GetCallerIdentity&Version=2011-06-15",
        "{ app: awst_app, tool: sts_get_caller_identity }",
    );
    s += &case("negative: other value", "", "Action=AssumeRole", none);
    s += &case("negative: missing", "", "Version=2011-06-15", none);
    s += &case(
        "negative: repeated",
        "",
        "Action=GetCallerIdentity&Action=AssumeRole",
        none,
    );
    s += &case(
        "negative: query and body conflict",
        "?Action=AssumeRole",
        "Action=GetCallerIdentity",
        none,
    );
    s += &format!(
        "- name: \"negative: wrong content type\"\n  connection: {{ credentials: {{ api_token: tok }}, metadata: {{ region: eu-west-1 }} }}\n  request: {{ method: POST, url: \"{url}\", headers: {{ content-type: text/plain }}, body: \"Action=GetCallerIdentity\" }}\n  expect: {none}\n"
    );
    s += &format!(
        "- name: \"negative: truncated body\"\n  connection: {{ credentials: {{ api_token: tok }}, metadata: {{ region: eu-west-1 }} }}\n  request: {{ method: POST, url: \"{url}\", {form}, body: \"Action=GetCallerIdentity\", body_truncated: true }}\n  expect: {none}\n"
    );
    s
}

struct W<'a> {
    h: &'a Harness,
    org: Uuid,
    ws: Uuid,
    owner: String,
    agent: Uuid,
}

async fn world(h: &Harness) -> W<'_> {
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let o = h.user("owner@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    h.bind_ws(ws, o, "owner").await;
    let (_, ot) = h.good_session(o).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'daneel','Daneel','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    W {
        h,
        org,
        ws,
        owner: cookie(&ot),
        agent,
    }
}

impl W<'_> {
    async fn call(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", &self.owner), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    /// An org-level connection of the app with the given `access` (`read` | `write`).
    async fn conn(&self, access: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, access) VALUES ($1,$2,NULL,'awst_app','awst','1.0.0','aws prod','active','x',$3)")
            .bind(id).bind(self.org).bind(access).execute(self.h.pool()).await.unwrap();
        id
    }

    /// The agent's custom access: reads allowed, writes asked.
    async fn grant(&self, conn: Uuid) {
        let (s, v) = self
            .call(
                Method::PUT,
                &format!("/v1/agents/{}/grants/connections/{conn}", self.agent),
                Some(json!({
                    "mode": "custom",
                    "allow_tools": ["s3_list_buckets", "s3_get_object", "sts_get_caller_identity"],
                    "ask_tools": ["s3_put_object", "s3_delete_object"],
                })),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }

    async fn sim(&self, method: &str, url: &str, body: Option<&str>) -> Value {
        let (s, v) = self
            .call(
                Method::POST,
                &format!("/v1/workspaces/{}/policy/simulate", self.ws),
                Some(json!({"agent_id": self.agent, "method": method, "url": url, "body": body})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }
}

async fn install(a: &Admin<'_>) {
    let v = a
        .upload_with(&yaml(), &[("tests/params.yaml", fixtures().as_bytes())])
        .await;
    let v = if v["status"] == "pending_approval" {
        let (s, b) = a.act(&v["id"], "approve").await;
        assert_eq!(s, 200, "{b}");
        b
    } else {
        v
    };
    assert_eq!(v["status"], "active", "{v}");
}

/// One case of the table: what the gateway decides for it (`gateway` is the gateway's own verdict,
/// asserted by `services/aurora-gateway/tests/it/apps.rs` for the same classification and by the
/// shared-code unit tests of `aurora_registry::identify`).
struct Case {
    what: &'static str,
    access: &'static str,
    method: &'static str,
    url: String,
    body: Option<&'static str>,
    verdict: &'static str,
    tool: Option<&'static str>,
}

#[tokio::test]
async fn simulate_connection_custom_grant_matches_gateway() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install(&a).await;
    let w = world(&h).await;
    // (P5-UX-4, C-980) A grant above an organization account's limit can no longer be written, so
    // the state this case needs (writes asked on a read-limited account: data saved before the
    // limit, or a limit lowered later) is made the way it arises: grant first, lower the limit.
    let conn = w.conn("write").await;
    w.grant(conn).await;
    sqlx::query("UPDATE app_connections SET access = 'read' WHERE id = $1")
        .bind(conn)
        .execute(h.pool())
        .await
        .unwrap();

    let cases = [
        Case {
            what: "an upload on a read-only connection is blocked, even though the grant asks",
            access: "read",
            method: "PUT",
            url: format!("https://{S3}/bucket/key.txt"),
            body: None,
            verdict: "block",
            tool: Some("s3_put_object"),
        },
        Case {
            what: "list buckets is a read the grant allows",
            access: "read",
            method: "GET",
            url: format!("https://{S3}/"),
            body: None,
            verdict: "allow",
            tool: Some("s3_list_buckets"),
        },
        Case {
            what: "a bare STS POST is untooled and blocked",
            access: "read",
            method: "POST",
            url: format!("https://{STS}/"),
            body: None,
            verdict: "block",
            tool: None,
        },
        Case {
            what: "GetCallerIdentity is the one STS action that is catalogued",
            access: "read",
            method: "POST",
            url: format!("https://{STS}/"),
            body: Some("Action=GetCallerIdentity&Version=2011-06-15"),
            verdict: "allow",
            tool: Some("sts_get_caller_identity"),
        },
        Case {
            what: "any other STS action is untooled and blocked",
            access: "read",
            method: "POST",
            url: format!("https://{STS}/"),
            body: Some("Action=AssumeRole"),
            verdict: "block",
            tool: None,
        },
    ];
    for c in &cases {
        let v = w.sim(c.method, &c.url, c.body).await;
        assert_eq!(
            v["verdict"], c.verdict,
            "{} (access {}): {v}",
            c.what, c.access
        );
        assert_eq!(
            v["winning_credential"]["id"],
            conn.to_string(),
            "{}: winner {v}",
            c.what
        );
        assert_eq!(v["winning_credential"]["kind"], "connection", "{}", c.what);
        assert_eq!(v["app"]["app_id"], "awst_app", "{}", c.what);
        assert_eq!(
            v["app"]["tool_id"],
            c.tool.map_or(Value::Null, Value::from),
            "{}: {v}",
            c.what
        );
        assert_eq!(v["candidates"][0]["kind"], "connection", "{}", c.what);
    }
    // The deciding check is in the trace, not hidden behind "no rule".
    let v = w
        .sim("PUT", &format!("https://{S3}/bucket/key.txt"), None)
        .await;
    let ws_trace = v["trace"]["workspace"]["considered"].to_string();
    assert!(ws_trace.contains("read_only_connection"), "{v}");
    let v = w.sim("POST", &format!("https://{STS}/"), None).await;
    assert!(
        v["trace"]["workspace"]["considered"]
            .to_string()
            .contains("block_untooled"),
        "{v}"
    );

    // The same connection switched to write: a delete is asked, a put is asked.
    sqlx::query("UPDATE app_connections SET access = 'write' WHERE id = $1")
        .bind(conn)
        .execute(h.pool())
        .await
        .unwrap();
    let v = w
        .sim("DELETE", &format!("https://{S3}/bucket/key.txt"), None)
        .await;
    assert_eq!(v["verdict"], "needs_approval", "{v}");
    assert_eq!(v["app"]["tool_id"], "s3_delete_object", "{v}");
    let v = w.sim("GET", &format!("https://{S3}/"), None).await;
    assert_eq!(v["verdict"], "allow", "{v}");

    // Plain HTTP is never injected (SEC-04): no connection is a candidate, the request is
    // uncredentialed.
    let v = w
        .sim("PUT", &format!("http://{S3}/bucket/key.txt"), None)
        .await;
    assert_eq!(v["winning_credential"], Value::Null, "{v}");
    assert_eq!(v["candidates"].as_array().unwrap().len(), 0, "{v}");
}

#[tokio::test]
async fn simulate_without_an_app_is_unchanged() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install(&a).await;
    let w = world(&h).await;
    let v = w.sim("GET", "https://unrelated.example/x", None).await;
    assert_eq!(v["verdict"], "allow", "{v}");
    assert_eq!(v["app"], Value::Null, "{v}");
    assert_eq!(v["winning_credential"], Value::Null, "{v}");
    let _ = w.org;
}
