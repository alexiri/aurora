#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! RFE-006 (D-50, SEC-45): freeze / unfreeze of agents, workspaces and organizations — authority
//! ordering, idempotency, audit, and the effects on turns, approvals, due work and the doors
//! that start agent work.

use crate::common::*;
use crate::hosted::{Live, Rig, rig};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

struct W {
    r: Rig,
    /// Workspace owner by binding (acts as `workspace_owner`).
    ws_owner: Uuid,
    /// Org owner (acts as `org_admin`; may freeze the organization).
    org_owner: Uuid,
    /// Org admin (acts as `org_admin`; may not freeze the organization).
    org_admin: Uuid,
    /// Instance admin, not a member of the org.
    ia: Uuid,
    /// Plain workspace member (the hosted rig's user).
    member: Uuid,
}

async fn world() -> W {
    let r = rig().await;
    let mk = |mail: &'static str| {
        let h = &r.h;
        async move { h.user(mail, mail.starts_with("ia")).await }
    };
    let ws_owner = mk("wso@x.io").await;
    let org_owner = mk("oo@x.io").await;
    let org_admin = mk("oa@x.io").await;
    let ia = mk("ia@x.io").await;
    r.h.member(r.org, ws_owner, "member", "active").await;
    r.h.bind_ws(r.ws, ws_owner, "owner").await;
    r.h.member(r.org, org_owner, "owner", "active").await;
    r.h.member(r.org, org_admin, "admin", "active").await;
    let member: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'm@x.io'")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    W {
        r,
        ws_owner,
        org_owner,
        org_admin,
        ia,
        member,
    }
}

impl W {
    async fn call(&self, who: Uuid, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (_, tok) = self.r.h.good_session(who).await;
        let (s, _, v) = self
            .r
            .h
            .send(
                m,
                uri,
                &[("cookie", &cookie(&tok)), ("origin", ORIGIN)],
                body,
            )
            .await;
        (s.as_u16(), v)
    }

    async fn post(&self, who: Uuid, uri: &str, body: Value) -> (u16, Value) {
        self.call(who, Method::POST, uri, Some(body)).await
    }

    async fn freeze(&self, who: Uuid, uri: &str) -> (u16, Value) {
        self.post(
            who,
            &format!("{uri}/freeze"),
            json!({"reason": "incident 42"}),
        )
        .await
    }

    async fn unfreeze(&self, who: Uuid, uri: &str) -> (u16, Value) {
        self.call(who, Method::POST, &format!("{uri}/unfreeze"), None)
            .await
    }

    fn agent_uri(agent: Uuid) -> String {
        format!("/v1/agents/{agent}")
    }

    async fn agent_view(&self, who: Uuid, agent: Uuid) -> Value {
        let (s, v) = self
            .call(who, Method::GET, &Self::agent_uri(agent), None)
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn audit_count(&self, action: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
            .bind(action)
            .fetch_one(self.r.h.pool())
            .await
            .unwrap()
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

#[tokio::test]
async fn agent_freeze_needs_can_own_is_idempotent_reversible_and_audited() {
    let w = world().await;
    let agent = w.r.agent("frozen-one", false).await;
    let uri = W::agent_uri(agent);
    // A plain member (can_use) cannot freeze; neither can an instance admin outside the org.
    for who in [w.member, w.ia] {
        let (s, v) = w.freeze(who, &uri).await;
        assert!(matches!(s, 403 | 404), "{s} {v}");
    }
    // Validation: a reason is required, 1..500 chars.
    for bad in [
        json!({"reason": ""}),
        json!({"reason": "  \n "}),
        json!({"reason": "x".repeat(501)}),
        json!({}),
    ] {
        let (s, v) = w.post(w.ws_owner, &format!("{uri}/freeze"), bad).await;
        assert_eq!(s, 400, "{v}");
    }
    let (s, v) = w.freeze(w.ws_owner, &uri).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["effective_frozen"], true);
    assert_eq!(v["frozen"]["level"], "agent");
    assert_eq!(v["frozen"]["authority"], "workspace_owner");
    assert_eq!(v["frozen"]["reason"], "incident 42");
    assert_eq!(v["frozen"]["by_label"], "wso@x.io");
    // Visible to anyone who can use the workspace, with the effective flag.
    let view = w.agent_view(w.member, agent).await;
    assert_eq!(view["effective_frozen"], true);
    assert_eq!(view["frozen"]["level"], "agent");
    assert_eq!(v["outcome"], "frozen");
    // (FIX-109, D-50) A repeat by the same authority changes nothing and says so.
    let (s, v) = w.freeze(w.ws_owner, &uri).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["outcome"], "unchanged");
    assert_eq!(w.audit_count("agent.freeze").await, 1);
    // A higher authority raises the record: who, reason and authority now are the caller's,
    // with an audit row marked `escalated`; the lower authority can no longer lift it.
    let (s, v) = w.freeze(w.org_owner, &uri).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["outcome"], "escalated");
    assert_eq!(v["frozen"]["by_label"], "oo@x.io");
    assert_eq!(v["frozen"]["authority"], "org_admin");
    assert_eq!(w.audit_count("agent.freeze").await, 2);
    let esc: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'agent.freeze' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(w.r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (
            esc["escalated"].clone(),
            esc["previous_authority"].clone(),
            esc["authority"].clone()
        ),
        (json!(true), json!("workspace_owner"), json!("org_admin"))
    );
    // A lower authority freezing again does not lower it.
    let (s, v) = w.freeze(w.ws_owner, &uri).await;
    assert_eq!(
        (s, v["outcome"].as_str(), v["frozen"]["authority"].as_str()),
        (200, Some("unchanged"), Some("org_admin")),
        "{v}"
    );
    // Unfreeze: members cannot; the workspace owner no longer can; the org owner can.
    assert!(matches!(w.unfreeze(w.member, &uri).await.0, 403 | 404));
    let (s, v) = w.unfreeze(w.ws_owner, &uri).await;
    assert_eq!((s, code(&v)), (403, "freeze_authority_insufficient"), "{v}");
    let (s, v) = w.unfreeze(w.org_owner, &uri).await;
    assert_eq!(s, 200, "{v}");
    // Repeating the unfreeze is a no-op.
    assert_eq!(w.unfreeze(w.ws_owner, &uri).await.0, 200);
    assert_eq!(v["effective_frozen"], false);
    assert!(v["frozen"].is_null());
    assert_eq!(w.audit_count("agent.unfreeze").await, 1);
    let view = w.agent_view(w.member, agent).await;
    assert_eq!(view["effective_frozen"], false);
    let meta: Value =
        sqlx::query_scalar("SELECT metadata FROM audit_log WHERE action = 'agent.freeze' LIMIT 1")
            .fetch_one(w.r.h.pool())
            .await
            .unwrap();
    assert_eq!(meta["reason"], "incident 42");
}

#[tokio::test]
async fn lifting_needs_at_least_the_authority_that_froze() {
    let w = world().await;
    let agent = w.r.agent("ladder", false).await;
    let a_uri = W::agent_uri(agent);
    let ws_uri = format!("/v1/workspaces/{}", w.r.ws);
    let org_uri = format!("/v1/orgs/{}", w.r.org);
    // An org admin freezes the agent: a workspace owner cannot lift it.
    let (s, v) = w.freeze(w.org_admin, &a_uri).await;
    assert_eq!(
        (s, v["frozen"]["authority"].as_str()),
        (200, Some("org_admin")),
        "{v}"
    );
    let (s, v) = w.unfreeze(w.ws_owner, &a_uri).await;
    assert_eq!((s, code(&v)), (403, "freeze_authority_insufficient"), "{v}");
    assert_eq!(
        w.agent_view(w.member, agent).await["effective_frozen"],
        true
    );
    assert_eq!(w.unfreeze(w.org_admin, &a_uri).await.0, 200);

    // Workspace: org owners/admins and workspace owners may freeze; the flags are independent.
    let (s, v) = w.freeze(w.ws_owner, &ws_uri).await;
    assert_eq!(
        (s, v["frozen"]["level"].as_str()),
        (200, Some("workspace")),
        "{v}"
    );
    let view = w.agent_view(w.member, agent).await;
    assert_eq!(view["effective_frozen"], true);
    assert_eq!(view["frozen"]["level"], "workspace");
    // Freezing the agent too, then unfreezing the agent, does not unfreeze the workspace.
    assert_eq!(w.freeze(w.ws_owner, &a_uri).await.0, 200);
    assert_eq!(w.unfreeze(w.ws_owner, &a_uri).await.0, 200);
    assert_eq!(
        w.agent_view(w.member, agent).await["effective_frozen"],
        true
    );
    // The workspace carries `frozen` too.
    let (s, v) = w.call(w.member, Method::GET, &ws_uri, None).await;
    assert_eq!(s, 200);
    assert_eq!(v["frozen"]["level"], "workspace");
    assert_eq!(w.unfreeze(w.ws_owner, &ws_uri).await.0, 200);
    assert_eq!(
        w.agent_view(w.member, agent).await["effective_frozen"],
        false
    );

    // An org admin's workspace freeze outranks a workspace owner.
    assert_eq!(w.freeze(w.org_admin, &ws_uri).await.0, 200);
    let (s, v) = w.unfreeze(w.ws_owner, &ws_uri).await;
    assert_eq!((s, code(&v)), (403, "freeze_authority_insufficient"), "{v}");
    assert_eq!(w.unfreeze(w.org_owner, &ws_uri).await.0, 200);

    // Organization: owners only (an admin is refused); an instance-admin freeze needs the
    // operator to lift it, and gives the operator no access to org data.
    assert_eq!(w.freeze(w.org_admin, &org_uri).await.0, 403);
    assert_eq!(w.freeze(w.ws_owner, &org_uri).await.0, 403);
    let (s, v) = w.freeze(w.org_owner, &org_uri).await;
    assert_eq!(
        (s, v["frozen"]["authority"].as_str()),
        (200, Some("org_admin")),
        "{v}"
    );
    let view = w.agent_view(w.member, agent).await;
    assert_eq!(
        (
            view["effective_frozen"].clone(),
            view["frozen"]["level"].clone()
        ),
        (json!(true), json!("org"))
    );
    assert_eq!(w.unfreeze(w.org_admin, &org_uri).await.0, 403);
    // The operator may lift an org-owner freeze.
    let admin_uri = format!("/v1/admin/orgs/{}", w.r.org);
    assert_eq!(w.unfreeze(w.ia, &admin_uri).await.0, 200);
    let (s, v) = w.freeze(w.ia, &admin_uri).await;
    assert_eq!(
        (s, v["frozen"]["authority"].as_str()),
        (200, Some("instance_admin")),
        "{v}"
    );
    let (s, v) = w.unfreeze(w.org_owner, &org_uri).await;
    assert_eq!((s, code(&v)), (403, "freeze_authority_insufficient"), "{v}");
    // The freeze gives the instance admin no org data access.
    let (s, _) = w.call(w.ia, Method::GET, &org_uri, None).await;
    assert_eq!(s, 404);
    let (s, _) = w
        .call(
            w.ia,
            Method::GET,
            &format!("/v1/workspaces/{}/agents", w.r.ws),
            None,
        )
        .await;
    assert_eq!(s, 404);
    // Owners keep the management plane while frozen.
    let (s, _) = w
        .call(
            w.org_owner,
            Method::GET,
            &format!("/v1/workspaces/{}/agents", w.r.ws),
            None,
        )
        .await;
    assert_eq!(s, 200);
    let (s, v) = w.call(w.org_owner, Method::GET, &org_uri, None).await;
    assert_eq!((s, v["frozen"]["level"].as_str()), (200, Some("org")));
    // Operator listing shows it; unknown ids are 404; non-admins are refused.
    let (s, v) = w
        .call(w.ia, Method::GET, "/v1/admin/orgs?frozen=true", None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["frozen"]["authority"], "instance_admin");
    assert_eq!(
        w.freeze(w.ia, &format!("/v1/admin/orgs/{}", Uuid::now_v7()))
            .await
            .0,
        404
    );
    assert_eq!(
        w.call(w.org_owner, Method::GET, "/v1/admin/orgs", None)
            .await
            .0,
        403
    );
    assert_eq!(w.unfreeze(w.ia, &admin_uri).await.0, 200);
    assert_eq!(
        w.agent_view(w.member, agent).await["effective_frozen"],
        false
    );
    for a in [
        "workspace.freeze",
        "workspace.unfreeze",
        "org.freeze",
        "org.unfreeze",
    ] {
        assert!(w.audit_count(a).await >= 1, "{a}");
    }
}

#[tokio::test]
async fn frozen_agent_refuses_the_doors_that_start_work() {
    let w = world().await;
    let agent = w.r.agent("doors", false).await;
    let conv = w.r.direct(agent).await;
    let (s, v) =
        w.r.user_call(
            Method::POST,
            &format!("/v1/agents/{agent}/schedules"),
            Some(json!({"name": "s", "prompt": "p", "cron": "0 9 * * *", "timezone": "UTC"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let sched = v["id"].as_str().unwrap().to_owned();
    // A queued turn exists when the freeze lands: it ends aborted / frozen.
    let queued = w.r.say(conv, "before").await;
    let turn = queued["turn"]["id"]
        .as_str()
        .or_else(|| queued["id"].as_str())
        .unwrap()
        .to_owned();
    assert_eq!(w.r.turn_status(&turn).await.0, "queued");
    assert_eq!(w.freeze(w.ws_owner, &W::agent_uri(agent)).await.0, 200);
    assert_eq!(
        w.r.turn_status(&turn).await,
        ("aborted".to_owned(), Some("frozen".to_owned()))
    );
    // Every door answers 423 `frozen`.
    let (s, v) =
        w.r.user_call(
            Method::POST,
            &format!("/v1/conversations/{conv}/messages"),
            Some(json!({"text": "after"})),
        )
        .await;
    assert_eq!((s, code(&v)), (423, "frozen"), "{v}");
    assert_eq!(v["error"]["details"]["level"], "agent");
    let (s, v) =
        w.r.user_call(
            Method::POST,
            &format!("/v1/schedules/{sched}/run-now"),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (423, "frozen"), "{v}");
    let (s, v) =
        w.r.user_call(Method::POST, &format!("/v1/turns/{turn}/send-now"), None)
            .await;
    assert_eq!((s, code(&v)), (423, "frozen"), "{v}");
    // The conversation is readable (management plane) and says why.
    let ev = w.r.events(conv).await;
    assert!(
        ev.iter()
            .any(|(t, p)| t == "notice" && p["code"] == "frozen")
    );
    // Unfreeze: messages flow again; the aborted turn is not revived.
    assert_eq!(w.unfreeze(w.ws_owner, &W::agent_uri(agent)).await.0, 200);
    w.r.say(conv, "again").await;
    assert_eq!(w.r.turn_status(&turn).await.0, "aborted");
}

#[tokio::test]
async fn running_turn_aborts_sandbox_is_parked_with_home_kept_and_nothing_dispatches() {
    let w = world().await;
    let lv: Live = w.r.boot("runner-one").await;
    let d = w.r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    w.r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    // Pending approvals of the agent are abandoned; pending action approvals are rejected.
    let appr = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, gateway_instance_id, method, host, path,
            summary, status, deadline) VALUES ($1,$2,$3,$4,$5,'GET','h','/','{}','pending', now() + interval '1 hour')",
    )
    .bind(appr)
    .bind(w.r.org)
    .bind(w.r.ws)
    .bind(lv.agent)
    .bind(Uuid::now_v7())
    .execute(w.r.h.pool())
    .await
    .unwrap();
    let act = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO action_approvals (id, workspace_id, agent_id, action_key, payload, summary, status, expires_at)
         VALUES ($1,$2,$3,'k','{}','s','pending', now() + interval '1 hour')",
    )
    .bind(act)
    .bind(w.r.ws)
    .bind(lv.agent)
    .execute(w.r.h.pool())
    .await
    .unwrap();
    // A follow-up that joined the running turn is promoted, then aborted with it.
    w.r.say(lv.conv, "and also").await;

    let mut l = sqlx::postgres::PgListener::connect_with(w.r.h.pool())
        .await
        .unwrap();
    l.listen("aurora_approval").await.unwrap();
    // Freezing the whole workspace is enough.
    let (s, v) = w
        .freeze(w.org_admin, &format!("/v1/workspaces/{}", w.r.ws))
        .await;
    assert_eq!(s, 200, "{v}");
    // (FIX-109) The approval stream event of a freeze-abandoned approval says `abandoned`.
    let mut event = None;
    while let Ok(Ok(n)) = tokio::time::timeout(std::time::Duration::from_secs(2), l.recv()).await {
        let p: Value = serde_json::from_str(n.payload()).unwrap();
        if p["id"] == json!(appr) {
            event = p["event"].as_str().map(str::to_owned);
            break;
        }
    }
    assert_eq!(event.as_deref(), Some("abandoned"));
    assert_eq!(
        w.r.turn_status(&turn).await,
        ("aborted".to_owned(), Some("frozen".to_owned()))
    );
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM turns WHERE agent_id = $1 AND status IN ('queued','dispatched','running','joining')",
    )
    .bind(lv.agent)
    .fetch_one(w.r.h.pool())
    .await
    .unwrap();
    assert_eq!(live, 0, "queued and joining rows end too");
    let st: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
        .bind(appr)
        .fetch_one(w.r.h.pool())
        .await
        .unwrap();
    assert_eq!(st, "abandoned");
    let (st, err): (String, Option<String>) =
        sqlx::query_as("SELECT status, error FROM action_approvals WHERE id = $1")
            .bind(act)
            .fetch_one(w.r.h.pool())
            .await
            .unwrap();
    assert_eq!((st.as_str(), err.as_deref()), ("rejected", Some("frozen")));

    // The next poll parks the sandbox (reason `frozen`, keep-awake reasons ignored) and no
    // turn is delivered.
    let items = w.r.work().await;
    let stop = items
        .iter()
        .find(|i| i["kind"] == "sandbox.stop")
        .expect("park");
    assert_eq!(stop["reason"], "frozen");
    assert!(items.iter().all(|i| i["kind"] != "turn.deliver"));
    assert_eq!(
        w.r.report(&lv, json!({"kind": "sandbox.status", "state": "stopped"}))
            .await,
        "applied"
    );
    let (state, reason, home_ref): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT state, stop_reason, home_ref FROM sandboxes WHERE agent_id = $1")
            .bind(lv.agent)
            .fetch_one(w.r.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("stopped", Some("frozen"))
    );
    let _ = home_ref;
    // A turn created in a race with the freeze never starts: no sandbox start, and the sweep
    // ends it.
    let racer = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text)
         VALUES ($1,$2,$3,'turn','queued','{\"kind\":\"web\"}','person','x','race')",
    )
    .bind(racer)
    .bind(lv.conv)
    .bind(lv.agent)
    .execute(w.r.h.pool())
    .await
    .unwrap();
    let items = w.r.work().await;
    assert!(
        items
            .iter()
            .all(|i| i["kind"] != "sandbox.start" && i["kind"] != "turn.deliver"),
        "{items:?}"
    );
    assert_eq!(
        w.r.turn_status(&racer.to_string()).await,
        ("aborted".to_owned(), Some("frozen".to_owned()))
    );
    // Unfreezing resumes service: a new message starts the sandbox again.
    assert_eq!(
        w.unfreeze(w.org_admin, &format!("/v1/workspaces/{}", w.r.ws))
            .await
            .0,
        200
    );
    w.r.say(lv.conv, "welcome back").await;
    let items = w.r.work().await;
    assert!(
        items.iter().any(|i| i["kind"] == "sandbox.start"),
        "{items:?}"
    );
}

#[tokio::test]
async fn byo_agent_token_endpoints_answer_423_while_frozen() {
    let w = world().await;
    let (s, v) = w
        .post(
            w.org_owner,
            &format!("/v1/workspaces/{}/agents", w.r.ws),
            json!({"name": "Byo", "kind": "byo"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let token = v["token"].as_str().unwrap().to_owned();
    let agent: Uuid = v["agent"]["id"].as_str().unwrap().parse().unwrap();
    let auth = bearer(&token);
    let get = |uri: &'static str, m: Method, b: Option<Value>| {
        let auth = auth.clone();
        let h = &w.r.h;
        async move {
            let (s, _, v) = h.send(m, uri, &[("authorization", &auth)], b).await;
            (s.as_u16(), v)
        }
    };
    let ctx = || json!({"label": "ci"});
    assert_eq!(
        get("/v1/agent/container-config", Method::GET, None).await.0,
        200
    );
    assert_eq!(
        w.freeze(w.org_owner, &format!("/v1/orgs/{}", w.r.org))
            .await
            .0,
        200
    );
    let (s, v) = get("/v1/agent/container-config", Method::GET, None).await;
    assert_eq!((s, code(&v)), (423, "frozen"), "{v}");
    assert_eq!(v["error"]["details"]["level"], "org");
    let (s, v) = get("/v1/agent/context-credentials", Method::POST, Some(ctx())).await;
    assert_eq!((s, code(&v)), (423, "frozen"), "{v}");
    // The token is not revoked: unfreezing restores it without redistribution.
    assert_eq!(
        w.unfreeze(w.org_owner, &format!("/v1/orgs/{}", w.r.org))
            .await
            .0,
        200
    );
    assert_eq!(
        get("/v1/agent/container-config", Method::GET, None).await.0,
        200
    );
    let _ = agent;
}
