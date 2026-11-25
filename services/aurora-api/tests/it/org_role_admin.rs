#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SEC-153's proving tests for FIX-234 (D-477, C-1160, C-1161, D-480): an organization role
//! administers workspaces, it does not use them.
//!
//! - The owner's case: an org admin with no binding in Bob's personal workspace can neither read
//!   nor act there by any route or gateway path; they can still freeze Bob's agent.
//! - The personal-workspace rules (C-1161): only the person adds people, group bindings and
//!   raises; remove, suspend, delete stay with `can_administer`; the person is protected.
//! - Self-grants: session-only, audited critical, owners told; a group the admin belongs to
//!   counts; someone else does not.
//! - The admin's own setups stop: a creator-anchored schedule is disabled at its next fire.
//! - `Workspace` GET by an administrator: `my_role` null, `administers: true`, `audience`.

use crate::common::*;
use aurora_api::domains::auth::mail::{Mail, MailError, Mailer};
use aurora_db::repo::gateway;
use axum::http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

/// Bob's personal workspace: the person bound `owner` (D-480), an agent, the admin with no
/// binding, and a workspace-scope connection of the org.
struct OwnerCase {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    bob: Uuid,
    bob_cookie: String,
    admin: Uuid,
    admin_cookie: String,
    admin_org_key: String,
    conv: Uuid,
    conn: Uuid,
}

async fn owner_case() -> OwnerCase {
    let h = Harness::new().await;
    let org = h.org().await;
    let bob = h.user("bob@x.io", false).await;
    h.member(org, bob, "member", "active").await;
    let ws = h.workspace_owned_by(org, bob).await;
    // The personal-workspace marker (a personal workspace as the creation path leaves it).
    sqlx::query("UPDATE workspaces SET personal_owner_user_id = $2, personal = true WHERE id = $1")
        .bind(ws)
        .bind(bob)
        .execute(h.pool())
        .await
        .unwrap();
    let agent = h.byo_agent(org, ws, "bot", "Bot").await;
    let admin = h.user("admin@x.io", false).await;
    h.member(org, admin, "admin", "active").await;
    let (_, bob_tok) = h.good_session(bob).await;
    let (_, admin_tok) = h.good_session(admin).await;
    let (_, admin_ok) = h.api_key(admin, org, None, false).await;
    // Bob's direct conversation.
    let (s, _, v) = h
        .send(
            Method::POST,
            &format!("/v1/agents/{agent}/conversations/direct"),
            &[("cookie", &cookie(&bob_tok)), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let conv: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // A workspace-scope connection (the org's own accounts are administered, this one is not).
    let conn = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'acme_app','acme','1.0.0','c','active','x')")
        .bind(conn)
        .bind(org)
        .bind(ws)
        .execute(h.pool())
        .await
        .unwrap();
    OwnerCase {
        h,
        org,
        ws,
        agent,
        bob,
        bob_cookie: cookie(&bob_tok),
        admin,
        admin_cookie: cookie(&admin_tok),
        admin_org_key: bearer(&admin_ok),
        conv,
        conn,
    }
}

impl OwnerCase {
    async fn call(&self, who: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        self.h.call(who, m, uri, body).await
    }

    async fn req(&self, who: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", who), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    async fn approval(&self, conv: Option<Uuid>) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO approvals (id, org_id, workspace_id, agent_id, gateway_instance_id, conversation_id,
                method, host, path, summary, status, deadline)
             VALUES ($1,$2,$3,$4,$5,$6,'POST','api.example.com','/x',
                '{\"title\":\"T\",\"fields\":[],\"text\":null,\"link\":null,\"group\":null,\"source\":\"generic\"}',
                'pending', now() + interval '5 minutes')",
        )
        .bind(id)
        .bind(self.org)
        .bind(self.ws)
        .bind(self.agent)
        .bind(Uuid::now_v7())
        .bind(conv)
        .execute(self.h.pool())
        .await
        .unwrap();
        id
    }
}

/// The owner's case: every use is refused with `workspace_access_required`; administering
/// (freeze) works and names `org_admin` as its authority.
#[tokio::test]
async fn an_admin_without_own_access_cannot_use_bobs_personal_workspace() {
    let f = owner_case().await;
    let ws = f.ws.to_string();
    let agent = f.agent.to_string();
    let conv = f.conv.to_string();

    // Using the agent: post a message, open a conversation, create a schedule. (`POST …/
    // conversations` makes `api` conversations and refuses sessions before the workspace is
    // looked at; the org key below proves the same denial on the workspace's law.)
    for (m, uri, body) in [
        (
            Method::POST,
            format!("/v1/agents/{agent}/conversations/direct"),
            None,
        ),
        (
            Method::POST,
            format!("/v1/agents/{agent}/schedules"),
            Some(json!({"name": "s", "prompt": "p", "cron": "0 9 * * *"})),
        ),
    ] {
        let (s, v) = f.req(&f.admin_cookie, m, &uri, body).await;
        assert_eq!(
            (s, code(&v)),
            (403, "workspace_access_required"),
            "{uri}: {v}"
        );
    }
    let (s, v) = f
        .req(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/agents/{agent}/conversations"),
            Some(json!({"external_ref": "x"})),
        )
        .await;
    assert_eq!(
        (s, code(&v)),
        (403, "forbidden"),
        "api conversations are key-only: {v}"
    );
    {
        let (s, _, v) =
            f.h.send(
                Method::POST,
                &format!("/v1/agents/{agent}/conversations"),
                &[("authorization", &f.admin_org_key), ("origin", ORIGIN)],
                Some(json!({"external_ref": "x"})),
            )
            .await;
        assert_eq!(
            (s.as_u16(), code(&v)),
            (403, "workspace_access_required"),
            "{v}"
        );
    }

    // Reading Bob's direct conversation, its export and the workspace search.
    for (m, uri) in [
        (Method::GET, format!("/v1/conversations/{conv}")),
        (Method::GET, format!("/v1/conversations/{conv}/export")),
        (Method::GET, format!("/v1/conversations/{conv}/events")),
        (
            Method::GET,
            format!("/v1/workspaces/{ws}/messages/search?q=hi"),
        ),
    ] {
        let (s, v) = f.req(&f.admin_cookie, m, &uri, None).await;
        assert_eq!(
            (s, code(&v)),
            (403, "workspace_access_required"),
            "{uri}: {v}"
        );
    }

    // The gateway's law agrees: the admin holds no `can_use`.
    assert!(
        !gateway::can_use(f.h.pool(), f.ws, f.admin).await.unwrap(),
        "the gateway must not accept the org admin"
    );

    // An approval of Bob's agent is invisible in the admin's feed; deciding it is 403.
    let appr = f.approval(Some(f.conv)).await;
    let (s, v) = f
        .call(&f.admin_cookie, Method::GET, "/v1/approvals/pending", None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        !v["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == json!(appr.to_string())),
        "{v}"
    );
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/approvals/{appr}/decision"),
            Some(json!({"decision": "approve"})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "workspace_access_required"), "{v}");

    // Granting Bob's workspace connection to another agent is refused (a workspace's
    // connection needs own `can_own`).
    let (s, v) = f
        .req(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/connections/{}/grants", f.conn),
            Some(json!({"agent_ids": [f.agent], "mode": "full"})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "workspace_access_required"), "{v}");

    // Freezing Bob's agent works — administering, authority `org_admin`.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/agents/{agent}/freeze"),
            Some(json!({"reason": "r"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["frozen"]["authority"], "org_admin", "{v}");

    // The org key is refused exactly like the session.
    let (s, _, v) =
        f.h.send(
            Method::GET,
            &format!("/v1/conversations/{conv}"),
            &[("authorization", &f.admin_org_key), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(
        (s.as_u16(), code(&v)),
        (403, "workspace_access_required"),
        "{v}"
    );
}

/// The personal-workspace rules (C-1161): the admin's adds and raises are refused, their
/// remove/suspend/delete work, the person is protected and can add people.
#[tokio::test]
async fn personal_workspace_adds_are_the_persons_alone() {
    let f = owner_case().await;
    let ws = f.ws;
    let carol = f.h.user("carol@x.io", false).await;
    f.h.member(f.org, carol, "member", "active").await;

    // Adding a person: 403 `personal_workspace_owner_only`.
    let uri = format!("/v1/workspaces/{ws}/access/{carol}");
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &uri,
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "personal_workspace_owner_only"), "{v}");
    // By e-mail: the same refusal.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/workspaces/{ws}/access/by-email"),
            Some(json!({"email": "new@x.io", "role": "member"})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "personal_workspace_owner_only"), "{v}");
    // A group binding: the same refusal.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/orgs/{}/groups", f.org),
            Some(json!({"name": "g"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let g: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/groups/{g}"),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "personal_workspace_owner_only"), "{v}");

    // The person adds Carol; then the admin's raise is refused, the remove works.
    let (s, v) = f
        .call(
            &f.bob_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/{carol}"),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/{carol}"),
            Some(json!({"role": "owner"})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "personal_workspace_owner_only"), "{v}");
    let (s, _) = f
        .call(
            &f.admin_cookie,
            Method::DELETE,
            &format!("/v1/workspaces/{ws}/access/{carol}"),
            None,
        )
        .await;
    assert_eq!(s, 204);

    // Suspending Carol works (not a raise, an administer act).
    f.h.bind_ws(ws, carol, "member").await;
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PATCH,
            &format!("/v1/workspaces/{ws}/access/{carol}"),
            Some(json!({"status": "suspended"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // The person is protected: never lowered, suspended or removed by the admin.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PATCH,
            &format!("/v1/workspaces/{ws}/access/{}", f.bob),
            Some(json!({"status": "suspended"})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "personal_workspace_person"), "{v}");
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::DELETE,
            &format!("/v1/workspaces/{ws}/access/{}", f.bob),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (409, "personal_workspace_person"), "{v}");

    // Deleting the workspace works (administering) — the slug confirms.
    let slug: String = sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(f.h.pool())
        .await
        .unwrap();
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::DELETE,
            &format!("/v1/workspaces/{ws}"),
            Some(json!({"confirm": slug})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
}

/// A self-grant needs a session, writes the critical audit row and tells the owners; a group the
/// admin belongs to counts; granting someone else is not a self-grant. The shared workspace (Bob
/// its own-right owner) is the stage: in Bob's personal workspace the person-alone rule would
/// refuse every add before the self-grant rules come into play.
#[tokio::test]
async fn self_grant_rules() {
    let f = owner_case().await;
    let ws = f.h.workspace_owned_by(f.org, f.bob).await;

    // An API key is refused.
    let (s, _, v) =
        f.h.send(
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/{}", f.admin),
            &[("authorization", &f.admin_org_key), ("origin", ORIGIN)],
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "session_required"), "{v}");

    // By session: 200, the critical audit row, and Bob (the workspace's owner) is told.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/{}", f.admin),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'workspace_access.self_grant' \
         AND workspace_id = $1",
    )
    .bind(ws)
    .fetch_all(f.h.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["via"], "user", "{rows:?}");
    let mail = f.h.mail.sent();
    assert!(
        mail.iter()
            .any(|m| m.to == "bob@x.io" && m.body.contains("gave themselves member access to")),
        "{mail:?}"
    );
    // (FIX-234 review 5) The sentence names the actor by display name, never the e-mail.
    let m = mail.iter().find(|m| m.to == "bob@x.io").unwrap();
    assert!(
        m.body
            .starts_with("U (admin) gave themselves member access to"),
        "{m:?}"
    );
    assert!(!m.body.contains("admin@x.io"), "{m:?}");

    // A group the admin belongs to counts as a self-grant too (`via: "group"`).
    let (s, v) = f
        .call(
            &f.bob_cookie,
            Method::DELETE,
            &format!("/v1/workspaces/{ws}/access/{}", f.admin),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/orgs/{}/groups", f.org),
            Some(json!({"name": "admins-g"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let g: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // Group membership is an organization act (org owner/admin), not the workspace's.
    let (s, _) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/groups/{g}/members/{}", f.admin),
            None,
        )
        .await;
    assert_eq!(s, 200);
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/groups/{g}"),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let via: Vec<String> = sqlx::query_scalar(
        "SELECT metadata->>'via' FROM audit_log WHERE action = 'workspace_access.self_grant' \
         AND workspace_id = $1 ORDER BY created_at",
    )
    .bind(ws)
    .fetch_all(f.h.pool())
    .await
    .unwrap();
    assert_eq!(via, vec!["user", "group"], "{via:?}");

    // Adding someone else is not a self-grant: no row, no mail.
    let carol = f.h.user("carol2@x.io", false).await;
    f.h.member(f.org, carol, "member", "active").await;
    let before = f.h.mail.sent().len();
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/{carol}"),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'workspace_access.self_grant' \
         AND workspace_id = $1 AND target_id = $2",
    )
    .bind(ws)
    .bind(carol.to_string())
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0);
    assert_eq!(f.h.mail.sent().len(), before);
}

struct FailMailer;

#[async_trait::async_trait]
impl Mailer for FailMailer {
    async fn send(&self, _: Mail) -> Result<(), MailError> {
        Err(MailError("smtp down".into()))
    }
}

/// (FIX-234 review round 3) A failing self-grant notice after the commit must not turn the
/// change into an error: the change answers 200, the member is in and the critical audit row
/// stands; only the notice is lost (logged).
#[tokio::test]
async fn a_failing_self_grant_notice_keeps_the_group_change() {
    let mut f = owner_case().await;
    let ws = f.h.workspace_owned_by(f.org, f.bob).await;
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/orgs/{}/groups", f.org),
            Some(json!({"name": "notice-g"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let g: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // The group is bound with `member` before the admin belongs to it: the binding itself is
    // no self-grant (the admin is not a member yet), the add below is.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/groups/{g}"),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // The failing mailer (the calendar suite's FailMailer pattern); the router serves from the
    // state, so it is rebuilt with it.
    f.h.state =
        f.h.state
            .clone()
            .with_mailer(std::sync::Arc::new(FailMailer));
    f.h.app = test_router(f.h.state.clone());

    // The admin adds themselves: a self-grant (session, critical row, notice) — and the
    // notice's mail fails.
    let before = f.h.mail.sent().len();
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/groups/{g}/members/{}", f.admin),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM group_members WHERE group_id = $1 AND user_id = $2",
    )
    .bind(g)
    .bind(f.admin)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "the change committed");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'workspace_access.self_grant' \
         AND workspace_id = $1 AND metadata->>'via' = 'group'",
    )
    .bind(ws)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1, "the critical audit row stands");
    assert_eq!(
        f.h.mail.sent().len(),
        before,
        "the failing mailer sent nothing"
    );
}

/// The admin's own setups stop: a creator-anchored schedule of an admin without own access is
/// disabled at its next fire (`authorization`).
#[tokio::test]
async fn an_admins_schedule_stops_at_its_next_fire() {
    let f = owner_case().await;
    // Bob creates the schedule; the "admin" is its creator, as an admin's leftover setup is.
    let (s, v) = f
        .call(
            &f.bob_cookie,
            Method::POST,
            &format!("/v1/agents/{}/schedules", f.agent),
            Some(json!({"name": "s", "prompt": "p", "cron": "0 9 * * *"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE schedules SET creator_user_id = $2 WHERE id = $1")
        .bind(id)
        .bind(f.admin)
        .execute(f.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE schedules SET next_fire_at = now() - interval '2 days' WHERE id = $1")
        .bind(id)
        .execute(f.h.pool())
        .await
        .unwrap();
    let mut tx = f.h.pool().begin().await.unwrap();
    aurora_api::domains::schedules::fire::arm9(&f.h.state, &mut tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let outcome: String = sqlx::query_scalar(
        "SELECT outcome FROM schedule_runs WHERE schedule_id = $1 \
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(id)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(outcome, "skipped_unauthorized");
    let (enabled, reason): (bool, Option<String>) =
        sqlx::query_as("SELECT enabled, disabled_reason FROM schedules WHERE id = $1")
            .bind(id)
            .fetch_one(f.h.pool())
            .await
            .unwrap();
    assert!(!enabled);
    assert_eq!(reason.as_deref(), Some("authorization"));
}

/// `Workspace` GET by an administrator: `my_role` null, `administers: true`, and the `audience`
/// counts own-right people (suspended excluded) with at most five owners.
#[tokio::test]
async fn workspace_get_by_an_administrator_names_the_audience() {
    let f = owner_case().await;
    let ws = f.ws;
    // Carol reaches the workspace through a group; Dave is suspended; six more owners.
    let carol = f.h.user("carol3@x.io", false).await;
    f.h.member(f.org, carol, "member", "active").await;
    let dave = f.h.user("dave@x.io", false).await;
    f.h.member(f.org, dave, "member", "active").await;
    let mut more = Vec::new();
    for i in 0..6 {
        let u = f.h.user(&format!("o{i}@x.io"), false).await;
        f.h.member(f.org, u, "member", "active").await;
        f.h.bind_ws(ws, u, "owner").await;
        more.push(u);
    }
    f.h.bind_ws(ws, dave, "member").await;
    sqlx::query(
        "UPDATE workspace_access SET status = 'suspended' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(dave)
    .execute(f.h.pool())
    .await
    .unwrap();
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/orgs/{}/groups", f.org),
            Some(json!({"name": "g3"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let g: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, _) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/groups/{g}/members/{carol}"),
            None,
        )
        .await;
    assert_eq!(s, 200);
    let (s, v) = f
        .call(
            &f.bob_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/groups/{g}"),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // The administrator: no own access, administers, the audience counts 8 people
    // (bob, carol, six owners; dave suspended excluded) and caps the owners at five.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::GET,
            &format!("/v1/workspaces/{ws}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["my_role"].is_null(), "{v}");
    assert_eq!(v["administers"], json!(true), "{v}");
    assert_eq!(v["audience"]["people"], json!(8), "{v}");
    assert_eq!(v["audience"]["owners"].as_array().unwrap().len(), 5, "{v}");
    assert_eq!(v["audience"]["owners_total"], json!(7), "{v}");

    // An own-right owner reads in full and is named first.
    let (s, v) = f
        .call(
            &f.bob_cookie,
            Method::GET,
            &format!("/v1/workspaces/{ws}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["my_role"], json!("owner"), "{v}");
    assert_eq!(v["administers"], json!(true), "{v}");
    assert_eq!(
        v["audience"]["owners"][0]["id"],
        json!(f.bob.to_string()),
        "{v}"
    );
}

/// The access report (C-1163): a seeded instance lists each of the five gaps and writes nothing.
#[tokio::test]
async fn the_access_report_lists_every_gap_and_writes_nothing() {
    let f = owner_case().await;
    let org = f.org;
    // 1: an ownerless shared workspace.
    let orphan = f.h.workspace(org).await;
    // 2: a creator-anchored schedule whose creator (the admin) has no own access, in the
    //    ownerless workspace.
    let agent2 = f.h.byo_agent(org, orphan, "bot2", "Bot2").await;
    sqlx::query("INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone, creator_user_id) VALUES ($1,$2,'nightly','p','0 9 * * *','UTC',$3)")
        .bind(Uuid::now_v7())
        .bind(agent2)
        .bind(f.admin)
        .execute(f.h.pool())
        .await
        .unwrap();
    // 3: a space binding bound by the admin, who has no access to the agent's workspace.
    sqlx::query("INSERT INTO connectors (id, token_hash, display_name, manifest, status) VALUES ('mattermost', $1, 'Mattermost', '{}', 'active')")
        .bind(vec![0u8; 32])
        .execute(f.h.pool())
        .await
        .unwrap();
    let integ = Uuid::now_v7();
    sqlx::query("INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label, config, credentials_ref, status) VALUES ($1,$2,'mattermost','T1','T1','{}','x','active')")
        .bind(integ)
        .bind(org)
        .execute(f.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO space_bindings (id, integration_id, space_ref, space_label, agent_id, listen, bound_by_user_id) VALUES ($1,$2,'ch/town','town-square',$3,'mentions',$4)")
        .bind(Uuid::now_v7())
        .bind(integ)
        .bind(f.agent)
        .bind(f.admin)
        .execute(f.h.pool())
        .await
        .unwrap();
    // 4: a live `auo_` org key of the admin.
    let key = Uuid::now_v7();
    sqlx::query("INSERT INTO api_keys (id, org_id, workspace_id, user_id, kind, name, token_hash, display_prefix) VALUES ($1,$2,NULL,$3,'user','org key',sha256('auo'::bytea),'auo_x')")
        .bind(key)
        .bind(org)
        .bind(f.admin)
        .execute(f.h.pool())
        .await
        .unwrap();
    // 5: a pending approval in the ownerless workspace (no own-right decider).
    let appr = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, gateway_instance_id, conversation_id,
            method, host, path, summary, status, deadline)
         VALUES ($1,$2,$3,$4,$5,NULL,'POST','api.example.com','/x',
            '{\"title\":\"T\",\"fields\":[],\"text\":null,\"link\":null,\"group\":null,\"source\":\"generic\"}',
            'pending', now() + interval '5 minutes')",
    )
    .bind(appr)
    .bind(org)
    .bind(orphan)
    .bind(agent2)
    .bind(Uuid::now_v7())
    .execute(f.h.pool())
    .await
    .unwrap();

    // Row counts of everything the report must not touch, before and after.
    async fn counts(p: &aurora_db::PgPool) -> (i64, i64, i64, i64, i64) {
        (
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspaces")
                .fetch_one(p)
                .await
                .unwrap(),
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM schedules")
                .fetch_one(p)
                .await
                .unwrap(),
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM approvals")
                .fetch_one(p)
                .await
                .unwrap(),
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM api_keys")
                .fetch_one(p)
                .await
                .unwrap(),
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM space_bindings")
                .fetch_one(p)
                .await
                .unwrap(),
        )
    }
    let before = counts(f.h.pool()).await;

    let report = aurora_api::access_report::run(&f.h.state).await.unwrap();
    let r = report
        .orgs
        .iter()
        .find(|o| o.org_id == org)
        .expect("the org is listed");

    // 1: the orphan (a shared workspace with no active own-right owner) — not Bob's personal
    //    workspace, whose person is its owner.
    assert!(
        r.ownerless
            .iter()
            .any(|w| w.workspace_id == orphan && w.personal.is_none()),
        "{:?}",
        r.ownerless
    );
    assert!(
        !r.ownerless.iter().any(|w| w.workspace_id == f.ws),
        "the personal workspace has its person as owner: {:?}",
        r.ownerless
    );
    // 2: the admin's schedule is named with its creator.
    let sched = r
        .failing_automations
        .iter()
        .find(|a| a.kind == "schedule")
        .expect("the failing schedule is listed");
    assert_eq!(sched.creator.as_ref().map(|(u, _)| *u), Some(f.admin));
    // 3: the space binding tied to the admin is named.
    assert!(
        r.channel_ties
            .iter()
            .any(|t| t.kind == "space_binding" && t.user_id == f.admin),
        "{:?}",
        r.channel_ties
    );
    // 4: the org key is named with its creator.
    assert!(
        r.org_keys
            .iter()
            .any(|k| k.key_id == key && k.creator_id == f.admin),
        "{:?}",
        r.org_keys
    );
    // 5: the undecidable approval is named.
    assert!(
        r.undecidable.iter().any(|a| a.approval_id == appr),
        "{:?}",
        r.undecidable
    );
    // The text rendering names the organization.
    let text = aurora_api::access_report::text(&report);
    assert!(text.contains("organization"), "{text}");

    // Nothing was written.
    let after = counts(f.h.pool()).await;
    assert_eq!(before, after);
}

/// (FIX-234 review 7) The report's transaction is enforced read-only: the report itself runs
/// inside it and a write added there is rejected by PostgreSQL instead of committing.
#[tokio::test]
async fn the_access_reports_transaction_rejects_writes() {
    let f = owner_case().await;
    let mut tx = f.h.pool().begin().await.unwrap();
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *tx)
        .await
        .unwrap();
    // The report itself is all SELECTs, so it completes inside the read-only transaction.
    let report = aurora_api::access_report::run_in(&mut tx).await.unwrap();
    assert!(
        report.orgs.iter().any(|o| o.org_id == f.org),
        "{:?}",
        report.orgs.iter().map(|o| o.org_id).collect::<Vec<_>>()
    );
    // A write in the same transaction fails.
    let err =
        sqlx::query("INSERT INTO workspaces (id, org_id, name, slug) VALUES ($1, $2, 'x', 'x')")
            .bind(Uuid::now_v7())
            .bind(f.org)
            .execute(&mut *tx)
            .await;
    assert!(
        err.is_err(),
        "a write inside the report's READ ONLY transaction must fail"
    );
}

// ---------------------------------------------------------------- review fixes

/// The acme app's package manifest: tools `list_items`, `get_item` (read) and `create_item`
/// (write) so grant bodies can name real tools. (`route_inventory.rs`'s behavioural walk
/// installs it too — one registry, one manifest.)
pub(crate) const YAML: &str = r#"package: { id: acme, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
permissions:
  token_hosts: [accounts.example.com]
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
          - { id: list_items, name: List, host: api.acme.example, path: /items, methods: [GET] }
          - { id: get_item, name: Get, host: api.acme.example, path: "/items/*", methods: [GET] }
      write:
        items:
          - { id: create_item, name: Create, host: api.acme.example, path: /items, methods: [POST] }
    hosts:
      - match: { host: "api.acme.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

/// (FIX-234 review 1a) Adding yourself to a group ALREADY bound to a workspace is a self-grant,
/// exactly like binding the group after joining it: an API key is refused (`session_required`),
/// a session writes the critical `workspace_access.self_grant` row (`via: "group"`) and tells the
/// workspace's other owners — and the access is real afterwards.
#[tokio::test]
async fn change_members_self_grant_rules_on_an_already_bound_group() {
    let f = owner_case().await;
    // A shared workspace Bob owns in his own right: the notice's recipient.
    let ws = f.h.workspace_owned_by(f.org, f.bob).await;
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/orgs/{}/groups", f.org),
            Some(json!({"name": "bound-g"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let g: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // Bind first, join second (the unsafe order the reviews found). The admin is not yet a
    // member, so the binding itself is no self-grant.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/groups/{g}"),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'workspace_access.self_grant' \
         AND workspace_id = $1",
    )
    .bind(ws)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0);

    // An API key is refused: joining a bound group is a self-grant and needs a session.
    let (s, _, v) =
        f.h.send(
            Method::PUT,
            &format!("/v1/groups/{g}/members/{}", f.admin),
            &[("authorization", &f.admin_org_key), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "session_required"), "{v}");

    // By session: allowed, the critical row and the notice to Bob.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/groups/{g}/members/{}", f.admin),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT target_id::text, metadata->>'via' FROM audit_log \
         WHERE action = 'workspace_access.self_grant' AND workspace_id = $1",
    )
    .bind(ws)
    .fetch_all(f.h.pool())
    .await
    .unwrap();
    assert_eq!(rows, vec![(g.to_string(), "group".into())], "{rows:?}");
    let mail = f.h.mail.sent();
    assert!(
        mail.iter()
            .any(|m| m.to == "bob@x.io" && m.body.contains("gave themselves member access to")),
        "{mail:?}"
    );

    // The access is real: the group now carries the admin into the workspace as `member`.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::GET,
            &format!("/v1/workspaces/{ws}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["my_role"], "member", "{v}");
}

/// (FIX-234 review 1b) A group bound to a PERSONAL workspace: only its person adds people to it,
/// by any caller — an administrator adding anyone (even the person) is 403
/// `personal_workspace_owner_only`, the person themselves succeeds, and a plain member without
/// admin rights is unaffected.
#[tokio::test]
async fn change_members_into_a_group_bound_to_a_personal_workspace() {
    let f = owner_case().await;
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/orgs/{}/groups", f.org),
            Some(json!({"name": "personal-g"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let g: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // The person binds the group to their personal workspace (C-1161: an add is the person's).
    let (s, v) = f
        .call(
            &f.bob_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{}/access/groups/{g}", f.ws),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    let carol = f.h.user("carol-r1@x.io", false).await;
    f.h.member(f.org, carol, "member", "active").await;

    // The person is an org admin too (group membership is an organization act — a plain member
    // could not add anyone to any group); the personal-workspace rule is what else binds them.
    sqlx::query("UPDATE org_members SET role = 'admin' WHERE org_id = $1 AND user_id = $2")
        .bind(f.org)
        .bind(f.bob)
        .execute(f.h.pool())
        .await
        .unwrap();

    // An administrator adding anyone — another member or the person — is refused.
    for target in [carol, f.bob] {
        let (s, v) = f
            .call(
                &f.admin_cookie,
                Method::PUT,
                &format!("/v1/groups/{g}/members/{target}"),
                None,
            )
            .await;
        assert_eq!((s, code(&v)), (403, "personal_workspace_owner_only"), "{v}");
    }

    // The person themselves can.
    let (s, v) = f
        .call(
            &f.bob_cookie,
            Method::PUT,
            &format!("/v1/groups/{g}/members/{carol}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // A plain member without admin rights is unaffected (still refuses to edit membership).
    let (s, v) =
        f.h.call_tok(
            &f.h.good_session(carol).await.1,
            Method::PUT,
            &format!("/v1/groups/{g}/members/{}", f.bob),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
}

/// (FIX-234 review 2) Removing a group binding stays with `can_administer` (C-1160/C-1161): an
/// org administrator without any binding unbinds a group (audited) — and still cannot ADD one
/// to a personal workspace (C-1161: an add or a raise is the person's).
#[tokio::test]
async fn an_administrator_unbinds_a_group_without_own_access() {
    let f = owner_case().await;
    let ws = f.h.workspace_owned_by(f.org, f.bob).await;
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/orgs/{}/groups", f.org),
            Some(json!({"name": "unbind-g"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let g: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // Bob (the workspace's own-right owner) binds the group.
    let (s, v) = f
        .call(
            &f.bob_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{ws}/access/groups/{g}"),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // The administrator has no binding in the workspace and still removes the binding.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::DELETE,
            &format!("/v1/workspaces/{ws}/access/groups/{g}"),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM group_workspace_access WHERE group_id = $1 AND workspace_id = $2",
    )
    .bind(g)
    .bind(ws)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'group_access.remove' \
         AND workspace_id = $1 AND target_id = $2",
    )
    .bind(ws)
    .bind(g.to_string())
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);

    // Adding a binding to a personal workspace is still the person's alone (C-1161).
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PUT,
            &format!("/v1/workspaces/{}/access/groups/{g}", f.ws),
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "personal_workspace_owner_only"), "{v}");
}

/// (FIX-234 review 3) An organization account's grants are its org's owners/admins (C-1160): an
/// administrator without own access revokes a grant of an org-scope connection (the
/// incident-response path, audited) and grants it in bulk; a workspace-scoped connection stays
/// own `can_own`, and the administrator still cannot read the agent itself.
#[tokio::test]
async fn an_admin_revokes_and_grants_an_org_account_without_own_access() {
    let f = owner_case().await;
    // The app's catalogue: without a registered package every grant is `custom_only` with an
    // empty tool list (SEC-67), so the bulk grant below needs the acme app registered.
    crate::plugins_lifecycle::setup(&f.h)
        .await
        .install(YAML)
        .await;
    // An org-scope connection (workspace_id NULL) with a `full` grant on Bob's agent.
    let conn = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,NULL,'acme_app','acme','1.0.0','c','active','x')")
        .bind(conn)
        .bind(f.org)
        .execute(f.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode) VALUES ($1,$2,$3,$4,'full')")
        .bind(Uuid::now_v7())
        .bind(f.ws)
        .bind(f.agent)
        .bind(conn)
        .execute(f.h.pool())
        .await
        .unwrap();

    // Reading the agent itself stays refused: the administrator manages, but is not a person.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::GET,
            &format!("/v1/agents/{}", f.agent),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (403, "workspace_access_required"), "{v}");

    // Revoke the grant: 204, the grant row is gone, audited on the agent.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::DELETE,
            &format!("/v1/agents/{}/grants/connections/{}", f.agent, conn),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM grants WHERE agent_id = $1 AND connection_id = $2",
    )
    .bind(f.agent)
    .bind(conn)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'grant.delete' AND target_id = $1",
    )
    .bind(f.agent.to_string())
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);

    // Grant it again in bulk: the credential-side route of the org's account.
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::POST,
            &format!("/v1/connections/{}/grants", conn),
            Some(json!({"agent_ids": [f.agent], "mode": "custom", "allow_tools": ["create_item"]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM grants WHERE agent_id = $1 AND connection_id = $2 AND mode = 'custom'",
    )
    .bind(f.agent)
    .bind(conn)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);

    // A workspace-scoped connection stays own `can_own`: the same administrator is refused.
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode) VALUES ($1,$2,$3,$4,'full')")
        .bind(Uuid::now_v7())
        .bind(f.ws)
        .bind(f.agent)
        .bind(f.conn)
        .execute(f.h.pool())
        .await
        .unwrap();
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::DELETE,
            &format!("/v1/agents/{}/grants/connections/{}", f.agent, f.conn),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (403, "workspace_access_required"), "{v}");
}

/// (FIX-234 review 4) A suspended person's personal workspace keeps its owner binding (the
/// migration backfills one regardless of the membership status), the binding grants nothing
/// while the membership is suspended, and restoring the membership re-activates the person's
/// access — and re-binds them when the binding is missing entirely.
#[tokio::test]
async fn a_suspended_persons_personal_workspace_keeps_its_owner() {
    let f = owner_case().await;
    let (ws, bob) = (f.ws, f.bob);
    f.h.set_member_status(f.org, bob, "suspended").await;

    // The owner binding survives the suspension, but grants nothing: the person has no row in
    // `own_workspace_access` (every binding is gated on the active membership).
    let (role, status): (String, String) = sqlx::query_as(
        "SELECT role, status FROM workspace_access WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(bob)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!((role.as_str(), status.as_str()), ("owner", "active"));
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM own_workspace_access WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(bob)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0);

    // Restore the membership: the person has access again, as owner.
    let uri = format!("/v1/orgs/{}/members/{}", f.org, bob);
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PATCH,
            &uri,
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM own_workspace_access WHERE workspace_id = $1 AND user_id = $2 \
         AND role = 'owner'",
    )
    .bind(ws)
    .bind(bob)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);

    // A person whose binding went missing while they were away is re-bound as owner, audited.
    sqlx::query("DELETE FROM workspace_access WHERE workspace_id = $1 AND user_id = $2")
        .bind(ws)
        .bind(bob)
        .execute(f.h.pool())
        .await
        .unwrap();
    f.h.set_member_status(f.org, bob, "suspended").await;
    let (s, v) = f
        .call(
            &f.admin_cookie,
            Method::PATCH,
            &uri,
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (role, status): (String, String) = sqlx::query_as(
        "SELECT role, status FROM workspace_access WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(bob)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!((role.as_str(), status.as_str()), ("owner", "active"));
    let from: Vec<String> = sqlx::query_scalar(
        "SELECT metadata->>'from' FROM audit_log WHERE action = 'workspace_access.personal_owner' \
         AND workspace_id = $1 AND target_id = $2 ORDER BY created_at",
    )
    .bind(ws)
    .bind(bob.to_string())
    .fetch_all(f.h.pool())
    .await
    .unwrap();
    assert_eq!(from, vec!["none".to_string()], "{from:?}");
}
