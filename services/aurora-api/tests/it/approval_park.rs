#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-011B-2 (D-264, D-265, SEC-74, C-822..C-829): deciding **parked** approvals. An approve makes
//! a `ready` single-use grant with its window and writes one platform-composed system follow-up in
//! the origin conversation, in the decision's transaction; a deny writes the "denied" follow-up;
//! a frozen agent or a gone conversation turns the grant `revoked`; every access-removal event
//! ends parked rows honestly; the resume window and the reminder are API jobs.

use crate::approvals::{Fx, fixture};
use crate::common::*;
use aurora_db::repo::approvals as repo;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) struct World {
    pub(crate) h: Harness,
    pub(crate) f: Fx,
    pub(crate) cookie: String,
    pub(crate) conv: Uuid,
}

pub(crate) async fn world() -> World {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let approver = h.user("ap@x.io", false).await;
    h.member(f.org, approver, "member", "active").await;
    h.bind_ws(f.ws, approver, "approver").await;
    let (_, t) = h.good_session(approver).await;
    let conv = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) VALUES ($1,$2,$3,'api',$1::text, now())")
        .bind(conv).bind(f.agent).bind(f.ws).execute(h.pool()).await.unwrap();
    World {
        cookie: cookie(&t),
        h,
        f,
        conv,
    }
}

/// A `pending` approval of the world's conversation; `parked` rows carry a salt and fingerprint.
pub(crate) async fn row(w: &World, parked: bool, batch: Option<&str>, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, conversation_id, \
            gateway_instance_id, method, host, path, summary, batch_id, status, deadline, \
            parked, fp_salt, fingerprint) \
         VALUES ($1,$2,$3,$4,$5,$6,'POST','api.example.com','/items', \
            jsonb_build_object('title', $7::text, 'fields', '[]'::jsonb, 'source', 'generic'), \
            $8,'pending', now() + interval '1 day', $9, \
            CASE WHEN $9 THEN '\\x0102'::bytea END, CASE WHEN $9 THEN '\\x0304'::bytea END)",
    )
    .bind(id)
    .bind(w.f.org)
    .bind(w.f.ws)
    .bind(w.f.agent)
    .bind(w.conv)
    .bind(w.f.gw)
    .bind(title)
    .bind(batch)
    .bind(parked)
    .execute(w.h.pool())
    .await
    .unwrap();
    id
}

pub(crate) async fn decide(w: &World, id: Uuid, decision: &str) -> (u16, Value) {
    let (s, _, v) =
        w.h.send(
            Method::POST,
            &format!("/v1/approvals/{id}/decision"),
            &[("cookie", &w.cookie), ("origin", ORIGIN)],
            Some(json!({"decision": decision})),
        )
        .await;
    (s.as_u16(), v)
}

/// `(resume_state, resume_expires_at is set, resume_turn_id is set)`.
pub(crate) async fn grant(w: &World, id: Uuid) -> (Option<String>, bool, bool) {
    sqlx::query_as(
        "SELECT resume_state, resume_expires_at IS NOT NULL, resume_turn_id IS NOT NULL \
         FROM approvals WHERE id = $1",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap()
}

/// The system follow-ups of the conversation: `(text, door)`.
pub(crate) async fn followups(w: &World) -> Vec<(String, Value)> {
    sqlx::query_as(
        "SELECT text, door FROM turns WHERE conversation_id = $1 AND speaker_kind = 'system' \
         ORDER BY created_at, id",
    )
    .bind(w.conv)
    .fetch_all(w.h.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn approving_a_parked_approval_makes_a_ready_grant_and_one_followup() {
    let w = world().await;
    let id = row(&w, true, None, "Deploy to staging").await;
    let (s, v) = decide(&w, id, "approve").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["parked"], true);
    assert_eq!(v["resume_state"], "ready");
    assert_eq!(v["resume"]["state"], "ready");
    assert!(v["resume"]["expires_at"].is_string(), "{v}");
    // Never the fingerprint or the salt, in any shape.
    let text = v.to_string();
    assert!(
        !text.contains("fingerprint") && !text.contains("fp_salt"),
        "{text}"
    );
    let (state, window, turn) = grant(&w, id).await;
    assert_eq!(
        (state.as_deref(), window, turn),
        (Some("ready"), true, true)
    );
    let secs: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM resume_expires_at - decided_at)::float8 FROM approvals WHERE id = $1",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(
        (secs - 1800.0).abs() < 2.0,
        "the window is approvals.park.resume_window_secs: {secs}"
    );
    let f = followups(&w).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert!(
        f[0].0
            .starts_with("Aurora: a person approved a request you were waiting for."),
        "{}",
        f[0].0
    );
    assert!(
        f[0].0
            .contains("Deploy to staging — POST api.example.com/items"),
        "{}",
        f[0].0
    );
    assert!(
        f[0].0
            .contains("Repeat exactly the same request once, byte for byte, before "),
        "{}",
        f[0].0
    );
    assert!(
        f[0].0
            .contains("(30 minutes). A changed request needs a new approval."),
        "{}",
        f[0].0
    );
    assert_eq!(
        f[0].1,
        json!({"kind": "system", "ref": format!("approval:{id}")})
    );
}

#[tokio::test]
async fn denying_a_parked_approval_writes_the_denied_followup_and_no_grant() {
    let w = world().await;
    let id = row(&w, true, None, "Wire money").await;
    let (s, v) = decide(&w, id, "deny").await;
    assert_eq!(s, 200, "{v}");
    assert!(v.get("resume").is_none() || v["resume"].is_null(), "{v}");
    assert_eq!(grant(&w, id).await.0, None);
    let f = followups(&w).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert!(
        f[0].0
            .starts_with("Aurora: a person denied a request you were waiting for."),
        "{}",
        f[0].0
    );
    assert!(
        f[0].0
            .contains("Do not try it again or look for another way to do it"),
        "{}",
        f[0].0
    );
}

#[tokio::test]
async fn a_held_approval_is_decided_as_before_with_no_follow_up() {
    let w = world().await;
    let id = row(&w, false, None, "Held").await;
    let (s, v) = decide(&w, id, "approve").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["parked"], false);
    assert_eq!(grant(&w, id).await, (None, false, false));
    assert!(followups(&w).await.is_empty());
}

#[tokio::test]
async fn a_batch_of_parked_approvals_writes_exactly_one_followup() {
    let w = world().await;
    let ids = [
        row(&w, true, Some("b1"), "One").await,
        row(&w, true, Some("b1"), "Two").await,
        row(&w, true, Some("b1"), "Three").await,
    ];
    let (s, _, v) =
        w.h.send(
            Method::POST,
            "/v1/approvals/batches/b1/decision",
            &[("cookie", &w.cookie), ("origin", ORIGIN)],
            Some(json!({"decision": "approve", "agent_id": w.f.agent})),
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    let f = followups(&w).await;
    assert_eq!(f.len(), 1, "one follow-up per conversation: {f:?}");
    assert_eq!(f[0].0.matches("\n- Approved by ").count(), 3, "{}", f[0].0);
    for id in ids {
        assert_eq!(grant(&w, id).await.0.as_deref(), Some("ready"));
    }
}

#[tokio::test]
async fn a_long_batch_is_capped_with_an_and_n_more_line() {
    let w = world().await;
    for i in 0..13 {
        row(&w, true, Some("big"), &format!("T{i}")).await;
    }
    let (s, _, v) =
        w.h.send(
            Method::POST,
            "/v1/approvals/batches/big/decision",
            &[("cookie", &w.cookie), ("origin", ORIGIN)],
            Some(json!({"decision": "deny", "agent_id": w.f.agent})),
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    let f = followups(&w).await;
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].0.matches("\n- Denied by ").count(), 10, "{}", f[0].0);
    assert!(f[0].0.contains("…and 3 more"), "{}", f[0].0);
}

#[tokio::test]
async fn a_frozen_agent_gets_no_followup_and_the_grant_is_revoked() {
    let w = world().await;
    let id = row(&w, true, None, "Frozen").await;
    sqlx::query("UPDATE agents SET frozen_at = now(), frozen_reason = 't', frozen_authority = 'org_admin' WHERE id = $1")
        .bind(w.f.agent)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = decide(&w, id, "approve").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "approved");
    assert_eq!(v["resume_state"], "revoked", "{v}");
    assert!(v.get("resume").is_none() || v["resume"].is_null());
    assert!(followups(&w).await.is_empty());
}

#[tokio::test]
async fn the_resume_window_ends_with_an_expiry_and_a_notice_and_no_turn() {
    let w = world().await;
    aurora_api::domains::approvals::transcript::spawn(w.h.state.clone());
    let id = row(&w, true, None, "Window").await;
    assert_eq!(decide(&w, id, "approve").await.0, 200);
    let before = followups(&w).await.len();
    sqlx::query(
        "UPDATE approvals SET resume_expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(id)
    .execute(w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        aurora_api::jobs::sweep_resume_windows(w.h.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        aurora_api::jobs::sweep_resume_windows(w.h.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(grant(&w, id).await.0.as_deref(), Some("expired"));
    assert_eq!(followups(&w).await.len(), before, "no turn");
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM transcript_events WHERE conversation_id = $1 AND type = 'notice' \
         AND payload->>'code' = 'approval_window_ended'",
    )
    .bind(w.conv)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(notices, 1);

    // (P5-UX-16, C-983) The card says it ran out: `approval.expired` with `resume_window`, after
    // `approval.pending` and `approval.decided`, once. The listener's subscription is lazy, so the
    // bus message is repeated with a deadline (the writer is idempotent).
    let count = |ty: &'static str| {
        let pool = w.h.pool().clone();
        let conv = w.conv;
        async move {
            sqlx::query_as::<_, (Value,)>(
                "SELECT payload FROM transcript_events WHERE conversation_id = $1 AND type = $2 \
                 AND payload->>'approval_id' = $3",
            )
            .bind(conv)
            .bind(ty)
            .bind(id.to_string())
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    let mut expired = Vec::new();
    for _ in 0..100 {
        aurora_db::bus::notify(
            w.h.pool(),
            aurora_db::bus::Channel::Approval,
            &json!({"id": id, "workspace_id": w.f.ws, "event": "expired"}),
        )
        .await
        .unwrap();
        expired = count("approval.expired").await;
        if !expired.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(expired.len(), 1, "{expired:?}");
    assert_eq!(expired[0].0["reason"], "resume_window");
    assert_eq!(count("approval.pending").await.len(), 1);
    let decided = count("approval.decided").await;
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0].0["decision"], "approve");
}

#[tokio::test]
async fn conversation_reset_and_deletion_end_parked_rows_and_grants() {
    let w = world().await;
    let ready = row(&w, true, None, "Ready").await;
    let pending = row(&w, true, None, "Pending").await;
    let held = row(&w, false, None, "Held").await;
    assert_eq!(decide(&w, ready, "approve").await.0, 200);
    let mut tx = w.h.pool().begin().await.unwrap();
    aurora_db::repo::lifecycle::reset_session(&mut tx, w.conv)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let pool = w.h.pool().clone();
    let st = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT status, resume_state FROM approvals WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(st(ready).await, ("approved".into(), Some("revoked".into())));
    assert_eq!(st(pending).await.0, "abandoned");
    assert_eq!(st(held).await.0, "pending", "held rows are the gateway's");
}

#[tokio::test]
async fn dependency_events_abandon_pending_parked_rows_and_revoke_ready_grants() {
    let w = world().await;
    let conn = Uuid::now_v7();
    let (ready, pending) = (
        row(&w, true, None, "Ready").await,
        row(&w, true, None, "Pending").await,
    );
    assert_eq!(decide(&w, ready, "approve").await.0, 200);
    sqlx::query("UPDATE approvals SET connection_id = $2 WHERE id = ANY($1)")
        .bind([ready, pending])
        .bind(conn)
        .execute(w.h.pool())
        .await
        .unwrap();
    let mut tx = w.h.pool().begin().await.unwrap();
    repo::abandon_pending_depending_on(
        &mut tx,
        repo::Dependency::Connection {
            connection_id: conn,
            agent_id: None,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(grant(&w, ready).await.0.as_deref(), Some("revoked"));
    let s: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
        .bind(pending)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(s, "abandoned");
}

#[tokio::test]
async fn the_reminder_goes_once_per_decider_at_half_time_and_never_to_a_watcher() {
    let w = world().await;
    let id = row(&w, true, None, "Reminder").await;
    // Created two hours ago with a deadline in two hours: half time has passed.
    sqlx::query("UPDATE approvals SET created_at = now() - interval '2 hours', deadline = now() + interval '2 hours' WHERE id = $1")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    let claims = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM approval_notifications WHERE approval_id = $1 AND stage = 'reminder'",
        )
        .bind(id)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
    };
    w.h.mail.clear();
    aurora_api::domains::approvals::notify::notify_once(&w.h.state)
        .await
        .unwrap();
    let first = claims().await;
    assert!(first >= 1, "a decider is reminded");
    let mails: Vec<_> =
        w.h.mail
            .sent()
            .into_iter()
            .filter(|m| m.subject.starts_with("Reminder"))
            .collect();
    assert_eq!(mails.len() as i64, first);
    assert!(
        mails[0]
            .body
            .contains("the agent has stopped waiting and will resume when you approve")
    );
    aurora_api::domains::approvals::notify::notify_once(&w.h.state)
        .await
        .unwrap();
    assert_eq!(claims().await, first, "once per (approval, user)");
    // The initial claim is unchanged: it is its own stage.
    let initial: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM approval_notifications WHERE approval_id = $1 AND stage = 'initial'",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(initial >= 1);
}

#[tokio::test]
async fn no_reminder_before_half_time() {
    let w = world().await;
    let id = row(&w, true, None, "Early").await;
    sqlx::query("UPDATE approvals SET created_at = now() - interval '1 hour', deadline = now() + interval '5 hours' WHERE id = $1")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    aurora_api::domains::approvals::notify::notify_once(&w.h.state)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM approval_notifications WHERE approval_id = $1 AND stage = 'reminder'",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn approval_limits_reports_the_live_values_and_nothing_else() {
    let w = world().await;
    let (s, _, v) =
        w.h.send(
            Method::GET,
            "/v1/policy/approval-limits",
            &[("cookie", &w.cookie)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    assert_eq!(v["timeout_secs"], 300);
    assert_eq!(v["max_timeout_secs"], 3600);
    assert_eq!(
        v["park"],
        json!({"enabled": false, "threshold_secs": 900, "max_timeout_secs": 86400, "resume_window_secs": 1800})
    );
    assert_eq!(v.as_object().unwrap().len(), 3, "{v}");
    let (s, _, _) =
        w.h.send(Method::GET, "/v1/policy/approval-limits", &[], None)
            .await;
    assert_eq!(s.as_u16(), 401);
}
