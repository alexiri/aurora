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

    // An organization admin who is not one of the workspace's people: the readiness view is a
    // can-use view (FIX-234, C-1160) — they are refused here and add the key through the
    // administer routes (covered by the secrets tests).
    let (s, v) = get(&r, &admin, a).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("workspace_access_required")),
        "{v}"
    );
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

    // (FIX-234) An organization admin without own access is refused (D-477).
    let (s, v) = get(&r, &admin, a).await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(v["error"]["code"], "workspace_access_required");

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

async fn ws_attention(r: &Rig, ck: &str) -> Value {
    let (s, _, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/workspaces/{}/attention", r.ws),
            &[("cookie", ck), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    v
}

async fn ws_counts(r: &Rig, ck: &str) -> Value {
    let (s, _, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/attention/counts?workspace={}", r.ws),
            &[("cookie", ck), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    v["workspace"].clone()
}

fn cannot_answer_items(v: &Value) -> Vec<&Value> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "agent_cannot_answer")
        .collect()
}

/// (P5-REFINE-20 batch 4, C-1080) An agent that cannot answer is a workspace attention item for the
/// people who can give it a key: the feed row, the count and the `agents` topic agree; a plain member
/// sees and counts none (D-384); giving a usable key removes it.
#[tokio::test]
async fn an_agent_that_cannot_answer_is_counted_for_owners_only() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    let owner = person(&r, "owner@x.io", "member", Some("owner")).await;

    let feed = ws_attention(&r, &owner).await;
    let items = cannot_answer_items(&feed);
    assert_eq!(items.len(), 1, "{feed}");
    let it = items[0];
    assert_eq!(it["id"], format!("agent_cannot_answer:{a}"));
    assert_eq!(it["title"], "rex can't answer: no AI key");
    assert_eq!(it["topic"], "agents");
    assert_eq!(it["decision"], false);
    assert_eq!(it["own"], false);
    assert_eq!(it["action"]["label"], "Fix");
    assert!(
        it["action"]["href"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/agents/{a}?tab=access")),
        "{it}"
    );
    assert_eq!(feed["count"], 1, "{feed}");
    let c = ws_counts(&r, &owner).await;
    assert_eq!(c["count"], feed["count"], "{c}");
    assert_eq!(c["topics"]["agents"], 1, "{c}");

    // A plain member cannot fix it: no row, nothing counted.
    let feed = ws_attention(&r, &r.cookie).await;
    assert!(cannot_answer_items(&feed).is_empty(), "{feed}");
    assert_eq!(feed["count"], 0, "{feed}");
    let c = ws_counts(&r, &r.cookie).await;
    assert_eq!(c["count"], 0, "{c}");

    // A key the agent can use makes it answer again: the item goes.
    org_key(&r, "Company key", "active", Some(a)).await;
    let feed = ws_attention(&r, &owner).await;
    assert!(cannot_answer_items(&feed).is_empty(), "{feed}");
    assert_eq!(ws_counts(&r, &owner).await["count"], 0);
}

/// A paused (frozen) agent is not "can't answer": it is paused.
#[tokio::test]
async fn a_paused_agent_is_not_a_cannot_answer_item() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    let owner = person(&r, "owner@x.io", "member", Some("owner")).await;
    sqlx::query("UPDATE agents SET frozen_at = now() WHERE id = $1")
        .bind(a)
        .execute(r.h.pool())
        .await
        .unwrap();
    let feed = ws_attention(&r, &owner).await;
    assert!(cannot_answer_items(&feed).is_empty(), "{feed}");
}

/// (P5-REFINE-20 batch 7, access-3) "Since" is when the agent lost its key, not when it was made: an agent created
/// two days ago whose key was taken away just now reads "just now" on Overview.
#[tokio::test]
async fn the_cannot_answer_item_is_since_the_key_was_taken_away() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    let owner = person(&r, "owner@x.io", "member", Some("owner")).await;
    sqlx::query("UPDATE agents SET created_at = now() - interval '2 days' WHERE id = $1")
        .bind(a)
        .execute(r.h.pool())
        .await
        .unwrap();
    let since = |feed: &Value| {
        let items = cannot_answer_items(feed);
        assert_eq!(items.len(), 1, "{feed}");
        chrono::DateTime::parse_from_rfc3339(items[0]["since"].as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    // Never had a key: since its creation.
    let age = chrono::Utc::now() - since(&ws_attention(&r, &owner).await);
    assert!(age > chrono::Duration::hours(47), "{age}");

    // Given a key, then the key is taken away: since the removal.
    let key = org_key(&r, "Company key", "active", Some(a)).await;
    let (s, _, v) =
        r.h.send(
            Method::DELETE,
            &format!("/v1/agents/{a}/grants/secrets/{key}"),
            &[("cookie", owner.as_str()), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 204, "{v}");
    let age = chrono::Utc::now() - since(&ws_attention(&r, &owner).await);
    assert!(age < chrono::Duration::minutes(5), "{age}");
}

// ---- FIX-249 (C-1168): `GET /v1/agents/{agent}/ai-keys` — the keys this agent does not have yet,
// the starter first, the dispatcher's order and reasons behind it. Metadata only, `can_own`. ----

async fn get_ai_keys(r: &Rig, ck: &str, agent: Uuid) -> (u16, Value) {
    let (s, _, v) =
        r.h.send(
            Method::GET,
            &format!("/v1/agents/{agent}/ai-keys"),
            &[("cookie", ck), ("origin", ORIGIN)],
            None,
        )
        .await;
    (s.as_u16(), v)
}

/// A workspace-level LLM key on the `anthropic` endpoint; granted to `agent` when given.
async fn ws_key(r: &Rig, name: &str, agent: Option<Uuid>) -> Uuid {
    let sec = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets (id, org_id, workspace_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id,
            host_pattern, injection, value_source, value_ref, status)
         VALUES ($1,$2,$3,$4,'llm','anthropic','api_key','anthropic','api.anthropic.com','{}','managed','x','active')",
    )
    .bind(sec)
    .bind(r.org)
    .bind(r.ws)
    .bind(name)
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

/// Makes `starter` the organization's starter key (the placement row the starter_ids read).
async fn make_starter(r: &Rig, starter: Uuid) {
    sqlx::query(
        "INSERT INTO instance_placement (singleton, mode, default_org_id, default_org_role, shared_workspaces,
            personal_enabled, personal_role, name_template, starter_grants, updated_at)
         VALUES (true, 'shared_org', $1, 'member', '[]'::jsonb, true, 'owner', '{name}', $2, now())
         ON CONFLICT (singleton) DO UPDATE SET default_org_id = $1, starter_grants = $2",
    )
    .bind(r.org)
    .bind(serde_json::json!([{"kind": "secret", "id": starter}]))
    .execute(r.h.pool())
    .await
    .unwrap();
}

fn items_of(v: &Value) -> &[Value] {
    v["items"].as_array().unwrap()
}

/// A workspace owner's session cookie: the route is `can_own`, like the grant route it feeds
/// (`r.cookie` is a plain member and stays the refusal check).
async fn owner_ck(r: &Rig) -> String {
    let owner = r.h.user("ws-owner@x.io", false).await;
    r.h.member(r.org, owner, "member", "active").await;
    r.h.bind_ws(r.ws, owner, "owner").await;
    let (_, sess) = r.h.good_session(owner).await;
    cookie(&sess)
}

#[tokio::test]
async fn ai_keys_two_usable_keys_the_starter_first() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    let first = org_key(&r, "First key", "active", None).await;
    let starter = org_key(&r, "Starter key", "active", None).await;
    make_starter(&r, starter).await;

    let (s, v) = get_ai_keys(&r, &owner_ck(&r).await, a).await;
    assert_eq!(s, 200, "{v}");
    let items = items_of(&v);
    assert_eq!(items.len(), 2, "{v}");
    assert_eq!(items[0]["secret_id"], starter.to_string(), "{v}");
    assert_eq!(items[0]["name"], "Starter key");
    assert_eq!(items[0]["scope"], "organization");
    assert_eq!(items[0]["starter"], true);
    assert_eq!(items[0]["usable"], true);
    assert_eq!(items[0]["reason"], Value::Null);
    assert_eq!(items[1]["secret_id"], first.to_string(), "{v}");
    assert_eq!(items[1]["starter"], false);
    assert_eq!(items[1]["usable"], true);
}

#[tokio::test]
async fn ai_keys_an_unusable_key_is_listed_with_its_reason_in_words() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    // A key whose provider is not the one its endpoint serves: the dispatcher's `provider_disabled`.
    let sec = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets (id, org_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id,
            host_pattern, injection, value_source, value_ref, status)
         VALUES ($1,$2,'OpenAI key','llm','openai','api_key','anthropic','api.anthropic.com','{}','managed','x','active')",
    )
    .bind(sec)
    .bind(r.org)
    .execute(r.h.pool())
    .await
    .unwrap();

    let (s, v) = get_ai_keys(&r, &owner_ck(&r).await, a).await;
    assert_eq!(s, 200, "{v}");
    let items = items_of(&v);
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["secret_id"], sec.to_string());
    assert_eq!(items[0]["usable"], false);
    assert_eq!(items[0]["reason"], "Its service is not allowed here.");
    // The reason is words, never the internal code or the endpoint's address.
    assert!(!v.to_string().contains("provider_disabled"), "{v}");
    assert!(!v.to_string().contains("api.anthropic.com"), "{v}");
}

#[tokio::test]
async fn ai_keys_a_key_already_granted_is_absent() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    org_key(&r, "Given key", "active", Some(a)).await;
    org_key(&r, "Other key", "active", None).await;

    let (s, v) = get_ai_keys(&r, &owner_ck(&r).await, a).await;
    assert_eq!(s, 200, "{v}");
    let items = items_of(&v);
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["name"], "Other key");
}

#[tokio::test]
async fn ai_keys_an_organization_key_and_a_workspace_key_both_appear() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    let org = org_key(&r, "Org key", "active", None).await;
    let ws = ws_key(&r, "Workspace key", None).await;

    let (s, v) = get_ai_keys(&r, &owner_ck(&r).await, a).await;
    assert_eq!(s, 200, "{v}");
    let items = items_of(&v);
    assert_eq!(items.len(), 2, "{v}");
    // The dispatcher's order on the shared endpoint: the workspace key first (C-1168).
    assert_eq!(items[0]["secret_id"], ws.to_string(), "{v}");
    assert_eq!(items[0]["scope"], "workspace");
    assert_eq!(items[1]["secret_id"], org.to_string(), "{v}");
    assert_eq!(items[1]["scope"], "organization");
}

#[tokio::test]
async fn ai_keys_a_member_who_cannot_own_is_refused_and_a_non_hosted_agent_is_not_there() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    org_key(&r, "Org key", "active", None).await;

    // The rig's own session is a plain member of the workspace: the grant route's refusal
    // (`require_own`, the same check the grants read and write routes use).
    let (s, v) = get_ai_keys(&r, &r.cookie, a).await;
    assert_eq!(s, 403, "{v}");
    assert_eq!(v["error"]["code"], "forbidden", "{v}");
    assert_eq!(
        v["error"]["details"]["who_can"]["role"], "workspace_owner",
        "{v}"
    );
    assert!(!v.to_string().contains("Org key"), "{v}");

    // A BYO agent has no AI keys of this kind.
    let byo = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'byo-k','byo-k','byo')")
        .bind(byo)
        .bind(r.org)
        .bind(r.ws)
        .execute(r.h.pool())
        .await
        .unwrap();
    // Refused alike: the kind of the agent is not told to someone who may not grant.
    let (s2, v2) = get_ai_keys(&r, &r.cookie, byo).await;
    assert_eq!(s2, s, "{v2}");
    assert_eq!(v2["error"], v["error"], "{v2}");

    // An owner who may grant is told there is no sandbox.
    let (s, v) = get_ai_keys(&r, &owner_ck(&r).await, byo).await;
    assert_eq!(s, 404, "{v}");
}

#[tokio::test]
async fn ai_keys_another_organizations_key_never_appears() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    let org2 = r.h.org().await;
    let ws2 = r.h.workspace(org2).await;
    let sec = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets (id, org_id, workspace_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id,
            host_pattern, injection, value_source, value_ref, status)
         VALUES ($1,$2,$3,'Other org key','llm','anthropic','api_key','anthropic','api.anthropic.com','{}','managed','x','active')",
    )
    .bind(sec)
    .bind(org2)
    .bind(ws2)
    .execute(r.h.pool())
    .await
    .unwrap();

    let (s, v) = get_ai_keys(&r, &owner_ck(&r).await, a).await;
    assert_eq!(s, 200, "{v}");
    assert!(items_of(&v).is_empty(), "{v}");
    assert!(!v.to_string().contains("Other org key"), "{v}");
}

#[tokio::test]
async fn ai_keys_metadata_only_never_a_value() {
    let r = rig().await;
    let a = r.agent("rex", false).await;
    org_key(&r, "Org key", "active", None).await;

    let (s, v) = get_ai_keys(&r, &owner_ck(&r).await, a).await;
    assert_eq!(s, 200, "{v}");
    let item = &items_of(&v)[0];
    let mut keys: Vec<&str> = item
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["name", "reason", "scope", "secret_id", "starter", "usable"],
        "{v}"
    );
    // No value, no endpoint address, no provider id: the key's metadata only.
    let body = v.to_string();
    assert!(!body.contains("\"x\""), "{v}");
    assert!(!body.contains("api.anthropic.com"), "{v}");
}
