//! Credential-side bulk grants (FIX-020, D-137, C-503..C-505, SEC-77): the bulk routes run the
//! same service function as the agent-side `PUT`, all-or-nothing, idempotent, audited.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use sqlx::postgres::PgListener;
use uuid::Uuid;

struct W {
    h: Harness,
    org: Uuid,
    ws1: Uuid,
    ws2: Uuid,
    /// Org admin: owns every workspace.
    admin: String,
    /// Owner of `ws1` only.
    owner1: String,
    /// Owner of `ws1`, plain member of `ws2` (sees it, cannot own it).
    owner1_member2: String,
    a1: Uuid,
    a1b: Uuid,
    a2: Uuid,
}

async fn agent(h: &Harness, org: Uuid, ws: Uuid, handle: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,$4,$5,'byo','medium','')")
        .bind(id).bind(org).bind(ws).bind(handle).bind(format!("Agent {handle}"))
        .execute(h.pool()).await.unwrap();
    id
}

async fn world() -> W {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws1 = h.workspace(org).await;
    let ws2 = h.workspace(org).await;
    let adm = h.user("adm@x.io", false).await;
    h.member(org, adm, "admin", "active").await;
    let o1 = h.user("o1@x.io", false).await;
    h.member(org, o1, "member", "active").await;
    h.bind_ws(ws1, o1, "owner").await;
    let o1m = h.user("o1m@x.io", false).await;
    h.member(org, o1m, "member", "active").await;
    h.bind_ws(ws1, o1m, "owner").await;
    h.bind_ws(ws2, o1m, "member").await;
    let (_, at) = h.good_session(adm).await;
    let (_, ot) = h.good_session(o1).await;
    let (_, omt) = h.good_session(o1m).await;
    let a1 = agent(&h, org, ws1, "one").await;
    let a1b = agent(&h, org, ws1, "oneb").await;
    let a2 = agent(&h, org, ws2, "two").await;
    W {
        h,
        org,
        ws1,
        ws2,
        admin: cookie(&at),
        owner1: cookie(&ot),
        owner1_member2: cookie(&omt),
        a1,
        a1b,
        a2,
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

    async fn secret(&self, scope: Option<Uuid>, name: &str) -> Uuid {
        let uri = match scope {
            Some(ws) => format!("/v1/workspaces/{ws}/secrets"),
            None => format!("/v1/orgs/{}/secrets", self.org),
        };
        let (s, v) = self
            .call(
                &self.admin,
                Method::POST,
                &uri,
                Some(json!({"name": name, "kind": "generic", "host_pattern": "api.example.com", "value": "v"})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v["id"].as_str().unwrap().parse().unwrap()
    }

    /// Installs the package of `acme_app` (FIX-084, SEC-67): an app the active snapshot does not
    /// hold is `custom_only`, so a test that grants `full` needs its app to be there.
    async fn install_acme(&self) {
        let yaml = r#"package: { id: acme, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: Acme }
apps:
  - about: { id: acme_app, name: Acme }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    hosts:
      - match: { host: api.acme.example }
        inject:
          - bearer: "{credentials.api_token}"
"#;
        let a = crate::plugins_lifecycle::setup(&self.h).await;
        let v = a.upload(yaml).await;
        let v = if v["status"] == "pending_approval" {
            let (s, b) = a.act(&v["id"], "approve").await;
            assert_eq!(s, 200, "{b}");
            b
        } else {
            v
        };
        assert_eq!(v["status"], "active", "{v}");
    }

    async fn conn(&self, ws: Option<Uuid>) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'acme_app','acme','1.0.0','c','active','x')")
            .bind(id).bind(self.org).bind(ws).execute(self.h.pool()).await.unwrap();
        id
    }

    async fn count(&self, table_cond: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM grants WHERE {table_cond}"))
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn grants_of(&self, col: &str, id: Uuid) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM grants WHERE {col} = $1"))
            .bind(id)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn audit(&self) -> Vec<Value> {
        sqlx::query_scalar::<_, Value>(
            "SELECT metadata FROM audit_log WHERE org_id = $1 AND action = 'grant.create' \
             AND critical ORDER BY created_at, id",
        )
        .bind(self.org)
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }
}

fn ids(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["agent_id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn org_secret_to_agents_of_two_owned_workspaces_is_idempotent_and_audited() {
    let w = world().await;
    let secret = w.secret(None, "org-key").await;
    let u = format!("/v1/secrets/{secret}/grants");
    let mut l = PgListener::connect_with(w.h.pool()).await.unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();

    let body = json!({"agent_ids": [w.a1, w.a2]});
    let (s, v) = w.call(&w.admin, Method::POST, &u, Some(body.clone())).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(ids(&v), vec![w.a1.to_string(), w.a2.to_string()]);
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["status"] == "created" && i["grant"]["secret_id"] == json!(secret))
    );
    assert_eq!(w.grants_of("secret_id", secret).await, 2);

    // gateway invalidation fires once per created grant, after commit
    let mut seen = Vec::new();
    for _ in 0..2 {
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), l.recv())
            .await
            .expect("notify")
            .unwrap();
        seen.push(n.payload().to_string());
    }
    assert!(seen.iter().any(|p| p.contains(&w.a1.to_string())));
    assert!(seen.iter().any(|p| p.contains(&w.a2.to_string())));

    // one audit row per created grant, via = credential
    let a = w.audit().await;
    assert_eq!(a.len(), 2, "{a:?}");
    assert!(
        a.iter()
            .all(|m| m["via"] == "credential" && m["secret_id"] == json!(secret))
    );

    // second identical POST: everything existing, same grant ids, no new audit row
    let first: Vec<Value> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["grant"]["id"].clone())
        .collect();
    let (s, v2) = w.call(&w.admin, Method::POST, &u, Some(body)).await;
    assert_eq!(s, 200, "{v2}");
    assert!(
        v2["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["status"] == "existing")
    );
    let second: Vec<Value> = v2["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["grant"]["id"].clone())
        .collect();
    assert_eq!(first, second);
    assert_eq!(
        w.audit().await.len(),
        2,
        "existing grants are not audited again"
    );
    assert_eq!(w.grants_of("secret_id", secret).await, 2);

    // a mixed request: existing + new
    let (s, v3) = w
        .call(
            &w.admin,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1, w.a1b]})),
        )
        .await;
    assert_eq!(s, 200, "{v3}");
    assert_eq!(v3["items"][0]["status"], "existing");
    assert_eq!(v3["items"][1]["status"], "created");
    assert_eq!(w.audit().await.len(), 3);
}

#[tokio::test]
async fn one_failing_agent_rolls_back_the_whole_request() {
    let w = world().await;
    let secret = w.secret(None, "org-key").await;
    let u = format!("/v1/secrets/{secret}/grants");

    // owner of ws1 only, not even a member of ws2... here a member of ws2 (visible, not owner)
    let (s, v) = w
        .call(
            &w.owner1_member2,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1, w.a2]})),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(v["error"]["code"], "forbidden");
    assert_eq!(v["error"]["details"]["agent_id"], json!(w.a2));
    assert_eq!(w.grants_of("secret_id", secret).await, 0, "nothing written");
    assert!(w.audit().await.is_empty(), "no audit row either");

    // an agent of a workspace the caller cannot see at all: 404
    let (s, v) = w
        .call(
            &w.owner1,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1, w.a2]})),
        )
        .await;
    assert_eq!(s, 404, "{v}");
    assert_eq!(v["error"]["details"]["agent_id"], json!(w.a2));
    assert_eq!(w.grants_of("secret_id", secret).await, 0);
    assert!(w.audit().await.is_empty());

    // an unknown agent: 404 with its id
    let ghost = Uuid::now_v7();
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1, ghost]})),
        )
        .await;
    assert_eq!(s, 404, "{v}");
    assert_eq!(v["error"]["details"]["agent_id"], json!(ghost));
    assert_eq!(w.grants_of("secret_id", secret).await, 0);

    // the same owner can grant the org secret to their own agents
    let (s, v) = w
        .call(
            &w.owner1,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1, w.a1b]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.grants_of("secret_id", secret).await, 2);
}

#[tokio::test]
async fn workspace_secret_is_refused_for_an_agent_of_another_workspace() {
    let w = world().await;
    let secret = w.secret(Some(w.ws1), "ws1-key").await;
    let u = format!("/v1/secrets/{secret}/grants");
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1, w.a2]})),
        )
        .await;
    assert_eq!(s, 404, "{v}");
    assert_eq!(v["error"]["details"]["agent_id"], json!(w.a2));
    assert_eq!(w.grants_of("secret_id", secret).await, 0, "nothing written");
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1, w.a1b]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // a proposed (not active) secret is not grantable either
    sqlx::query("UPDATE secrets SET status = 'proposed' WHERE id = $1")
        .bind(secret)
        .execute(w.h.pool())
        .await
        .unwrap();
    let before = w.grants_of("secret_id", secret).await;
    let other = w.secret(Some(w.ws1), "ws1-key2").await;
    sqlx::query("UPDATE secrets SET status = 'proposed' WHERE id = $1")
        .bind(other)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, _) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/secrets/{other}/grants"),
            Some(json!({"agent_ids": [w.a1]})),
        )
        .await;
    assert_eq!(s, 404);
    assert_eq!(before, 2);
    assert_eq!(w.grants_of("secret_id", other).await, 0);
}

#[tokio::test]
async fn bulk_size_and_duplicates_are_rejected_with_400() {
    let w = world().await;
    let secret = w.secret(None, "org-key").await;
    let conn = w.conn(None).await;
    for (uri, extra) in [
        (format!("/v1/secrets/{secret}/grants"), json!({})),
        (
            format!("/v1/connections/{conn}/grants"),
            json!({"mode": "full"}),
        ),
    ] {
        let mut cases = vec![
            json!([]),
            json!((0..51).map(|_| Uuid::now_v7()).collect::<Vec<_>>()),
            json!([w.a1, w.a1]),
        ];
        // exactly GRANT_BULK_MAX passes validation (unknown agents then 404)
        let fifty = (0..50).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
        for agent_ids in cases.drain(..) {
            let mut body = extra.clone();
            body["agent_ids"] = agent_ids;
            let (s, v) = w.call(&w.admin, Method::POST, &uri, Some(body)).await;
            assert_eq!(s, 400, "{uri}: {v}");
        }
        let mut body = extra.clone();
        body["agent_ids"] = json!(fifty);
        let (s, v) = w.call(&w.admin, Method::POST, &uri, Some(body)).await;
        assert_eq!(s, 404, "50 agents is within the limit: {v}");
    }
    assert_eq!(w.count("true").await, 0);
}

#[tokio::test]
async fn connection_bulk_grants_check_like_the_agent_side_and_never_modify_existing() {
    let w = world().await;
    w.install_acme().await;
    let conn = w.conn(None).await;
    let u = format!("/v1/connections/{conn}/grants");

    // custom validation is the agent-side validation, reported with the agent
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1, w.a2], "mode": "custom"})),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "grant_invalid");
    assert_eq!(v["error"]["details"]["reason"], "empty_custom");
    assert_eq!(v["error"]["details"]["agent_id"], json!(w.a1));
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1], "mode": "custom", "allow_tools": ["x"], "ask_tools": ["x"]})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["details"]["reason"].as_str()),
        (422, Some("tool_in_both")),
        "{v}"
    );
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1], "mode": "full", "ask_timeout_secs": 120})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["details"]["reason"].as_str()),
        (422, Some("ask_timeout_without_ask_tools")),
        "{v}"
    );
    assert_eq!(
        w.grants_of("connection_id", conn).await,
        0,
        "nothing written"
    );

    // an existing custom grant on a2 stays exactly as it was
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode, allow_tools, ask_tools) VALUES ($1,$2,$3,$4,'custom','{get_item}','{}')")
        .bind(Uuid::now_v7()).bind(w.ws2).bind(w.a2).bind(conn)
        .execute(w.h.pool()).await.unwrap();
    // mixed batch with a forbidden agent: nothing written
    let (s, v) = w
        .call(
            &w.owner1_member2,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1, w.a2], "mode": "full"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["details"]["agent_id"].clone()),
        (403, json!(w.a2)),
        "{v}"
    );
    assert_eq!(w.grants_of("connection_id", conn).await, 1);

    let mut l = PgListener::connect_with(w.h.pool()).await.unwrap();
    l.listen("aurora_gateway_invalidate").await.unwrap();
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1, w.a2, w.a1b], "mode": "full"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let st: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["status"].as_str().unwrap())
        .collect();
    assert_eq!(st, vec!["created", "existing", "created"]);
    assert_eq!(v["items"][0]["grant"]["mode"], "full");
    assert_eq!(v["items"][1]["grant"]["mode"], "custom");
    assert_eq!(v["items"][1]["grant"]["allow_tools"], json!(["get_item"]));
    let (mode, allow): (String, Vec<String>) = sqlx::query_as(
        "SELECT mode, allow_tools FROM grants WHERE agent_id = $1 AND connection_id = $2",
    )
    .bind(w.a2)
    .bind(conn)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (mode.as_str(), allow),
        ("custom", vec!["get_item".to_string()])
    );
    // invalidation for created grants only; derived rules for created grants only
    let mut seen = Vec::new();
    for _ in 0..2 {
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), l.recv())
            .await
            .expect("notify")
            .unwrap();
        seen.push(n.payload().to_string());
    }
    assert!(seen.iter().any(|p| p.contains(&w.a1.to_string())));
    assert!(seen.iter().any(|p| p.contains(&w.a1b.to_string())));
    let rules: i64 = sqlx::query_scalar("SELECT count(*) FROM derived_rules WHERE source_kind='grant' AND source_id IN (SELECT id FROM grants WHERE connection_id = $1)")
        .bind(conn).fetch_one(w.h.pool()).await.unwrap();
    assert_eq!(rules, 2);

    let a = w.audit().await;
    assert_eq!(a.len(), 2, "{a:?}");
    assert!(
        a.iter()
            .all(|m| m["via"] == "credential" && m["connection_id"] == json!(conn))
    );
    assert!(a.iter().all(|m| m["grant"]["mode"] == "full"));

    // idempotent
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &u,
            Some(json!({"agent_ids": [w.a1, w.a2, w.a1b], "mode": "full"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["status"] == "existing")
    );
    assert_eq!(w.audit().await.len(), 2);
}

#[tokio::test]
async fn lists_show_only_agents_of_workspaces_the_caller_can_own() {
    let w = world().await;
    w.install_acme().await;
    let secret = w.secret(None, "org-key").await;
    let conn = w.conn(None).await;
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/secrets/{secret}/grants"),
            Some(json!({"agent_ids": [w.a1, w.a2]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/connections/{conn}/grants"),
            Some(json!({"agent_ids": [w.a1, w.a2], "mode": "full"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    for uri in [
        format!("/v1/secrets/{secret}/grants"),
        format!("/v1/connections/{conn}/grants"),
    ] {
        let (s, v) = w.call(&w.admin, Method::GET, &uri, None).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["items"].as_array().unwrap().len(), 2);
        // owner of ws1 only (and plain member of ws2 for owner1_member2): only ws1's agent
        for who in [&w.owner1, &w.owner1_member2] {
            let (s, v) = w.call(who, Method::GET, &uri, None).await;
            assert_eq!(s, 200, "{v}");
            let items = v["items"].as_array().unwrap();
            assert_eq!(items.len(), 1, "{v}");
            assert_eq!(items[0]["agent_id"], json!(w.a1));
            assert_eq!(items[0]["agent_name"], "Agent one");
            assert_eq!(items[0]["workspace_id"], json!(w.ws1));
            assert!(items[0]["workspace_name"].is_string());
        }
    }
}

#[tokio::test]
async fn agent_side_put_goes_through_the_same_path_and_is_unchanged() {
    let w = world().await;
    let secret = w.secret(None, "org-key").await;
    let uri = format!("/v1/agents/{}/grants/secrets/{secret}", w.a1);
    // a member who cannot own the workspace is refused exactly like the bulk route
    let (s, _) = w
        .call(
            &w.owner1_member2,
            Method::PUT,
            &format!("/v1/agents/{}/grants/secrets/{secret}", w.a2),
            None,
        )
        .await;
    assert_eq!(s, 403);
    let (s, v) = w.call(&w.owner1, Method::PUT, &uri, None).await;
    assert_eq!(s, 200, "{v}");
    // replaying the PUT keeps being audited without `via`
    let (s, _) = w.call(&w.owner1, Method::PUT, &uri, None).await;
    assert_eq!(s, 200);
    let a = w.audit().await;
    assert_eq!(a.len(), 2);
    assert!(a.iter().all(|m| m.get("via").is_none()));
}
