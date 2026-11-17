#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-011B-2 (D-264(d), C-825, C-826, SEC-74): what the revocation hooks leave behind for a
//! **parked** approval — the state change, the transcript notice where a transcript remains
//! (`approval_grant_revoked` for a `ready` grant, `approval_abandoned` for a pending parked row,
//! nothing for a held row), the metric `aurora_approvals_grants_revoked_total{cause}`, and no
//! turn. The state transitions themselves are covered in `approval_park.rs`.

use crate::approval_park::{World, decide, followups, grant, row, world};
use crate::common::*;
use aurora_api::domains::approvals::resume;
use aurora_db::repo::approvals as repo;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

/// `(code, text)` of the conversation's notices about approvals, oldest first.
async fn notices(w: &World) -> Vec<(String, String, Uuid)> {
    let rows: Vec<(Value,)> = sqlx::query_as(
        "SELECT payload FROM transcript_events WHERE conversation_id = $1 AND type = 'notice' \
         AND payload ? 'approval_id' ORDER BY seq",
    )
    .bind(w.conv)
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    rows.into_iter()
        .map(|(p,)| {
            (
                p["code"].as_str().unwrap().to_owned(),
                p["text"].as_str().unwrap().to_owned(),
                p["approval_id"].as_str().unwrap().parse().unwrap(),
            )
        })
        .collect()
}

async fn status(w: &World, id: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, resume_state FROM approvals WHERE id = $1")
        .bind(id)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

const REVOKED: &str = "aurora_approvals_grants_revoked_total{cause=\"frozen\"}";

#[tokio::test]
async fn freezing_the_agent_ends_parked_rows_with_notices_and_never_revives() {
    let w = world().await;
    let ready = row(&w, true, None, "Ready").await;
    let pending = row(&w, true, None, "Pending").await;
    let held = row(&w, false, None, "Held").await;
    assert_eq!(decide(&w, ready, "approve").await.0, 200);
    let turns_before = followups(&w).await.len();
    let owner = w.h.user("own@x.io", false).await;
    w.h.member(w.f.org, owner, "member", "active").await;
    w.h.bind_ws(w.f.ws, owner, "owner").await;
    let (_, t) = w.h.good_session(owner).await;
    let before = counter_value(REVOKED);
    let (s, _, v) =
        w.h.send(
            Method::POST,
            &format!("/v1/agents/{}/freeze", w.f.agent),
            &[("cookie", &cookie(&t)), ("origin", ORIGIN)],
            Some(json!({"reason": "incident 42"})),
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    assert_eq!(
        status(&w, ready).await,
        ("approved".into(), Some("revoked".into()))
    );
    assert_eq!(status(&w, pending).await.0, "abandoned");
    assert_eq!(status(&w, held).await.0, "abandoned", "FIX-084 as before");
    let n = notices(&w).await;
    let of = |id: Uuid| -> Vec<&str> {
        n.iter()
            .filter(|(_, _, a)| *a == id)
            .map(|(c, _, _)| c.as_str())
            .collect()
    };
    assert_eq!(of(ready), ["approval_grant_revoked"], "{n:?}");
    assert_eq!(of(pending), ["approval_abandoned"], "{n:?}");
    assert!(of(held).is_empty(), "a held row has no notice: {n:?}");
    assert!(
        n.iter()
            .find(|(c, _, _)| c == "approval_grant_revoked")
            .unwrap()
            .1
            .contains("paused by an administrator"),
        "{n:?}"
    );
    assert_eq!(counter_value(REVOKED) - before, 1.0);
    assert_eq!(followups(&w).await.len(), turns_before, "no turn");
    // Unfreeze revives nothing.
    let (s, _, v) =
        w.h.send(
            Method::POST,
            &format!("/v1/agents/{}/unfreeze", w.f.agent),
            &[("cookie", &cookie(&t)), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    assert_eq!(grant(&w, ready).await.0.as_deref(), Some("revoked"));
    assert_eq!(status(&w, pending).await.0, "abandoned");
}

#[tokio::test]
async fn a_session_epoch_bump_and_a_dependency_event_write_their_notices() {
    let w = world().await;
    let (ready, pending) = (
        row(&w, true, None, "Ready").await,
        row(&w, true, None, "Pending").await,
    );
    assert_eq!(decide(&w, ready, "approve").await.0, 200);
    // The session ends (harness switch / `reset_session` redaction).
    let before = counter_value("aurora_approvals_grants_revoked_total{cause=\"session_reset\"}");
    let mut tx = w.h.pool().begin().await.unwrap();
    resume::end_parked(&mut tx, repo::Scope::Conversation(w.conv), "session_reset")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let n = notices(&w).await;
    assert_eq!(
        n.iter()
            .map(|(c, _, _)| c.as_str())
            .collect::<Vec<_>>()
            .len(),
        2,
        "{n:?}"
    );
    assert!(n.iter().any(|(c, t, a)| c == "approval_grant_revoked"
        && *a == ready
        && t.contains("session was reset")));
    assert!(
        n.iter()
            .any(|(c, _, a)| c == "approval_abandoned" && *a == pending)
    );
    assert_eq!(
        counter_value("aurora_approvals_grants_revoked_total{cause=\"session_reset\"}") - before,
        1.0
    );
    // A second call finds nothing: no duplicate notices.
    let mut tx = w.h.pool().begin().await.unwrap();
    resume::end_parked(&mut tx, repo::Scope::Conversation(w.conv), "session_reset")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(notices(&w).await.len(), 2);

    // An access-removal event (FIX-084 predicate) on a fresh pair.
    let conn = Uuid::now_v7();
    let (ready2, pending2) = (
        row(&w, true, None, "Ready2").await,
        row(&w, true, None, "Pending2").await,
    );
    assert_eq!(decide(&w, ready2, "approve").await.0, 200);
    sqlx::query("UPDATE approvals SET connection_id = $2 WHERE id = ANY($1)")
        .bind([ready2, pending2])
        .bind(conn)
        .execute(w.h.pool())
        .await
        .unwrap();
    let mut tx = w.h.pool().begin().await.unwrap();
    let ended = repo::abandon_pending_depending_on(
        &mut tx,
        repo::Dependency::Connection {
            connection_id: conn,
            agent_id: None,
        },
    )
    .await
    .unwrap();
    resume::notify_ended(&mut tx, &ended, "access_removed")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let n = notices(&w).await;
    let last: Vec<&str> = n
        .iter()
        .filter(|(_, _, a)| *a == ready2 || *a == pending2)
        .map(|(c, _, _)| c.as_str())
        .collect();
    assert_eq!(last.len(), 2, "{n:?}");
    assert!(last.contains(&"approval_grant_revoked") && last.contains(&"approval_abandoned"));
}
