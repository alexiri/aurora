//! (P5-UX-10, C-934) `GET /v1/agents/{agent}/apps/{app}/access`: what actually happens for each
//! tool of an app for one agent, with a reason only when something other than the agent's own
//! access decided; and its agreement with `GET /v1/agents/{agent}/effective`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::grants_connections::{W, world_with};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const YAML: &str = r#"package: { id: acme, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
apps:
  - about: { id: acme_app, name: Acme, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
      - id: other_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    tools:
      read:
        items:
          - { id: list_items, name: List, description: Lists things, host: api.acme.example, path: /items, methods: [GET] }
          - { id: get_item, name: Get, host: api.acme.example, path: "/items/*", methods: [GET] }
          - { id: other_only, name: Other, host: api.acme.example, path: /other, methods: [GET], connect_methods: [other_key] }
      write:
        items:
          - { id: create_item, name: Create, description: Makes a thing, host: api.acme.example, path: /items, methods: [POST] }
    hosts:
      - match: { host: "api.acme.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

async fn conn(w: &W, ws: Option<Uuid>, access: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, access, connect_method) VALUES ($1,$2,$3,'acme_app','acme','1.0.0','Main','active','x',$4,'api_key')")
        .bind(id).bind(w.org).bind(ws).bind(access).execute(w.h.pool()).await.unwrap();
    id
}

async fn give(w: &W, conn: Uuid, body: Value) {
    let u = format!("/v1/agents/{}/grants/connections/{conn}", w.agent);
    let (s, v) = w.call(&w.owner, Method::PUT, &u, Some(body)).await;
    assert_eq!(s, 200, "{v}");
}

async fn access(w: &W, who: &str) -> Value {
    let u = format!("/v1/agents/{}/apps/acme_app/access", w.agent);
    let (s, v) = w.call(who, Method::GET, &u, None).await;
    assert_eq!(s, 200, "{v}");
    v
}

fn tool<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["accounts"][0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool_id"] == id)
        .unwrap_or_else(|| panic!("no tool {id} in {v}"))
}

#[tokio::test]
async fn read_only_reads_allowed_writes_blocked_no_reason() {
    let w = world_with(YAML).await;
    let c = conn(&w, Some(w.ws), "read").await;
    give(&w, c, json!({"level": "read"})).await;
    let v = access(&w, &w.member).await;
    assert_eq!(
        v["accounts"][0]["ref"],
        json!({"type": "connection", "id": c})
    );
    assert_eq!(v["accounts"][0]["name"], "Main");
    let list = tool(&v, "list_items");
    assert_eq!(
        (list["decision"].as_str(), list["access"].as_str()),
        (Some("allow"), Some("read"))
    );
    assert_eq!(list["description"], "Lists things");
    let create = tool(&v, "create_item");
    assert_eq!(
        (create["decision"].as_str(), create["access"].as_str()),
        (Some("block"), Some("write"))
    );
    for t in v["accounts"][0]["tools"].as_array().unwrap() {
        if t["tool_id"] != "other_only" {
            assert!(t.get("reason").is_none(), "{t}");
        }
    }
}

#[tokio::test]
async fn ask_before_changes_writes_ask_no_reason() {
    let w = world_with(YAML).await;
    let c = conn(&w, Some(w.ws), "write").await;
    give(&w, c, json!({"level": "ask"})).await;
    let v = access(&w, &w.member).await;
    assert_eq!(tool(&v, "get_item")["decision"], "allow");
    let create = tool(&v, "create_item");
    assert_eq!(create["decision"], "ask");
    assert!(create.get("reason").is_none(), "{create}");
}

#[tokio::test]
async fn an_organizations_read_only_limit_blocks_writes_with_its_reason() {
    let w = world_with(YAML).await;
    let c = conn(&w, None, "write").await;
    give(&w, c, json!({"level": "ask"})).await;
    // The organization limits the account to Read only afterwards.
    sqlx::query("UPDATE app_connections SET access = 'read' WHERE id = $1")
        .bind(c)
        .execute(w.h.pool())
        .await
        .unwrap();
    let org_name: String = sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
        .bind(w.org)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let v = access(&w, &w.member).await;
    let create = tool(&v, "create_item");
    assert_eq!(create["decision"], "block", "{v}");
    assert_eq!(create["reason"]["layer"], "limit");
    assert_eq!(create["reason"]["label"], format!("Limited by {org_name}"));
    let get = tool(&v, "get_item");
    assert_eq!(get["decision"], "allow");
    assert!(get.get("reason").is_none(), "{get}");
}

#[tokio::test]
async fn an_org_rule_that_blocks_the_host_names_the_rule() {
    let w = world_with(YAML).await;
    let c = conn(&w, Some(w.ws), "write").await;
    give(&w, c, json!({"level": "act"})).await;
    let u = format!("/v1/orgs/{}/policy", w.org);
    let rules = json!([{"name": "No Acme at all", "action": "block",
        "targets": [{"type": "network", "host": "api.acme.example"}]}]);
    let (s, v) = w
        .call(
            &w.owner,
            Method::PUT,
            &format!("{u}/draft"),
            Some(json!({"revision": 0, "default_action": "none", "rules": rules})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &format!("{u}/publish"),
            Some(json!({"revision": 1})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let v = access(&w, &w.member).await;
    let get = tool(&v, "get_item");
    assert_eq!(get["decision"], "block", "{v}");
    assert_eq!(get["reason"]["layer"], "org_rule");
    assert_eq!(get["reason"]["label"], "No Acme at all");
    assert_eq!(get["reason"]["link"], "internet_access");
}

#[tokio::test]
async fn a_tool_the_app_does_not_offer_for_the_connect_method_is_package() {
    let w = world_with(YAML).await;
    let c = conn(&w, Some(w.ws), "write").await;
    give(&w, c, json!({"level": "act"})).await;
    let v = access(&w, &w.member).await;
    let t = tool(&v, "other_only");
    assert_eq!(t["decision"], "block", "{v}");
    assert_eq!(t["reason"]["layer"], "package");
    assert_eq!(tool(&v, "list_items")["decision"], "allow");
}

#[tokio::test]
async fn it_agrees_with_the_effective_view_for_every_tool() {
    let w = world_with(YAML).await;
    let c = conn(&w, Some(w.ws), "write").await;
    give(
        &w,
        c,
        json!({"mode": "custom", "allow_tools": ["get_item"], "ask_tools": ["create_item"]}),
    )
    .await;
    let v = access(&w, &w.member).await;
    let eu = format!("/v1/agents/{}/effective", w.agent);
    let (_, e) = w.call(&w.member, Method::GET, &eu, None).await;
    let mut n = 0;
    for et in e["tools"].as_array().unwrap() {
        let t = tool(&v, et["tool_id"].as_str().unwrap());
        let want = match et["decision"].as_str().unwrap() {
            "needs_approval" => "ask",
            other => other,
        };
        assert_eq!(t["decision"], want, "{et} vs {t}");
        n += 1;
    }
    assert_eq!(n, 4, "{e}");
}

#[tokio::test]
async fn an_agent_without_the_account_lists_nothing_and_an_unknown_app_is_404() {
    let w = world_with(YAML).await;
    let _ = conn(&w, Some(w.ws), "write").await;
    let v = access(&w, &w.member).await;
    assert!(v["accounts"].as_array().unwrap().is_empty());
    let u = format!("/v1/agents/{}/apps/nope/access", w.agent);
    assert_eq!(w.call(&w.member, Method::GET, &u, None).await.0, 404);
}
