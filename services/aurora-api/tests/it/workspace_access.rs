#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-181 (D-396, C-1041). `WorkspaceAuth::can_use()` was `true` by construction, so the
//! `if !ws.can_use() { 403 }` branches in the agent-scoped route families were dead; the real check
//! is the construction itself (an active member with an effective workspace role). One test per
//! route family proves a non-member, an unbound org member and an inactive (suspended) member are
//! refused where the dead branch stood, and that a bound member passes (so the refusal is the access
//! law, not a wrong path). The F7 tests prove list routes authorise before they parse a query.

use crate::common::*;
use http::Method;
use serde_json::Value;
use uuid::Uuid;

struct World {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    /// Active member bound to the workspace as `member`.
    member: String,
    /// Active org member with no workspace binding.
    unbound: String,
    /// Suspended org member that still holds an owner binding.
    suspended: String,
    /// A user in no organization.
    outsider: String,
}

async fn world() -> World {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) \
         VALUES ($1,$2,$3,'bot','Bot','byo','medium','')",
    )
    .bind(agent)
    .bind(org)
    .bind(ws)
    .execute(h.pool())
    .await
    .unwrap();
    let mut cookies = Vec::new();
    for (email, org_status, bind) in [
        ("member@x.io", Some("active"), Some("member")),
        ("unbound@x.io", Some("active"), None),
        ("suspended@x.io", Some("suspended"), Some("owner")),
        ("outsider@x.io", None, None),
    ] {
        let u = h.user(email, false).await;
        if let Some(status) = org_status {
            h.member(org, u, "member", status).await;
        }
        if let Some(role) = bind {
            h.bind_ws(ws, u, role).await;
        }
        let (_, tok) = h.good_session(u).await;
        cookies.push(cookie(&tok));
    }
    let mut it = cookies.into_iter();
    World {
        member: it.next().unwrap(),
        unbound: it.next().unwrap(),
        suspended: it.next().unwrap(),
        outsider: it.next().unwrap(),
        h,
        org,
        ws,
        agent,
    }
}

async fn status(w: &World, method: Method, uri: &str, cookie: &str) -> (u16, Value) {
    let (s, _, v) = w.h.send(method, uri, &[("cookie", cookie)], None).await;
    (s.as_u16(), v)
}

/// `GET uri` is refused (404) for every principal that is not an active workspace member, and is
/// not refused for the bound member.
async fn assert_family(w: &World, uri: &str) {
    for (who, c) in [
        ("unbound org member", &w.unbound),
        ("suspended member", &w.suspended),
        ("user outside the org", &w.outsider),
    ] {
        let (s, v) = status(w, Method::GET, uri, c).await;
        assert_eq!(s, 404, "{uri} as {who}: {v}");
    }
    let (s, v) = status(w, Method::GET, uri, &w.member).await;
    assert!(s != 404 && s != 403, "{uri} as the bound member: {s} {v}");
}

#[tokio::test]
async fn agent_skills_refuse_a_non_member_and_an_inactive_member() {
    let w = world().await;
    assert_family(&w, &format!("/v1/agents/{}/skills", w.agent)).await;
    assert_family(&w, &format!("/v1/workspaces/{}/skills", w.ws)).await;
}

#[tokio::test]
async fn schedules_refuse_a_non_member_and_an_inactive_member() {
    let w = world().await;
    assert_family(&w, &format!("/v1/agents/{}/schedules", w.agent)).await;
}

#[tokio::test]
async fn webhooks_refuse_a_non_member_and_an_inactive_member() {
    let w = world().await;
    assert_family(&w, &format!("/v1/agents/{}/webhooks", w.agent)).await;
}

#[tokio::test]
async fn eval_runs_refuse_a_non_member_and_an_inactive_member() {
    let w = world().await;
    assert_family(&w, &format!("/v1/agents/{}/eval-runs", w.agent)).await;
}

#[tokio::test]
async fn peer_tasks_refuse_a_non_member_and_an_inactive_member() {
    let w = world().await;
    assert_family(&w, &format!("/v1/agents/{}/peer-tasks", w.agent)).await;
}

#[tokio::test]
async fn conversations_refuse_a_non_member_and_an_inactive_member() {
    let w = world().await;
    assert_family(&w, &format!("/v1/agents/{}/conversations", w.agent)).await;
}

/// (F7) A malformed query never answers 400 to someone who may not list: the answer is the
/// authorisation's (404/403), and only a person allowed to list sees the 400.
#[tokio::test]
async fn list_routes_authorise_before_they_parse_the_query() {
    let w = world().await;
    let ws_keys = format!("/v1/workspaces/{}/api-keys?cursor=%25%25bad", w.ws);
    let org_keys = format!("/v1/orgs/{}/api-keys?cursor=%25%25bad", w.org);
    let peer = format!("/v1/agents/{}/peer-tasks?limit=0&status=nope", w.agent);
    let convs = format!("/v1/agents/{}/conversations?limit=0&kind=nope", w.agent);
    let runs = format!("/v1/agents/{}/eval-runs?limit=0", w.agent);
    let ws_access = format!("/v1/workspaces/{}/access?limit=0", w.ws);
    let ws_overrides = format!("/v1/orgs/{}/workspace-overrides?limit=0", w.org);
    let org_workspaces = format!("/v1/orgs/{}/workspaces?limit=0", w.org);

    // Not a member of the workspace: 404, never 400.
    for uri in [&ws_keys, &peer, &convs, &runs, &ws_access] {
        for c in [&w.unbound, &w.suspended, &w.outsider] {
            let (s, v) = status(&w, Method::GET, uri, c).await;
            assert_eq!(s, 404, "{uri}: {v}");
        }
    }
    // A plain org member is not an org admin: 403 on the admin-only lists, never 400. (A person
    // outside the org sees 404.)
    for uri in [&org_keys, &ws_overrides] {
        let (s, v) = status(&w, Method::GET, uri, &w.unbound).await;
        assert_eq!(s, 403, "{uri}: {v}");
        let (s, v) = status(&w, Method::GET, uri, &w.outsider).await;
        assert_eq!(s, 404, "{uri}: {v}");
    }
    // The org's workspace list: a person outside the org is refused before the limit is checked.
    let (s, v) = status(&w, Method::GET, &org_workspaces, &w.outsider).await;
    assert_eq!(s, 404, "{org_workspaces}: {v}");
    // A person who may list does get the 400. (Eval runs are hosted-only: the byo test agent
    // answers 422 first, so that route is covered by the refusals above only.)
    let _ = &runs;
    for uri in [&peer, &convs] {
        let (s, v) = status(&w, Method::GET, uri, &w.member).await;
        assert_eq!(s, 400, "{uri}: {v}");
    }
}
