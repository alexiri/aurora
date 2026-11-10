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
const CUSTOM_ONLY: &str = "    grants: { custom_only: true }\n";
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
        json!({"custom_only": true, "block_untooled": false})
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
        json!({"custom_only": true, "block_untooled": false})
    );
    // C-504: the bulk response carries them at the top as well.
    assert_eq!(
        v["app_restrictions"],
        json!({"custom_only": true, "block_untooled": false})
    );
    let (s, v) = w
        .call(Method::GET, &format!("/v1/agents/{}/grants", w.agent), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["connections"][0]["app_restrictions"],
        json!({"custom_only": true, "block_untooled": false})
    );
    let (s, v) = w
        .call(Method::GET, &format!("/v1/connections/{conn}/grants"), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["items"][0]["app_restrictions"],
        json!({"custom_only": true, "block_untooled": false})
    );
    // The catalogue and the app detail carry the flags too (C-414).
    let (s, v) = w.call(Method::GET, "/v1/apps/cust_app", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["app_restrictions"],
        json!({"custom_only": true, "block_untooled": false})
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
        json!({"custom_only": true, "block_untooled": false})
    );
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

    let mut l = PgListener::connect_with(w.h.pool()).await.unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();
    let (s, ap) = a.act(&v["id"], "approve").await;
    assert_eq!(s, 200, "{ap}");

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
