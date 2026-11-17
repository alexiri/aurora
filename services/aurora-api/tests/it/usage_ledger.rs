#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-USAGE-1 (D-359, C-1001): the `usage_daily` ledger grows in the transaction that ends a turn, once,
//! under the default model's key for the turn and its time and per `by_model` entry for tokens, from a
//! value the trust rule has reduced, and survives the deletion of the conversation and the agent.

use crate::hosted::{Live, Rig};
use crate::model_roles::{agent_on, rig};
use serde_json::{Value, json};
use uuid::Uuid;

type Row = (
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
    i64,
    i64,
);

fn entry(model: &str, input: i64, output: i64) -> Value {
    json!({"model": model, "input_tokens": input, "output_tokens": output,
           "cache_read_tokens": 1, "cache_write_tokens": 2})
}

/// `(provider, endpoint_id, model, turns, turn_seconds, input, output)` of an agent's ledger rows.
async fn ledger(r: &Rig, agent: Uuid) -> Vec<Row> {
    sqlx::query_as(
        "SELECT provider, endpoint_id, model, turns, turn_seconds, input_tokens, output_tokens
         FROM usage_daily WHERE agent_id = $1 ORDER BY model",
    )
    .bind(agent)
    .fetch_all(r.h.pool())
    .await
    .unwrap()
}

async fn started(r: &Rig, lv: &Live) -> String {
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(
        r.report(lv, json!({"kind": "turn.started", "turn_id": turn}))
            .await,
        "applied"
    );
    turn
}

#[tokio::test]
async fn a_finished_turn_adds_tokens_per_model_and_one_turn_to_the_default() {
    let r = rig().await;
    let a = agent_on(&r, "meter", "listed").await;
    let lv = r.boot_agent(a).await;
    let turn = started(&r, &lv).await;
    let res = json!({"kind": "turn.result", "turn_id": turn, "status": "done",
        "usage": {"by_model": [entry("m-def", 100, 10), entry("m-big", 200, 20)]}});
    assert_eq!(r.report(&lv, res.clone()).await, "applied");
    let want: Vec<Row> = vec![
        (
            Some("litellm".into()),
            Some("listed".into()),
            Some("m-big".into()),
            0,
            0,
            200,
            20,
        ),
        (
            Some("litellm".into()),
            Some("listed".into()),
            Some("m-def".into()),
            1,
            0,
            100,
            10,
        ),
    ];
    let got = ledger(&r, a).await;
    // turn_seconds is the floor of a sub-second test turn
    assert_eq!(
        got.iter()
            .map(|g| (g.2.clone(), g.3, g.5, g.6))
            .collect::<Vec<_>>(),
        want.iter()
            .map(|g| (g.2.clone(), g.3, g.5, g.6))
            .collect::<Vec<_>>(),
        "{got:?}"
    );
    assert!(got.iter().all(|g| g.0.as_deref() == Some("litellm")));
    assert!(got.iter().all(|g| g.1.as_deref() == Some("listed")));

    // a replayed terminal report adds nothing
    assert_eq!(r.report(&lv, res).await, "duplicate");
    assert_eq!(ledger(&r, a).await, got);
}

#[tokio::test]
async fn a_turn_that_never_started_adds_nothing() {
    let r = rig().await;
    let a = agent_on(&r, "idle", "listed").await;
    let conv = r.direct(a).await;
    r.say(conv, "hello").await;
    // queued, never dispatched: the user aborts it
    let (turn,): (Uuid,) =
        sqlx::query_as("SELECT id FROM turns WHERE conversation_id = $1 AND role = 'turn'")
            .bind(conv)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    let mut tx = r.h.pool().begin().await.unwrap();
    let done = aurora_api::domains::conversations::service::finish_turn(
        &mut tx, turn, "aborted", None, None, None,
    )
    .await
    .unwrap();
    assert!(done.is_some());
    tx.commit().await.unwrap();
    assert_eq!(ledger(&r, a).await, vec![]);
}

#[tokio::test]
async fn an_aborted_turn_holding_only_a_streaming_claim_is_counted_after_the_fold() {
    let r = rig().await;
    let a = agent_on(&r, "stream", "listed").await;
    let lv = r.boot_agent(a).await;
    let turn = started(&r, &lv).await;
    let claim = json!({"by_model": [entry("m-def", 100, 10), entry("m-not-allowed", 500, 50)]});
    assert_eq!(
        r.report(
            &lv,
            json!({"kind": "turn.events", "turn_id": turn, "events": [
                {"n": 1, "event": {"kind": "usage", "usage": claim}}]})
        )
        .await,
        "applied"
    );
    // the stored claim was reduced when it was stored
    let u: Value = sqlx::query_scalar("SELECT usage FROM turns WHERE id = $1::uuid")
        .bind(&turn)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(u["unverified_models"], true, "{u}");
    // the turn ends with no terminal usage
    assert_eq!(
        r.report(
            &lv,
            json!({"kind": "turn.result", "turn_id": turn, "status": "aborted"})
        )
        .await,
        "applied"
    );
    let got = ledger(&r, a).await;
    assert_eq!(
        got.iter()
            .map(|g| (g.2.clone(), g.3, g.5))
            .collect::<Vec<_>>(),
        [(Some("m-def".into()), 1, 600)],
        "the foreign entry is folded into the default's: {got:?}"
    );
}

#[tokio::test]
async fn deleting_the_conversation_and_the_agent_leaves_the_ledger_alone() {
    let r = rig().await;
    let a = agent_on(&r, "gone", "listed").await;
    let lv = r.boot_agent(a).await;
    let turn = started(&r, &lv).await;
    assert_eq!(
        r.report(
            &lv,
            json!({"kind": "turn.result", "turn_id": turn, "status": "done",
                "usage": {"input_tokens": 7, "output_tokens": 3}})
        )
        .await,
        "applied"
    );
    let before = ledger(&r, a).await;
    assert_eq!(before.len(), 1);
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(lv.conv)
        .execute(r.h.pool())
        .await
        .unwrap();
    assert_eq!(ledger(&r, a).await, before);
    sqlx::query("DELETE FROM agents WHERE id = $1")
        .bind(a)
        .execute(r.h.pool())
        .await
        .unwrap();
    assert_eq!(ledger(&r, a).await, before);
}
