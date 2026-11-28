#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Deciding an approval raised in a delegated task while the task is being cancelled. Both take
//! the conversations, the link and the task before the approval row, so one waits for the other
//! and neither deadlocks.

use crate::peers_rig::{Rig, rig};
use aurora_api::domains::approvals::resume;
use aurora_api::domains::peer_tasks::service::{self as peers, Close, RequesterCancel};
use aurora_db::repo::approvals;
use aurora_db::repo::conversations as convs;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// A task P asked agent `a` to give agent `b`, with a pending gateway approval of `b`'s in it.
struct Held {
    rig: Rig,
    task: Uuid,
    task_conversation: Uuid,
    approval: Uuid,
}

async fn held(batch: Option<&str>) -> Held {
    let rig = rig().await;
    let a = rig.agent(rig.ws1, "alpha").await;
    let b = rig.agent(rig.ws1, "bravo").await;
    let (origin, turn) = rig.person_turn(a, rig.p.id).await;
    let task = delegate(&rig, (a, origin, turn), "@bravo").await;
    let task_conversation = rig.peer_conversation(task).await;
    let approval = pending_approval(&rig, b, task_conversation, batch).await;
    Held {
        rig,
        task,
        task_conversation,
        approval,
    }
}

/// The agent sends a message in its conversation's turn, which ends: the task it opened.
async fn delegate(rig: &Rig, (agent, conv, turn): (Uuid, Uuid, Uuid), to: &str) -> Uuid {
    let sent = rig
        .tool(
            agent,
            conv,
            Some(turn),
            "message_agent",
            json!({"agent": to, "text": "do it"}),
        )
        .await;
    rig.finish_turn(turn).await;
    Rig::id(&Rig::ok(&sent)["peer_task_id"])
}

/// A pending gateway approval of `agent`, raised in `conversation`.
async fn pending_approval(rig: &Rig, agent: Uuid, conversation: Uuid, batch: Option<&str>) -> Uuid {
    let pool = rig.h.pool();
    let gateway = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO gateway_instances (id, hostname, version, started_at, last_seen_at)
         VALUES ($1,'h','0',now(),now())",
    )
    .bind(gateway)
    .execute(pool)
    .await
    .unwrap();
    let approval = Uuid::now_v7();
    let ws = rig.agent_ws(agent).await;
    let turn = rig.running_turn(conversation).await;
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, conversation_id, turn_id,
            gateway_instance_id, method, host, path, summary, status, deadline, batch_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,'POST','api.example.com','/x',
            '{\"title\":\"Push\",\"source\":\"generic\"}','pending',
            now() + interval '5 minutes', $8)",
    )
    .bind(approval)
    .bind(rig.org)
    .bind(ws)
    .bind(agent)
    .bind(conversation)
    .bind(turn)
    .bind(gateway)
    .bind(batch)
    .execute(pool)
    .await
    .unwrap();
    approval
}

/// A delegation chain: P asked alpha, alpha's task is bravo's, and bravo, in that task's
/// conversation, delegated on to charlie. The approval is charlie's, in the second task's
/// conversation; bravo cancels that task, which locks its own conversation (the origin) first.
#[tokio::test]
async fn a_decision_waits_for_the_middle_agent_cancelling_the_task_it_delegated() {
    let rig = rig().await;
    let a = rig.agent(rig.ws1, "alpha").await;
    let b = rig.agent(rig.ws1, "bravo").await;
    let c = rig.agent(rig.ws1, "charlie").await;
    let (origin, turn) = rig.person_turn(a, rig.p.id).await;
    let first = delegate(&rig, (a, origin, turn), "@bravo").await;
    let bravos = rig.peer_conversation(first).await;
    if let Some(t) = rig.running_turn(bravos).await {
        rig.finish_turn(t).await;
    }
    let turn = rig.turn(bravos, b, "person", Some(rig.p.id)).await;
    let second = delegate(&rig, (b, bravos, turn), "@charlie").await;
    let charlies = rig.peer_conversation(second).await;
    let approval = pending_approval(&rig, c, charlies, None).await;
    let pool = rig.h.pool();

    let mut cancel = pool.begin().await.unwrap();
    let pid = backend_pid(&mut cancel).await;
    assert!(convs::lock(&mut cancel, bravos).await.unwrap());
    let uri = format!("/v1/approvals/{approval}/decision");
    let decide = rig.post(&rig.p, &uri, Some(json!({"decision": "approve"})));
    let finish = async {
        until_somebody_waits_for(pool, pid).await;
        let done = peers::cancel(&mut cancel, b, second)
            .await
            .map(|r| r.map(|t| t.status))
            .map_err(|e| e.message().to_owned());
        assert_eq!(done, Ok(Ok("cancelled".to_owned())));
        cancel.commit().await.unwrap();
    };
    let ((status, body), ()) = tokio::join!(decide, finish);
    assert_eq!(
        (status, &body["error"]["code"]),
        (409, &json!("already_decided")),
        "{body}"
    );
}

/// A batch over two task conversations of one agent, while the requester cancels the task whose
/// conversation the batch locks second. The batch holds no row while it waits, and decides what
/// the cancel left.
#[tokio::test]
async fn a_batch_over_two_tasks_waits_for_a_cancel_of_one() {
    let rig = rig().await;
    let a = rig.agent(rig.ws1, "alpha").await;
    let b = rig.agent(rig.ws1, "bravo").await;
    let d = rig.agent(rig.ws1, "delta").await;
    let mut tasks = Vec::new();
    for requester in [a, d] {
        let (origin, turn) = rig.person_turn(requester, rig.p.id).await;
        let task = delegate(&rig, (requester, origin, turn), "@bravo").await;
        let conversation = rig.peer_conversation(task).await;
        let approval = pending_approval(&rig, b, conversation, Some("batch-2")).await;
        tasks.push((task, conversation, approval));
    }
    // The older conversation is locked second.
    let (task, conversation, _) = tasks[0];
    let pool = rig.h.pool();
    let mut cancel = pool.begin().await.unwrap();
    let pid = backend_pid(&mut cancel).await;
    assert!(convs::lock(&mut cancel, conversation).await.unwrap());
    let body = json!({"decision": "deny", "agent_id": b});
    let decide = rig.post(&rig.p, "/v1/approvals/batches/batch-2/decision", Some(body));
    let finish = async {
        until_somebody_waits_for(pool, pid).await;
        let done = peers::cancel_by_requester(&mut cancel, rig.p.id, task)
            .await
            .map(|c| matches!(c, RequesterCancel::Done(_)))
            .map_err(|e| e.message().to_owned());
        assert_eq!(done, Ok(true));
        cancel.commit().await.unwrap();
    };
    let ((status, body), ()) = tokio::join!(decide, finish);
    assert_eq!((status, &body["decided"]), (200, &json!(1)), "{body}");
    let mut statuses = Vec::new();
    for (_, _, approval) in &tasks {
        let s: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
            .bind(approval)
            .fetch_one(pool)
            .await
            .unwrap();
        statuses.push(s);
    }
    assert_eq!(statuses, ["abandoned", "denied"]);
}

async fn backend_pid(conn: &mut sqlx::PgConnection) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(conn)
        .await
        .unwrap()
}

/// Returns once some backend waits for a lock that backend `pid` holds (asks the server until it
/// says so; the timeout only keeps a broken test from hanging).
async fn until_somebody_waits_for(pool: &PgPool, pid: i32) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                  WHERE $1 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(pid)
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("nobody waited for the held lock");
}

/// P decides the held gateway approval in the dashboard.
async fn decide(h: &Held) -> (u16, serde_json::Value) {
    let uri = format!("/v1/approvals/{}/decision", h.approval);
    h.rig
        .post(&h.rig.p, &uri, Some(json!({"decision": "approve"})))
        .await
}

async fn approval_status(h: &Held) -> String {
    sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
        .bind(h.approval)
        .fetch_one(h.rig.h.pool())
        .await
        .unwrap()
}

/// The requester's cancel holds the task conversation (its first lock) when the decision
/// arrives. The decision waits there, holding no approval row, so the cancel finishes.
#[tokio::test]
async fn a_decision_waits_for_a_cancel_that_holds_the_task_conversation() {
    let h = held(None).await;
    let pool = h.rig.h.pool();
    let mut cancel = pool.begin().await.unwrap();
    let pid = backend_pid(&mut cancel).await;
    assert!(convs::lock(&mut cancel, h.task_conversation).await.unwrap());
    let decide = decide(&h);
    let finish_cancel = async {
        until_somebody_waits_for(pool, pid).await;
        let done = peers::cancel_by_requester(&mut cancel, h.rig.p.id, h.task)
            .await
            .map(|c| matches!(c, RequesterCancel::Done(_)));
        assert_eq!(done.map_err(|e| e.message().to_owned()), Ok(true));
        cancel.commit().await.unwrap();
    };
    let ((status, body), ()) = tokio::join!(decide, finish_cancel);
    assert_eq!(
        (status, &body["error"]["code"]),
        (409, &json!("already_decided")),
        "{body}"
    );
    assert_eq!(approval_status(&h).await, "abandoned");
}

/// A close that never takes the task conversation (the tool, the channel card, the expiry
/// sweep) holds the link, its first lock here, when the decision arrives.
#[tokio::test]
async fn a_decision_waits_for_a_close_that_holds_the_link() {
    let h = held(None).await;
    a_close_that_holds_the_link(&h, decide(&h)).await;
}

#[tokio::test]
async fn a_batch_decision_waits_for_a_close_that_holds_the_link() {
    let h = held(Some("batch-1")).await;
    let agent: Uuid = sqlx::query_scalar("SELECT agent_id FROM approvals WHERE id = $1")
        .bind(h.approval)
        .fetch_one(h.rig.h.pool())
        .await
        .unwrap();
    let decide = async {
        h.rig
            .post(
                &h.rig.p,
                "/v1/approvals/batches/batch-1/decision",
                Some(json!({"decision": "deny", "agent_id": agent})),
            )
            .await
    };
    a_close_that_holds_the_link(&h, decide).await;
}

/// Turns the held approval into a parked one: a revocation then writes a notice into its
/// conversation after it has ended the row.
async fn park(h: &Held) {
    sqlx::query(
        "UPDATE approvals SET parked = true, fp_salt = '\\x00', fingerprint = '\\x00'
          WHERE id = $1",
    )
    .bind(h.approval)
    .execute(h.rig.h.pool())
    .await
    .unwrap();
}

/// A revocation has ended the parked row and has its notice still to write when the decision of
/// the same approval arrives. The decision waits; the revocation finishes.
#[tokio::test]
async fn a_decision_waits_for_a_revocation_of_the_same_parked_approval() {
    let h = held(None).await;
    park(&h).await;
    let pool = h.rig.h.pool();
    let agent: Uuid = sqlx::query_scalar("SELECT agent_id FROM approvals WHERE id = $1")
        .bind(h.approval)
        .fetch_one(pool)
        .await
        .unwrap();
    let mut revoke = pool.begin().await.unwrap();
    let pid = backend_pid(&mut revoke).await;
    let ended = approvals::abandon_pending_depending_on(
        &mut revoke,
        approvals::Dependency::AgentSecrets { agent_id: agent },
    )
    .await
    .unwrap();
    assert_eq!(ended.len(), 1);
    let decide = decide(&h);
    let finish = async {
        until_somebody_waits_for(pool, pid).await;
        let noticed = resume::notify_ended(&mut revoke, &ended, "access_removed")
            .await
            .map_err(|e| e.message().to_owned());
        assert_eq!(noticed, Ok(()));
        revoke.commit().await.unwrap();
    };
    let ((status, body), ()) = tokio::join!(decide, finish);
    assert_eq!(
        (status, &body["error"]["code"]),
        (409, &json!("already_decided")),
        "{body}"
    );
    assert_eq!(approval_status(&h).await, "abandoned");
}

/// The same moment of a revocation, with the requester's cancel arriving: the cancel waits.
#[tokio::test]
async fn a_cancel_waits_for_a_revocation_of_its_tasks_parked_approval() {
    let h = held(None).await;
    park(&h).await;
    let pool = h.rig.h.pool();
    let mut revoke = pool.begin().await.unwrap();
    let pid = backend_pid(&mut revoke).await;
    let ended = approvals::end_parked_of(
        &mut revoke,
        approvals::Scope::Conversation(h.task_conversation),
    )
    .await
    .unwrap();
    assert_eq!(ended.len(), 1);
    let cancel = async {
        let mut tx = pool.begin().await.unwrap();
        let done = peers::cancel_by_requester(&mut tx, h.rig.p.id, h.task)
            .await
            .map(|c| matches!(c, RequesterCancel::Done(_)))
            .map_err(|e| e.message().to_owned());
        if done.is_ok() {
            tx.commit().await.unwrap();
        }
        done
    };
    let finish = async {
        until_somebody_waits_for(pool, pid).await;
        let noticed = resume::notify_ended(&mut revoke, &ended, "access_removed")
            .await
            .map_err(|e| e.message().to_owned());
        if noticed.is_ok() {
            revoke.commit().await.unwrap();
        }
        noticed
    };
    let (cancelled, noticed) = tokio::join!(cancel, finish);
    assert_eq!((cancelled, noticed), (Ok(true), Ok(())));
    assert_eq!(h.rig.task_status(h.task).await, "cancelled");
}

/// A freeze ends the rows of many conversations and then writes into them. It waits for a
/// writer that holds a conversation without holding any of its rows.
#[tokio::test]
async fn a_freeze_waits_for_a_conversation_before_it_ends_the_rows() {
    let h = held(None).await;
    park(&h).await;
    let pool = h.rig.h.pool();
    let agent: Uuid = sqlx::query_scalar("SELECT agent_id FROM approvals WHERE id = $1")
        .bind(h.approval)
        .fetch_one(pool)
        .await
        .unwrap();
    let mut holder = pool.begin().await.unwrap();
    let pid = backend_pid(&mut holder).await;
    assert!(convs::lock(&mut holder, h.task_conversation).await.unwrap());
    let freeze = async {
        let mut tx = pool.begin().await.unwrap();
        let ended = aurora_db::repo::freeze::abandon_pending_approvals(&mut tx, &[agent])
            .await
            .map_err(|e| e.to_string())?;
        resume::notify_ended(&mut tx, &ended, "frozen")
            .await
            .map_err(|e| e.message().to_owned())?;
        tx.commit().await.map_err(|e| e.to_string())
    };
    let finish = async {
        until_somebody_waits_for(pool, pid).await;
        let ended = approvals::abandon_pending_of_conversation(&mut holder, h.task_conversation)
            .await
            .map(|e| e.len())
            .map_err(|e| e.to_string());
        if ended.is_ok() {
            holder.commit().await.unwrap();
        }
        ended
    };
    let (frozen, ended) = tokio::join!(freeze, finish);
    assert_eq!((frozen, ended), (Ok(()), Ok(1)));
}

/// The same for the sweep of resume windows that ran out.
#[tokio::test]
async fn the_resume_window_sweep_waits_for_a_conversation_before_it_ends_the_rows() {
    let h = held(None).await;
    park(&h).await;
    let pool = h.rig.h.pool();
    sqlx::query(
        "UPDATE approvals SET status = 'approved', resume_state = 'ready',
            resume_expires_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(h.approval)
    .execute(pool)
    .await
    .unwrap();
    let mut holder = pool.begin().await.unwrap();
    let pid = backend_pid(&mut holder).await;
    assert!(convs::lock(&mut holder, h.task_conversation).await.unwrap());
    let sweep = async {
        aurora_api::jobs::sweep_resume_windows(pool)
            .await
            .map_err(|e| e.message().to_owned())
    };
    let finish = async {
        until_somebody_waits_for(pool, pid).await;
        let revoked = approvals::end_parked_of(
            &mut holder,
            approvals::Scope::Conversation(h.task_conversation),
        )
        .await
        .map(|e| e.len())
        .map_err(|e| e.to_string());
        if revoked.is_ok() {
            holder.commit().await.unwrap();
        }
        revoked
    };
    let (swept, revoked) = tokio::join!(sweep, finish);
    // The revocation had the conversation first: it ends the grant and the sweep finds none.
    assert_eq!((swept, revoked), (Ok(0), Ok(1)));
}

async fn a_close_that_holds_the_link(
    h: &Held,
    decide: impl Future<Output = (u16, serde_json::Value)>,
) {
    let pool = h.rig.h.pool();
    // The receiving turn ended while the approval waits, so the decision is the task's wake and
    // takes the link.
    sqlx::query("UPDATE peer_tasks SET status = 'waiting' WHERE id = $1")
        .bind(h.task)
        .execute(pool)
        .await
        .unwrap();
    let mut close = pool.begin().await.unwrap();
    let pid = backend_pid(&mut close).await;
    sqlx::query(
        "SELECT 1 FROM agent_links WHERE id = (SELECT link_id FROM peer_tasks WHERE id = $1)
         FOR UPDATE",
    )
    .bind(h.task)
    .execute(&mut *close)
    .await
    .unwrap();
    let finish_close = async {
        until_somebody_waits_for(pool, pid).await;
        let task = aurora_db::repo::peer_tasks::get(&mut *close, h.task)
            .await
            .unwrap()
            .unwrap();
        let closed = peers::close_with_promotion(&mut close, &task, &Close::Cancelled(None))
            .await
            .map(|c| c.is_some());
        assert_eq!(closed.map_err(|e| e.message().to_owned()), Ok(true));
        close.commit().await.unwrap();
    };
    let ((status, body), ()) = tokio::join!(decide, finish_close);
    assert!(status < 500, "{status} {body}");
    assert_eq!(approval_status(h).await, "abandoned");
}
