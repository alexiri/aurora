//! P5-UX-11 (D-353, C-947, C-990, SEC-131): the limit of an organization's account, lowering it,
//! and `personal_of` on the organization's workspaces.
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
"#;

const MIGRATION: &str = include_str!(
    "../../../../crates/aurora-db/migrations/20261201000251_app_connections_limit_level.sql"
);

async fn world() -> W {
    world_with(YAML).await
}

/// An organization's account of `acme_app` with the given limit (and the `access` it implies).
async fn org_account(w: &W, limit: &str) -> Uuid {
    let id = Uuid::now_v7();
    let access = if limit == "read" { "read" } else { "write" };
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, access, limit_level) VALUES ($1,$2,NULL,'acme_app','acme','1.0.0','Acme','active','x',$3,$4)")
        .bind(id).bind(w.org).bind(access).bind(limit)
        .execute(w.h.pool()).await.unwrap();
    id
}

async fn agent(w: &W, handle: &str) -> Uuid {
    w.h.byo_agent(w.org, w.ws, handle, handle).await
}

async fn put(w: &W, agent: Uuid, conn: Uuid, body: Value) -> (u16, Value) {
    w.call(
        &w.owner,
        Method::PUT,
        &format!("/v1/agents/{agent}/grants/connections/{conn}"),
        Some(body),
    )
    .await
}

async fn patch_limit(w: &W, who: &str, conn: Uuid, limit: &str) -> (u16, Value) {
    w.call(
        who,
        Method::PATCH,
        &format!("/v1/connections/{conn}"),
        Some(json!({"limit": limit})),
    )
    .await
}

/// `(mode, allow, ask)` of an agent's grant.
async fn grant(w: &W, agent: Uuid, conn: Uuid) -> Option<(String, Vec<String>, Vec<String>)> {
    sqlx::query_as(
        "SELECT mode, allow_tools, ask_tools FROM grants WHERE agent_id = $1 AND connection_id = $2",
    )
    .bind(agent)
    .bind(conn)
    .fetch_optional(w.h.pool())
    .await
    .unwrap()
}

async fn stored(w: &W, conn: Uuid) -> (Option<String>, String) {
    sqlx::query_as("SELECT limit_level, access FROM app_connections WHERE id = $1")
        .bind(conn)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_owned()).collect()
}

/// A workspace owner who is only a member of the organization.
async fn ws_owner(w: &W) -> String {
    let u = w.h.user("wsowner@x.io", false).await;
    w.h.member(w.org, u, "member", "active").await;
    w.h.bind_ws(w.ws, u, "owner").await;
    let (_, t) = w.h.good_session(u).await;
    cookie(&t)
}

#[tokio::test]
async fn personal_of_is_only_on_personal_workspaces() {
    let w = world().await;
    let me = w.h.user("alice@x.io", false).await;
    sqlx::query("UPDATE users SET display_name = 'Alice' WHERE id = $1")
        .bind(me)
        .execute(w.h.pool())
        .await
        .unwrap();
    let personal = w.h.workspace(w.org).await;
    sqlx::query("UPDATE workspaces SET personal_owner_user_id = $2, personal = true WHERE id = $1")
        .bind(personal)
        .bind(me)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (st, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/orgs/{}/workspaces", w.org),
            None,
        )
        .await;
    assert_eq!(st, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    let by = |id: Uuid| {
        items
            .iter()
            .find(|i| i["id"] == id.to_string())
            .unwrap_or_else(|| panic!("{id} missing in {v}"))
    };
    assert_eq!(by(personal)["personal_of"]["name"], "Alice", "{v}");
    assert_eq!(by(personal)["personal_of"]["id"], me.to_string());
    assert!(by(w.ws).get("personal_of").is_none(), "{v}");
}

#[tokio::test]
async fn the_migration_fills_the_limit_from_access_and_only_for_organization_accounts() {
    let w = world().await;
    let read = org_account(&w, "read").await;
    let write = org_account(&w, "act").await;
    sqlx::query("UPDATE app_connections SET limit_level = NULL WHERE id = ANY($1)")
        .bind(vec![read, write])
        .execute(w.h.pool())
        .await
        .unwrap();
    // The fill of the migration file itself, as it ships.
    let fill = &MIGRATION[MIGRATION.find("UPDATE app_connections").unwrap()..];
    sqlx::query(fill).execute(w.h.pool()).await.unwrap();
    assert_eq!(stored(&w, read).await.0.as_deref(), Some("read"));
    assert_eq!(stored(&w, write).await.0.as_deref(), Some("act"));
    // A workspace's account has none, and the database refuses one.
    let ws_conn = Uuid::now_v7();
    let err = sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, limit_level) VALUES ($1,$2,$3,'acme_app','acme','1.0.0','c','active','x','read')")
        .bind(ws_conn).bind(w.org).bind(w.ws)
        .execute(w.h.pool()).await;
    assert!(err.is_err(), "a workspace account cannot carry a limit");
}

#[tokio::test]
async fn a_choice_above_each_limit_is_refused_custom_choices_included() {
    let w = world().await;
    let bot = w.agent;
    // Ask before changes: nothing may allow a change without asking.
    let ask = org_account(&w, "ask").await;
    for body in [
        json!({"level": "act"}),
        json!({"mode": "full"}),
        json!({"mode": "custom", "allow_tools": ["list_items", "create_item"]}),
    ] {
        let (st, v) = put(&w, bot, ask, body.clone()).await;
        assert_eq!(
            (st, v["error"]["code"].as_str()),
            (422, Some("above_account_limit")),
            "{body}: {v}"
        );
        assert_eq!(v["error"]["details"]["limit"], "ask");
    }
    for body in [
        json!({"level": "ask"}),
        json!({"level": "read"}),
        json!({"mode": "custom", "allow_tools": ["list_items"], "ask_tools": ["create_item"]}),
    ] {
        let (st, v) = put(&w, bot, ask, body.clone()).await;
        assert_eq!(st, 200, "{body}: {v}");
    }
    // Read only: not even asking for a change.
    let read = org_account(&w, "read").await;
    for body in [
        json!({"level": "ask"}),
        json!({"level": "act"}),
        json!({"mode": "custom", "allow_tools": ["list_items"], "ask_tools": ["delete_item"]}),
    ] {
        let (st, v) = put(&w, bot, read, body.clone()).await;
        assert_eq!(
            (st, v["error"]["code"].as_str()),
            (422, Some("above_account_limit")),
            "{body}: {v}"
        );
    }
    assert_eq!(put(&w, bot, read, json!({"level": "read"})).await.0, 200);
    // Change without asking: everything is within it.
    let act = org_account(&w, "act").await;
    assert_eq!(put(&w, bot, act, json!({"level": "act"})).await.0, 200);
}

#[tokio::test]
async fn lowering_the_limit_lowers_every_agent_above_it_and_audits_each() {
    let w = world().await;
    let conn = org_account(&w, "act").await;
    let a_act = w.agent;
    let a_free = agent(&w, "bot2").await; // custom: a change allowed without asking
    let a_ask = agent(&w, "bot3").await;
    let a_read = agent(&w, "bot4").await;
    let a_only_write = agent(&w, "bot5").await; // custom with nothing but a change
    assert_eq!(put(&w, a_act, conn, json!({"level": "act"})).await.0, 200);
    assert_eq!(
        put(
            &w,
            a_free,
            conn,
            json!({"mode": "custom", "allow_tools": ["list_items", "create_item"], "ask_tools": ["delete_item"]})
        )
        .await
        .0,
        200
    );
    assert_eq!(put(&w, a_ask, conn, json!({"level": "ask"})).await.0, 200);
    assert_eq!(put(&w, a_read, conn, json!({"level": "read"})).await.0, 200);
    assert_eq!(
        put(
            &w,
            a_only_write,
            conn,
            json!({"mode": "custom", "allow_tools": ["create_item"]})
        )
        .await
        .0,
        200
    );

    // The preview lists who would change, writes nothing.
    let (st, v) = w
        .call(
            &w.owner,
            Method::GET,
            &format!("/v1/connections/{conn}/limit-preview?limit=ask"),
            None,
        )
        .await;
    assert_eq!(st, 200, "{v}");
    let lowered = v["lowered"].as_array().unwrap();
    assert_eq!(lowered.len(), 3, "{v}");
    let row = |h: &str| {
        lowered
            .iter()
            .find(|l| l["agent"]["handle"] == h)
            .unwrap_or_else(|| panic!("{h} not in {v}"))
    };
    assert_eq!(
        (row("bot")["from"].as_str(), row("bot")["to"].as_str()),
        (Some("act"), Some("ask"))
    );
    assert_eq!(row("bot")["workspace"]["id"], w.ws.to_string());
    assert_eq!(row("bot2")["from"], "custom");
    assert_eq!(
        row("bot5")["to"],
        "custom",
        "a changes-only grant stays custom"
    );
    assert_eq!(
        grant(&w, a_act, conn).await.unwrap().0,
        "full",
        "a preview changes nothing"
    );

    // Lower to Ask before changes.
    let (st, v) = patch_limit(&w, &w.owner, conn, "ask").await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["limit"], "ask");
    assert_eq!(stored(&w, conn).await, (Some("ask".into()), "write".into()));
    assert_eq!(
        grant(&w, a_act, conn).await.unwrap(),
        (
            "custom".into(),
            s(&["list_items", "get_item"]),
            s(&["create_item", "delete_item"])
        ),
        "a Change without asking level becomes Ask before changes"
    );
    let free = grant(&w, a_free, conn).await.unwrap();
    assert_eq!(free.1, s(&["list_items"]), "{free:?}");
    assert!(
        free.2.contains(&"create_item".to_owned()),
        "an allowed change is now asked: {free:?}"
    );
    assert_eq!(
        grant(&w, a_ask, conn).await.unwrap().2,
        s(&["create_item", "delete_item"]),
        "an agent already within the limit is not touched"
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'grant.create' AND metadata->>'via' = 'limit' AND org_id = $1",
    )
    .bind(w.org)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 3, "one audit row per lowered agent");
    let lim: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'connection.update' AND target_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(conn.to_string())
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(lim["extra"]["to_limit"], "ask", "{lim}");
    assert_eq!(lim["extra"]["agents_lowered"], 3, "{lim}");

    // Lower to Read only: changes are removed; a grant with nothing else left is removed.
    let (st, v) = patch_limit(&w, &w.owner, conn, "read").await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(stored(&w, conn).await, (Some("read".into()), "read".into()));
    for (a, reads) in [
        (a_act, s(&["list_items", "get_item"])),
        (a_free, s(&["list_items"])),
        (a_ask, s(&["list_items", "get_item"])),
    ] {
        let g = grant(&w, a, conn).await.unwrap();
        assert_eq!((g.1, g.2), (reads, vec![]), "{a}");
    }
    assert!(
        grant(&w, a_only_write, conn).await.is_none(),
        "nothing of a changes-only grant is left"
    );
    let gone: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'grant.delete' AND metadata->>'via' = 'limit'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(gone, 1);
    assert_eq!(grant(&w, a_read, conn).await.unwrap().0, "custom");
}

/// The decision the gateway reaches for a change by the agent (`POST /items`), through the check
/// that shares the gateway's decision code (C-930).
async fn verdict(w: &W, agent: Uuid) -> Value {
    let (st, v) = w
        .call(
            &w.owner,
            Method::POST,
            &format!("/v1/workspaces/{}/policy/simulate", w.ws),
            Some(json!({"agent_id": agent, "method": "POST", "url": "https://api.acme.example/items"})),
        )
        .await;
    assert_eq!(st, 200, "{v}");
    v["verdict"].clone()
}

#[tokio::test]
async fn after_lowering_a_change_that_went_through_is_held_for_approval() {
    let w = world().await;
    let conn = org_account(&w, "act").await;
    assert_eq!(put(&w, w.agent, conn, json!({"level": "act"})).await.0, 200);
    assert_eq!(verdict(&w, w.agent).await, "allow");
    // Lowering to Ask before changes: the same change now asks.
    assert_eq!(patch_limit(&w, &w.owner, conn, "ask").await.0, 200);
    assert_eq!(verdict(&w, w.agent).await, "needs_approval");
    // And under Read only the change is not made at all.
    assert_eq!(patch_limit(&w, &w.owner, conn, "read").await.0, 200);
    assert_eq!(verdict(&w, w.agent).await, "block");
}

#[tokio::test]
async fn raising_the_limit_changes_nobody_and_a_workspace_owner_cannot_change_it() {
    let w = world().await;
    let conn = org_account(&w, "read").await;
    assert_eq!(
        put(&w, w.agent, conn, json!({"level": "read"})).await.0,
        200
    );
    let before = grant(&w, w.agent, conn).await;
    let (st, v) = patch_limit(&w, &w.owner, conn, "ask").await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["limit"], "ask");
    assert_eq!(stored(&w, conn).await, (Some("ask".into()), "write".into()));
    assert_eq!(
        grant(&w, w.agent, conn).await,
        before,
        "raising lowers and raises nobody"
    );
    // After raising, Ask before changes can be chosen and Change without asking cannot.
    assert_eq!(put(&w, w.agent, conn, json!({"level": "ask"})).await.0, 200);
    assert_eq!(put(&w, w.agent, conn, json!({"level": "act"})).await.0, 422);

    // A workspace owner who is not an organization admin cannot change the limit or see a preview.
    let wo = ws_owner(&w).await;
    for who in [&wo, &w.member] {
        let (st, v) = patch_limit(&w, who, conn, "act").await;
        assert_eq!(st, 403, "{v}");
        let (st, _) = w
            .call(
                who,
                Method::GET,
                &format!("/v1/connections/{conn}/limit-preview?limit=read"),
                None,
            )
            .await;
        assert_eq!(st, 403);
    }
    assert_eq!(stored(&w, conn).await.0.as_deref(), Some("ask"));
    // The workspace owner can still give the account within its limit.
    let a3 = agent(&w, "bot3").await;
    let (st, v) = w
        .call(
            &wo,
            Method::PUT,
            &format!("/v1/agents/{a3}/grants/connections/{conn}"),
            Some(json!({"level": "ask"})),
        )
        .await;
    assert_eq!(st, 200, "{v}");
    // A workspace's account has no limit.
    let ws_conn = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'acme_app','acme','1.0.0','c','active','x')")
        .bind(ws_conn).bind(w.org).bind(w.ws)
        .execute(w.h.pool()).await.unwrap();
    let (st, v) = patch_limit(&w, &w.owner, ws_conn, "read").await;
    assert_eq!(st, 400, "{v}");
}

#[tokio::test]
async fn the_old_access_field_on_an_organizations_account_maps_to_a_limit() {
    let w = world().await;
    let conn = org_account(&w, "act").await;
    let b = agent(&w, "bot2").await;
    assert_eq!(put(&w, b, conn, json!({"level": "act"})).await.0, 200);
    let (st, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &format!("/v1/connections/{conn}"),
            Some(json!({"access": "read"})),
        )
        .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["limit"], "read");
    assert_eq!(stored(&w, conn).await, (Some("read".into()), "read".into()));
    assert_eq!(
        grant(&w, b, conn).await.unwrap().2,
        Vec::<String>::new(),
        "the old field lowers agents too"
    );
    let (st, _) = w
        .call(
            &w.owner,
            Method::PATCH,
            &format!("/v1/connections/{conn}"),
            Some(json!({"access": "read", "limit": "ask"})),
        )
        .await;
    assert_eq!(st, 400);
}
