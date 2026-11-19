//! P5-UX-4 (D-344, D-345, C-917, C-980, SEC-131): the accounts lists and the access choice.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
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
    tools:
      read:
        items:
          - { id: list_items, name: List items, host: api.acme.example, path: /items, methods: [GET] }
          - { id: get_item, name: Get an item, host: api.acme.example, path: "/items/*", methods: [GET] }
      write:
        items:
          - { id: create_item, name: Create an item, host: api.acme.example, path: /items, methods: [POST] }
          - { id: delete_item, name: Delete an item, host: api.acme.example, path: "/items/*", methods: [DELETE] }
    hosts:
      - match: { host: "api.acme.example" }
        inject:
          - bearer: "{credentials.api_token}"
  - about: { id: lock_app, name: Lock, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    grants: { custom_only: true, block_untooled: true }
    tools:
      read:
        items:
          - { id: look, name: Look, host: api.lock.example, path: /x, methods: [GET] }
      write:
        items:
          - { id: change, name: Change, host: api.lock.example, path: /x, methods: [POST] }
    hosts:
      - match: { host: "api.lock.example" }
        inject:
          - bearer: "{credentials.api_token}"
  - about: { id: read_app, name: Reader, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    tools:
      read:
        items:
          - { id: peek, name: Peek, host: api.read.example, path: /x, methods: [GET] }
    hosts:
      - match: { host: "api.read.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

async fn world() -> W {
    world_with(YAML).await
}

/// A connection of `app` in the workspace (`ws = Some`) or the organization, with `access`.
async fn conn(w: &W, app: &str, ws: Option<Uuid>, access: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, access) VALUES ($1,$2,$3,$4,'acme','1.0.0',$5,'active','x',$6)")
        .bind(id).bind(w.org).bind(ws).bind(app).bind(format!("{app} account")).bind(access)
        .execute(w.h.pool()).await.unwrap();
    id
}

async fn secret(w: &W, ws: Option<Uuid>, name: &str, kind: &str) -> Uuid {
    let id = Uuid::now_v7();
    let (prov, mode, ep) = if kind == "llm" {
        (Some("anthropic"), Some("api_key"), Some("anthropic"))
    } else {
        (None, None, None)
    };
    sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'api.example.com','{}','managed','x')")
        .bind(id).bind(w.org).bind(ws).bind(name).bind(kind).bind(prov).bind(mode).bind(ep)
        .execute(w.h.pool()).await.unwrap();
    id
}

async fn second_agent(w: &W) -> Uuid {
    w.h.byo_agent(w.org, w.ws, "bot2", "BotTwo").await
}

fn put_url(agent: Uuid, conn: Uuid) -> String {
    format!("/v1/agents/{agent}/grants/connections/{conn}")
}

async fn put(w: &W, who: &str, agent: Uuid, conn: Uuid, body: Value) -> (u16, Value) {
    w.call(who, Method::PUT, &put_url(agent, conn), Some(body))
        .await
}

async fn list(w: &W, who: &str, query: &str) -> Value {
    let (s, v) = w
        .call(
            who,
            Method::GET,
            &format!("/v1/workspaces/{}/accounts{query}", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    v
}

fn find(v: &Value, id: Uuid) -> &Value {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["ref"]["id"] == id.to_string())
        .unwrap_or_else(|| panic!("account {id} not in {v}"))
}

async fn access_of(w: &W, conn: Uuid) -> String {
    sqlx::query_scalar("SELECT access FROM app_connections WHERE id = $1")
        .bind(conn)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn the_list_has_an_app_a_website_an_ai_key_and_the_organizations_account() {
    let w = world().await;
    let app = conn(&w, "acme_app", Some(w.ws), "write").await;
    let org_app = conn(&w, "acme_app", None, "write").await;
    let site = secret(&w, Some(w.ws), "Docs site", "generic").await;
    let key = secret(&w, Some(w.ws), "Anthropic key", "llm").await;
    assert_eq!(
        put(&w, &w.owner, w.agent, app, json!({"level": "ask"}))
            .await
            .0,
        200
    );
    let (s, _, _) =
        w.h.send(
            Method::PUT,
            &format!("/v1/agents/{}/grants/secrets/{site}", w.agent),
            &[("cookie", w.owner.as_str()), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s, 200);

    let v = list(&w, &w.owner, "").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 4, "{v}");
    let a = find(&v, app);
    assert_eq!(
        (a["kind"].as_str(), a["scope"].as_str()),
        (Some("app"), Some("workspace"))
    );
    assert_eq!(a["app"]["name"], "Acme");
    assert_eq!(a["levels"], json!(["read", "ask", "act"]));
    assert_eq!(a["agents"][0]["level"], "ask");
    assert_eq!(a["agents"][0]["agent"]["handle"], "bot");
    assert_eq!(
        a["can"],
        json!({"give_access": true, "change_access": true, "manage": true})
    );
    assert!(
        a.get("limit").is_none(),
        "no limit on a workspace account: {a}"
    );
    assert!(a.get("managed_by").is_none());
    let s = find(&v, site);
    assert_eq!(s["kind"], "website");
    assert_eq!(
        (s["levels"].clone(), s["agents"][0]["level"].clone()),
        (json!([]), json!("use"))
    );
    assert_eq!(find(&v, key)["kind"], "ai_key");
    assert_eq!(find(&v, key)["agents"], json!([]));
    let o = find(&v, org_app);
    assert_eq!(o["scope"], "organization");
    assert_eq!(o["limit"], "act");
    assert_eq!(o["managed_by"]["role"], "org_admin");
    // Filters.
    let only = list(&w, &w.owner, "?kind=website").await;
    assert_eq!(only["items"].as_array().unwrap().len(), 1);
    let (s, _) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/workspaces/{}/accounts?kind=nope", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 400);

    // A member sees the list and may change nothing.
    let m = list(&w, &w.member, "").await;
    assert_eq!(
        find(&m, app)["can"],
        json!({"give_access": false, "change_access": false, "manage": false})
    );
    assert_eq!(find(&m, org_app)["can"]["manage"], false);
}

#[tokio::test]
async fn a_read_only_app_has_no_choice_and_levels_follow_custom_only() {
    let w = world().await;
    let reader = conn(&w, "read_app", Some(w.ws), "read").await;
    let lock = conn(&w, "lock_app", Some(w.ws), "write").await;
    assert_eq!(
        put(&w, &w.owner, w.agent, reader, json!({"level": "read"}))
            .await
            .0,
        200
    );
    let v = list(&w, &w.owner, "").await;
    let r = find(&v, reader);
    assert_eq!(
        (r["levels"].clone(), r["agents"][0]["level"].clone()),
        (json!([]), json!("use"))
    );
    let l = find(&v, lock);
    assert_eq!(l["levels"], json!(["read", "ask"]));
    assert_eq!(l["app"]["custom_only"], true);
}

#[tokio::test]
async fn two_agents_use_one_account_with_different_access_and_no_limit() {
    let w = world().await;
    let bot2 = second_agent(&w).await;
    let c = conn(&w, "acme_app", Some(w.ws), "read").await;
    assert_eq!(
        put(&w, &w.owner, w.agent, c, json!({"level": "read"}))
            .await
            .0,
        200
    );
    assert_eq!(access_of(&w, c).await, "read");
    assert_eq!(
        put(&w, &w.owner, bot2, c, json!({"level": "ask"})).await.0,
        200
    );
    assert_eq!(access_of(&w, c).await, "write", "one agent needs writes");
    let v = list(&w, &w.owner, "").await;
    let a = find(&v, c);
    assert!(a.get("limit").is_none(), "{a}");
    let levels: Vec<(&str, &str)> = a["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x["agent"]["handle"].as_str().unwrap(),
                x["level"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(levels, vec![("bot", "read"), ("bot2", "ask")]);
    // Removing the second agent's access recomputes the connection.
    let (s, _, _) =
        w.h.send(
            Method::DELETE,
            &put_url(bot2, c),
            &[("cookie", w.owner.as_str()), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_eq!(access_of(&w, c).await, "read");
    // The agent-filtered list: given first.
    let other = conn(&w, "acme_app", Some(w.ws), "write").await;
    let f = list(&w, &w.owner, &format!("?agent_id={bot2}")).await;
    let ids: Vec<&str> = f["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["ref"]["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&other.to_string().as_str()));
    assert!(
        f["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["given"] == false),
        "{f}"
    );
    let f = list(&w, &w.owner, &format!("?agent_id={}", w.agent)).await;
    assert_eq!(
        f["items"][0]["ref"]["id"],
        c.to_string(),
        "the given account comes first: {f}"
    );
    assert_eq!(f["items"][0]["given"], true);
}

#[tokio::test]
async fn an_ask_choice_with_one_allowed_change_reads_as_ask_with_always_allowed() {
    let w = world().await;
    let c = conn(&w, "acme_app", Some(w.ws), "write").await;
    let all_reads = ["list_items", "get_item"];
    let (s, v) = put(
        &w, &w.owner, w.agent, c,
        json!({"mode": "custom", "allow_tools": [all_reads[0], all_reads[1], "delete_item"], "ask_tools": ["create_item"]}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let a = list(&w, &w.owner, "").await;
    let x = &find(&a, c)["agents"][0];
    assert_eq!(x["level"], "ask", "{x}");
    assert_eq!(
        x["always_allowed"],
        json!([{"tool_id": "delete_item", "name": "Delete an item"}])
    );
    // Choosing a level again clears the always-allowed changes.
    assert_eq!(
        put(&w, &w.owner, w.agent, c, json!({"level": "ask"}))
            .await
            .0,
        200
    );
    let a = list(&w, &w.owner, "").await;
    let x = &find(&a, c)["agents"][0];
    assert_eq!(x["level"], "ask");
    assert!(x.get("always_allowed").is_none(), "{x}");
    // A blocked read is custom, with its nearest level.
    let (s, v) = put(
        &w, &w.owner, w.agent, c,
        json!({"mode": "custom", "allow_tools": ["list_items", "delete_item"], "ask_tools": ["create_item"]}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let a = list(&w, &w.owner, "").await;
    let x = &find(&a, c)["agents"][0];
    assert_eq!(x["level"], "custom", "{x}");
    // The blocked read and the change that is allowed instead of asked.
    assert_eq!(x["custom"], json!({"base": "ask", "differs": 2}));
}

#[tokio::test]
async fn a_level_is_expanded_by_the_server_and_a_bad_body_is_refused() {
    let w = world().await;
    let c = conn(&w, "acme_app", Some(w.ws), "write").await;
    let (s, v) = put(&w, &w.owner, w.agent, c, json!({"level": "read"})).await;
    assert_eq!((s, v["mode"].as_str()), (200, Some("custom")), "{v}");
    assert_eq!(v["allow_tools"], json!(["list_items", "get_item"]));
    assert_eq!(v["ask_tools"], json!([]));
    let (s, v) = put(&w, &w.owner, w.agent, c, json!({"level": "act"})).await;
    assert_eq!((s, v["mode"].as_str()), (200, Some("full")), "{v}");
    for body in [
        json!({}),
        json!({"level": "ask", "mode": "full"}),
        json!({"level": "ask", "allow_tools": ["get_item"]}),
    ] {
        let (s, v) = put(&w, &w.owner, w.agent, c, body).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("grant_invalid")),
            "{v}"
        );
    }
    let (s, v) = put(
        &w,
        &w.owner,
        w.agent,
        c,
        json!({"level": "ask", "extra": 1}),
    )
    .await;
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn act_on_a_custom_only_app_is_refused_and_a_non_owner_is_forbidden() {
    let w = world().await;
    let lock = conn(&w, "lock_app", Some(w.ws), "write").await;
    let (s, v) = put(&w, &w.owner, w.agent, lock, json!({"level": "act"})).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("full_grant_forbidden")),
        "{v}"
    );
    assert_eq!(
        put(&w, &w.owner, w.agent, lock, json!({"level": "ask"}))
            .await
            .0,
        200
    );
    let c = conn(&w, "acme_app", Some(w.ws), "write").await;
    let (s, v) = put(&w, &w.member, w.agent, c, json!({"level": "read"})).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("forbidden")),
        "{v}"
    );
    assert_eq!(
        access_of(&w, c).await,
        "write",
        "a refused change changes nothing"
    );
    // Organization accounts list: owners and admins only.
    let (s, _) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/orgs/{}/accounts", w.org),
            None,
        )
        .await;
    assert_eq!(s, 403);
}

#[tokio::test]
async fn an_organizations_limit_cuts_the_choices_and_refuses_what_is_above_it() {
    let w = world().await;
    let org_read = conn(&w, "acme_app", None, "read").await;
    // Data from before the limit: a saved full grant on a read-limited account reads as `read`.
    let g = Uuid::now_v7();
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode) VALUES ($1,$2,$3,$4,'full')")
        .bind(g).bind(w.ws).bind(w.agent).bind(org_read)
        .execute(w.h.pool()).await.unwrap();
    let v = list(&w, &w.owner, "").await;
    let a = find(&v, org_read);
    assert_eq!(a["limit"], "read");
    assert_eq!(a["levels"], json!(["read"]));
    assert_eq!(
        a["agents"][0]["level"], "read",
        "above the limit reads as the limit: {a}"
    );
    assert!(a["managed_by"]["people"].as_array().is_some());

    for body in [
        json!({"level": "ask"}),
        json!({"level": "act"}),
        json!({"mode": "custom", "allow_tools": ["list_items", "create_item"]}),
        json!({"mode": "custom", "allow_tools": ["list_items"], "ask_tools": ["delete_item"]}),
        json!({"mode": "full"}),
    ] {
        let (s, v) = put(&w, &w.owner, w.agent, org_read, body.clone()).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("above_account_limit")),
            "{body}: {v}"
        );
        assert_eq!(v["error"]["details"]["who_can"]["role"], "org_admin", "{v}");
        assert_eq!(v["error"]["details"]["limit"], "read");
    }
    // The grant above the limit stands until someone chooses; a choice within it is accepted.
    let (s, v) = put(&w, &w.owner, w.agent, org_read, json!({"level": "read"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        access_of(&w, org_read).await,
        "read",
        "an organization's access is its limit"
    );
    // Another path: the credential-side bulk route refuses the same.
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &format!("/v1/connections/{org_read}/grants"),
            Some(json!({"agent_ids": [w.agent], "mode": "full"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("above_account_limit")),
        "{v}"
    );

    // The organization's own list shows every workspace's agents on its accounts.
    let org_act = conn(&w, "acme_app", None, "write").await;
    assert_eq!(
        put(&w, &w.owner, w.agent, org_act, json!({"level": "act"}))
            .await
            .0,
        200
    );
    let (s, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/orgs/{}/accounts", w.org),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        find(&v, org_act)["agents"][0]["workspace"]["id"],
        w.ws.to_string()
    );
    assert_eq!(find(&v, org_act)["limit"], "act");
    assert_eq!(find(&v, org_act)["can"]["manage"], true);
}
