#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-APIGAP-1 (D-399, C-1045, C-1046): `POST /v1/agents/{agent}/sandbox/wake` and
//! `SandboxView.idle_stop_after_secs`.

use crate::hosted::{Rig, rig_with};
use http::Method;
use serde_json::Value;
use uuid::Uuid;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

async fn sandbox_on(r: &Rig, handle: &str, state: &str) -> (Uuid, Uuid) {
    let agent = r.agent(handle, false).await;
    let sb = Uuid::now_v7();
    sqlx::query("INSERT INTO sandboxes (id, agent_id, state) VALUES ($1,$2,$3)")
        .bind(sb)
        .bind(agent)
        .bind(state)
        .execute(r.h.pool())
        .await
        .unwrap();
    (agent, sb)
}

async fn wake(r: &Rig, agent: Uuid) -> (u16, Value) {
    r.term_call(
        Method::POST,
        &format!("/v1/agents/{agent}/sandbox/wake"),
        &[],
        None,
    )
    .await
}

async fn wake_at(r: &Rig, sb: Uuid) -> Option<chrono::DateTime<chrono::Utc>> {
    sqlx::query_scalar("SELECT terminal_wake_at FROM sandboxes WHERE id = $1")
        .bind(sb)
        .fetch_one(r.h.pool())
        .await
        .unwrap()
}

/// Asleep → 202 and the wake demand (the start the due-work arm acts on); audited.
#[tokio::test]
async fn wake_from_asleep_is_202_and_requests_the_start() {
    let r = rig_with(|_| {}).await;
    for (i, state) in ["stopped", "unprovisioned"].into_iter().enumerate() {
        let (agent, sb) = sandbox_on(&r, &format!("asleep-{i}"), state).await;
        assert!(wake_at(&r, sb).await.is_none());
        let (s, v) = wake(&r, agent).await;
        assert_eq!(s, 202, "{v}");
        assert_eq!(v["agent_id"], agent.to_string());
        assert!(wake_at(&r, sb).await.is_some(), "{state}: demand set");
        // The audit row is best-effort after the response: poll it.
        let mut meta: Option<Value> = None;
        for _ in 0..100 {
            meta = sqlx::query_scalar(
                "SELECT metadata FROM audit_log WHERE action = 'agent.sandbox_wake' AND target_id = $1",
            )
            .bind(agent.to_string())
            .fetch_optional(r.h.pool())
            .await
            .unwrap();
            if meta.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(meta.expect("audit row")["previous_state"], state);
    }
}

/// Starting or running → 200, the current view, and no second start.
#[tokio::test]
async fn wake_when_awake_is_200_and_requests_nothing() {
    let r = rig_with(|_| {}).await;
    for (i, state) in ["running", "starting"].into_iter().enumerate() {
        let (agent, sb) = sandbox_on(&r, &format!("awake-{i}"), state).await;
        let (s, v) = wake(&r, agent).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["state"], state);
        assert!(wake_at(&r, sb).await.is_none(), "{state}: no demand");
    }
}

#[tokio::test]
async fn wake_failed_frozen_byo_and_foreign_are_refused() {
    let r = rig_with(|_| {}).await;
    // failed → 409 sandbox_failed.
    let (agent, sb) = sandbox_on(&r, "failed-sb", "failed").await;
    let (s, v) = wake(&r, agent).await;
    assert_eq!((s, code(&v)), (409, "sandbox_failed"), "{v}");
    assert!(wake_at(&r, sb).await.is_none());
    // frozen → 423.
    let (agent, sb) = sandbox_on(&r, "frozen-sb", "stopped").await;
    sqlx::query(
        "UPDATE agents SET frozen_at = now(), frozen_reason = 'incident', \
         frozen_authority = 'workspace_owner' WHERE id = $1",
    )
    .bind(agent)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = wake(&r, agent).await;
    assert_eq!((s, code(&v)), (423, "frozen"), "{v}");
    assert!(wake_at(&r, sb).await.is_none());
    // BYO → 409 not_hosted.
    let byo = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'byo','claude_code')",
    )
    .bind(byo)
    .bind(r.org)
    .bind(r.ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = wake(&r, byo).await;
    assert_eq!((s, code(&v)), (409, "not_hosted"), "{v}");
    // An agent of another workspace is invisible: 404.
    let other_org = r.h.org().await;
    let other_ws = r.h.workspace(other_org).await;
    let foreign = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) \
         VALUES ($1,$2,$3,$1::text,$1::text,'hosted','fake')",
    )
    .bind(foreign)
    .bind(other_org)
    .bind(other_ws)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, _) = wake(&r, foreign).await;
    assert_eq!(s, 404);
}

/// The shared hourly wake budget (12) refuses with 429 `rate_limited`.
#[tokio::test]
async fn wake_past_the_shared_budget_is_429() {
    let r = rig_with(|_| {}).await;
    let (agent, _) = sandbox_on(&r, "busy-sb", "stopped").await;
    for _ in 0..12 {
        assert_eq!(wake(&r, agent).await.0, 202);
    }
    let (s, v) = wake(&r, agent).await;
    assert_eq!((s, code(&v)), (429, "rate_limited"), "{v}");
}

/// `idle_stop_after_secs` reflects `hosted.idle_stop_secs`; null when idle stop is off.
#[tokio::test]
async fn sandbox_view_carries_the_idle_timeout() {
    for (cfg, want) in [(1200_u64, Some(1200_u64)), (0, None)] {
        let r = rig_with(move |c| c.hosted.idle_stop_secs = cfg).await;
        let (agent, _) = sandbox_on(&r, "idle-sb", "running").await;
        let (s, v) = r
            .term_call(
                Method::GET,
                &format!("/v1/agents/{agent}/sandbox"),
                &[],
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        match want {
            Some(n) => assert_eq!(v["idle_stop_after_secs"], n),
            None => assert!(v["idle_stop_after_secs"].is_null(), "{v}"),
        }
    }
    // An agent without a sandbox row still says it.
    let r = rig_with(|_| {}).await;
    let agent = r.agent("no-row", false).await;
    let (_, v) = r
        .term_call(
            Method::GET,
            &format!("/v1/agents/{agent}/sandbox"),
            &[],
            None,
        )
        .await;
    assert_eq!(v["state"], "unprovisioned");
    assert!(v["idle_stop_after_secs"].is_u64(), "{v}");
}
