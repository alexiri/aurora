//! P4-GRANTS (D-123, SEC-67; C-410..C-413): apps that are `custom_only`: `full_grant_forbidden`
//! on every grant-writing route, `app_restrictions` in responses, and the conversion of existing
//! full grants by the activation of a version that introduces `custom_only`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use crate::plugins_lifecycle::{Admin, setup};
use http::Method;
use serde_json::{Value, json};
use sqlx::postgres::PgListener;
use uuid::Uuid;

/// Package `cust` (app `cust_app`) with the given `grants:` line (may be empty) and host.
fn yaml(version: &str, grants: &str, host: &str) -> String {
    format!(
        r#"package: {{ id: cust, version: {version}, contract: 1, min_core: 0.1.0, publisher: acme, name: P }}
apps:
  - about: {{ id: cust_app, name: Cust }}
    credentials: [ {{ name: api_token, secret: true }} ]
    connect:
      - id: api_key
        kind: fields
        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]
{grants}    tools:
      read:
        wildcard: {{ id: all_read, name: All reads }}
        items:
          - {{ id: list_items, name: List, host: "{host}", path: /items, methods: [GET] }}
          - {{ id: get_item, name: Get, host: "{host}", path: "/items/*", methods: [GET] }}
      write:
        items:
          - {{ id: create_item, name: Create, host: "{host}", path: /items, methods: [POST] }}
    hosts:
      - match: {{ host: "{host}" }}
        inject:
          - bearer: "{{credentials.api_token}}"
"#
    )
}

const HOST: &str = "api.cust.example";
// (FIX-085) `custom_only` implies `block_untooled`: the manifest says both.
const CUSTOM_ONLY: &str = "    grants: { custom_only: true, block_untooled: true }\n";
const BLOCK_UNTOOLED: &str = "    grants: { block_untooled: true }\n";

struct W<'a> {
    h: &'a Harness,
    org: Uuid,
    ws: Uuid,
    ws2: Uuid,
    owner: String,
    agent: Uuid,
    agent2: Uuid,
    agent_ws2: Uuid,
}

async fn agent(h: &Harness, org: Uuid, ws: Uuid, handle: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,$4,$5,'byo','medium','')")
        .bind(id).bind(org).bind(ws).bind(handle).bind(format!("Agent {handle}"))
        .execute(h.pool()).await.unwrap();
    id
}

async fn world(h: &Harness) -> W<'_> {
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let ws2 = h.workspace(org).await;
    let o = h.user("owner@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    h.bind_ws(ws, o, "owner").await;
    h.bind_ws(ws2, o, "owner").await;
    let (_, ot) = h.good_session(o).await;
    let agent1 = agent(h, org, ws, "one").await;
    let agent2 = agent(h, org, ws, "two").await;
    let agent_ws2 = agent(h, org, ws2, "three").await;
    W {
        h,
        org,
        ws,
        ws2,
        owner: cookie(&ot),
        agent: agent1,
        agent2,
        agent_ws2,
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

    /// An org-level connection of `cust_app`.
    async fn conn(&self) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,NULL,'cust_app','cust','1.0.0','cc','active','x')")
            .bind(id).bind(self.org).execute(self.h.pool()).await.unwrap();
        id
    }

    fn put(&self, agent: Uuid, conn: Uuid) -> String {
        format!("/v1/agents/{agent}/grants/connections/{conn}")
    }

    async fn grant_row(&self, agent: Uuid, conn: Uuid) -> (String, Vec<String>, Vec<String>) {
        sqlx::query_as(
            "SELECT mode, allow_tools, ask_tools FROM grants WHERE agent_id = $1 AND connection_id = $2",
        )
        .bind(agent)
        .bind(conn)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }

    async fn rules(&self, agent: Uuid, conn: Uuid) -> Vec<Value> {
        sqlx::query_scalar::<_, Value>(
            "SELECT d.rule FROM derived_rules d JOIN grants g ON g.id = d.source_id \
             WHERE d.source_kind = 'grant' AND g.agent_id = $1 AND g.connection_id = $2 ORDER BY d.ordinal",
        )
        .bind(agent)
        .bind(conn)
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }
}

/// FIX-084: a gateway hold of `agent` (that would use `conn`, if any), still `pending`.
async fn pend(w: &W<'_>, gw: Uuid, agent: Uuid, ws: Uuid, conn: Option<Uuid>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, gateway_instance_id, method, host, path,
            connection_id, summary, status, deadline)
         VALUES ($1,$2,$3,$4,$5,'POST','api.cust.example','/items',$6,
            '{\"title\":\"T\",\"fields\":[],\"text\":null,\"link\":null,\"group\":null,\"source\":\"generic\"}',
            'pending', now() + interval '5 minutes')",
    )
    .bind(id)
    .bind(w.org)
    .bind(ws)
    .bind(agent)
    .bind(gw)
    .bind(conn)
    .execute(w.h.pool())
    .await
    .unwrap();
    id
}

async fn gateway_instance(h: &Harness) -> Uuid {
    let gw = Uuid::now_v7();
    sqlx::query("INSERT INTO gateway_instances (id, hostname, version, started_at, last_seen_at) VALUES ($1,'h','0',now(),now())")
        .bind(gw)
        .execute(h.pool())
        .await
        .unwrap();
    gw
}

async fn status_of(h: &Harness, approval: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
        .bind(approval)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

async fn install(a: &Admin<'_>, version: &str, grants: &str, host: &str) -> Value {
    let v = a.upload(&yaml(version, grants, host)).await;
    let v = if v["status"] == "pending_approval" {
        let (s, b) = a.act(&v["id"], "approve").await;
        assert_eq!(s, 200, "{b}");
        b
    } else {
        v
    };
    assert_eq!(v["status"], "active", "{v}");
    v
}

async fn audit_meta(h: &Harness, action: &str) -> Vec<Value> {
    sqlx::query_scalar("SELECT metadata FROM audit_log WHERE action = $1 ORDER BY id")
        .bind(action)
        .fetch_all(h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn full_grant_forbidden_on_every_route_and_app_restrictions_everywhere() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install(&a, "1.0.0", &format!("{CUSTOM_ONLY}{}", ""), HOST).await;
    let w = world(&h).await;
    let conn = w.conn().await;

    // Agent-side PUT.
    let (s, v) = w
        .call(
            Method::PUT,
            &w.put(w.agent, conn),
            Some(json!({"mode": "full"})),
        )
        .await;
    assert_eq!(
        (
            s,
            v["error"]["code"].as_str(),
            v["error"]["details"]["app_id"].as_str()
        ),
        (422, Some("full_grant_forbidden"), Some("cust_app")),
        "{v}"
    );
    // Credential-side bulk: the same function, all-or-nothing (details name the agent).
    let (s, v) = w
        .call(
            Method::POST,
            &format!("/v1/connections/{conn}/grants"),
            Some(json!({"agent_ids": [w.agent, w.agent2], "mode": "full"})),
        )
        .await;
    assert_eq!(
        (
            s,
            v["error"]["code"].as_str(),
            v["error"]["details"]["app_id"].as_str()
        ),
        (422, Some("full_grant_forbidden"), Some("cust_app")),
        "{v}"
    );
    assert!(v["error"]["details"]["agent_id"].is_string(), "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM grants WHERE connection_id = $1")
        .bind(conn)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0, "nothing written");

    // Custom grants of a custom_only app are unchanged: allow, ask, block-rest; new tools arrive
    // blocked (not listed); responses carry app_restrictions.
    let body = json!({"mode": "custom", "allow_tools": ["get_item"], "ask_tools": ["create_item"]});
    let (s, v) = w
        .call(Method::PUT, &w.put(w.agent, conn), Some(body.clone()))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["app_restrictions"],
        json!({"custom_only": true, "block_untooled": true})
    );
    let r = w.rules(w.agent, conn).await;
    assert_eq!(r.len(), 3);
    assert_eq!(r[0]["targets"][0]["tools"], json!(["get_item"]));
    assert_eq!(r[1]["require_approval"], true);
    assert_eq!(r[2]["action"], "block");
    assert!(r[2]["targets"][0].get("tools").is_none());

    // Switching an existing custom grant to full is refused too, leaving it intact.
    let (s, v) = w
        .call(
            Method::PUT,
            &w.put(w.agent, conn),
            Some(json!({"mode": "full"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("full_grant_forbidden"))
    );
    assert_eq!(w.grant_row(w.agent, conn).await.0, "custom");

    // Bulk custom works and reports the restrictions; GET lists carry them.
    let (s, v) = w
        .call(
            Method::POST,
            &format!("/v1/connections/{conn}/grants"),
            Some(json!({"agent_ids": [w.agent2], "mode": "custom", "allow_tools": ["list_items"]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["items"][0]["grant"]["app_restrictions"],
        json!({"custom_only": true, "block_untooled": true})
    );
    // C-504: the bulk response carries them at the top as well.
    assert_eq!(
        v["app_restrictions"],
        json!({"custom_only": true, "block_untooled": true})
    );
    let (s, v) = w
        .call(Method::GET, &format!("/v1/agents/{}/grants", w.agent), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["connections"][0]["app_restrictions"],
        json!({"custom_only": true, "block_untooled": true})
    );
    let (s, v) = w
        .call(Method::GET, &format!("/v1/connections/{conn}/grants"), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["items"][0]["app_restrictions"],
        json!({"custom_only": true, "block_untooled": true})
    );
    // The catalogue and the app detail carry the flags too (C-414).
    let (s, v) = w.call(Method::GET, "/v1/apps/cust_app", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["app_restrictions"],
        json!({"custom_only": true, "block_untooled": true})
    );
    let (s, v) = w.call(Method::GET, "/v1/apps", None).await;
    assert_eq!(s, 200, "{v}");
    let item = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["app_id"] == "cust_app")
        .unwrap();
    assert_eq!(
        item["app_restrictions"],
        json!({"custom_only": true, "block_untooled": true})
    );
}

/// FIX-084 / SEC-67: an app the active snapshot does not hold (its package was deactivated) is
/// `custom_only` + `block_untooled`; it never reads as "no restrictions".
#[tokio::test]
async fn full_grant_is_refused_while_the_apps_package_is_deactivated() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let v = install(&a, "1.0.0", "", HOST).await;
    let w = world(&h).await;
    let conn = w.conn().await;
    let (s, b) = a.act(&v["id"], "disable").await;
    assert_eq!(s, 200, "{b}");
    let (s, v) = w
        .call(
            Method::PUT,
            &w.put(w.agent, conn),
            Some(json!({"mode": "full"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("full_grant_forbidden")),
        "{v}"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM grants WHERE connection_id = $1")
        .bind(conn)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0, "nothing written");
}

/// FIX-084: every writer that removes or narrows the access a held request would use abandons
/// exactly the pending approvals that depend on it.
#[tokio::test]
async fn writers_that_remove_access_abandon_exactly_the_dependent_pending_approvals() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install(&a, "1.0.0", "", HOST).await;
    let w = world(&h).await;
    let gw = gateway_instance(w.h).await;
    let conn = w.conn().await;
    let other_conn = w.conn().await;
    let wide = json!({"mode": "custom", "allow_tools": ["get_item", "list_items"], "ask_tools": ["create_item"]});
    for ag in [w.agent, w.agent2] {
        for c in [conn, other_conn] {
            let (s, v) = w.call(Method::PUT, &w.put(ag, c), Some(wide.clone())).await;
            assert_eq!(s, 200, "{v}");
        }
    }

    // A widening replace and a re-shuffle between allow and ask keep every hold.
    let held = pend(&w, gw, w.agent, w.ws, Some(conn)).await;
    let shuffled =
        json!({"mode": "custom", "allow_tools": ["get_item", "list_items", "create_item"]});
    let (s, v) = w
        .call(Method::PUT, &w.put(w.agent, conn), Some(shuffled))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(status_of(w.h, held).await, "pending", "no tool was removed");

    // Narrowing (a tool is dropped): only this agent's holds on this connection end.
    let same_agent_other_conn = pend(&w, gw, w.agent, w.ws, Some(other_conn)).await;
    let other_agent = pend(&w, gw, w.agent2, w.ws, Some(conn)).await;
    let secret_hold = pend(&w, gw, w.agent, w.ws, None).await;
    let narrow = json!({"mode": "custom", "allow_tools": ["get_item"]});
    let mut l = PgListener::connect_with(w.h.pool()).await.unwrap();
    l.listen("aurora_approval").await.unwrap();
    let (s, v) = w
        .call(Method::PUT, &w.put(w.agent, conn), Some(narrow))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(status_of(w.h, held).await, "abandoned");
    // (FIX-122) The stream/wake event of a revocation-abandoned approval is `abandoned`, the
    // name the freeze path uses.
    let mut event = None;
    while let Ok(Ok(n)) = tokio::time::timeout(std::time::Duration::from_secs(2), l.recv()).await {
        let p: Value = serde_json::from_str(n.payload()).unwrap();
        if p["id"] == json!(held) {
            event = p["event"].as_str().map(str::to_owned);
            break;
        }
    }
    assert_eq!(event.as_deref(), Some("abandoned"));
    assert_eq!(status_of(w.h, same_agent_other_conn).await, "pending");
    assert_eq!(status_of(w.h, other_agent).await, "pending");
    assert_eq!(status_of(w.h, secret_hold).await, "pending");

    // Revoking the grant: the same scope.
    let (s, v) = w.call(Method::DELETE, &w.put(w.agent2, conn), None).await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(status_of(w.h, other_agent).await, "abandoned");
    assert_eq!(status_of(w.h, same_agent_other_conn).await, "pending");

    // Revoking a secret grant ends the agent's holds that use no app connection.
    let secret = Uuid::now_v7();
    sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,'k','generic','api.cust.example','{\"kind\":\"bearer\"}','managed','x:')")
        .bind(secret).bind(w.org).bind(w.ws).execute(w.h.pool()).await.unwrap();
    let (s, v) = w
        .call(
            Method::PUT,
            &format!("/v1/agents/{}/grants/secrets/{secret}", w.agent),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(
            Method::DELETE,
            &format!("/v1/agents/{}/grants/secrets/{secret}", w.agent),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(status_of(w.h, secret_hold).await, "abandoned");
    assert_eq!(status_of(w.h, same_agent_other_conn).await, "pending");

    // Disabling the connection ends every agent's holds on it; the other connection is untouched.
    let on_other_a = pend(&w, gw, w.agent, w.ws, Some(other_conn)).await;
    let on_other_b = pend(&w, gw, w.agent2, w.ws, Some(other_conn)).await;
    let (s, v) = w
        .call(
            Method::PATCH,
            &format!("/v1/connections/{other_conn}"),
            Some(json!({"status": "disabled"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    for id in [same_agent_other_conn, on_other_a, on_other_b] {
        assert_eq!(status_of(w.h, id).await, "abandoned");
    }

    // Deleting the connection does the same.
    let kept = pend(&w, gw, w.agent, w.ws, None).await;
    let on_conn = pend(&w, gw, w.agent, w.ws, Some(conn)).await;
    let (s, v) = w
        .call(Method::DELETE, &format!("/v1/connections/{conn}"), None)
        .await;
    assert_eq!(s / 100, 2, "{v}");
    assert_eq!(status_of(w.h, on_conn).await, "abandoned");
    assert_eq!(status_of(w.h, kept).await, "pending");
}

#[tokio::test]
async fn an_app_without_flags_still_accepts_full_and_reports_both_false() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install(&a, "1.0.0", "", HOST).await;
    let w = world(&h).await;
    let conn = w.conn().await;
    let (s, v) = w
        .call(
            Method::PUT,
            &w.put(w.agent, conn),
            Some(json!({"mode": "full"})),
        )
        .await;
    assert_eq!((s, v["mode"].as_str()), (200, Some("full")), "{v}");
    assert_eq!(
        v["app_restrictions"],
        json!({"custom_only": false, "block_untooled": false})
    );
}

#[tokio::test]
async fn block_untooled_alone_keeps_full_available_and_converts_nothing() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install(&a, "1.0.0", "", HOST).await;
    let w = world(&h).await;
    let conn = w.conn().await;
    let (s, _) = w
        .call(
            Method::PUT,
            &w.put(w.agent, conn),
            Some(json!({"mode": "full"})),
        )
        .await;
    assert_eq!(s, 200);
    // 1.1.0 adds only block_untooled: no conversion, nothing held, auto-activates.
    let v = a.upload(&yaml("1.1.0", BLOCK_UNTOOLED, HOST)).await;
    assert_eq!(v["status"], "active", "{v}");
    assert_eq!(w.grant_row(w.agent, conn).await.0, "full");
    let (s, v) = w
        .call(
            Method::PUT,
            &w.put(w.agent2, conn),
            Some(json!({"mode": "full"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["app_restrictions"],
        json!({"custom_only": false, "block_untooled": true})
    );
    assert!(audit_meta(w.h, "grant.converted_by_app").await.is_empty());
}

#[tokio::test]
async fn activation_introducing_custom_only_converts_full_grants_atomically_and_is_held() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install(&a, "1.0.0", "", HOST).await;
    let w = world(&h).await;
    let conn = w.conn().await;
    // Two full grants in workspace 1, one in workspace 2, one custom grant that must stay as is.
    for ag in [w.agent, w.agent_ws2] {
        let (s, v) = w
            .call(Method::PUT, &w.put(ag, conn), Some(json!({"mode": "full"})))
            .await;
        assert_eq!(s, 200, "{v}");
    }
    let (s, _) = w
        .call(
            Method::PUT,
            &w.put(w.agent2, conn),
            Some(json!({"mode": "custom", "allow_tools": ["get_item"], "ask_tools": ["create_item"]})),
        )
        .await;
    assert_eq!(s, 200);
    let before = w.rules(w.agent, conn).await;
    assert_eq!(before.len(), 1);

    // 1.1.0 introduces custom_only: held with converts_grants; nothing changes until approval.
    let v = a.upload(&yaml("1.1.0", CUSTOM_ONLY, HOST)).await;
    assert_eq!(
        (v["status"].as_str(), v["status_reason"].as_str()),
        (Some("pending_approval"), Some("converts_grants")),
        "{v}"
    );
    let det = a.status(&v["id"]).await;
    assert_eq!(det["impact"]["grants_to_convert"], 2, "{det}");
    assert_eq!(
        det["impact"]["by_app"],
        json!([{"app_id": "cust_app", "connections": 1, "needing_reauth": 0,
                "grants_to_convert": 2, "grant_workspaces": 2}])
    );
    // Counts only: no workspace, agent or connection id leaks.
    let text = det.to_string();
    for id in [w.ws, w.ws2, w.agent, w.agent_ws2, conn] {
        assert!(!text.contains(&id.to_string()), "{text}");
    }
    assert_eq!(w.grant_row(w.agent, conn).await.0, "full");

    // (FIX-084) Requests held under the full grants; the custom grant's hold and an unrelated
    // hold of the same agent must survive the conversion.
    let gw = gateway_instance(w.h).await;
    let held_full = pend(&w, gw, w.agent, w.ws, Some(conn)).await;
    let held_full_ws2 = pend(&w, gw, w.agent_ws2, w.ws2, Some(conn)).await;
    let held_custom = pend(&w, gw, w.agent2, w.ws, Some(conn)).await;
    let held_other = pend(&w, gw, w.agent, w.ws, None).await;

    let mut l = PgListener::connect_with(w.h.pool()).await.unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();
    let (s, ap) = a.act(&v["id"], "approve").await;
    assert_eq!(s, 200, "{ap}");
    assert_eq!(status_of(w.h, held_full).await, "abandoned");
    assert_eq!(status_of(w.h, held_full_ws2).await, "abandoned");
    assert_eq!(status_of(w.h, held_custom).await, "pending");
    assert_eq!(status_of(w.h, held_other).await, "pending");

    // Converted: exactly the whole catalogue as allow_tools (items and the group wildcard),
    // nothing to ask, derived rules recompiled as custom (allow + block rest).
    for ag in [w.agent, w.agent_ws2] {
        let (mode, allow, ask) = w.grant_row(ag, conn).await;
        assert_eq!(mode, "custom");
        let mut allow = allow;
        allow.sort();
        assert_eq!(allow, ["all_read", "create_item", "get_item", "list_items"]);
        assert!(ask.is_empty());
        let r = w.rules(ag, conn).await;
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!(r[0]["action"], "allow");
        assert_eq!(r[0]["require_approval"], false);
        assert_eq!(r[1]["action"], "block");
        assert!(r[1]["targets"][0].get("tools").is_none());
    }
    // The pre-existing custom grant is untouched.
    let (mode, allow, ask) = w.grant_row(w.agent2, conn).await;
    assert_eq!(
        (mode.as_str(), allow, ask),
        (
            "custom",
            vec!["get_item".to_owned()],
            vec!["create_item".to_owned()]
        )
    );

    // One security-critical audit row per converted grant, plus the approval's counts.
    let rows = audit_meta(w.h, "grant.converted_by_app").await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(
        rows.iter()
            .all(|m| m["app_id"] == "cust_app" && m["version"] == "1.1.0")
    );
    let approve = audit_meta(w.h, "plugin.approve").await;
    assert_eq!(approve.last().unwrap()["grants_converted"], 2);
    // Gateways are told per agent.
    let mut seen = Vec::new();
    while let Ok(Ok(Some(n))) =
        tokio::time::timeout(std::time::Duration::from_millis(300), l.try_recv()).await
    {
        seen.push(n.payload().to_owned());
    }
    for ag in [w.agent, w.agent_ws2] {
        assert!(
            seen.iter()
                .any(|p| p.contains("agent") && p.contains(&ag.to_string())),
            "{seen:?}"
        );
    }

    // The Access tab can say so (C-413): standing conversions, until the owner edits the grant.
    let (s, g) = w
        .call(Method::GET, &format!("/v1/agents/{}/grants", w.agent), None)
        .await;
    assert_eq!(s, 200, "{g}");
    assert_eq!(
        g["converted_by_app"][0]["connection_id"],
        conn.to_string(),
        "{g}"
    );
    assert_eq!(g["converted_by_app"][0]["app_id"], "cust_app");
    let (_, g2) = w
        .call(
            Method::GET,
            &format!("/v1/agents/{}/grants", w.agent2),
            None,
        )
        .await;
    assert_eq!(
        g2["converted_by_app"],
        json!([]),
        "a grant that was never converted"
    );
    let (s, _) = w
        .call(
            Method::PUT,
            &w.put(w.agent_ws2, conn),
            Some(json!({"mode": "custom", "allow_tools": ["get_item"]})),
        )
        .await;
    assert_eq!(s, 200);
    let (_, g3) = w
        .call(
            Method::GET,
            &format!("/v1/agents/{}/grants", w.agent_ws2),
            None,
        )
        .await;
    assert_eq!(
        g3["converted_by_app"],
        json!([]),
        "edited after the conversion"
    );

    // Nothing is converted back when a later version drops the flag, and full is allowed again.
    let v = a.upload(&yaml("1.2.0", "", HOST)).await;
    assert_eq!(v["status"], "active", "{v}");
    assert_eq!(w.grant_row(w.agent, conn).await.0, "custom");
    let (s, _) = w
        .call(
            Method::PUT,
            &w.put(w.agent, conn),
            Some(json!({"mode": "full"})),
        )
        .await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn a_failing_activation_leaves_full_grants_and_audit_untouched() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    // `other` owns `shared.example`; cust 1.1.0 claims it too and introduces custom_only.
    let other = "package: { id: other, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: O }\napps:\n  - about: { id: other_app, name: O }\n    credentials: [ { name: k, secret: true } ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ { name: k, label: k, type: secret, store: credentials, required: true } ]\n    hosts:\n      - match: { host: shared.example }\n        inject:\n          - bearer: \"{credentials.k}\"\n";
    let o = a.upload(other).await;
    assert_eq!(a.act(&o["id"], "approve").await.0, 200);
    install(&a, "1.0.0", "", HOST).await;
    let w = world(&h).await;
    let conn = w.conn().await;
    let (s, _) = w
        .call(
            Method::PUT,
            &w.put(w.agent, conn),
            Some(json!({"mode": "full"})),
        )
        .await;
    assert_eq!(s, 200);
    let v = a
        .upload(&yaml("1.1.0", CUSTOM_ONLY, "shared.example"))
        .await;
    assert_ne!(v["status"], "active", "{v}");
    let (s, e) = a.act(&v["id"], "approve").await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("claim_conflict_unresolved")),
        "{e}"
    );
    // The conversion ran before the snapshot compile failed: it must be gone with the activation.
    assert_eq!(w.grant_row(w.agent, conn).await.0, "full");
    assert_eq!(w.rules(w.agent, conn).await.len(), 1);
    assert!(audit_meta(w.h, "grant.converted_by_app").await.is_empty());
}

/// FIX-064: a `full` grant PUT racing the activation that makes the app `custom_only` never
/// leaves a `full` grant behind: the PUT either sees the new restriction (422) or its grant is
/// written before the activation's conversion, which then converts it. (The grant writer takes
/// the activation's compile lock in shared mode before reading the restrictions.)
#[tokio::test]
async fn a_full_grant_racing_the_custom_only_activation_never_survives_it() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install(&a, "1.0.0", "", HOST).await;
    let w = world(&h).await;
    let conn = w.conn().await;
    // Few racers: the harness pool has 5 connections and each writer holds one while its
    // restriction read takes another.
    let agents = [w.agent, w.agent2];
    // An existing full grant holds the upload for approval (`converts_grants`); the approval,
    // i.e. the activation that converts, races the PUTs.
    let (s, v) = w
        .call(
            Method::PUT,
            &w.put(w.agent_ws2, conn),
            Some(json!({"mode": "full"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let held = a.upload(&yaml("1.1.0", CUSTOM_ONLY, HOST)).await;
    assert_eq!(held["status"], "pending_approval", "{held}");
    let approve = async {
        let (s, b) = a.act(&held["id"], "approve").await;
        assert_eq!(s, 200, "{b}");
    };
    let puts = futures::future::join_all(agents.iter().map(|ag| {
        let w = &w;
        async move {
            let (s, v) = w
                .call(
                    Method::PUT,
                    &w.put(*ag, conn),
                    Some(json!({"mode": "full"})),
                )
                .await;
            assert!(
                s == 200 || (s == 422 && v["error"]["code"] == "full_grant_forbidden"),
                "{s} {v}"
            );
        }
    }));
    tokio::join!(approve, puts);
    let active: String = sqlx::query_scalar(
        "SELECT version FROM plugin_packages WHERE package_id = 'cust' AND status = 'active'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(active, "1.1.0", "the activation completed");
    let fulls: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM grants WHERE connection_id = $1 AND mode = 'full'",
    )
    .bind(conn)
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(fulls, 0, "no full grant on a custom_only app");
}

/// FIX-064: the same race through the *upload* path. With no full grant at the time of the check
/// the version activates automatically; a `full` PUT racing it must still never survive (it is
/// refused, or it is counted and holds the version for approval, which then converts it).
#[tokio::test]
async fn a_full_grant_racing_an_auto_activating_custom_only_upload_never_survives_it() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    install(&a, "1.0.0", "", HOST).await;
    let w = world(&h).await;
    let conn = w.conn().await;
    let agents = [w.agent, w.agent2];
    let upload = async {
        let v = a.upload(&yaml("1.1.0", CUSTOM_ONLY, HOST)).await;
        if v["status"] == "pending_approval" {
            let (s, b) = a.act(&v["id"], "approve").await;
            assert_eq!(s, 200, "{b}");
        }
    };
    let puts = futures::future::join_all(agents.iter().map(|ag| {
        let w = &w;
        async move {
            let (s, v) = w
                .call(
                    Method::PUT,
                    &w.put(*ag, conn),
                    Some(json!({"mode": "full"})),
                )
                .await;
            assert!(
                s == 200 || (s == 422 && v["error"]["code"] == "full_grant_forbidden"),
                "{s} {v}"
            );
        }
    }));
    tokio::join!(upload, puts);
    let active: String = sqlx::query_scalar(
        "SELECT version FROM plugin_packages WHERE package_id = 'cust' AND status = 'active'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(active, "1.1.0", "the upload ended active");
    let fulls: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM grants WHERE connection_id = $1 AND mode = 'full'",
    )
    .bind(conn)
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(fulls, 0, "no full grant on a custom_only app");
}
