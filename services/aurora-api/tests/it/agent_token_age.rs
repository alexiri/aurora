#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BYO agent-token maximum age and use tracking (phase 3, RFE-013, D-60, D-107, SEC-53,
//! C-310..C-317): the effective maximum, mint and regenerate, the retrofit with its grace, the
//! API refusal, `last_used_at`, and the stale / expiring lists (`18 §BYO token age`).

use crate::common::*;
use chrono::{DateTime, Duration, Utc};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    owner: String,
    member: String,
}

async fn world_with(instance_days: Option<u32>) -> W {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| {
        c.gateway_ca_dir = d;
        c.governance.agent_token_max_age_days = instance_days;
    })
    .await;
    let org = h.org().await;
    let o = h.user("o@x.io", false).await;
    let m = h.user("m@x.io", false).await;
    h.member(org, o, "owner", "active").await;
    h.member(org, m, "member", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, m, "member").await;
    let (_, owner) = h.good_session(o).await;
    let (_, member) = h.good_session(m).await;
    W {
        h,
        org,
        ws,
        owner,
        member,
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

fn ts(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str().map(|s| s.parse().unwrap())
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
    async fn bearer(&self, uri: &str, tok: &str) -> (u16, Value) {
        let b = bearer(tok);
        let (s, _, v) = self
            .h
            .send(Method::GET, uri, &[("authorization", b.as_str())], None)
            .await;
        (s.as_u16(), v)
    }
    /// Creates a BYO agent as the owner; `(agent id, token, expires_at)`.
    async fn byo(&self, name: &str) -> (Uuid, String, Option<DateTime<Utc>>) {
        let (s, v) = self
            .call(
                Method::POST,
                &format!("/v1/workspaces/{}/agents", self.ws),
                &self.owner,
                Some(json!({"name": name, "kind": "byo"})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        (
            v["agent"]["id"].as_str().unwrap().parse().unwrap(),
            v["token"].as_str().unwrap().to_owned(),
            ts(&v["expires_at"]),
        )
    }
    async fn gov_get(&self) -> Value {
        self.call(
            Method::GET,
            &format!("/v1/orgs/{}/governance", self.org),
            &self.owner,
            None,
        )
        .await
        .1
    }
    async fn gov_put(&self, doc: Value) -> (u16, Value) {
        let cur = self.gov_get().await;
        self.call(
            Method::PUT,
            &format!("/v1/orgs/{}/governance", self.org),
            &self.owner,
            Some(json!({"document": doc, "revision": cur["revision"]})),
        )
        .await
    }
    async fn set_days(&self, days: Option<i64>) -> (u16, Value) {
        let doc = match days {
            Some(d) => json!({"agent_tokens": {"max_age_days": d}}),
            None => json!({}),
        };
        self.gov_put(doc).await
    }
    async fn expiry(&self, agent: Uuid) -> Option<DateTime<Utc>> {
        sqlx::query_scalar(
            "SELECT expires_at FROM gateway_credentials
              WHERE agent_id = $1 AND kind = 'agent' AND revoked_at IS NULL",
        )
        .bind(agent)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }
    async fn sql(&self, q: &str, agent: Uuid) {
        sqlx::query(q)
            .bind(agent)
            .execute(self.h.pool())
            .await
            .unwrap();
    }
}

fn near(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    (a - b).num_seconds().abs() < 120
}

// ---- effective maximum age -------------------------------------------------------------

#[tokio::test]
async fn effective_maximum_none_instance_org_both_and_bounds() {
    // None by default: no expiry, the effective view says so.
    let w = world_with(None).await;
    let eff = w
        .call(
            Method::GET,
            &format!("/v1/orgs/{}/governance/effective", w.org),
            &w.owner,
            None,
        )
        .await
        .1;
    assert_eq!(eff["agent_tokens"]["max_age_days"]["value"], Value::Null);
    assert_eq!(eff["agent_tokens"]["max_age_days"]["source"], "instance");
    let (_, _, exp) = w.byo("A").await;
    assert!(exp.is_none());

    // Org only.
    let (s, v) = w.set_days(Some(10)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["document"]["agent_tokens"]["max_age_days"], 10);
    let eff = w
        .call(
            Method::GET,
            &format!("/v1/orgs/{}/governance/effective", w.org),
            &w.owner,
            None,
        )
        .await
        .1;
    assert_eq!(eff["agent_tokens"]["max_age_days"]["value"], 10);
    assert_eq!(eff["agent_tokens"]["max_age_days"]["source"], "org");
    let (_, _, exp) = w.byo("B").await;
    assert!(near(exp.unwrap(), Utc::now() + Duration::days(10)));

    // Removing the org value drops the limit for new tokens.
    assert_eq!(w.set_days(None).await.0, 200);
    let (_, _, exp) = w.byo("C").await;
    assert!(exp.is_none());

    // Bounds and types.
    for bad in [json!(0), json!(3651), json!(-1), json!(1.5), json!("7")] {
        let (s, v) = w
            .gov_put(json!({"agent_tokens": {"max_age_days": bad}}))
            .await;
        assert_eq!((s, code(&v)), (422, "governance_invalid"), "{bad}: {v}");
        assert_eq!(
            v["error"]["details"]["fields"][0]["path"],
            "agent_tokens.max_age_days"
        );
    }
    assert_eq!(w.set_days(Some(1)).await.0, 200);
    assert_eq!(w.set_days(Some(3650)).await.0, 200);
    // An empty object is omitted from the stored document.
    let (s, v) = w.gov_put(json!({"agent_tokens": {}})).await;
    assert_eq!(s, 200);
    assert!(v["document"].get("agent_tokens").is_none(), "{v}");
}

#[tokio::test]
async fn org_tighter_than_instance_wins_and_org_above_instance_is_refused() {
    let w = world_with(Some(30)).await;
    let inst = w
        .call(Method::GET, "/v1/governance/instance", &w.owner, None)
        .await
        .1;
    assert_eq!(inst["agent_tokens"]["max_age_days"], 30);
    let eff = |org: Uuid| format!("/v1/orgs/{org}/governance/effective");
    let e = w.call(Method::GET, &eff(w.org), &w.owner, None).await.1;
    assert_eq!(e["agent_tokens"]["max_age_days"]["value"], 30);
    assert_eq!(e["agent_tokens"]["max_age_days"]["source"], "instance");
    let (_, _, exp) = w.byo("A").await;
    assert!(near(exp.unwrap(), Utc::now() + Duration::days(30)));

    let (s, v) = w.set_days(Some(31)).await;
    assert_eq!(
        (s, code(&v)),
        (422, "governance_below_instance_minimum"),
        "{v}"
    );
    assert_eq!(
        v["error"]["details"]["fields"][0]["path"],
        "agent_tokens.max_age_days"
    );
    assert_eq!(w.set_days(Some(30)).await.0, 200);

    assert_eq!(w.set_days(Some(7)).await.0, 200);
    let e = w.call(Method::GET, &eff(w.org), &w.owner, None).await.1;
    assert_eq!(e["agent_tokens"]["max_age_days"]["value"], 7);
    assert_eq!(e["agent_tokens"]["max_age_days"]["source"], "org");
    let (_, _, exp) = w.byo("B").await;
    assert!(near(exp.unwrap(), Utc::now() + Duration::days(7)));
}

// ---- mint, regenerate, context cap -----------------------------------------------------------

#[tokio::test]
async fn mint_and_regenerate_set_expires_at_and_context_credentials_are_capped() {
    let w = world_with(Some(30)).await;
    let (agent, token, exp) = w.byo("A").await;
    let exp = exp.unwrap();
    assert!(near(exp, Utc::now() + Duration::days(30)));
    assert_eq!(w.expiry(agent).await.unwrap(), exp);

    // Regenerate returns the new `expires_at`.
    let (s, v) = w
        .call(
            Method::POST,
            &format!("/v1/agents/{agent}/token"),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(near(
        ts(&v["expires_at"]).unwrap(),
        Utc::now() + Duration::days(30)
    ));
    let new_token = v["token"].as_str().unwrap().to_owned();
    assert_ne!(new_token, token);

    // A hosted agent has no token and no expiry.
    // A context credential never outlives the agent token that minted it.
    w.sql(
        "UPDATE gateway_credentials SET expires_at = now() + interval '120 seconds'
          WHERE agent_id = $1 AND kind = 'agent' AND revoked_at IS NULL",
        agent,
    )
    .await;
    let b = bearer(&new_token);
    let (s, _, v) =
        w.h.send(
            Method::POST,
            "/v1/agent/context-credentials",
            &[("authorization", b.as_str())],
            Some(json!({"label": "x", "ttl_secs": 3600})),
        )
        .await;
    assert_eq!(s.as_u16(), 201, "{v}");
    let ctx_exp = ts(&v["expires_at"]).unwrap();
    assert!(ctx_exp <= w.expiry(agent).await.unwrap(), "{ctx_exp}");
    assert!(ctx_exp < Utc::now() + Duration::seconds(130));
}

#[tokio::test]
async fn context_credential_without_a_token_expiry_keeps_its_ttl() {
    let w = world_with(None).await;
    let (_, token, _) = w.byo("A").await;
    let b = bearer(&token);
    let (s, _, v) =
        w.h.send(
            Method::POST,
            "/v1/agent/context-credentials",
            &[("authorization", b.as_str())],
            Some(json!({"label": "x", "ttl_secs": 3600})),
        )
        .await;
    assert_eq!(s.as_u16(), 201, "{v}");
    assert!(near(
        ts(&v["expires_at"]).unwrap(),
        Utc::now() + Duration::hours(1)
    ));
}

// ---- retrofit -------------------------------------------------------------------------------

async fn retrofit_audit(w: &W) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'governance.update' ORDER BY created_at",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn retrofit_on_introduce_and_tighten_keeps_the_grace_and_never_extends() {
    let w = world_with(None).await;
    let (old, _, _) = w.byo("Old").await; // created 100 days ago
    let (young, _, _) = w.byo("Young").await; // created 5 days ago
    w.sql(
        "UPDATE gateway_credentials SET created_at = now() - interval '100 days'
          WHERE agent_id = $1 AND kind = 'agent'",
        old,
    )
    .await;
    w.sql(
        "UPDATE gateway_credentials SET created_at = now() - interval '5 days'
          WHERE agent_id = $1 AND kind = 'agent'",
        young,
    )
    .await;

    // Introduced at 30 days: the old token gets the 7-day minimum, the young one created + 30.
    assert_eq!(w.set_days(Some(30)).await.0, 200);
    let now = Utc::now();
    assert!(near(w.expiry(old).await.unwrap(), now + Duration::days(7)));
    assert!(near(
        w.expiry(young).await.unwrap(),
        now + Duration::days(25)
    ));
    let audits = retrofit_audit(&w).await;
    assert_eq!(audits.last().unwrap()["agent_tokens_retrofitted"], 2);

    // Tightened to 10 days: young -> created + 10 = 5 days left, which is below the grace.
    assert_eq!(w.set_days(Some(10)).await.0, 200);
    let now = Utc::now();
    assert!(near(
        w.expiry(young).await.unwrap(),
        now + Duration::days(7)
    ));
    // The old token already has about 7 days: never extended, never re-shortened.
    assert!(w.expiry(old).await.unwrap() <= now + Duration::days(7) + Duration::seconds(5));
    let audits = retrofit_audit(&w).await;
    assert_eq!(audits.last().unwrap()["agent_tokens_retrofitted"], 1);

    // Loosening (or removing) never extends an existing expiry; the count is 0 and present.
    let before = w.expiry(young).await;
    assert_eq!(w.set_days(Some(60)).await.0, 200);
    assert_eq!(w.expiry(young).await, before);
    assert_eq!(
        retrofit_audit(&w).await.last().unwrap()["agent_tokens_retrofitted"],
        0
    );
    assert_eq!(w.set_days(None).await.0, 200);
    assert_eq!(w.expiry(young).await, before);
    assert_eq!(
        retrofit_audit(&w).await.last().unwrap()["agent_tokens_retrofitted"],
        0
    );
}

#[tokio::test]
async fn instance_retrofit_at_start_writes_one_system_audit_row_only_when_something_changed() {
    let w = world_with(Some(30)).await;
    let (a, _, _) = w.byo("A").await;
    // A token minted before the limit existed: no expiry, created 100 days ago.
    w.sql(
        "UPDATE gateway_credentials SET expires_at = NULL, created_at = now() - interval '100 days'
          WHERE agent_id = $1 AND kind = 'agent'",
        a,
    )
    .await;
    let n = aurora_api::domains::governance::service::retrofit_instance(&w.h.state)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert!(near(
        w.expiry(a).await.unwrap(),
        Utc::now() + Duration::days(7)
    ));
    let rows: Vec<(String, Option<Uuid>, Value)> = sqlx::query_as(
        "SELECT actor_kind, org_id, metadata FROM audit_log WHERE action = 'governance.update'",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "system");
    assert_eq!(rows[0].1, None);
    assert_eq!(rows[0].2["agent_tokens_retrofitted"], 1);
    assert_eq!(rows[0].2["max_age_days"], 30);

    // A second start finds nothing to change: no new row.
    assert_eq!(
        aurora_api::domains::governance::service::retrofit_instance(&w.h.state)
            .await
            .unwrap(),
        0
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'governance.update'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(count, 1);
}

// ---- API refusal and last_used_at ---------------------------------------------------------------

#[tokio::test]
async fn expired_token_is_refused_by_the_agent_routes_with_agent_token_expired() {
    let w = world_with(None).await;
    let (agent, token, _) = w.byo("A").await;
    let (s, _) = w.bearer("/v1/agent/container-config", &token).await;
    assert_eq!(s, 200);
    w.sql(
        "UPDATE gateway_credentials SET expires_at = now() - interval '1 second'
          WHERE agent_id = $1 AND kind = 'agent'",
        agent,
    )
    .await;
    let (s, v) = w.bearer("/v1/agent/container-config", &token).await;
    assert_eq!((s, code(&v)), (401, "agent_token_expired"), "{v}");
    let b = bearer(&token);
    let (s, _, v) =
        w.h.send(
            Method::POST,
            "/v1/agent/context-credentials",
            &[("authorization", b.as_str())],
            Some(json!({"label": "x"})),
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (401, "agent_token_expired"), "{v}");
    // A revoked or unknown token stays `unauthenticated`.
    let (s, v) = w
        .bearer(
            "/v1/agent/container-config",
            "aug_notatoken0000000000000000000000",
        )
        .await;
    assert_eq!((s, code(&v)), (401, "unauthenticated"), "{v}");
    w.sql(
        "UPDATE gateway_credentials SET revoked_at = now() WHERE agent_id = $1",
        agent,
    )
    .await;
    let (s, v) = w.bearer("/v1/agent/container-config", &token).await;
    assert_eq!((s, code(&v)), (401, "unauthenticated"), "{v}");
}

#[tokio::test]
async fn last_used_at_is_written_on_agent_token_routes_at_most_once_a_minute() {
    let w = world_with(None).await;
    let (agent, token, _) = w.byo("A").await;
    let used = |w: &W| {
        let pool = w.h.pool().clone();
        async move {
            sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
                "SELECT last_used_at FROM gateway_credentials
                  WHERE agent_id = $1 AND kind = 'agent'",
            )
            .bind(agent)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert!(used(&w).await.is_none());
    assert_eq!(w.bearer("/v1/agent/container-config", &token).await.0, 200);
    // The write is off the request path: wait for it.
    let mut first = None;
    for _ in 0..50 {
        first = used(&w).await;
        if first.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let first = first.expect("last_used_at written");
    assert!(near(first, Utc::now()));
    // A second call inside the minute does not move it.
    assert_eq!(w.bearer("/v1/agent/container-config", &token).await.0, 200);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(used(&w).await, Some(first));
    // The repository statement itself is conditional: older than a minute is rewritten.
    w.sql(
        "UPDATE gateway_credentials SET last_used_at = now() - interval '2 minutes'
          WHERE agent_id = $1 AND kind = 'agent'",
        agent,
    )
    .await;
    let id: Uuid = sqlx::query_scalar(
        "SELECT id FROM gateway_credentials WHERE agent_id = $1 AND kind = 'agent'",
    )
    .bind(agent)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        aurora_db::repo::gateway::touch_last_used(w.h.pool(), &[id])
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        aurora_db::repo::gateway::touch_last_used(w.h.pool(), &[id])
            .await
            .unwrap(),
        0,
        "a second write inside the minute is skipped"
    );
}

// ---- stale / expiring lists -------------------------------------------------------------------

#[tokio::test]
async fn stale_and_expiring_lists_thresholds_filters_authorization_and_no_secrets() {
    let w = world_with(None).await;
    let (fresh, fresh_tok, _) = w.byo("Fresh").await;
    let (never, never_tok, _) = w.byo("Never").await;
    let (used, used_tok, _) = w.byo("Used").await;
    let (expired, expired_tok, _) = w.byo("Expired").await;
    // Never used for 31 days (counted from creation) -> stale.
    w.sql(
        "UPDATE gateway_credentials SET created_at = now() - interval '31 days'
          WHERE agent_id = $1 AND kind = 'agent'",
        never,
    )
    .await;
    // Created long ago but used yesterday -> not stale; expiring in 13 days.
    w.sql(
        "UPDATE gateway_credentials SET created_at = now() - interval '90 days',
                last_used_at = now() - interval '1 day', expires_at = now() + interval '13 days'
          WHERE agent_id = $1 AND kind = 'agent'",
        used,
    )
    .await;
    // Already expired (counts as expiring) and unused since creation 40 days ago.
    w.sql(
        "UPDATE gateway_credentials SET created_at = now() - interval '40 days',
                expires_at = now() - interval '1 day'
          WHERE agent_id = $1 AND kind = 'agent'",
        expired,
    )
    .await;
    // Fresh: expires in 15 days -> not expiring.
    w.sql(
        "UPDATE gateway_credentials SET expires_at = now() + interval '15 days'
          WHERE agent_id = $1 AND kind = 'agent'",
        fresh,
    )
    .await;

    let list = |q: &str| format!("/v1/workspaces/{}/agent-tokens{q}", w.ws);
    let names = |v: &Value| -> Vec<String> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["agent"]["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let (s, all) = w.call(Method::GET, &list(""), &w.owner, None).await;
    assert_eq!(s, 200, "{all}");
    // Ordered created_at desc, id desc: Fresh, Used? no - by credential creation time.
    let mut got = names(&all);
    got.sort();
    assert_eq!(got, ["Expired", "Fresh", "Never", "Used"]);
    let by_name = |n: &str| -> Value {
        all["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["agent"]["name"] == n)
            .unwrap()
            .clone()
    };
    assert_eq!(by_name("Never")["stale"], true);
    assert_eq!(by_name("Never")["last_used_at"], Value::Null);
    assert_eq!(by_name("Used")["stale"], false);
    assert_eq!(by_name("Used")["expiring"], true);
    assert_eq!(by_name("Fresh")["stale"], false);
    assert_eq!(by_name("Fresh")["expiring"], false);
    assert_eq!(by_name("Expired")["stale"], true);
    assert_eq!(by_name("Expired")["expiring"], true);
    assert!(by_name("Fresh")["workspace"]["slug"].is_string());
    assert_eq!(by_name("Fresh")["agent"]["id"], fresh.to_string());

    // No token material: only the display prefix.
    let text = all.to_string();
    for t in [&fresh_tok, &never_tok, &used_tok, &expired_tok] {
        assert!(!text.contains(t.as_str()));
    }
    assert!(by_name("Fresh").get("token").is_none());
    assert!(by_name("Fresh").get("token_hash").is_none());

    // Filters combine with AND.
    let (_, v) = w
        .call(Method::GET, &list("?stale=true"), &w.owner, None)
        .await;
    let mut n = names(&v);
    n.sort();
    assert_eq!(n, ["Expired", "Never"]);
    let (_, v) = w
        .call(Method::GET, &list("?stale=false"), &w.owner, None)
        .await;
    let mut n = names(&v);
    n.sort();
    assert_eq!(n, ["Fresh", "Used"]);
    let (_, v) = w
        .call(
            Method::GET,
            &list("?expiring_within_days=14"),
            &w.owner,
            None,
        )
        .await;
    let mut n = names(&v);
    n.sort();
    assert_eq!(n, ["Expired", "Used"]);
    let (_, v) = w
        .call(
            Method::GET,
            &list("?expiring_within_days=14&stale=true"),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(names(&v), ["Expired"]);
    for bad in ["0", "3651", "x"] {
        let (s, v) = w
            .call(
                Method::GET,
                &list(&format!("?expiring_within_days={bad}")),
                &w.owner,
                None,
            )
            .await;
        assert_eq!(s, 400, "{bad}: {v}");
    }

    // Pagination.
    let (_, p1) = w.call(Method::GET, &list("?limit=3"), &w.owner, None).await;
    assert_eq!(p1["items"].as_array().unwrap().len(), 3);
    let cur = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = w
        .call(
            Method::GET,
            &list(&format!("?limit=3&cursor={cur}")),
            &w.owner,
            None,
        )
        .await;
    assert_eq!(p2["items"].as_array().unwrap().len(), 1);
    assert!(p2["next_cursor"].is_null());

    // Revoked tokens are not listed; a hosted agent has none.
    w.sql(
        "UPDATE gateway_credentials SET revoked_at = now() WHERE agent_id = $1 AND kind = 'agent'",
        fresh,
    )
    .await;
    let (_, v) = w.call(Method::GET, &list(""), &w.owner, None).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 3);

    // Org route: org owner/admin only; the workspace member is refused on both.
    let org_list = format!("/v1/orgs/{}/agent-tokens", w.org);
    let (s, v) = w.call(Method::GET, &org_list, &w.owner, None).await;
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 3);
    assert_eq!(w.call(Method::GET, &org_list, &w.member, None).await.0, 403);
    assert_eq!(w.call(Method::GET, &list(""), &w.member, None).await.0, 403);
}

#[tokio::test]
async fn agent_byo_token_view_is_visible_to_whoever_sees_the_agent() {
    let w = world_with(Some(30)).await;
    let (agent, token, exp) = w.byo("A").await;
    let get = |tok: String| {
        let w = &w;
        async move {
            w.call(Method::GET, &format!("/v1/agents/{agent}"), &tok, None)
                .await
        }
    };
    // The workspace member (can_use) sees the token info and never the token.
    let (s, v) = get(w.member.clone()).await;
    assert_eq!(s, 200, "{v}");
    let t = &v["byo"]["token"];
    assert_eq!(ts(&t["expires_at"]), exp);
    assert_eq!(t["last_used_at"], Value::Null);
    assert_eq!(t["stale"], false);
    assert_eq!(t["expiring"], false);
    assert!(t["display_prefix"].as_str().unwrap().starts_with("aug_"));
    assert!(!v.to_string().contains(&token));
    // Listing the workspace agents carries it too.
    let (_, l) = w
        .call(
            Method::GET,
            &format!("/v1/workspaces/{}/agents", w.ws),
            &w.member,
            None,
        )
        .await;
    assert_eq!(
        l["items"][0]["byo"]["token"]["display_prefix"],
        t["display_prefix"]
    );
    // No live token -> null.
    w.sql(
        "UPDATE gateway_credentials SET revoked_at = now() WHERE agent_id = $1",
        agent,
    )
    .await;
    let (_, v) = get(w.owner.clone()).await;
    assert_eq!(v["byo"]["token"], Value::Null);
}
