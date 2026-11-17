//! P5-UX-16 (D-348, D-353, C-982, C-983, SEC-131): the sentence and origin of an approval, Always
//! allow, and the transcript events of an approval in a conversation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use crate::grants_connections::{W, world_with};
use http::Method;
use serde_json::{Value, json};
use std::time::Duration;
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
      write:
        items:
          - { id: create_item, name: Create an item, host: api.acme.example, path: /items, methods: [POST] }
          - { id: delete_item, name: Delete an item, host: api.acme.example, path: "/items/*", methods: [DELETE] }
    hosts:
      - match: { host: "api.acme.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

struct World {
    w: W,
    conn: Uuid,
    conv: Uuid,
    gw: Uuid,
    /// The derived ask rule's logical id.
    ask_rule: Uuid,
    approver: String,
}

async fn conn(w: &W, ws: Option<Uuid>, access: &str, label: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, access) VALUES ($1,$2,$3,'acme_app','acme','1.0.0',$4,'active','x',$5)")
        .bind(id).bind(w.org).bind(ws).bind(label).bind(access)
        .execute(w.h.pool()).await.unwrap();
    id
}

/// A workspace with an agent whose access is *Ask before changes* on one account of the app.
async fn world() -> World {
    let w = world_with(YAML).await;
    let conn = conn(&w, Some(w.ws), "write", "Bob's Acme").await;
    let (s, v) = w
        .call(
            &w.owner,
            Method::PUT,
            &format!("/v1/agents/{}/grants/connections/{conn}", w.agent),
            Some(json!({"level": "ask"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let ask_rule: Uuid = sqlx::query_scalar(
        "SELECT d.logical_id FROM derived_rules d JOIN grants g ON g.id = d.source_id \
         WHERE d.source_kind='grant' AND g.agent_id=$1 AND d.ordinal=1",
    )
    .bind(w.agent)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    let conv = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) VALUES ($1,$2,$3,'api',$1::text, now())")
        .bind(conv).bind(w.agent).bind(w.ws).execute(w.h.pool()).await.unwrap();
    let gw = Uuid::now_v7();
    sqlx::query("INSERT INTO gateway_instances (id, hostname, version, started_at, last_seen_at) VALUES ($1,'h','0',now(),now())")
        .bind(gw).execute(w.h.pool()).await.unwrap();
    // A person who may approve but does not own the workspace.
    let ap = w.h.user("approver@x.io", false).await;
    w.h.member(w.org, ap, "member", "active").await;
    w.h.bind_ws(w.ws, ap, "approver").await;
    let (_, t) = w.h.good_session(ap).await;
    World {
        approver: cookie(&t),
        w,
        conn,
        conv,
        gw,
        ask_rule,
    }
}

/// A pending approval of the world's agent for `tool` on `connection`, asked by `rule`.
async fn approval(x: &World, tool: Option<&str>, connection: Option<Uuid>, rule: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, conversation_id, gateway_instance_id, \
            method, host, path, app_id, tool_id, connection_id, rule_logical_id, rule_name, summary, status, deadline) \
         VALUES ($1,$2,$3,$4,$5,$6,'POST','api.acme.example','/items','acme_app',$7,$8,$9,'Grant: x (ask)', \
            '{\"title\":\"create an item\",\"fields\":[],\"text\":null,\"link\":null,\"group\":null,\"source\":\"template\"}', \
            'pending', now() + interval '5 minutes')",
    )
    .bind(id).bind(x.w.org).bind(x.w.ws).bind(x.w.agent).bind(x.conv).bind(x.gw)
    .bind(tool).bind(connection).bind(rule)
    .execute(x.w.h.pool()).await.unwrap();
    id
}

async fn decide(x: &World, who: &str, id: Uuid, body: Value) -> (u16, Value) {
    x.w.call(
        who,
        Method::POST,
        &format!("/v1/approvals/{id}/decision"),
        Some(body),
    )
    .await
}

async fn list_pending(x: &World, who: &str) -> Value {
    let (s, v) =
        x.w.call(
            who,
            Method::GET,
            &format!("/v1/workspaces/{}/approvals?status=pending", x.w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    v
}

async fn status(x: &World, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM approvals WHERE id=$1")
        .bind(id)
        .fetch_one(x.w.h.pool())
        .await
        .unwrap()
}

async fn grant_lists(x: &World) -> (Vec<String>, Vec<String>) {
    sqlx::query_as(
        "SELECT allow_tools, ask_tools FROM grants WHERE agent_id=$1 AND connection_id=$2",
    )
    .bind(x.w.agent)
    .bind(x.conn)
    .fetch_one(x.w.h.pool())
    .await
    .unwrap()
}

async fn events(x: &World, ty: &str) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT payload FROM transcript_events WHERE conversation_id=$1 AND type=$2 ORDER BY seq",
    )
    .bind(x.conv)
    .bind(ty)
    .fetch_all(x.w.h.pool())
    .await
    .unwrap()
}

/// Polls a transcript condition with a deadline (the bus listener writes `approval.pending`).
async fn wait_events(x: &World, ty: &str, n: usize) -> Vec<Value> {
    for _ in 0..200 {
        let v = events(x, ty).await;
        if v.len() >= n {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no {n} `{ty}` events");
}

#[tokio::test]
async fn an_approval_reads_as_a_sentence_and_says_who_asked() {
    let x = world().await;
    let id = approval(&x, Some("create_item"), Some(x.conn), x.ask_rule).await;
    let v = list_pending(&x, &x.w.owner).await;
    let a = &v["items"][0];
    assert_eq!(a["id"], id.to_string());
    assert_eq!(a["sentence"]["agent"]["name"], "Bot");
    assert_eq!(a["sentence"]["action"], "create an item");
    assert_eq!(
        a["sentence"]["app"],
        json!({"app_id": "acme_app", "name": "Acme"})
    );
    assert_eq!(a["sentence"]["account"]["name"], "Bob's Acme");
    assert_eq!(a["sentence"]["account"]["ref"]["id"], x.conn.to_string());
    assert_eq!(a["asked_by"]["kind"], "access");
    assert_eq!(a["asked_by"]["tool_id"], "create_item");
    assert_eq!(a["can_always_allow"], true);
    // An approver who does not own the workspace reads the same question without the option.
    let v = list_pending(&x, &x.approver).await;
    assert_eq!(v["items"][0]["asked_by"]["kind"], "access");
    assert_eq!(v["items"][0]["can_always_allow"], false);

    // A generic title falls back to the tool's name, then to the request.
    sqlx::query(
        "UPDATE approvals SET summary = jsonb_set(summary, '{source}', '\"generic\"') WHERE id=$1",
    )
    .bind(id)
    .execute(x.w.h.pool())
    .await
    .unwrap();
    let v = list_pending(&x, &x.w.owner).await;
    assert_eq!(v["items"][0]["sentence"]["action"], "create an item");
    sqlx::query(
        "UPDATE approvals SET tool_id = NULL, app_id = NULL, connection_id = NULL WHERE id=$1",
    )
    .bind(id)
    .execute(x.w.h.pool())
    .await
    .unwrap();
    let v = list_pending(&x, &x.w.owner).await;
    assert_eq!(
        v["items"][0]["sentence"]["action"],
        "send a POST request to api.acme.example"
    );
    assert!(v["items"][0]["sentence"]["app"].is_null());
}

#[tokio::test]
async fn always_allow_moves_only_the_asked_tool_and_approves_the_same_kind() {
    let x = world().await;
    let a1 = approval(&x, Some("create_item"), Some(x.conn), x.ask_rule).await;
    let a2 = approval(&x, Some("create_item"), Some(x.conn), x.ask_rule).await;
    let other = approval(&x, Some("delete_item"), Some(x.conn), x.ask_rule).await;
    assert_eq!(
        grant_lists(&x).await,
        (
            vec!["list_items".to_owned()],
            vec!["create_item".to_owned(), "delete_item".to_owned()]
        )
    );
    let (s, v) = decide(
        &x,
        &x.w.owner,
        a1,
        json!({"decision": "approve", "always": true}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "approved");
    assert_eq!(
        v["always_applied"]["tool"],
        json!({"id": "create_item", "name": "Create an item"})
    );
    assert_eq!(v["always_applied"]["also_approved"], 1);
    // The other approval of the same kind went ahead; the other tool still asks.
    assert_eq!(status(&x, a2).await, "approved");
    assert_eq!(status(&x, other).await, "pending");
    // Only the asked tool moved; the access stays Ask before changes with the change listed.
    assert_eq!(
        grant_lists(&x).await,
        (
            vec!["list_items".to_owned(), "create_item".to_owned()],
            vec!["delete_item".to_owned()]
        )
    );
    let (s, acc) =
        x.w.call(
            &x.w.owner,
            Method::GET,
            &format!("/v1/workspaces/{}/accounts", x.w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{acc}");
    let row = acc["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["ref"]["id"] == x.conn.to_string())
        .unwrap();
    let entry = &row["agents"][0];
    assert_eq!(entry["level"], "ask", "{entry}");
    assert_eq!(
        entry["always_allowed"][0]["tool_id"], "create_item",
        "{entry}"
    );
    // The access change is audited with the approval id.
    let meta: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action='grant.create' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(x.w.h.pool())
    .await
    .unwrap();
    assert_eq!(meta["via"], "approval", "{meta}");
    assert_eq!(meta["approval_id"], a1.to_string(), "{meta}");
    // The answers appear in the conversation: the card, then the answer with the Always allow.
    let decided = events(&x, "approval.decided").await;
    assert_eq!(decided.len(), 2, "{decided:?}");
    let first = decided
        .iter()
        .find(|e| e["approval_id"] == a1.to_string())
        .unwrap();
    assert_eq!(first["decision"], "approve");
    assert_eq!(first["always"]["tool"]["name"], "Create an item");
    assert_eq!(first["always"]["also_approved"], 1);
    assert_eq!(
        events(&x, "approval.pending").await.len(),
        2,
        "pending is written before the answer"
    );
}

#[tokio::test]
async fn always_allow_is_refused_where_sec_131_does_not_allow_it() {
    let x = world().await;
    let id = approval(&x, Some("create_item"), Some(x.conn), x.ask_rule).await;
    let before = grant_lists(&x).await;
    // An approver who is not an owner may answer, not choose Always allow.
    let (s, v) = decide(
        &x,
        &x.approver,
        id,
        json!({"decision": "approve", "always": true}),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("always_not_available")),
        "{v}"
    );
    // `always` goes with approve only.
    let (s, v) = decide(
        &x,
        &x.w.owner,
        id,
        json!({"decision": "deny", "always": true}),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("always_not_available")),
        "{v}"
    );
    // A question asked by a rule is not the access's to change.
    let rule = Uuid::now_v7();
    let gen_id = Uuid::now_v7();
    sqlx::query("INSERT INTO policy_generations (id, org_id, workspace_id, number, status) VALUES ($1,$2,$3,1,'published')")
        .bind(gen_id).bind(x.w.org).bind(x.w.ws).execute(x.w.h.pool()).await.unwrap();
    sqlx::query("INSERT INTO policy_rules (id, generation_id, logical_id, priority, name, identities, targets, action, require_approval) VALUES ($1,$2,$3,1,'Ask on Acme','[]','[]','allow',true)")
        .bind(Uuid::now_v7()).bind(gen_id).bind(rule).execute(x.w.h.pool()).await.unwrap();
    let ruled = approval(&x, Some("create_item"), Some(x.conn), rule).await;
    let v = list_pending(&x, &x.w.owner).await;
    let a = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == ruled.to_string())
        .unwrap();
    assert_eq!(
        a["asked_by"],
        json!({"kind": "rule", "rule_id": rule.to_string(), "rule_name": "Ask on Acme", "scope": "workspace"})
    );
    assert_eq!(a["can_always_allow"], false);
    let (s, v) = decide(
        &x,
        &x.w.owner,
        ruled,
        json!({"decision": "approve", "always": true}),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("always_not_available")),
        "{v}"
    );
    // An organization's account whose limit is below Change without asking.
    let org_conn = conn(&x.w, None, "read", "Org Acme").await;
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode, ask_tools) VALUES ($1,$2,$3,$4,'custom','{create_item}')")
        .bind(Uuid::now_v7()).bind(x.w.ws).bind(x.w.agent).bind(org_conn).execute(x.w.h.pool()).await.unwrap();
    let g: Uuid = sqlx::query_scalar("SELECT id FROM grants WHERE connection_id=$1")
        .bind(org_conn)
        .fetch_one(x.w.h.pool())
        .await
        .unwrap();
    let logical = Uuid::now_v7();
    sqlx::query("INSERT INTO derived_rules (id, org_id, workspace_id, source_kind, source_id, ordinal, logical_id, rule) VALUES ($1,$2,$3,'grant',$4,1,$5,'{}')")
        .bind(Uuid::now_v7()).bind(x.w.org).bind(x.w.ws).bind(g).bind(logical)
        .execute(x.w.h.pool()).await.unwrap();
    let limited = approval(&x, Some("create_item"), Some(org_conn), logical).await;
    let (s, v) = decide(
        &x,
        &x.w.owner,
        limited,
        json!({"decision": "approve", "always": true}),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("always_not_available")),
        "{v}"
    );
    // Nothing moved, nothing was decided by the refusals.
    assert_eq!(grant_lists(&x).await, before);
    for a in [id, ruled, limited] {
        assert_eq!(status(&x, a).await, "pending");
    }
    // Allow once still works for the approver on the same question.
    let (s, v) = decide(&x, &x.approver, id, json!({"decision": "approve"})).await;
    assert_eq!(s, 200, "{v}");
    assert!(v.get("always_applied").is_none_or(Value::is_null));
}

#[tokio::test]
async fn a_new_approval_and_its_expiry_appear_in_the_conversation() {
    let x = world().await;
    aurora_api::domains::approvals::transcript::spawn(x.w.h.state.clone());
    // The listener is subscribed lazily: give it a moment by retrying the notify with a deadline.
    let id = approval(&x, Some("create_item"), Some(x.conn), x.ask_rule).await;
    let mut seen = Vec::new();
    for _ in 0..100 {
        aurora_db::bus::notify(
            x.w.h.pool(),
            aurora_db::bus::Channel::Approval,
            &json!({"id": id, "workspace_id": x.w.ws, "event": "created"}),
        )
        .await
        .unwrap();
        seen = events(&x, "approval.pending").await;
        if !seen.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(seen.len(), 1, "one card, however often the bus repeats");
    assert_eq!(seen[0]["approval_id"], id.to_string());
    assert_eq!(seen[0]["summary"], "Bot wants to create an item in Acme");
    assert_eq!(seen[0]["parked"], false);
    assert!(seen[0]["expires_at"].is_string());
    // It runs out: the sweep expires it and tells the conversation.
    sqlx::query("UPDATE approvals SET status='expired', resolved_at=now() WHERE id=$1")
        .bind(id)
        .execute(x.w.h.pool())
        .await
        .unwrap();
    aurora_db::bus::notify(
        x.w.h.pool(),
        aurora_db::bus::Channel::Approval,
        &json!({"id": id, "workspace_id": x.w.ws, "event": "expired"}),
    )
    .await
    .unwrap();
    let expired = wait_events(&x, "approval.expired", 1).await;
    assert_eq!(expired[0]["reason"], "deadline");
    assert_eq!(events(&x, "approval.pending").await.len(), 1);
}
