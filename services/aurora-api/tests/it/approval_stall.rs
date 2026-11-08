#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P2-011A (RFE-011a, D-56, SEC-49): the stall rule of stale-turn recovery with approvals. A turn
//! blocked on a `pending` gateway hold is exempt from `turn_stalled`; the clock restarts at the
//! latest `resolved_at`; `TURN_CEILING` still applies; action approvals never exempt.

use crate::hosted::{Live, Rig, rig};
use serde_json::json;
use uuid::Uuid;

/// A started turn with a stale `last_progress_at` (11 minutes ago, past `TURN_STALL`).
async fn stale_turn(name: &str) -> (Rig, Live, String) {
    let r = rig().await;
    let lv = r.boot(name).await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    sqlx::query(
        "UPDATE turns SET last_progress_at = now() - interval '11 minutes' WHERE id = $1::uuid",
    )
    .bind(&turn)
    .execute(r.h.pool())
    .await
    .unwrap();
    (r, lv, turn)
}

/// A gateway hold of the turn: `status`, `deadline` and `resolved_at` as offsets (seconds from now).
async fn hold(r: &Rig, lv: &Live, turn: &str, status: &str, deadline: i64, resolved: Option<i64>) {
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, conversation_id, turn_id, \
         gateway_instance_id, method, host, path, summary, status, deadline, resolved_at) \
         VALUES ($1, $2, $3, $4, $5, $6::uuid, $7, 'GET', 'api.example.com', '/x', \
                 '{\"title\":\"Deploy\",\"fields\":[],\"source\":\"generic\"}', $8, \
                 now() + make_interval(secs => $9), \
                 CASE WHEN $10::float8 IS NULL THEN NULL \
                      ELSE now() + make_interval(secs => $10) END)",
    )
    .bind(Uuid::now_v7())
    .bind(r.org)
    .bind(r.ws)
    .bind(lv.agent)
    .bind(lv.conv)
    .bind(turn)
    .bind(Uuid::now_v7())
    .bind(status)
    .bind(deadline as f64)
    .bind(resolved.map(|s| s as f64))
    .execute(r.h.pool())
    .await
    .unwrap();
}

async fn status(r: &Rig, turn: &str) -> (String, Option<String>) {
    r.work().await;
    r.turn_status(turn).await
}

#[tokio::test]
async fn pending_approval_exempts_the_turn_from_stall_failure() {
    let (r, lv, turn) = stale_turn("stall-pending").await;
    // A 20-minute hold: far beyond TURN_STALL, still pending, inside its deadline.
    hold(&r, &lv, &turn, "pending", 20 * 60, None).await;
    assert_eq!(status(&r, &turn).await.0, "running", "exempt while pending");
    assert_eq!(status(&r, &turn).await.0, "running", "and on every sweep");
}

#[tokio::test]
async fn stall_clock_restarts_at_resolution_for_every_terminal_status() {
    for (st, name) in [
        ("approved", "stall-approved"),
        ("denied", "stall-denied"),
        ("expired", "stall-expired"),
        ("abandoned", "stall-abandoned"),
    ] {
        let (r, lv, turn) = stale_turn(name).await;
        // Resolved a minute ago: the turn had no chance to progress yet, so it is not stalled.
        hold(&r, &lv, &turn, st, -60, Some(-60)).await;
        assert_eq!(
            status(&r, &turn).await.0,
            "running",
            "{st}: clock restarted"
        );
        // Resolved 11 minutes ago with nothing since: stalled again.
        sqlx::query("UPDATE approvals SET resolved_at = now() - interval '11 minutes' WHERE turn_id = $1::uuid")
            .bind(&turn)
            .execute(r.h.pool())
            .await
            .unwrap();
        assert_eq!(
            status(&r, &turn).await,
            ("failed".into(), Some("turn_stalled".into())),
            "{st}"
        );
    }
}

#[tokio::test]
async fn a_pending_hold_past_its_deadline_does_not_exempt() {
    let (r, lv, turn) = stale_turn("stall-late").await;
    // Still `pending` in the table but past its deadline: the exemption ends with the deadline.
    hold(&r, &lv, &turn, "pending", -30, None).await;
    assert_eq!(
        status(&r, &turn).await,
        ("failed".into(), Some("turn_stalled".into()))
    );
}

#[tokio::test]
async fn ceiling_still_fails_a_turn_with_a_pending_approval() {
    let (r, lv, turn) = stale_turn("stall-ceiling").await;
    hold(&r, &lv, &turn, "pending", 3600, None).await;
    sqlx::query("UPDATE turns SET dispatched_at = now() - interval '7 hours' WHERE id = $1::uuid")
        .bind(&turn)
        .execute(r.h.pool())
        .await
        .unwrap();
    let items = r.work().await;
    assert!(items.iter().any(|i| i["kind"] == "turn.abort"));
    assert_eq!(
        r.turn_status(&turn).await,
        ("failed".into(), Some("time_limit".into()))
    );
}

#[tokio::test]
async fn action_approvals_do_not_exempt_the_turn() {
    let (r, lv, turn) = stale_turn("stall-action").await;
    // The agent received `pending_approval` and continued: it is not blocked on this row.
    sqlx::query(
        "INSERT INTO action_approvals (id, workspace_id, agent_id, conversation_id, turn_id, \
         action_key, payload, summary, status, expires_at) \
         VALUES ($1, $2, $3, $4, $5::uuid, 'send_message', '{}', 'Send a message', 'pending', \
                 now() + interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(r.ws)
    .bind(lv.agent)
    .bind(lv.conv)
    .bind(&turn)
    .execute(r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        status(&r, &turn).await,
        ("failed".into(), Some("turn_stalled".into()))
    );
}
