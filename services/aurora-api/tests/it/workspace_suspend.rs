#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-APIGAP-4 (D-399, C-1050, C-1051, SEC-138): `joined_at` on workspace People, and one person
//! suspended in one workspace. A suspended binding grants nothing on any surface of that workspace
//! (routes of every FIX-181 family, workspace API keys, the access law the terminal, SSH, Files and
//! previews re-check, approval decisions and mail, the gateway's speaker law, group-derived
//! access); their open streams end; their speaker turns there end; other workspaces are untouched;
//! restore gives everything back. Refusals: self, an implicit org owner/admin, an IdP-claimed
//! binding, a person without a direct binding, a caller who is not an owner. Both are audited.

use crate::common::*;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

struct World {
    h: Harness,
    org: Uuid,
    /// The workspace where the suspension happens.
    ws: Uuid,
    /// Another workspace of the same org the person also uses.
    other: Uuid,
    agent: Uuid,
    other_agent: Uuid,
    /// Workspace owner (direct binding) who suspends.
    owner: Uuid,
    owner_cookie: String,
    /// The person: `approver` in `ws` and in `other`.
    person: Uuid,
    person_cookie: String,
    /// The person's workspace API key for `ws`.
    person_key: String,
}

async fn world() -> World {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let other = h.workspace(org).await;
    let agent = h.byo_agent(org, ws, "bot", "Bot").await;
    let other_agent = h.byo_agent(org, other, "bot2", "Bot2").await;
    let boss = h.user("boss@x.io", false).await;
    h.member(org, boss, "owner", "active").await;
    let owner = h.user("owner@x.io", false).await;
    h.member(org, owner, "member", "active").await;
    h.bind_ws(ws, owner, "owner").await;
    let person = h.user("person@x.io", false).await;
    h.member(org, person, "member", "active").await;
    h.bind_ws(ws, person, "approver").await;
    h.bind_ws(other, person, "approver").await;
    let (_, otok) = h.good_session(owner).await;
    let (_, ptok) = h.good_session(person).await;
    let (_, key) = h.api_key(person, org, Some(ws), false).await;
    World {
        h,
        org,
        ws,
        other,
        agent,
        other_agent,
        owner,
        owner_cookie: cookie(&otok),
        person,
        person_cookie: cookie(&ptok),
        person_key: bearer(&key),
    }
}

impl World {
    async fn set(&self, cookie: &str, user: Uuid, status: &str) -> (u16, Value) {
        self.h
            .call(
                cookie,
                Method::PATCH,
                &format!("/v1/workspaces/{}/access/{user}", self.ws),
                Some(json!({ "status": status })),
            )
            .await
    }
    async fn get_as_person(&self, uri: &str) -> u16 {
        self.h
            .call(&self.person_cookie, Method::GET, uri, None)
            .await
            .0
    }
    /// The workspace routes of every FIX-181 family, plus the workspace itself, its approvals and
    /// its accounts (the connections family as a `can_use` member sees it; `/connections` itself
    /// is owner-only).
    fn family_uris(&self, ws: Uuid, agent: Uuid) -> Vec<String> {
        vec![
            format!("/v1/workspaces/{ws}"),
            format!("/v1/agents/{agent}"),
            format!("/v1/agents/{agent}/conversations"),
            format!("/v1/agents/{agent}/skills"),
            format!("/v1/agents/{agent}/schedules"),
            format!("/v1/agents/{agent}/webhooks"),
            format!("/v1/agents/{agent}/peer-tasks"),
            format!("/v1/agents/{agent}/eval-runs"),
            format!("/v1/workspaces/{ws}/approvals"),
            format!("/v1/workspaces/{ws}/action-approvals"),
            format!("/v1/workspaces/{ws}/accounts"),
        ]
    }
    async fn law(&self, ws: Uuid) -> (bool, bool, bool) {
        let p = self.h.pool();
        (
            aurora_db::authz::can_use(p, self.person, ws).await.unwrap(),
            aurora_db::authz::can_approve(p, self.person, ws)
                .await
                .unwrap(),
            aurora_db::repo::gateway::can_use(p, ws, self.person)
                .await
                .unwrap(),
        )
    }
}

fn row(list: &Value, user: Uuid) -> &Value {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["user_id"] == json!(user))
        .unwrap_or_else(|| panic!("no row for {user} in {list}"))
}

#[tokio::test]
async fn joined_at_is_the_binding_time_and_null_for_an_implicit_owner() {
    let w = world().await;
    let created: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT created_at FROM workspace_access WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(w.ws)
    .bind(w.person)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    let (s, v) =
        w.h.call(
            &w.owner_cookie,
            Method::GET,
            &format!("/v1/workspaces/{}/access", w.ws),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let p = row(&v, w.person);
    assert_eq!(p["status"], "active");
    let joined: chrono::DateTime<chrono::Utc> =
        serde_json::from_value(p["joined_at"].clone()).unwrap();
    assert_eq!(joined, created);
    // The org owner has access only through the org role: no direct row, no recorded start.
    let boss = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["implicit"] == json!(true))
        .unwrap();
    assert!(boss["joined_at"].is_null(), "{boss}");
    assert_eq!(boss["status"], "active");
}

/// (P5-REFINE-6, C-1086) `signed_in` is `users.last_login_at IS NOT NULL`, on direct and implicit
/// rows; the callers are unchanged (a member who does not own the workspace is refused).
#[tokio::test]
async fn signed_in_says_whether_the_person_has_signed_in_yet() {
    let w = world().await;
    sqlx::query("UPDATE users SET last_login_at = NULL")
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
        .bind(w.person)
        .execute(w.h.pool())
        .await
        .unwrap();
    let uri = format!("/v1/workspaces/{}/access", w.ws);
    let (s, v) = w.h.call(&w.owner_cookie, Method::GET, &uri, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(row(&v, w.person)["signed_in"], json!(true), "{v}");
    assert_eq!(row(&v, w.owner)["signed_in"], json!(false), "{v}");
    let boss = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["implicit"] == json!(true))
        .unwrap();
    assert_eq!(boss["signed_in"], json!(false), "{boss}");
    // The approver is not an owner of the workspace: refused as before.
    let (s, _) = w.h.call(&w.person_cookie, Method::GET, &uri, None).await;
    assert_eq!(s, 403);
}

#[tokio::test]
async fn a_suspension_refuses_every_family_and_restore_gives_it_back() {
    let w = world().await;
    for uri in w.family_uris(w.ws, w.agent) {
        let s = w.get_as_person(&uri).await;
        assert!(s != 404 && s != 403, "{uri} before: {s}");
    }
    assert_eq!(w.law(w.ws).await, (true, true, true));

    let (s, v) = w.set(&w.owner_cookie, w.person, "suspended").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "suspended");
    assert_eq!(v["role"], "approver", "the role is kept");

    for uri in w.family_uris(w.ws, w.agent) {
        let s = w.get_as_person(&uri).await;
        assert!(s == 404 || s == 403, "{uri} while suspended: {s}");
    }
    // A workspace API key of the person is refused too (not revoked).
    let (s, _) =
        w.h.get(
            &format!("/v1/workspaces/{}", w.ws),
            &[("authorization", &w.person_key)],
        )
        .await;
    assert_eq!(s.as_u16(), 404);
    // The access law every other surface re-checks (terminal, SSH, Files, previews, approvals,
    // fire-time authority) and the gateway's speaker law.
    assert_eq!(w.law(w.ws).await, (false, false, false));
    let revoked: bool = sqlx::query_scalar(
        "SELECT revoked_at IS NOT NULL FROM api_keys WHERE user_id = $1 AND workspace_id = $2",
    )
    .bind(w.person)
    .bind(w.ws)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(!revoked, "keys are refused, not revoked");
    // Approval mail and push no longer go to them.
    let deciders =
        aurora_db::repo::approval_notify::deciders_with(w.h.pool(), w.org, w.ws, None, false)
            .await
            .unwrap();
    assert!(deciders.iter().all(|d| d.user_id != w.person));

    // Other workspaces of the same org are unaffected.
    for uri in w.family_uris(w.other, w.other_agent) {
        let s = w.get_as_person(&uri).await;
        assert!(s != 404 && s != 403, "{uri} in the other workspace: {s}");
    }
    assert_eq!(w.law(w.other).await, (true, true, true));

    // Still listed, marked suspended.
    let (_, list) =
        w.h.call(
            &w.owner_cookie,
            Method::GET,
            &format!("/v1/workspaces/{}/access", w.ws),
            None,
        )
        .await;
    assert_eq!(row(&list, w.person)["status"], "suspended");

    // The same status again is a no-op.
    let (s, _) = w.set(&w.owner_cookie, w.person, "suspended").await;
    assert_eq!(s, 200);

    let (s, v) = w.set(&w.owner_cookie, w.person, "active").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "active");
    for uri in w.family_uris(w.ws, w.agent) {
        let s = w.get_as_person(&uri).await;
        assert!(s != 404 && s != 403, "{uri} after restore: {s}");
    }
    assert_eq!(w.law(w.ws).await, (true, true, true));
    let (s, _) =
        w.h.get(
            &format!("/v1/workspaces/{}", w.ws),
            &[("authorization", &w.person_key)],
        )
        .await;
    assert_eq!(s.as_u16(), 200);

    // Audit: one row each, security-critical; the no-op wrote none.
    let rows: Vec<(String, bool, Value)> = sqlx::query_as(
        "SELECT action, critical, metadata FROM audit_log
         WHERE action IN ('workspace_access.suspend', 'workspace_access.restore')
           AND workspace_id = $1 ORDER BY created_at",
    )
    .bind(w.ws)
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].0, "workspace_access.suspend");
    assert!(rows[0].1);
    assert_eq!(rows[0].2["change"]["status"], "active → suspended");
    assert_eq!(rows[1].0, "workspace_access.restore");
    assert_eq!(rows[1].2["change"]["status"], "suspended → active");
}

#[tokio::test]
async fn group_access_does_not_survive_a_suspension() {
    let w = world().await;
    let g = Uuid::now_v7();
    sqlx::query("INSERT INTO groups (id, org_id, name) VALUES ($1, $2, 'ops')")
        .bind(g)
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO group_members (group_id, org_id, user_id) VALUES ($1, $2, $3)")
        .bind(g)
        .bind(w.org)
        .bind(w.person)
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO group_workspace_access (group_id, workspace_id, org_id, role) \
         VALUES ($1, $2, $3, 'owner')",
    )
    .bind(g)
    .bind(w.ws)
    .bind(w.org)
    .execute(w.h.pool())
    .await
    .unwrap();
    assert!(
        aurora_db::authz::can_own(w.h.pool(), w.person, w.ws)
            .await
            .unwrap()
    );
    let (s, v) = w.set(&w.owner_cookie, w.person, "suspended").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.law(w.ws).await, (false, false, false));
    assert!(
        aurora_db::repo::groups::via_groups(w.h.pool(), w.org, w.person, &[w.ws])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        w.get_as_person(&format!("/v1/workspaces/{}", w.ws)).await,
        404
    );
    // Not listed among the people who derive access.
    let (_, list) =
        w.h.call(
            &w.owner_cookie,
            Method::GET,
            &format!("/v1/workspaces/{}/access?expand=groups", w.ws),
            None,
        )
        .await;
    let derived = list["derived_members"]["items"].as_array().unwrap();
    assert!(
        derived.iter().all(|d| d["user"]["id"] != json!(w.person)),
        "{list}"
    );
    // The group-derived row of someone else carries a joined date and `active`.
    let (s, _) = w.set(&w.owner_cookie, w.person, "active").await;
    assert_eq!(s, 200);
    let (_, list) =
        w.h.call(
            &w.owner_cookie,
            Method::GET,
            &format!("/v1/workspaces/{}/access?expand=groups", w.ws),
            None,
        )
        .await;
    let d = list["derived_members"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["user"]["id"] == json!(w.person))
        .unwrap();
    assert_eq!(d["status"], "active");
    assert!(d["joined_at"].is_string(), "{d}");
}

#[tokio::test]
async fn refusals_self_implicit_idp_unbound_and_non_owner() {
    let w = world().await;
    // Yourself.
    let (s, v) = w.set(&w.owner_cookie, w.owner, "suspended").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("cannot_suspend_self")),
        "{v}"
    );
    // An org admin (implicit owner) who also holds a direct row.
    let admin = w.h.user("admin@x.io", false).await;
    w.h.member(w.org, admin, "admin", "active").await;
    w.h.bind_ws(w.ws, admin, "member").await;
    let (s, v) = w.set(&w.owner_cookie, admin, "suspended").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("implicit_access")),
        "{v}"
    );
    // An IdP-claimed binding.
    let claimed = w.h.user("claimed@x.io", false).await;
    w.h.member(w.org, claimed, "member", "active").await;
    sqlx::query(
        "INSERT INTO workspace_access (workspace_id, user_id, role, idp_role, idp_source) \
         VALUES ($1, $2, 'member', 'member', 'idp:corp:eng')",
    )
    .bind(w.ws)
    .bind(claimed)
    .execute(w.h.pool())
    .await
    .unwrap();
    let (s, v) = w.set(&w.owner_cookie, claimed, "suspended").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("managed_by_idp")),
        "{v}"
    );
    // No direct binding (an org member with no access here).
    let nobody = w.h.user("nobody@x.io", false).await;
    w.h.member(w.org, nobody, "member", "active").await;
    let (s, v) = w.set(&w.owner_cookie, nobody, "suspended").await;
    assert_eq!(s, 404, "{v}");
    // A caller who is not a workspace owner (an approver).
    let (s, v) = w.set(&w.person_cookie, w.owner, "suspended").await;
    assert!(s == 403 || s == 404, "{s} {v}");
    // A suspended owner cannot act at all.
    let (s, _) = w.set(&w.owner_cookie, w.person, "suspended").await;
    assert_eq!(s, 200);
    w.h.call(
        &w.owner_cookie,
        Method::PUT,
        &format!("/v1/workspaces/{}/access/{}", w.ws, w.person),
        Some(json!({"role": "owner"})),
    )
    .await;
    let (s, _) = w.set(&w.person_cookie, w.owner, "suspended").await;
    assert_eq!(
        s, 404,
        "a suspended person (even with role owner) is not a member"
    );
    // PUT changed the role and left the suspension in place.
    let st: String = sqlx::query_scalar(
        "SELECT status FROM workspace_access WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(w.ws)
    .bind(w.person)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(st, "suspended");
}

#[tokio::test]
async fn their_speaker_turns_in_this_workspace_end_and_others_do_not() {
    let w = world().await;
    let mut turns = Vec::new();
    for (ws, agent) in [(w.ws, w.agent), (w.other, w.other_agent)] {
        let conv = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) \
             VALUES ($1, $2, $3, 'api', $1::text, now())",
        )
        .bind(conv)
        .bind(agent)
        .bind(ws)
        .execute(w.h.pool())
        .await
        .unwrap();
        let turn = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, \
             speaker_user_id, speaker_label, text) \
             VALUES ($1, $2, $3, 'turn', 'queued', '{}', 'person', $4, 'P', 'hi')",
        )
        .bind(turn)
        .bind(conv)
        .bind(agent)
        .bind(w.person)
        .execute(w.h.pool())
        .await
        .unwrap();
        turns.push(turn);
    }
    let (s, v) = w.set(&w.owner_cookie, w.person, "suspended").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        turn_status(&w, turns[0]).await,
        ("aborted".to_owned(), Some("user_suspended".to_owned()))
    );
    assert_eq!(turn_status(&w, turns[1]).await.0, "queued");
}

async fn turn_status(w: &World, t: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, error_code FROM turns WHERE id = $1")
        .bind(t)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn their_open_conversation_stream_ends() {
    let w = world().await;
    let (s, v) =
        w.h.call(
            &w.person_cookie,
            Method::POST,
            &format!("/v1/agents/{}/conversations/direct", w.agent),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let conv = v["id"].as_str().unwrap().to_owned();
    // Periodic re-check effectively off: only the access ping can end the stream.
    let app = test_router(
        w.h.state
            .clone()
            .with_stream_recheck(Duration::from_secs(3600)),
    );
    let res = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/conversations/{conv}/stream"))
                .header("cookie", &w.person_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let mut body = Body::new(res.into_body());
    let (s, _) = w.set(&w.owner_cookie, w.person, "suspended").await;
    assert_eq!(s, 200);
    // What the `aurora_auth_revoked { kind: "access" }` listener does on the NOTIFY the PATCH
    // committed.
    w.h.state
        .session_cache
        .invalidate(&json!({"kind": "access", "id": w.person, "org_id": w.org}));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let ended = loop {
        match tokio::time::timeout_at(deadline, body.frame()).await {
            Err(_) => break false,
            Ok(None) | Ok(Some(Err(_))) => break true,
            Ok(Some(Ok(_))) => {}
        }
    };
    assert!(ended, "the stream must end after the suspension");
}
