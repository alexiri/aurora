#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-185 (D-401): suspending a member in an organization (`PATCH /v1/orgs/{org}/members/{user}`)
//! ends the person's queued and running speaker turns in every workspace of the org, in the same
//! transaction. Another member's turns are untouched; restoring starts nothing.

use crate::common::*;
use http::Method;
use serde_json::json;
use uuid::Uuid;

async fn add_turn(h: &Harness, ws: Uuid, agent: Uuid, speaker: Uuid, status: &str) -> (Uuid, Uuid) {
    let conv = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) \
         VALUES ($1, $2, $3, 'api', $1::text, now())",
    )
    .bind(conv)
    .bind(agent)
    .bind(ws)
    .execute(h.pool())
    .await
    .unwrap();
    let turn = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, \
         speaker_user_id, speaker_label, text) \
         VALUES ($1, $2, $3, 'turn', $5, '{}', 'person', $4, 'P', 'hi')",
    )
    .bind(turn)
    .bind(conv)
    .bind(agent)
    .bind(speaker)
    .bind(status)
    .execute(h.pool())
    .await
    .unwrap();
    (conv, turn)
}

async fn turn_state(h: &Harness, t: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, error_code FROM turns WHERE id = $1")
        .bind(t)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn org_suspension_ends_the_persons_turns_in_every_workspace() {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws1 = h.workspace(org).await;
    let ws2 = h.workspace(org).await;
    let a1 = h.byo_agent(org, ws1, "bot", "Bot").await;
    let a2 = h.byo_agent(org, ws2, "bot2", "Bot2").await;
    let boss = h.user("boss@x.io", false).await;
    h.member(org, boss, "owner", "active").await;
    let person = h.user("person@x.io", false).await;
    h.member(org, person, "member", "active").await;
    let other = h.user("other@x.io", false).await;
    h.member(org, other, "member", "active").await;
    let (_, tok) = h.good_session(boss).await;
    let ck = cookie(&tok);

    let (c_queued, t_queued) = add_turn(&h, ws1, a1, person, "queued").await;
    let (_, t_running) = add_turn(&h, ws2, a2, person, "running").await;
    let (_, t_other) = add_turn(&h, ws1, a1, other, "queued").await;

    let uri = format!("/v1/orgs/{org}/members/{person}");
    let (s, v) = h
        .call(
            &ck,
            Method::PATCH,
            &uri,
            Some(json!({"status": "suspended"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    for t in [t_queued, t_running] {
        assert_eq!(
            turn_state(&h, t).await,
            ("aborted".to_owned(), Some("user_suspended".to_owned()))
        );
    }
    assert_eq!(turn_state(&h, t_other).await.0, "queued");
    // The notice is in the transcript (written in the transaction; read back, bounded).
    let mut text: Option<String> = None;
    for _ in 0..50 {
        text = sqlx::query_scalar(
            "SELECT payload->>'text' FROM transcript_events \
             WHERE conversation_id = $1 AND type = 'notice'",
        )
        .bind(c_queued)
        .fetch_optional(h.pool())
        .await
        .unwrap();
        if text.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        text.as_deref(),
        Some("This turn was stopped: the person was suspended in this organization.")
    );

    // Restoring starts nothing: the ended turns stay ended.
    let (s, v) = h
        .call(&ck, Method::PATCH, &uri, Some(json!({"status": "active"})))
        .await;
    assert_eq!(s, 200, "{v}");
    for t in [t_queued, t_running] {
        assert_eq!(turn_state(&h, t).await.0, "aborted");
    }
    assert_eq!(turn_state(&h, t_other).await.0, "queued");
}
