#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    owner: String,
    member: String,
    owner_id: Uuid,
    member_id: Uuid,
    /// An org member with no access to the workspace (no `can_use`).
    outsider_id: Uuid,
}

async fn world() -> W {
    let h = Harness::new().await;
    let org = h.org().await;
    let o = h.user("o@x.io", false).await;
    let m = h.user("m@x.io", false).await;
    let x = h.user("x@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    h.member(org, m, "member", "active").await;
    h.member(org, x, "member", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, m, "member").await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    let (_, owner) = h.good_session(o).await;
    let (_, member) = h.good_session(m).await;
    W {
        h,
        org,
        ws,
        agent,
        owner,
        member,
        owner_id: o,
        member_id: m,
        outsider_id: x,
    }
}

impl W {
    async fn call(&self, m: Method, uri: &str, tok: &str, body: Option<Value>) -> (u16, Value) {
        let c = cookie(tok);
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", c.as_str()), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }
    fn wsu(&self, tail: &str) -> String {
        format!("/v1/workspaces/{}/policy{tail}", self.ws)
    }
    fn orgu(&self, tail: &str) -> String {
        format!("/v1/orgs/{}/policy{tail}", self.org)
    }
    async fn put(&self, url: &str, rev: i64, default: &str, rules: Value) -> (u16, Value) {
        self.call(
            Method::PUT,
            url,
            &self.owner,
            Some(json!({"revision": rev, "default_action": default, "rules": rules})),
        )
        .await
    }
    async fn sim(&self, tok: &str, url: &str) -> (u16, Value) {
        self.call(
            Method::POST,
            &self.wsu("/simulate"),
            tok,
            Some(json!({"agent_id": self.agent, "method": "GET", "url": url})),
        )
        .await
    }
    async fn secret(&self, name: &str, org_level: bool) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,$4,'generic','api.example.com','{\"kind\":\"bearer\"}','managed','x:')")
            .bind(id).bind(self.org).bind((!org_level).then_some(self.ws)).bind(name)
            .execute(self.h.pool()).await.unwrap();
        id
    }
}

fn net(host: &str) -> Value {
    json!([{"type": "network", "host": host}])
}

// P2-011A (RFE-011a, SEC-49): `approval_timeout_secs` is bounded at authoring, only valid on
// require-approval rules, round-trips, and simulate shows the timeout a request would get.
#[tokio::test]
async fn approval_timeout_authoring_bounds_and_simulate() {
    let w = world().await;
    let rule = |t: Value, approval: bool| {
        json!([{"name": "ask", "action": "allow", "require_approval": approval,
                "approval_timeout_secs": t, "targets": net("a.example.com")}])
    };
    // Default instance maximum is 3600 s, the minimum 60 s.
    for bad in [json!(59), json!(3601), json!(-5), json!(100_000)] {
        let (s, v) = w
            .put(&w.wsu("/draft"), 0, "none", rule(bad.clone(), true))
            .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some("approval_timeout_out_of_range")),
            "{bad}: {v}"
        );
        assert_eq!(
            v["error"]["details"]["fields"][0]["path"],
            "/rules/0/approval_timeout_secs"
        );
    }
    // A timeout on a rule that does not require approval is refused.
    let (s, v) = w
        .put(&w.wsu("/draft"), 0, "none", rule(json!(600), false))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("policy_invalid")),
        "{v}"
    );
    // Both bounds are inclusive and the value round-trips; absent/null = no preference.
    let (s, v) = w
        .put(&w.wsu("/draft"), 0, "none", rule(json!(60), true))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["rules"][0]["approval_timeout_secs"], 60);
    let (s, v) = w
        .put(&w.wsu("/draft"), 1, "none", rule(json!(3600), true))
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .put(&w.wsu("/draft"), 2, "none", rule(json!(null), true))
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["rules"][0]["approval_timeout_secs"].is_null());
    // Simulate: the draft's timeout, else the instance default (300 s).
    let (s, v) = w
        .put(&w.wsu("/draft"), 3, "none", rule(json!(1200), true))
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(
            Method::POST,
            &w.wsu("/simulate"),
            &w.owner,
            Some(json!({"agent_id": w.agent, "method": "GET", "url": "https://a.example.com/x", "use_draft": true})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["verdict"], "needs_approval");
    assert_eq!(v["approval_timeout_secs"], 1200);
    let (_, v) = w
        .put(&w.wsu("/draft"), 4, "none", rule(json!(null), true))
        .await;
    let _ = v;
    let (_, v) = w
        .call(
            Method::POST,
            &w.wsu("/simulate"),
            &w.owner,
            Some(json!({"agent_id": w.agent, "method": "GET", "url": "https://a.example.com/x", "use_draft": true})),
        )
        .await;
    assert_eq!(v["approval_timeout_secs"], 300);
}

#[tokio::test]
async fn draft_validation_stale_publish_rollback() {
    let w = world().await;
    let (s, v) = w.call(Method::GET, &w.wsu(""), &w.owner, None).await;
    assert_eq!(s, 200);
    assert_eq!(v["published"]["number"], 1);
    assert_eq!(v["draft"]["number"], 2);
    assert_eq!(v["draft"]["draft_revision"], 0);

    let rule = json!([{"name": "r1", "action": "allow", "targets": net("a.example.com")}]);
    let (s, v) = w.put(&w.wsu("/draft"), 0, "none", rule).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["draft_revision"], 1);
    let lid = v["rules"][0]["logical_id"].as_str().unwrap().to_owned();
    // stale
    let (s, v) = w.put(&w.wsu("/draft"), 0, "none", json!([])).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("stale_revision"))
    );
    // invalid patterns / out-of-scope identities and targets
    let bad = json!([{"name": "x", "action": "block", "require_approval": true,
        "identities": [{"kind": "agent", "id": Uuid::now_v7()}],
        "targets": [{"type": "network", "host": "a.*.b.*"},
                    {"type": "network", "host": "ok.com", "path": "nope"}]}]);
    let (s, v) = w.put(&w.wsu("/draft"), 1, "none", bad).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("policy_invalid"))
    );
    let paths: Vec<&str> = v["error"]["details"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"/rules/0/targets/0/host"), "{paths:?}");
    assert!(paths.contains(&"/rules/0/targets/1/path"));
    assert!(paths.contains(&"/rules/0/require_approval"));
    let oos = json!([{"name": "x", "action": "allow",
        "identities": [{"kind": "agent", "id": Uuid::now_v7()}],
        "targets": [{"type": "secret", "id": Uuid::now_v7()}]}]);
    let (s, v) = w.put(&w.wsu("/draft"), 1, "none", oos).await;
    assert_eq!(s, 422);
    let paths: Vec<&str> = v["error"]["details"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"/rules/0/identities/0/id"));
    assert!(paths.contains(&"/rules/0/targets/0/id"));

    // publish: gen 2 published, fresh draft 3, previous superseded; logical_id kept
    let (s, v) = w
        .call(
            Method::POST,
            &w.wsu("/publish"),
            &w.owner,
            Some(json!({"revision": 1})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["published"]["number"], 2);
    assert_eq!(v["published"]["status"], "published");
    assert_eq!(v["published"]["rules"][0]["logical_id"], lid.as_str());
    assert_eq!(v["draft"]["number"], 3);
    assert_eq!(v["draft"]["rules"].as_array().unwrap().len(), 1);
    let (s, v) = w
        .call(
            Method::POST,
            &w.wsu("/publish"),
            &w.owner,
            Some(json!({"revision": 1})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("stale_revision"))
    );
    let (_, v) = w
        .call(Method::GET, &w.wsu("/generations"), &w.owner, None)
        .await;
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert!(
        items
            .iter()
            .any(|g| g["number"] == 1 && g["status"] == "superseded")
    );

    // rollback to gen 1 (empty) publishes a copy as gen 3
    let (s, v) = w
        .call(
            Method::POST,
            &w.wsu("/rollback"),
            &w.owner,
            Some(json!({"generation": 1})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["published"]["number"], 3);
    assert_eq!(v["published"]["rules"].as_array().unwrap().len(), 0);
    assert_eq!(v["draft"]["number"], 4);
    let (s, _) = w
        .call(
            Method::POST,
            &w.wsu("/rollback"),
            &w.owner,
            Some(json!({"generation": 99})),
        )
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn member_cannot_read_simulate_or_edit() {
    // I-27: policy reads, generations, warnings and simulation (whose trace discloses the
    // rules) follow the matrix: workspace `can_own`, org owner/admin.
    let w = world().await;
    let (s, v) = w.call(Method::GET, &w.wsu(""), &w.member, None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("forbidden")),
        "{v}"
    );
    for tail in ["/generations", "/warnings"] {
        let (s, _) = w.call(Method::GET, &w.wsu(tail), &w.member, None).await;
        assert_eq!(s, 403, "ws {tail}");
        let (s, _) = w.call(Method::GET, &w.orgu(tail), &w.member, None).await;
        assert_eq!(s, 403, "org {tail}");
    }
    let (s, _) = w.call(Method::GET, &w.wsu(""), &w.owner, None).await;
    assert_eq!(s, 200);
    let (s, _) = w
        .call(
            Method::PUT,
            &w.wsu("/draft"),
            &w.member,
            Some(json!({"revision": 0, "default_action": "none", "rules": []})),
        )
        .await;
    assert_eq!(s, 403);
    let (s, _) = w
        .call(
            Method::POST,
            &w.wsu("/publish"),
            &w.member,
            Some(json!({"revision": 0})),
        )
        .await;
    assert_eq!(s, 403);
    let (s, _) = w.sim(&w.member, "https://api.example.com/x").await;
    assert_eq!(s, 403);
    let (s, _) = w.sim(&w.owner, "https://api.example.com/x").await;
    assert_eq!(s, 200);
    // org policy: a plain member can neither read nor write
    let (s, _) = w.call(Method::GET, &w.orgu(""), &w.member, None).await;
    assert_eq!(s, 403);
    let (s, _) = w.call(Method::GET, &w.orgu(""), &w.owner, None).await;
    assert_eq!(s, 200);
    let (s, _) = w
        .call(
            Method::PUT,
            &w.orgu("/draft"),
            &w.member,
            Some(json!({"revision": 0, "default_action": "none", "rules": []})),
        )
        .await;
    assert_eq!(s, 403);
}

#[tokio::test]
async fn secret_grant_derived_rule_and_simulate_granted_allow() {
    let w = world().await;
    let sec = w.secret("api-key", false).await;
    // workspace default block: injected requests need an allow
    w.put(&w.wsu("/draft"), 0, "block", json!([])).await;
    w.call(
        Method::POST,
        &w.wsu("/publish"),
        &w.owner,
        Some(json!({"revision": 1})),
    )
    .await;

    let (_, v) = w.sim(&w.owner, "https://api.example.com/x").await;
    assert_eq!(v["candidates"][0]["selected"], false);
    assert_eq!(v["winning_credential"], Value::Null);

    let gurl = format!("/v1/agents/{}/grants/secrets/{sec}", w.agent);
    let (s, v) = w.call(Method::PUT, &gurl, &w.owner, None).await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM derived_rules WHERE source_kind='grant'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
    // member cannot grant
    let (s, _) = w.call(Method::PUT, &gurl, &w.member, None).await;
    assert_eq!(s, 403);
    // grants listing is `can_own` (I-27): members use `/effective` instead
    let (s, _) = w
        .call(
            Method::GET,
            &format!("/v1/agents/{}/grants", w.agent),
            &w.member,
            None,
        )
        .await;
    assert_eq!(s, 403);
    let (_, v) = w
        .call(
            Method::GET,
            &format!("/v1/agents/{}/grants", w.agent),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(v["secrets"].as_array().unwrap().len(), 1);

    let (s, v) = w.sim(&w.owner, "https://api.example.com/x").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["verdict"], "allow");
    assert_eq!(v["winning_credential"]["id"], sec.to_string());
    assert_eq!(
        v["trace"]["workspace"]["considered"][0]["source"],
        "derived"
    );

    let (_, v) = w
        .call(
            Method::GET,
            &format!("/v1/agents/{}/effective", w.agent),
            &w.member,
            None,
        )
        .await;
    assert_eq!(v["credentials"][0]["selected"], true);
    assert_eq!(
        v["credentials"][0]["selected_by"].as_array().unwrap().len(),
        1
    );

    let (s, _) = w.call(Method::DELETE, &gurl, &w.owner, None).await;
    assert_eq!(s, 204);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM derived_rules")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    let (s, _) = w.call(Method::DELETE, &gurl, &w.owner, None).await;
    assert_eq!(s, 404);
    let (_, v) = w.sim(&w.owner, "https://api.example.com/x").await;
    // not injected any more: the default block only applies to credentialed requests
    assert_eq!(v["verdict"], "allow");
    assert_eq!(v["winning_credential"], Value::Null);
}

// RFE-023 (D-66, C-350): a `block` Default Rule does not apply to a credentialed request to a
// governed LLM host; simulate says so with a `matched: false` default entry, the verdict stays null
// and no new top-level reason appears. A non-LLM credentialed request is still default-denied.
#[tokio::test]
async fn rfe023_simulate_reports_default_rule_skipped_for_llm_host() {
    let w = world().await;
    let llm = Uuid::now_v7();
    sqlx::query("INSERT INTO secrets (id, org_id, workspace_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,'ant','llm','anthropic','api_key','anthropic','api.anthropic.com','{\"kind\":\"bearer\"}','managed','x:')")
        .bind(llm).bind(w.org).bind(w.ws).execute(w.h.pool()).await.unwrap();
    let generic = w.secret("api-key", false).await;
    // Allow rules select both credentials (so they are injected) but their body condition is not
    // met: no rule produces a verdict and the Default Rule `block` is what is left to decide.
    let cond = json!([{"on": "body", "op": "contains", "value": "never-present"}]);
    let rules = json!([
        {"name": "llm-if", "action": "allow", "conditions": cond,
         "targets": [{"type": "secret", "id": llm}]},
        {"name": "generic-if", "action": "allow", "conditions": cond,
         "targets": [{"type": "secret", "id": generic}]},
    ]);
    let (s, v) = w.put(&w.wsu("/draft"), 0, "block", rules).await;
    assert_eq!(s, 200, "{v}");
    let (s, _) = w
        .call(
            Method::POST,
            &w.wsu("/publish"),
            &w.owner,
            Some(json!({"revision": 1})),
        )
        .await;
    assert_eq!(s, 200);

    let nil = "00000000-0000-0000-0000-000000000000";
    let (s, v) = w
        .sim(&w.owner, "https://api.anthropic.com/v1/messages")
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["winning_credential"]["id"], llm.to_string(), "{v}");
    let ws_trace = &v["trace"]["workspace"];
    assert_eq!(ws_trace["verdict"], Value::Null, "{v}");
    let last = ws_trace["considered"].as_array().unwrap().last().unwrap();
    assert_eq!(last["rule_logical_id"], nil);
    assert_eq!(last["rule_name"], "Default rule");
    assert_eq!(last["source"], "default");
    assert_eq!(last["matched"], false);
    assert_eq!(last["reason"], "default_rule_skipped_llm_host");
    assert_ne!(v["reason"], "default_denied");
    assert_ne!(v["guidance_code"], "blocked_by_default_policy");
    // the org scope has no Default Rule: no entry there
    assert!(
        v["trace"]["organization"]["considered"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["reason"] != "default_rule_skipped_llm_host")
    );

    // a credentialed non-LLM host is still default-denied, with the ordinary default entry
    let (_, v) = w.sim(&w.owner, "https://api.example.com/x").await;
    assert_eq!(v["verdict"], "block", "{v}");
    assert_eq!(v["reason"], "default_denied");
    let last = v["trace"]["workspace"]["considered"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["reason"], "default_action");
    assert_eq!(last["matched"], true);
}

#[tokio::test]
async fn simulate_org_block_over_workspace_allow_and_egress_default() {
    let w = world().await;
    let (s, v) = w
        .put(
            &w.orgu("/draft"),
            0,
            "none",
            json!([{"name": "no-api", "action": "block", "targets": net("api.example.com")}]),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    w.call(
        Method::POST,
        &w.orgu("/publish"),
        &w.owner,
        Some(json!({"revision": 1})),
    )
    .await;
    let (s, v) = w
        .put(
            &w.wsu("/draft"),
            0,
            "none",
            json!([{"name": "yes-api", "action": "allow", "targets": net("api.example.com")}]),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // draft not yet published: published policy allows nothing special, draft shows the allow
    let (_, v) = w.sim(&w.owner, "https://api.example.com/x").await;
    assert_eq!(v["verdict"], "block");
    w.call(
        Method::POST,
        &w.wsu("/publish"),
        &w.owner,
        Some(json!({"revision": 1})),
    )
    .await;

    let (s, v) = w.sim(&w.owner, "https://api.example.com/x").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["verdict"], "block");
    assert_eq!(v["reason"], "rule");
    assert_eq!(v["trace"]["organization"]["verdict"], "block");
    assert_eq!(v["trace"]["workspace"]["verdict"], "allow");

    // warnings: ws allow fully overridden by the org block
    w.put(
        &w.wsu("/draft"),
        0,
        "none",
        json!([{"name": "yes-api", "action": "allow", "targets": net("api.example.com")}]),
    )
    .await;
    let (_, v) = w
        .call(Method::GET, &w.wsu("/warnings"), &w.owner, None)
        .await;
    assert_eq!(v["items"][0]["code"], "overridden_by_org");

    // egress default: uncredentialed block in the workspace
    let (s, v) = w
        .call(
            Method::PUT,
            &w.wsu("/draft"),
            &w.owner,
            Some(json!({
        "revision": 1, "default_action": "none", "uncredentialed_default": "block",
        "uncredentialed_allowlist": [{"host": "ok.example.org"}], "rules": []})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    w.call(
        Method::POST,
        &w.wsu("/publish"),
        &w.owner,
        Some(json!({"revision": 2})),
    )
    .await;
    let (_, v) = w.sim(&w.owner, "https://other.example.net/x").await;
    assert_eq!(v["verdict"], "block");
    assert_eq!(v["reason"], "egress_denied");
    let (_, v) = w.sim(&w.owner, "https://ok.example.org/x").await;
    assert_eq!(v["verdict"], "allow");
    // use_draft substitutes the draft
    let (_, v) = w.call(Method::POST, &w.wsu("/simulate"), &w.owner,
        Some(json!({"agent_id": w.agent, "method": "GET", "url": "https://other.example.net/", "use_draft": true}))).await;
    assert_eq!(v["reason"], "egress_denied");
    // bad input
    let (s, _) = w
        .call(
            Method::POST,
            &w.wsu("/simulate"),
            &w.owner,
            Some(json!({"agent_id": w.agent, "method": "GET", "url": "nonsense"})),
        )
        .await;
    assert_eq!(s, 400);
    let (s, _) = w
        .call(
            Method::POST,
            &w.wsu("/simulate"),
            &w.owner,
            Some(json!({"agent_id": Uuid::now_v7(), "method": "GET", "url": "https://a.com/"})),
        )
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn warnings_shadowed_rule() {
    let w = world().await;
    let rules = json!([
        {"name": "wide", "action": "allow", "targets": net("*.example.com")},
        {"name": "narrow", "action": "block", "targets": net("api.example.com")}]);
    let (s, v) = w.put(&w.wsu("/draft"), 0, "none", rules).await;
    assert_eq!(s, 200, "{v}");
    let narrow = v["rules"][1]["logical_id"].clone();
    let (s, v) = w
        .call(Method::GET, &w.wsu("/warnings"), &w.owner, None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["items"][0]["code"], "shadowed");
    assert_eq!(v["items"][0]["rule_logical_id"], narrow);
}

/// SEC-32: publishing a policy is audited.
#[tokio::test]
async fn sec32_policy_publish_is_audited() {
    let w = world().await;
    let rule = json!([{"name": "r1", "action": "allow", "targets": net("a.example.com")}]);
    let (s, v) = w.put(&w.wsu("/draft"), 0, "none", rule).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(
            Method::POST,
            &w.wsu("/publish"),
            &w.owner,
            Some(json!({"revision": 1})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let mut n = 0i64;
    for _ in 0..100 {
        n = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='policy.publish'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
        if n > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(n, 1);
}

// ---- P1-003 (RFE-003): workspace open-lane posture and tunnel simulation ----

impl W {
    async fn sim_open(&self, url: &str, use_draft: bool) -> (u16, Value) {
        self.call(
            Method::POST,
            &self.wsu("/simulate"),
            &self.owner,
            Some(
                json!({"agent_id": self.agent, "method": "CONNECT", "url": url,
                        "lane": "open", "use_draft": use_draft}),
            ),
        )
        .await
    }
}

#[tokio::test]
async fn open_lane_posture_is_stored_published_and_workspace_only() {
    let w = world().await;
    let (_, v) = w.call(Method::GET, &w.wsu(""), &w.owner, None).await;
    assert_eq!(v["published"]["open_lane_posture"], "enabled");
    let wr = &w;
    let put = |p: Value, rev: i64| async move {
        let mut b = json!({"revision": rev, "default_action": "none", "rules": []});
        b["open_lane_posture"] = p;
        wr.call(Method::PUT, &wr.wsu("/draft"), &wr.owner, Some(b))
            .await
    };
    let (s, v) = put(json!("allowlist_only"), 0).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["open_lane_posture"], "allowlist_only");
    let (s, v) = put(json!("sometimes"), 1).await;
    assert_eq!(s, 400, "{v}");
    // not published yet: the published generation (what the gateway reads) still says enabled
    let (_, v) = w.call(Method::GET, &w.wsu(""), &w.owner, None).await;
    assert_eq!(v["published"]["open_lane_posture"], "enabled");
    assert_eq!(v["draft"]["open_lane_posture"], "allowlist_only");
    let (s, _) = w
        .call(
            Method::POST,
            &w.wsu("/publish"),
            &w.owner,
            Some(json!({"revision": 1})),
        )
        .await;
    assert_eq!(s, 200);
    let (_, v) = w.call(Method::GET, &w.wsu(""), &w.owner, None).await;
    assert_eq!(v["published"]["open_lane_posture"], "allowlist_only");
    // the posture is carried by the next draft (a copy of the published generation)
    assert_eq!(v["draft"]["open_lane_posture"], "allowlist_only");
    // omitted on a whole-draft replace = enabled
    let (s, v) = w
        .call(
            Method::PUT,
            &w.wsu("/draft"),
            &w.owner,
            Some(json!({"revision": 0, "default_action": "none", "rules": []})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["open_lane_posture"], "enabled");
    // org scope has no posture of its own
    let (s, v) = w
        .call(
            Method::PUT,
            &w.orgu("/draft"),
            &w.owner,
            Some(json!({"revision": 0, "default_action": "none", "rules": [], "open_lane_posture": "disabled"})),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "policy_invalid");
}

#[tokio::test]
async fn simulate_open_lane_applies_posture_and_connect_time_rules() {
    let w = world().await;
    // path-scoped block: the request lane lets other paths through, the tunnel is refused
    let (s, v) = w
        .put(
            &w.wsu("/draft"),
            0,
            "none",
            json!([{"name": "admin", "action": "block",
                    "targets": [{"type": "network", "host": "api.example.com", "path": "/admin/**"}]}]),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = w.sim_open("https://api.example.com", true).await;
    assert_eq!(v["verdict"], "block", "{v}");
    assert_eq!(v["reason"], "rule");
    assert_eq!(v["guidance_code"], "blocked_by_policy");
    assert_eq!(v["winning_credential"], Value::Null);
    let (_, v) = w
        .call(
            Method::POST,
            &w.wsu("/simulate"),
            &w.owner,
            Some(json!({"agent_id": w.agent, "method": "GET", "url": "https://api.example.com/ok", "use_draft": true})),
        )
        .await;
    assert_eq!(v["verdict"], "allow", "request lane unchanged");
    // posture: disabled in the draft
    let (s, v) = w
        .call(
            Method::PUT,
            &w.wsu("/draft"),
            &w.owner,
            Some(json!({"revision": 1, "default_action": "none", "rules": [],
                        "open_lane_posture": "disabled"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = w.sim_open("https://api.example.com", true).await;
    assert_eq!(v["verdict"], "block");
    assert_eq!(v["reason"], "open_lane");
    assert_eq!(v["guidance_code"], "open_lane_disabled");
    // published posture is still enabled
    let (_, v) = w.sim_open("https://api.example.com", false).await;
    assert_eq!(v["verdict"], "allow");
    // allowlist_only with and without a covering entry; a path entry never admits a tunnel
    let wr = &w;
    let put = |list: Value, rev: i64| async move {
        wr.call(
            Method::PUT,
            &wr.wsu("/draft"),
            &wr.owner,
            Some(
                json!({"revision": rev, "default_action": "none", "rules": [],
                        "open_lane_posture": "allowlist_only", "uncredentialed_allowlist": list}),
            ),
        )
        .await
    };
    let (_, v) = put(json!([]), 2).await;
    assert_eq!(v["open_lane_posture"], "allowlist_only");
    let (_, v) = w.sim_open("https://api.example.com", true).await;
    assert_eq!(
        (v["verdict"].as_str(), v["reason"].as_str()),
        (Some("block"), Some("egress_denied")),
        "{v}"
    );
    assert_eq!(v["guidance_code"], "blocked_by_egress_policy");
    put(json!([{"host": "api.example.com", "path": "/**"}]), 3).await;
    let (_, v) = w.sim_open("https://api.example.com", true).await;
    assert_eq!(v["verdict"], "block");
    put(json!([{"host": "api.example.com"}]), 4).await;
    let (_, v) = w.sim_open("https://api.example.com", true).await;
    assert_eq!(v["verdict"], "allow", "{v}");
    // input: the open lane takes https://host[:port]
    for bad in [
        "http://api.example.com",
        "https://api.example.com/x",
        "https://api.example.com/?q=1",
    ] {
        let (s, v) = w.sim_open(bad, true).await;
        assert_eq!(s, 400, "{bad}: {v}");
    }
    // members cannot simulate
    let (s, _) = w
        .call(
            Method::POST,
            &w.wsu("/simulate"),
            &w.member,
            Some(json!({"agent_id": w.agent, "method": "CONNECT", "url": "https://a.com", "lane": "open"})),
        )
        .await;
    assert_eq!(s, 403);
}

// ---- (phase 2, RFE-005, SEC-44) speaker binding in simulate, effective and warnings ----------

impl W {
    async fn sim_as(&self, speaker: Option<Uuid>, lane: Option<&str>) -> (u16, Value) {
        let mut body = json!({"agent_id": self.agent, "method": "GET",
                              "url": "https://api.example.com/x", "use_draft": true});
        if let Some(s) = speaker {
            body["speaker_user_id"] = json!(s);
        }
        if let Some(l) = lane {
            body["lane"] = json!(l);
            body["method"] = json!("CONNECT");
            body["url"] = json!("https://api.example.com");
        }
        self.call(
            Method::POST,
            &self.wsu("/simulate"),
            &self.owner,
            Some(body),
        )
        .await
    }

    async fn effective(&self, tok: &str, query: &str) -> (u16, Value) {
        self.call(
            Method::GET,
            &format!("/v1/agents/{}/effective{query}", self.agent),
            tok,
            None,
        )
        .await
    }

    /// Publishes a workspace policy of `rules`.
    async fn publish(&self, rules: Value) {
        let (s, v) = self.put(&self.wsu("/draft"), 0, "none", rules).await;
        assert_eq!(s, 200, "{v}");
        let (s, v) = self
            .call(
                Method::POST,
                &self.wsu("/publish"),
                &self.owner,
                Some(json!({"revision": 1})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }
}

#[tokio::test]
async fn rfe005_simulate_binds_user_rules_to_the_speaker() {
    let w = world().await;
    let (s, v) = w
        .put(
            &w.wsu("/draft"),
            0,
            "none",
            json!([{"name": "no-member", "action": "block",
                    "identities": [{"kind": "user", "id": w.member_id}],
                    "targets": net("api.example.com")}]),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = w.sim_as(Some(w.member_id), None).await;
    assert_eq!(v["verdict"], "block", "the named speaker is blocked: {v}");
    let (_, v) = w.sim_as(Some(w.owner_id), None).await;
    assert_eq!(v["verdict"], "allow", "another speaker: {v}");
    let (_, v) = w.sim_as(None, None).await;
    assert_eq!(
        v["verdict"], "allow",
        "no speaker never matches a user rule: {v}"
    );
    // Tunnels carry no speaker, so a user rule never matches them either.
    let (_, v) = w.sim_as(Some(w.member_id), Some("open")).await;
    assert_eq!(v["verdict"], "allow", "open lane: {v}");
    // A speaker without `can_use` cannot occur in the gateway: refused with a field error.
    let (s, v) = w.sim_as(Some(w.outsider_id), None).await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(
        v["error"]["details"]["fields"][0]["path"],
        "/speaker_user_id"
    );
}

#[tokio::test]
async fn rfe005_simulate_user_allow_rule_selects_credential_only_for_its_speaker() {
    let w = world().await;
    let sec = w.secret("api-key", false).await;
    w.put(
        &w.wsu("/draft"),
        0,
        "none",
        json!([{"name": "member-key", "action": "allow",
                "identities": [{"kind": "user", "id": w.member_id}],
                "targets": [{"type": "secret", "id": sec}]}]),
    )
    .await;
    let (_, v) = w.sim_as(Some(w.member_id), None).await;
    assert_eq!(v["winning_credential"]["id"], sec.to_string(), "{v}");
    for speaker in [None, Some(w.owner_id)] {
        let (_, v) = w.sim_as(speaker, None).await;
        assert_eq!(v["winning_credential"], Value::Null, "{speaker:?}: {v}");
    }
}

#[tokio::test]
async fn rfe005_effective_view_takes_a_speaker() {
    let w = world().await;
    let sec = w.secret("api-key", false).await;
    w.publish(json!([{"name": "member-key", "action": "allow",
                      "identities": [{"kind": "user", "id": w.member_id}],
                      "targets": [{"type": "secret", "id": sec}]}]))
        .await;
    let selected = |v: &Value| v["credentials"][0]["selected"].as_bool().unwrap();
    // Default: the caller.
    let (s, v) = w.effective(&w.member, "").await;
    assert_eq!(s, 200, "{v}");
    assert!(selected(&v), "the member's own view: {v}");
    assert_eq!(v["credentials"][0]["depends_on_speaker"], true);
    let (_, v) = w.effective(&w.owner, "").await;
    assert!(!selected(&v), "the owner is not the named person: {v}");
    assert_eq!(v["credentials"][0]["depends_on_speaker"], true);
    // Explicit speakers.
    let (_, v) = w
        .effective(&w.owner, &format!("?speaker={}", w.member_id))
        .await;
    assert!(selected(&v), "{v}");
    let (_, v) = w.effective(&w.member, "?speaker=none").await;
    assert!(!selected(&v), "speakerless traffic: {v}");
    // Invalid speakers: not a uuid, or no `can_use`.
    for bad in ["garbage".to_owned(), w.outsider_id.to_string()] {
        let (s, v) = w.effective(&w.owner, &format!("?speaker={bad}")).await;
        assert_eq!(s, 400, "{bad}: {v}");
        assert_eq!(v["error"]["details"]["fields"][0]["path"], "/speaker");
    }
}

#[tokio::test]
async fn rfe005_warnings_flag_user_rules() {
    let w = world().await;
    let sec = w.secret("api-key", false).await;
    let (s, v) = w
        .put(
            &w.wsu("/draft"),
            0,
            "none",
            json!([
                {"name": "only-user", "action": "allow",
                 "identities": [{"kind": "user", "id": w.member_id}],
                 "targets": [{"type": "secret", "id": sec}]},
                {"name": "user-or-agent", "action": "allow",
                 "identities": [{"kind": "user", "id": w.member_id},
                                {"kind": "agent", "id": w.agent}],
                 "targets": [{"type": "secret", "id": sec}]},
                {"name": "user-block", "action": "block",
                 "identities": [{"kind": "user", "id": w.member_id}],
                 "targets": net("api.example.com")}]),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let id_of = |i: usize| v["rules"][i]["logical_id"].clone();
    let (s, warn) = w
        .call(Method::GET, &w.wsu("/warnings"), &w.owner, None)
        .await;
    assert_eq!(s, 200, "{warn}");
    let items = warn["items"].as_array().unwrap();
    let codes = |code: &str| -> Vec<Value> {
        items
            .iter()
            .filter(|i| i["code"] == code)
            .map(|i| i["rule_logical_id"].clone())
            .collect()
    };
    assert_eq!(
        codes("user_rule_needs_speaker"),
        vec![id_of(0), id_of(1), id_of(2)],
        "{warn}"
    );
    assert_eq!(
        codes("user_rule_no_agent_fallback"),
        vec![id_of(0)],
        "only the user-only allow rule that grants credentials: {warn}"
    );
    let msg = items
        .iter()
        .find(|i| i["code"] == "user_rule_needs_speaker")
        .unwrap()["message"]
        .as_str()
        .unwrap();
    assert!(msg.contains("only turns spoken by"), "{msg}");
}
