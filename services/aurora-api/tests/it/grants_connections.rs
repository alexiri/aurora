#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_registry::builder::PackageBuilder;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::postgres::PgListener;
use tower::ServiceExt;
use uuid::Uuid;

const YAML: &str = r#"package: { id: acme, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
permissions:
  token_hosts: [accounts.example.com]
oauth_clients:
  - id: goog
    authorize_url: https://accounts.example.com/auth
    token_url: https://accounts.example.com/token
    fields:
      - { name: client_id, label: Client ID }
      - { name: client_secret, label: Secret, secret: true }
    setup_note: Create an OAuth app first.
apps:
  - about: { id: acme_app, name: Acme, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
      - id: oauth
        kind: oauth2
        client: goog
    agent: { default_blocklist: [ "blocked.example" ] }
    tools:
      read:
        items:
          - { id: list_items, name: List, host: api.acme.example, path: /items, methods: [GET] }
          - { id: get_item, name: Get, host: api.acme.example, path: "/items/*", methods: [GET] }
      write:
        items:
          - { id: create_item, name: Create, host: api.acme.example, path: /items, methods: [POST] }
    hosts:
      - match: { host: "api.acme.example" }
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
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    W {
        agent,
        h,
        org,
        ws,
        owner: cookie(&ot),
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

    async fn conn(&self, ws: Option<Uuid>) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'acme_app','acme','1.0.0','c','active','x')")
            .bind(id).bind(self.org).bind(ws).execute(self.h.pool()).await.unwrap();
        id
    }
}

impl W {
    fn gurl(&self, conn: Uuid) -> String {
        format!("/v1/agents/{}/grants/connections/{conn}", self.agent)
    }
    async fn rules(&self, grant: Uuid) -> Vec<Value> {
        sqlx::query_scalar::<_, Value>(
            "SELECT rule FROM derived_rules WHERE source_kind='grant' AND source_id=$1 ORDER BY ordinal",
        )
        .bind(grant)
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }
    async fn publish_org_block(&self, tool: &str) {
        let u = format!("/v1/orgs/{}/policy", self.org);
        let rules = json!([{"name": "no create", "action": "block",
            "targets": [{"type": "app", "app_id": "acme_app", "tools": [tool]}]}]);
        let (s, v) = self
            .call(
                &self.owner,
                Method::PUT,
                &format!("{u}/draft"),
                Some(json!({"revision": 0, "default_action": "none", "rules": rules})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        let (s, v) = self
            .call(
                &self.owner,
                Method::POST,
                &format!("{u}/publish"),
                Some(json!({"revision": 1})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }
}

fn reason(v: &Value) -> Option<&str> {
    v["error"]["details"]["reason"].as_str()
}

#[tokio::test]
async fn full_and_custom_grants_compile_exactly_and_validate() {
    let w = world().await;
    let conn = w.conn(Some(w.ws)).await;
    let org_conn = w.conn(None).await;
    let u = w.gurl(conn);
    // can_own required
    let body = json!({"mode": "full"});
    assert_eq!(
        w.call(&w.member, Method::PUT, &u, Some(body.clone()))
            .await
            .0,
        403
    );

    let mut l = PgListener::connect_with(w.h.pool()).await.unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();
    let (s, v) = w.call(&w.owner, Method::PUT, &u, Some(body)).await;
    assert_eq!((s, v["mode"].as_str()), (200, Some("full")), "{v}");
    let p = l.recv().await.unwrap();
    assert!(p.payload().contains("agent") && p.payload().contains(&w.agent.to_string()));
    let gid: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let rules = w.rules(gid).await;
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["action"], "allow");
    assert_eq!(rules[0]["require_approval"], false);
    assert_eq!(
        rules[0]["targets"],
        json!([{"type": "connection", "id": conn}])
    );
    assert_eq!(
        rules[0]["identities"],
        json!([{"kind": "agent", "id": w.agent}])
    );

    // validation
    for (body, want) in [
        (json!({"mode": "custom"}), "empty_custom"),
        (
            json!({"mode": "custom", "allow_tools": ["get_item"], "ask_tools": ["get_item"]}),
            "tool_in_both",
        ),
        (
            json!({"mode": "custom", "allow_tools": ["nope"]}),
            "unknown_tool",
        ),
    ] {
        let (s, v) = w.call(&w.owner, Method::PUT, &u, Some(body)).await;
        assert_eq!((s, reason(&v)), (422, Some(want)), "{v}");
        assert_eq!(v["error"]["code"], "grant_invalid");
    }
    assert_eq!(
        w.rules(gid).await.len(),
        1,
        "failed writes leave rules intact"
    );

    // custom replaces the full grant: allow, ask, block-rest
    let body = json!({"mode": "custom", "allow_tools": ["get_item", "get_item"], "ask_tools": ["create_item"]});
    let (s, v) = w.call(&w.owner, Method::PUT, &u, Some(body)).await;
    assert_eq!(
        (s, v["id"].as_str()),
        (200, Some(gid.to_string().as_str())),
        "{v}"
    );
    assert_eq!(v["allow_tools"], json!(["get_item"]));
    let r = w.rules(gid).await;
    assert_eq!(r.len(), 3);
    assert_eq!(
        r[0]["targets"],
        json!([{"type": "connection", "id": conn, "tools": ["get_item"]}])
    );
    assert_eq!(
        (r[0]["action"].as_str(), r[0]["require_approval"].as_bool()),
        (Some("allow"), Some(false))
    );
    assert_eq!(
        r[1]["targets"],
        json!([{"type": "connection", "id": conn, "tools": ["create_item"]}])
    );
    assert_eq!(
        (r[1]["action"].as_str(), r[1]["require_approval"].as_bool()),
        (Some("allow"), Some(true))
    );
    assert_eq!(r[2]["targets"], json!([{"type": "connection", "id": conn}]));
    assert_eq!(r[2]["action"], "block");

    // (RFE-011a) ask_timeout_secs rides on the derived ask rule, bounded like a rule's timeout.
    for bad in [30, 99_999] {
        let body = json!({"mode": "custom", "ask_tools": ["create_item"], "ask_timeout_secs": bad});
        let (s, v) = w.call(&w.owner, Method::PUT, &u, Some(body)).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("approval_timeout_out_of_range")),
            "{v}"
        );
    }
    let body = json!({"mode": "custom", "allow_tools": ["get_item"], "ask_timeout_secs": 600});
    let (s, v) = w.call(&w.owner, Method::PUT, &u, Some(body)).await;
    assert_eq!(
        (s, reason(&v)),
        (422, Some("ask_timeout_without_ask_tools")),
        "{v}"
    );
    let body = json!({"mode": "custom", "allow_tools": ["get_item"], "ask_tools": ["create_item"], "ask_timeout_secs": 900});
    let (s, v) = w.call(&w.owner, Method::PUT, &u, Some(body)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ask_timeout_secs"], 900);
    let r = w.rules(gid).await;
    assert_eq!(r[1]["approval_timeout_secs"], 900);
    assert!(
        r[0].get("approval_timeout_secs")
            .is_none_or(|x| x.is_null())
    );
    let body = json!({"mode": "custom", "allow_tools": ["get_item"], "ask_tools": ["create_item"]});
    let (_, v) = w.call(&w.owner, Method::PUT, &u, Some(body)).await;
    assert!(
        v["ask_timeout_secs"].is_null(),
        "replacing the grant clears the timeout"
    );

    // ask-only custom: no allow rule
    let (s, _) = w
        .call(
            &w.owner,
            Method::PUT,
            &u,
            Some(json!({"mode": "custom", "ask_tools": ["create_item"]})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(w.rules(gid).await.len(), 2);
    // back to full removes stale ordinals
    w.call(&w.owner, Method::PUT, &u, Some(json!({"mode": "full"})))
        .await;
    assert_eq!(w.rules(gid).await.len(), 1);

    // listings are `can_own` (I-27): a plain workspace member is denied, the owner reads
    for uri in [
        format!("/v1/agents/{}/grants", w.agent),
        format!("/v1/connections/{conn}/grants"),
    ] {
        let (s, v) = w.call(&w.member, Method::GET, &uri, None).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (403, Some("forbidden")),
            "{uri}: {v}"
        );
    }
    let (s, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/agents/{}/grants", w.agent),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connections"][0]["connection_id"], json!(conn));
    let (s, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/connections/{conn}/grants"),
            None,
        )
        .await;
    assert_eq!((s, v["items"].as_array().map(Vec::len)), (200, Some(1)));

    // org connection grantable; rule lives in the agent's workspace scope
    let (s, v) = w
        .call(
            &w.owner,
            Method::PUT,
            &w.gurl(org_conn),
            Some(json!({"mode": "full"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let scope: Option<Uuid> =
        sqlx::query_scalar("SELECT workspace_id FROM derived_rules WHERE source_id=$1")
            .bind(v["id"].as_str().unwrap().parse::<Uuid>().unwrap())
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(scope, Some(w.ws));
    // connection of another workspace is invisible
    let ws2 = w.h.workspace(w.org).await;
    let other = w.conn(Some(ws2)).await;
    assert_eq!(
        w.call(
            &w.owner,
            Method::PUT,
            &w.gurl(other),
            Some(json!({"mode": "full"}))
        )
        .await
        .0,
        404
    );

    // revoke
    assert_eq!(w.call(&w.owner, Method::DELETE, &u, None).await.0, 204);
    assert_eq!(w.call(&w.owner, Method::DELETE, &u, None).await.0, 404);
    assert!(w.rules(gid).await.is_empty());
}

fn decision<'a>(v: &'a Value, tool: &str) -> &'a Value {
    v["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool_id"] == tool)
        .unwrap_or(&Value::Null)
}

#[tokio::test]
async fn effective_view_decisions_ceiling_and_inert_tools() {
    let w = world().await;
    let conn = w.conn(Some(w.ws)).await;
    let eu = format!("/v1/agents/{}/effective", w.agent);
    let (_, v) = w.call(&w.member, Method::GET, &eu, None).await;
    assert_eq!(v["credentials"][0]["kind"], "connection");
    assert_eq!(v["credentials"][0]["selected"], false);
    assert!(v["tools"].as_array().unwrap().is_empty());

    let body = json!({"mode": "custom", "allow_tools": ["get_item"], "ask_tools": ["create_item"]});
    let (s, v) = w
        .call(&w.owner, Method::PUT, &w.gurl(conn), Some(body))
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = w.call(&w.member, Method::GET, &eu, None).await;
    assert_eq!(v["credentials"][0]["selected"], true);
    assert_eq!(decision(&v, "get_item")["decision"], "allow");
    assert_eq!(decision(&v, "create_item")["decision"], "needs_approval");
    // a catalogue tool in neither list arrives blocked
    assert_eq!(decision(&v, "list_items")["decision"], "block");
    assert_eq!(decision(&v, "get_item")["org_ceiling"], Value::Null);

    // a tool removed by a package upgrade stays in the list, inert
    sqlx::query("UPDATE grants SET allow_tools = array_append(allow_tools, 'removed_tool') WHERE connection_id=$1")
        .bind(conn).execute(w.h.pool()).await.unwrap();
    let (_, v) = w.call(&w.member, Method::GET, &eu, None).await;
    let t = decision(&v, "removed_tool");
    assert_eq!(
        (t["inert"].as_bool(), t["decision"].as_str()),
        (Some(true), Some("inert"))
    );

    // an org rule blocking a tool shows as the ceiling
    w.publish_org_block("create_item").await;
    let (_, v) = w.call(&w.member, Method::GET, &eu, None).await;
    let t = decision(&v, "create_item");
    assert_eq!(
        (t["decision"].as_str(), t["org_ceiling"].as_str()),
        (Some("block"), Some("block")),
        "{v}"
    );
    assert_eq!(decision(&v, "get_item")["decision"], "allow");
}

#[tokio::test]
async fn app_detail_lists_oauth_clients_for_byoc_forms() {
    let w = world().await;
    let (s, v) = w
        .call(&w.member, Method::GET, "/v1/apps/acme_app", None)
        .await;
    assert_eq!(s, 200, "{v}");
    let c = &v["oauth_clients"][0];
    assert_eq!(c["client_key"], "goog");
    assert_eq!(c["setup_note"], "Create an OAuth app first.");
    assert_eq!(
        c["fields"][1],
        json!({"name": "client_secret", "label": "Secret", "secret": true})
    );
    assert!(
        c["redirect_uri"]
            .as_str()
            .unwrap()
            .ends_with("/v1/oauth/callback")
    );
}
