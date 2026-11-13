#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-065: action-approval framework and agent links (policy, `message_agent` handler).

use crate::common::cookie;
use crate::peers_rig::{Person, Rig, rig};
use aurora_api::domains::action_approvals::{handlers, service};
use serde_json::{Value, json};
use uuid::Uuid;

struct Cross {
    rig: Rig,
    a: Uuid,
    b: Uuid,
    conv: Uuid,
    turn: Uuid,
}

/// Agent `a` (ws1) talking to person P, with agent `b` in ws2.
async fn cross() -> Cross {
    let rig = rig().await;
    let a = rig.agent(rig.ws1, "alpha").await;
    let b = rig.agent(rig.ws2, "bravo").await;
    let (conv, turn) = rig.person_turn(a, rig.p.id).await;
    Cross {
        rig,
        a,
        b,
        conv,
        turn,
    }
}

impl Cross {
    async fn send(&self, text: &str) -> Value {
        let r = self
            .rig
            .tool(
                self.a,
                self.conv,
                Some(self.turn),
                "message_agent",
                json!({"agent": self.b, "text": text}),
            )
            .await;
        Rig::ok(&r)
    }

    async fn approvals(&self, ws: Uuid, who: &Person) -> Vec<Value> {
        let (s, v) = self
            .rig
            .get(who, &format!("/v1/workspaces/{ws}/action-approvals"))
            .await;
        assert_eq!(s, 200, "{v}");
        v["items"].as_array().unwrap().clone()
    }
}

#[tokio::test]
async fn cross_workspace_message_needs_both_sides_and_flips_policies() {
    let c = cross().await;
    let v = c.send("please check the invoices").await;
    assert_eq!(v["status"], "pending_approval");
    assert_eq!(v["approval_ids"].as_array().unwrap().len(), 2);
    // First message created the link with both sides `ask`.
    let (pa, pb): (String, String) = sqlx::query_as("SELECT policy_a, policy_b FROM agent_links")
        .fetch_one(c.rig.h.pool())
        .await
        .unwrap();
    assert_eq!((pa.as_str(), pb.as_str()), ("ask", "ask"));

    // Each workspace's operators see only their side; summaries are platform-composed.
    let mine = c.approvals(c.rig.ws1, &c.rig.p).await;
    let theirs = c.approvals(c.rig.ws2, &c.rig.q).await;
    assert_eq!((mine.len(), theirs.len()), (1, 1));
    for a in mine.iter().chain(theirs.iter()) {
        assert_eq!(a["status"], "pending");
        assert_eq!(a["action_key"], "message_agent");
        assert!(a["summary"].as_str().unwrap().contains("Agent alpha"));
        assert!(
            !a["summary"].as_str().unwrap().contains("invoices"),
            "no model text"
        );
        assert_eq!(a["agent_id"], json!(c.a));
    }
    let id_a = Rig::id(&mine[0]["id"]);
    let id_b = Rig::id(&theirs[0]["id"]);

    // Access: P cannot see or decide ws2's approval; R (no binding) sees nothing at all.
    assert_eq!(c.rig.decide(&c.rig.p, id_b, "approve").await.0, 404);
    assert_eq!(c.rig.decide(&c.rig.r, id_a, "approve").await.0, 404);
    assert_eq!(
        c.rig
            .get(
                &c.rig.r,
                &format!("/v1/workspaces/{}/action-approvals", c.rig.ws1)
            )
            .await
            .0,
        404
    );

    // Approving our side flips it to allow but nothing is sent yet.
    let (s, v) = c.rig.decide(&c.rig.p, id_a, "approve").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "executed");
    assert_eq!(v["decided_by"]["id"], json!(c.rig.p.id));
    let tasks: i64 = sqlx::query_scalar("SELECT count(*) FROM peer_tasks")
        .fetch_one(c.rig.h.pool())
        .await
        .unwrap();
    assert_eq!(tasks, 0);
    let (pa, pb): (String, String) = sqlx::query_as(
        "SELECT CASE WHEN agent_a_id = $1 THEN policy_a ELSE policy_b END,
                CASE WHEN agent_a_id = $1 THEN policy_b ELSE policy_a END FROM agent_links",
    )
    .bind(c.a)
    .fetch_one(c.rig.h.pool())
    .await
    .unwrap();
    assert_eq!((pa.as_str(), pb.as_str()), ("allow", "ask"));

    // The other side approves: the frozen message is delivered as a peer task.
    let (s, v) = c.rig.decide(&c.rig.q, id_b, "approve").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "executed");
    let (status, to): (String, Uuid) = sqlx::query_as("SELECT status, to_agent_id FROM peer_tasks")
        .fetch_one(c.rig.h.pool())
        .await
        .unwrap();
    assert_eq!((status.as_str(), to), ("open", c.b));
    let n = c.rig.notices(c.conv, "action_approval_executed").await;
    assert_eq!(n.len(), 2);
    assert!(n[1]["text"].as_str().unwrap().contains("approved and sent"));
}

#[tokio::test]
async fn same_workspace_link_starts_allow_without_approval() {
    let rig = rig().await;
    let a = rig.agent(rig.ws1, "alpha").await;
    let a2 = rig.agent(rig.ws1, "alpha2").await;
    let (conv, turn) = rig.person_turn(a, rig.p.id).await;
    let r = rig
        .tool(
            a,
            conv,
            Some(turn),
            "message_agent",
            json!({"agent": "@alpha2", "text": "hi"}),
        )
        .await;
    assert_eq!(Rig::ok(&r)["status"], "sent");
    let (pa, pb): (String, String) = sqlx::query_as("SELECT policy_a, policy_b FROM agent_links")
        .fetch_one(rig.h.pool())
        .await
        .unwrap();
    assert_eq!((pa.as_str(), pb.as_str()), ("allow", "allow"));
    let approvals: i64 = sqlx::query_scalar("SELECT count(*) FROM action_approvals")
        .fetch_one(rig.h.pool())
        .await
        .unwrap();
    assert_eq!(approvals, 0);
    let (links, _) = rig.get(&rig.p, &format!("/v1/agents/{a2}/links")).await;
    assert_eq!(links, 200);
}

#[tokio::test]
async fn deny_rejects_and_second_decision_conflicts() {
    let c = cross().await;
    let v = c.send("hello").await;
    let id = Rig::id(&v["approval_id"]);
    let other_side = Rig::id(&v["approval_ids"][1]);
    let (s, v) = c.rig.decide(&c.rig.p, id, "deny").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "rejected");
    let n = c.rig.notices(c.conv, "action_approval_rejected").await;
    assert_eq!(n.len(), 1);
    // A repeated or opposite decision is refused.
    for d in ["deny", "approve"] {
        let (s, v) = c.rig.decide(&c.rig.p, id, d).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (409, Some("already_decided"))
        );
    }
    // Nothing was sent and the denied side stays `ask`.
    let tasks: i64 = sqlx::query_scalar("SELECT count(*) FROM peer_tasks")
        .fetch_one(c.rig.h.pool())
        .await
        .unwrap();
    assert_eq!(tasks, 0);
    // A fresh request is possible again once every earlier request is decided.
    assert_eq!(c.rig.decide(&c.rig.q, other_side, "deny").await.0, 200);
    let again = c.send("second try").await;
    assert_eq!(again["status"], "pending_approval");
}

#[tokio::test]
async fn pending_request_for_same_side_is_not_duplicated() {
    let c = cross().await;
    c.send("first").await;
    let r = c
        .rig
        .tool(
            c.a,
            c.conv,
            Some(c.turn),
            "message_agent",
            json!({"agent": c.b, "text": "second"}),
        )
        .await;
    assert!(Rig::err(&r).contains("already waiting for approval"));
}

#[tokio::test]
async fn outcome_follow_up_turn_only_when_origin_turn_finished() {
    let c = cross().await;
    let v = c.send("hello").await;
    let ids: Vec<Uuid> = v["approval_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(Rig::id)
        .collect();
    // Origin turn still running: notice only.
    assert_eq!(c.rig.decide(&c.rig.p, ids[0], "deny").await.0, 200);
    assert_eq!(
        c.rig
            .notices(c.conv, "action_approval_rejected")
            .await
            .len(),
        1
    );
    assert_eq!(c.rig.turns(c.conv).await.len(), 1);
    // Origin turn finished: a system follow-up turn carries the result line.
    c.rig.finish_turn(c.turn).await;
    assert_eq!(c.rig.decide(&c.rig.q, ids[1], "deny").await.0, 200);
    let turns = c.rig.turns(c.conv).await;
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].2, "system");
    assert!(turns[1].3.contains("was denied"));
}

#[tokio::test]
async fn expiry_marks_expired_notifies_and_blocks_late_decisions() {
    let c = cross().await;
    let v = c.send("hello").await;
    let ids: Vec<Uuid> = v["approval_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(Rig::id)
        .collect();
    c.rig.finish_turn(c.turn).await;
    sqlx::query(
        "UPDATE action_approvals SET expires_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(ids[0])
    .execute(c.rig.h.pool())
    .await
    .unwrap();
    assert_eq!(service::sweep(c.rig.h.pool()).await.unwrap(), 1);
    // Idempotent.
    assert_eq!(service::sweep(c.rig.h.pool()).await.unwrap(), 0);
    let status: String = sqlx::query_scalar("SELECT status FROM action_approvals WHERE id = $1")
        .bind(ids[0])
        .fetch_one(c.rig.h.pool())
        .await
        .unwrap();
    assert_eq!(status, "expired");
    assert_eq!(
        c.rig.notices(c.conv, "action_approval_expired").await.len(),
        1
    );
    let turns = c.rig.turns(c.conv).await;
    assert_eq!(turns.last().unwrap().2, "system");
    assert_eq!(c.rig.decide(&c.rig.p, ids[0], "approve").await.0, 409);

    // A row past its deadline that the sweep has not reached yet cannot be approved either.
    sqlx::query(
        "UPDATE action_approvals SET expires_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(ids[1])
    .execute(c.rig.h.pool())
    .await
    .unwrap();
    let (s, v) = c.rig.decide(&c.rig.q, ids[1], "approve").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("already_decided"))
    );
    let status: String = sqlx::query_scalar("SELECT status FROM action_approvals WHERE id = $1")
        .bind(ids[1])
        .fetch_one(c.rig.h.pool())
        .await
        .unwrap();
    assert_eq!(status, "expired");
}

#[tokio::test]
async fn unknown_action_key_fails_loud_and_stays_pending() {
    let c = cross().await;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO action_approvals (id, workspace_id, agent_id, action_key, payload, summary,
            status, expires_at)
         VALUES ($1,$2,$3,'no_such_action','{}','x','pending', now() + interval '1 hour')",
    )
    .bind(id)
    .bind(c.rig.ws1)
    .bind(c.a)
    .execute(c.rig.h.pool())
    .await
    .unwrap();
    for d in ["approve", "deny"] {
        let (s, v) = c.rig.decide(&c.rig.p, id, d).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (500, Some("internal_error")),
            "{v}"
        );
    }
    let status: String = sqlx::query_scalar("SELECT status FROM action_approvals WHERE id = $1")
        .bind(id)
        .fetch_one(c.rig.h.pool())
        .await
        .unwrap();
    assert_eq!(status, "pending");
    assert!(handlers::handler("no_such_action").is_none());
    assert!(handlers::keys().contains(&"message_agent"));
}

#[tokio::test]
async fn handler_failure_is_recorded_as_failed_after_a_durable_approval() {
    let c = cross().await;
    let v = c.send("hello").await;
    let ids: Vec<Uuid> = v["approval_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(Rig::id)
        .collect();
    // The receiving side blocks the link before the sender's side is approved.
    let (_, links) = c
        .rig
        .get(&c.rig.q, &format!("/v1/agents/{}/links", c.b))
        .await;
    let link = &links["items"][0];
    let (s, v) = c
        .rig
        .put(
            &c.rig.q,
            &format!("/v1/agent-links/{}/policy", link["id"].as_str().unwrap()),
            json!({"agent_id": c.b, "policy": "blocked", "revision": link["revision"]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = c.rig.decide(&c.rig.p, ids[0], "approve").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "failed");
    assert!(v["error"].as_str().unwrap().contains("blocked"));
    assert_eq!(
        c.rig.notices(c.conv, "action_approval_failed").await.len(),
        1
    );
}

#[tokio::test]
async fn blocked_side_refuses_messages() {
    let c = cross().await;
    c.send("hello").await;
    let (_, links) = c
        .rig
        .get(&c.rig.p, &format!("/v1/agents/{}/links", c.a))
        .await;
    let link = &links["items"][0];
    assert_eq!(link["self"]["policy"], "ask");
    assert_eq!(link["peer"]["handle"], "bravo");
    assert_eq!(link["peer"]["policy"], "ask");
    let id = link["id"].as_str().unwrap();
    let (s, v) = c
        .rig
        .put(
            &c.rig.p,
            &format!("/v1/agent-links/{id}/policy"),
            json!({"agent_id": c.a, "policy": "blocked", "revision": link["revision"]}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["self"]["policy"], "blocked");
    assert_eq!(v["revision"], 1);
    let r = c
        .rig
        .tool(
            c.a,
            c.conv,
            Some(c.turn),
            "message_agent",
            json!({"agent": c.b, "text": "again"}),
        )
        .await;
    assert!(Rig::err(&r).contains("blocked"));
    // The other side sees the blocked peer too, and allow on its own side changes nothing.
    let (_, links) = c
        .rig
        .get(&c.rig.q, &format!("/v1/agents/{}/links", c.b))
        .await;
    assert_eq!(links["items"][0]["peer"]["policy"], "blocked");
}

/// (FIX-110, docs/07 §Agent links, RFE-012) Consenting to cross-workspace messaging (`allow`)
/// needs `can_approve`: a plain workspace member gets 403 `approver_required`, an approver 200;
/// the restricting policies (`blocked`, `ask`) stay with `can_use`.
#[tokio::test]
async fn link_allow_needs_an_approver_but_blocking_does_not() {
    let c = cross().await;
    c.send("hello").await;
    let m = c.rig.h.user("m@x.io", false).await;
    c.rig.h.member(c.rig.org, m, "member", "active").await;
    c.rig.h.bind_ws(c.rig.ws1, m, "member").await;
    let (_, tok) = c.rig.h.good_session(m).await;
    let member = Person {
        id: m,
        cookie: cookie(&tok),
    };
    let (_, links) = c
        .rig
        .get(&c.rig.p, &format!("/v1/agents/{}/links", c.a))
        .await;
    let id = links["items"][0]["id"].as_str().unwrap().to_owned();
    let uri = format!("/v1/agent-links/{id}/policy");
    let (s, v) = c
        .rig
        .put(
            &member,
            &uri,
            json!({"agent_id": c.a, "policy": "allow", "revision": 0}),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("approver_required")),
        "{v}"
    );
    let (s, v) = c
        .rig
        .put(
            &member,
            &uri,
            json!({"agent_id": c.a, "policy": "blocked", "revision": 0}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = c
        .rig
        .put(
            &c.rig.p,
            &uri,
            json!({"agent_id": c.a, "policy": "allow", "revision": 1}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn policy_update_is_confined_to_own_side_and_checks_revision() {
    let c = cross().await;
    c.send("hello").await;
    let (_, links) = c
        .rig
        .get(&c.rig.p, &format!("/v1/agents/{}/links", c.a))
        .await;
    let id = links["items"][0]["id"].as_str().unwrap().to_owned();
    let uri = format!("/v1/agent-links/{id}/policy");
    // P cannot touch the other (ws2) side: invisible to them.
    let (s, _) = c
        .rig
        .put(
            &c.rig.p,
            &uri,
            json!({"agent_id": c.b, "policy": "allow", "revision": 0}),
        )
        .await;
    assert_eq!(s, 404);
    // An agent outside the link is not found either.
    let other = c.rig.agent(c.rig.ws1, "other").await;
    let (s, _) = c
        .rig
        .put(
            &c.rig.p,
            &uri,
            json!({"agent_id": other, "policy": "allow", "revision": 0}),
        )
        .await;
    assert_eq!(s, 404);
    // Stale revision.
    let (s, v) = c
        .rig
        .put(
            &c.rig.p,
            &uri,
            json!({"agent_id": c.a, "policy": "allow", "revision": 7}),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("stale_revision"))
    );
    // Invalid policy value.
    let (s, _) = c
        .rig
        .put(
            &c.rig.p,
            &uri,
            json!({"agent_id": c.a, "policy": "sometimes", "revision": 0}),
        )
        .await;
    assert!(s == 400 || s == 422, "{s}");
    // The right revision works; replaying it is now stale.
    let (s, v) = c
        .rig
        .put(
            &c.rig.p,
            &uri,
            json!({"agent_id": c.a, "policy": "allow", "revision": 0}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["revision"], 1);
    assert_eq!(v["peer"]["policy"], "ask");
    let (s, _) = c
        .rig
        .put(
            &c.rig.p,
            &uri,
            json!({"agent_id": c.a, "policy": "ask", "revision": 0}),
        )
        .await;
    assert_eq!(s, 409);
}

#[tokio::test]
async fn member_cannot_decide_action_approvals() {
    // RFE-012 (D-55, SEC-28): a plain member sees neither the list nor can decide (403); an
    // approver can; a role change applies on the very next request.
    let c = cross().await;
    let v = c.send("hello").await;
    let id = Rig::id(&v["approval_id"]);
    let h = &c.rig.h;
    let m = h.user("plain@x.io", false).await;
    h.member(c.rig.org, m, "member", "active").await;
    h.bind_ws(c.rig.ws1, m, "member").await;
    let (_, tok) = h.good_session(m).await;
    let mp = Person {
        id: m,
        cookie: cookie(&tok),
    };
    let (s, v) = c
        .rig
        .get(
            &mp,
            &format!("/v1/workspaces/{}/action-approvals", c.rig.ws1),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("approver_required"))
    );
    let (s, v) = c.rig.decide(&mp, id, "approve").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("approver_required"))
    );
    let status: String = sqlx::query_scalar("SELECT status FROM action_approvals WHERE id = $1")
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(status, "pending");
    // Role change takes effect on the next request.
    sqlx::query(
        "UPDATE workspace_access SET role = 'approver' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(c.rig.ws1)
    .bind(m)
    .execute(h.pool())
    .await
    .unwrap();
    assert_eq!(c.rig.decide(&mp, id, "deny").await.0, 200);
}
