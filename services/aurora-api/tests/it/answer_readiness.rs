#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-REFINE-5 (D-414, D-408, C-1062): `GET /v1/agents/{agent}/answer-readiness` — the reason an
//! agent cannot answer and the fix chosen for the person looking; roles, never names; key names
//! only to people who can own the workspace.

use crate::common::*;
use crate::hosted::{Rig, rig};
use http::Method;
use serde_json::Value;
use uuid::Uuid;

async fn get(r: &Rig, ck: &str, agent: Uuid) -> (u16, Value) {
    let (s, _, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/agents/{agent}/answer-readiness"),
            &[("cookie", ck), ("origin", ORIGIN)],
            None,
        )
        .await;
    (s.as_u16(), v)
}

/// A session for a new person with this org role and workspace role (`None` = not bound).
async fn person(r: &Rig, email: &str, org_role: &str, ws_role: Option<&str>) -> String {
    let u = r.h.user(email, false).await;
    r.h.member(r.org, u, org_role, "active").await;
    if let Some(w) = ws_role {
        r.h.bind_ws(r.ws, u, w).await;
    }
    let (_, s) = r.h.good_session(u).await;
    cookie(&s)
}

/// An org-level LLM key on the `anthropic` endpoint; granted to `agent` when given.
async fn org_key(r: &Rig, name: &str, status: &str, agent: Option<Uuid>) -> Uuid {
    let sec = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets (id, org_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id,
            host_pattern, injection, value_source, value_ref, status)
         VALUES ($1,$2,$3,'llm','anthropic','api_key','anthropic','api.anthropic.com','{}','managed','x',$4)",
    )
    .bind(sec)
    .bind(r.org)
    .bind(name)
    .bind(status)
    .execute(r.h.pool())
    .await
    .unwrap();
    if let Some(a) = agent {
        sqlx::query(
            "INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(Uuid::now_v7())
        .bind(r.ws)
        .bind(a)
        .bind(sec)
        .execute(r.h.pool())
        .await
        .unwrap();
    }
    sec
}

#[tokio::test]
async fn no_key_at_all_member_asks_admin_org_admin_adds_one() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    let owner = person(&r, "owner@x.io", "member", Some("owner")).await;
    let admin = person(&r, "admin2@x.io", "admin", None).await;

    // A plain member: the reason, and the roles that can fix it, never names.
    let (s, v) = get(&r, &r.cookie, a).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["can_answer"], false);
    assert_eq!(v["reason"], "no_ai_key");
    assert_eq!(v["usable_keys"], 0);
    assert_eq!(v["fix"]["action"], "ask_admin");
    assert_eq!(v["fix"]["who_can"]["role"], "org_admin");
    assert_eq!(v["fix"]["who_can"]["people"], serde_json::json!([]));

    // A workspace owner who is not an administrator cannot add the organization's first key.
    let (s, v) = get(&r, &owner, a).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["fix"]["action"], "ask_admin", "{v}");

    // An organization admin can.
    let (s, v) = get(&r, &admin, a).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["reason"], "no_ai_key");
    assert_eq!(v["fix"], serde_json::json!({"action": "add_key"}));
}

#[tokio::test]
async fn a_usable_key_not_given_is_offered_to_owners_and_never_named_to_members() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    let key = org_key(&r, "Company key", "active", None).await;
    let owner = person(&r, "owner@x.io", "member", Some("owner")).await;
    let admin = person(&r, "admin2@x.io", "admin", None).await;

    let (s, v) = get(&r, &owner, a).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["reason"], "no_ai_key");
    assert_eq!(v["fix"]["action"], "give_key");
    assert_eq!(v["fix"]["key"]["secret_id"], key.to_string());
    assert_eq!(v["fix"]["key"]["name"], "Company key");
    assert_eq!(v["fix"]["key"]["scope"], "organization");

    // An organization admin is an implicit owner: the same fix.
    let (_, v) = get(&r, &admin, a).await;
    assert_eq!(v["fix"]["action"], "give_key", "{v}");

    // A member cannot give it: the roles that can, and not a trace of the key's name.
    let (s, v) = get(&r, &r.cookie, a).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["fix"]["action"], "ask_admin");
    assert_eq!(v["fix"]["who_can"]["role"], "workspace_owner");
    assert!(!v.to_string().contains("Company key"), "{v}");
}

#[tokio::test]
async fn a_granted_usable_key_can_answer_and_an_unusable_one_says_so() {
    let r = rig().await;
    let ok = r.agent("ok", false).await;
    org_key(&r, "k-ok", "active", Some(ok)).await;
    let (s, v) = get(&r, &r.cookie, ok).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["can_answer"], true);
    assert_eq!(v["reason"], Value::Null);
    assert_eq!(v["usable_keys"], 1);
    assert_eq!(v["fix"], Value::Null);

    let bad = r.agent("bad", false).await;
    org_key(&r, "k-bad", "proposed", Some(bad)).await;
    let (_, v) = get(&r, &r.cookie, bad).await;
    assert_eq!(v["can_answer"], false);
    assert_eq!(v["reason"], "key_not_usable");
    assert_eq!(v["usable_keys"], 0);
}

#[tokio::test]
async fn byo_agents_can_answer_and_strangers_see_nothing() {
    let r = rig().await;
    let byo = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'byo-one','byo-one','byo')")
        .bind(byo)
        .bind(r.org)
        .bind(r.ws)
        .execute(r.h.pool())
        .await
        .unwrap();
    let (s, v) = get(&r, &r.cookie, byo).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["can_answer"], true);
    assert_eq!(v["fix"], Value::Null);

    // Not a member of the workspace: the agent is not there.
    let stranger = {
        let u = r.h.user("s@x.io", false).await;
        r.h.member(r.org, u, "member", "active").await;
        let (_, s) = r.h.good_session(u).await;
        cookie(&s)
    };
    let (s, _) = get(&r, &stranger, byo).await;
    assert_eq!(s, 404);
}
