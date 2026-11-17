#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-NARR (D-279, D-280, D-286, C-846..C-850, SEC-111): live narration cards. A submodule of
//! `channels_inbox.rs` (the fake-connector world lives there); the hooks the lifecycle and the
//! event ingest call are driven directly.

use super::*;
use aurora_api::domains::channels::{mirror, narration};
use aurora_db::repo::turns;

const AGE: &str = "UPDATE channel_ops SET created_at = created_at - interval '10 seconds' WHERE dedupe_key LIKE 'narr:%'";

impl W {
    async fn narrate_on(&self, agent: Uuid) {
        sqlx::query("UPDATE agents SET narration_cards = true WHERE id = $1")
            .bind(agent)
            .execute(self.h.pool())
            .await
            .unwrap();
    }

    async fn turn_row(&self, turn: Uuid) -> aurora_db::repo::turns::TurnRow {
        turns::get(self.h.pool(), turn).await.unwrap().unwrap()
    }

    /// The supervisor's `turn.started` report: running, then the lifecycle hook.
    async fn start_turn(&self, turn: Uuid) {
        sqlx::query("UPDATE turns SET status='running', started_at = now() WHERE id=$1")
            .bind(turn)
            .execute(self.h.pool())
            .await
            .unwrap();
        let row = self.turn_row(turn).await;
        let mut tx = self.h.pool().begin().await.unwrap();
        mirror::on_turn_started(&mut tx, &row).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// One tool step as the event ingest writes it, then the ingest's refresh hook.
    async fn tools(&self, turn: Uuid, steps: &[(&str, &str, Option<bool>)]) {
        let row = self.turn_row(turn).await;
        let mut tx = self.h.pool().begin().await.unwrap();
        for (i, (name, summary, ok)) in steps.iter().enumerate() {
            let call = format!("c{i}");
            conv::append_event(
                &mut tx,
                row.conversation_id,
                Some(turn),
                "tool.started",
                &json!({"turn_id": turn, "call_id": call, "name": name, "summary": summary}),
            )
            .await
            .unwrap();
            if let Some(ok) = ok {
                conv::append_event(
                    &mut tx,
                    row.conversation_id,
                    Some(turn),
                    "tool.finished",
                    &json!({"turn_id": turn, "call_id": call, "ok": ok}),
                )
                .await
                .unwrap();
            }
        }
        narration::refresh(&mut tx, &row).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// The card post as delivered by the connector (`message_ref` `P9`).
    async fn card_delivered(&self) {
        sqlx::query(
            "UPDATE channel_ops SET status='done', result='{\"message_ref\":\"P9\"}'
              WHERE dedupe_key ~ '^narr:[0-9a-f-]{36}$'",
        )
        .execute(self.h.pool())
        .await
        .unwrap();
    }

    /// Every narration op: `(dedupe_key, kind, payload)`, oldest first.
    async fn narr_ops(&self) -> Vec<(String, String, Value)> {
        sqlx::query_as(
            "SELECT dedupe_key, kind, payload FROM channel_ops WHERE dedupe_key LIKE 'narr:%'
              ORDER BY created_at, id",
        )
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }

    /// A DM world with one agent that narrates and one turn started: `(world, agent, turn)`.
    async fn narrating_dm() -> (W, Uuid, Uuid) {
        let w = world().await;
        let a = w.agent("rex").await;
        w.narrate_on(a).await;
        w.presence(w.integ, a, "dedicated", Some("BD1")).await;
        w.link(w.integ, "U1", w.member).await;
        w.post(w.integ, "e1", dm("U1", "q", "BD1")).await;
        let turn = w.turns().await[0];
        w.start_turn(turn).await;
        (w, a, turn)
    }
}

#[tokio::test]
async fn a_narrating_agent_gets_one_card_per_turn_and_a_restart_posts_no_second() {
    let (w, _a, turn) = W::narrating_dm().await;
    let ops = w.narr_ops().await;
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0].0, format!("narr:{turn}"));
    assert_eq!(ops[0].1, "message.post");
    let n = &ops[0].2["content"]["narration"];
    assert_eq!(n["status"], "running");
    assert_eq!(n["turn_id"], turn.to_string());
    // A restarted projector (the started hook runs again) resumes the same card.
    w.start_turn(turn).await;
    assert_eq!(w.narr_ops().await.len(), 1);
}

#[tokio::test]
async fn an_agent_with_the_setting_off_gets_the_receipt_and_no_card() {
    let w = world().await;
    let a = w.agent("rex").await;
    w.presence(w.integ, a, "dedicated", Some("BD1")).await;
    w.link(w.integ, "U1", w.member).await;
    w.post(w.integ, "e1", dm("U1", "q", "BD1")).await;
    let turn = w.turns().await[0];
    w.start_turn(turn).await;
    assert!(w.narr_ops().await.is_empty());
    assert_eq!(w.ops("reaction.add").await.len(), 1, "the receipt is untouched");
    let on: bool = sqlx::query_scalar("SELECT narration_cards FROM agents WHERE id=$1")
        .bind(a)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert!(!on, "off by default (migration 000231)");
}

#[tokio::test]
async fn a_connector_without_edits_gets_no_card() {
    let w = world().await;
    let a = w.agent("rex").await;
    w.narrate_on(a).await;
    let i = w
        .integration(json!({"_capabilities": {"edits": false}}), true)
        .await;
    w.presence(i, a, "shared", None).await;
    w.link(i, "U1", w.member).await;
    w.post(i, "p1", dm("U1", "q", "B1")).await;
    let turn = w.turns().await[0];
    w.start_turn(turn).await;
    w.tools(turn, &[("Read", "", Some(true))]).await;
    assert!(w.narr_ops().await.is_empty());
    w.finish_turn(turn, "done", Some("answer")).await;
    assert_eq!(w.ops("message.post").await.len(), 1, "the answer alone");
}

#[tokio::test]
async fn steps_update_in_place_and_are_cut_to_the_last_twelve() {
    let (w, _a, turn) = W::narrating_dm().await;
    // Nothing to update until the card is delivered.
    w.tools(turn, &[("Read", "src/a.rs", Some(true))]).await;
    assert_eq!(w.narr_ops().await.len(), 1);
    w.card_delivered().await;
    sqlx::query(AGE).execute(w.h.pool()).await.unwrap();
    let many: Vec<(String, String, Option<bool>)> = (0..14)
        .map(|i| (format!("Tool{i}"), String::new(), Some(i != 3)))
        .collect();
    let refs: Vec<(&str, &str, Option<bool>)> =
        many.iter().map(|(n, s, o)| (n.as_str(), s.as_str(), *o)).collect();
    w.tools(turn, &refs).await;
    let ops = w.narr_ops().await;
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert_eq!(ops[1].1, "message.update");
    assert_eq!(ops[1].2["message_ref"], "P9");
    let n = &ops[1].2["content"]["narration"];
    assert_eq!(n["status"], "running");
    assert_eq!(n["steps"].as_array().unwrap().len(), 12);
    assert_eq!(n["earlier"], 3, "15 steps, 12 listed");
    assert_eq!(n["steps"][11]["status"], "ok");
}

#[tokio::test]
async fn a_burst_is_one_update_per_interval() {
    let (w, _a, turn) = W::narrating_dm().await;
    w.card_delivered().await;
    sqlx::query(AGE).execute(w.h.pool()).await.unwrap();
    w.tools(turn, &[("Read", "", Some(true))]).await;
    // Inside the interval: nothing more is written, however many steps arrive.
    w.tools(turn, &[("Grep", "", Some(true))]).await;
    w.tools(turn, &[("Edit", "", None)]).await;
    assert_eq!(w.narr_ops().await.len(), 2, "the post and one update");
    // After it, the sweep flushes the trailing state as one update showing all of it.
    sqlx::query(AGE).execute(w.h.pool()).await.unwrap();
    assert_eq!(narration::sweep(w.h.pool()).await.unwrap(), 1);
    let ops = w.narr_ops().await;
    assert_eq!(ops.len(), 3);
    assert_eq!(ops[2].2["content"]["narration"]["steps"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn the_card_collapses_when_the_answer_posts_and_the_close_is_never_dropped() {
    let (w, _a, turn) = W::narrating_dm().await;
    w.card_delivered().await;
    sqlx::query(AGE).execute(w.h.pool()).await.unwrap();
    w.tools(turn, &[("Read", "", Some(true)), ("Bash", "", Some(false))]).await;
    // The space is at its write cap: the collapse still goes out.
    sqlx::query(
        "INSERT INTO channel_ops (id, connector_id, integration_id, kind, payload, dedupe_key, status)
         SELECT gen_random_uuid(), 'fake', $1, 'message.update', '{\"space_ref\":\"D1\"}'::jsonb,
                'narr:filler-' || g, 'done' FROM generate_series(1, 12) g",
    )
    .bind(w.integ)
    .execute(w.h.pool())
    .await
    .unwrap();
    w.finish_turn(turn, "done", Some("the answer")).await;
    let ops = w.narr_ops().await;
    let end = ops
        .iter()
        .find(|o| o.0 == format!("narr:{turn}:end"))
        .expect("the collapse is queued");
    assert_eq!(end.1, "message.update");
    let n = &end.2["content"]["narration"];
    assert_eq!(n["collapsed"], true);
    assert_eq!(n["status"], "done");
    assert_eq!(n["step_count"], 2);
    assert_eq!(n["failed_count"], 1);
    assert!(n.get("steps").is_none(), "{n}");
    assert!(w.ops("message.delete").await.is_empty(), "collapsed, not deleted (D-286)");
    // Idempotent: a second finish (a restart, the sweep) queues no second collapse.
    let row = w.turn_row(turn).await;
    let mut c = w.h.pool().acquire().await.unwrap();
    narration::finish(&mut c, &row).await.unwrap();
    let again = w.narr_ops().await;
    assert_eq!(again.iter().filter(|o| o.0.ends_with(":end")).count(), 1);
}

#[tokio::test]
async fn a_failed_turn_collapses_as_failed_and_an_aborted_one_as_stopped() {
    for (status, shown) in [("failed", "failed"), ("aborted", "stopped")] {
        let (w, _a, turn) = W::narrating_dm().await;
        w.card_delivered().await;
        w.finish_turn(turn, status, None).await;
        let ops = w.narr_ops().await;
        let end = ops.iter().find(|o| o.0.ends_with(":end")).unwrap();
        assert_eq!(end.2["content"]["narration"]["status"], shown, "{status}");
    }
}

#[tokio::test]
async fn a_collapse_waiting_for_its_post_is_queued_by_the_sweep() {
    let (w, _a, turn) = W::narrating_dm().await;
    // The turn ends while the card's post is still pending: no message to edit yet.
    w.finish_turn(turn, "done", Some("answer")).await;
    assert!(w.narr_ops().await.iter().all(|o| !o.0.ends_with(":end")));
    w.card_delivered().await;
    assert_eq!(narration::sweep(w.h.pool()).await.unwrap(), 1);
    assert!(w.narr_ops().await.iter().any(|o| o.0.ends_with(":end")));
    assert_eq!(narration::sweep(w.h.pool()).await.unwrap(), 0);
}

#[tokio::test]
async fn a_card_carries_names_only_in_a_shared_space_and_the_summary_in_a_direct_one() {
    // Direct: the sanitized summary is shown, nothing else of the turn.
    let (w, _a, turn) = W::narrating_dm().await;
    w.card_delivered().await;
    sqlx::query(AGE).execute(w.h.pool()).await.unwrap();
    let row = w.turn_row(turn).await;
    let mut tx = w.h.pool().begin().await.unwrap();
    // (`activity` and the deltas are ephemeral: they are never in the transcript the card reads.)
    for (ty, p) in [(
        "text",
        json!({"turn_id": turn, "message_id": "m", "text": "CANARY-ANSWER"}),
    )] {
        conv::append_event(&mut tx, row.conversation_id, Some(turn), ty, &p)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    w.tools(turn, &[("Read", "reading notes.md", Some(true))]).await;
    let all = serde_json::to_string(&w.narr_ops().await).unwrap();
    assert!(all.contains("reading notes.md"), "{all}");
    assert!(!all.contains("CANARY"), "{all}");

    // Shared space: names only.
    let w = world().await;
    let a = w.agent("rex").await;
    w.narrate_on(a).await;
    w.presence(w.integ, a, "shared", None).await;
    w.link(w.integ, "U1", w.member).await;
    w.bind_space(w.integ, "C1", a, "mentions", false).await;
    w.post(w.integ, "e1", chan("U1", "q", None, true)).await;
    let turn = w.turns().await[0];
    w.start_turn(turn).await;
    w.card_delivered().await;
    sqlx::query(AGE).execute(w.h.pool()).await.unwrap();
    w.tools(turn, &[("Read", "CANARY-ARGUMENT /etc/secret", Some(true))]).await;
    let ops = w.narr_ops().await;
    assert_eq!(ops.len(), 2, "{ops:?}");
    let all = serde_json::to_string(&ops).unwrap();
    assert!(all.contains("\"Read\""), "{all}");
    assert!(!all.contains("CANARY"), "{all}");
    assert!(ops[1].2["content"]["narration"]["steps"][0].get("detail").is_none());
}

#[tokio::test]
async fn a_space_at_its_card_cap_gets_no_card_for_a_further_turn_and_its_answer_is_unchanged() {
    let (w, a, _first) = W::narrating_dm().await;
    // Two more open cards of the space (the cap is three at a time).
    sqlx::query(
        "INSERT INTO channel_ops (id, connector_id, integration_id, kind, payload, dedupe_key, status, created_at)
         SELECT gen_random_uuid(), 'fake', $1, 'message.post',
                '{\"target\":{\"space_ref\":\"D1\"}}'::jsonb, 'narr:' || gen_random_uuid(), 'done',
                now() - interval '5 minutes' FROM generate_series(1, 2)",
    )
    .bind(w.integ)
    .execute(w.h.pool())
    .await
    .unwrap();
    // The first turn is over (its card stays open until collapsed); a fourth turn comes in.
    sqlx::query("UPDATE turns SET status='done', finished_at=now() WHERE status='running'")
        .execute(w.h.pool())
        .await
        .unwrap();
    w.post(w.integ, "e2", {
        let mut p = dm("U1", "q2", "BD1");
        p["message_ref"] = json!("m2");
        p
    })
    .await;
    let _ = a;
    let t = w.turns().await;
    assert_eq!(t.len(), 2, "{t:?}");
    let before = w.narr_ops().await.len();
    let next = *t.last().unwrap();
    w.start_turn(next).await;
    assert_eq!(w.narr_ops().await.len(), before, "no card for the further turn");
    w.finish_turn(next, "done", Some("still answered")).await;
    assert!(w.ops("message.post").await.iter().any(|p| p["content"]["text"] == "still answered"));
}

#[tokio::test]
async fn a_space_at_its_write_cap_drops_updates() {
    let (w, _a, turn) = W::narrating_dm().await;
    w.card_delivered().await;
    sqlx::query(AGE).execute(w.h.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO channel_ops (id, connector_id, integration_id, kind, payload, dedupe_key, status)
         SELECT gen_random_uuid(), 'fake', $1, 'message.update', '{\"space_ref\":\"D1\"}'::jsonb,
                'narr:filler-' || g, 'done' FROM generate_series(1, 10) g",
    )
    .bind(w.integ)
    .execute(w.h.pool())
    .await
    .unwrap();
    w.tools(turn, &[("Read", "", Some(true))]).await;
    assert_eq!(
        w.narr_ops().await.iter().filter(|o| o.0.starts_with(&format!("narr:{turn}:"))).count(),
        0,
        "no update inside a full minute"
    );
}

#[tokio::test]
async fn a_rate_limited_update_is_dropped_but_the_post_and_the_close_are_retried() {
    let (w, _a, turn) = W::narrating_dm().await;
    let kinds = |key: String| {
        let pool = w.h.pool().clone();
        async move {
            let id: Uuid = sqlx::query_scalar("SELECT id FROM channel_ops WHERE dedupe_key=$1")
                .bind(key)
                .fetch_one(&pool)
                .await
                .unwrap();
            aurora_db::repo::channel_ops::is_droppable_narration_update(&pool, id)
                .await
                .unwrap()
        }
    };
    assert!(!kinds(format!("narr:{turn}")).await, "the post is retried");
    sqlx::query("INSERT INTO channel_ops (id, connector_id, integration_id, kind, payload, dedupe_key, status) SELECT gen_random_uuid(), 'fake', $1, 'message.update', '{}'::jsonb, k, 'pending' FROM unnest(ARRAY[$2::text, $3::text]) k")
        .bind(w.integ)
        .bind(format!("narr:{turn}:77"))
        .bind(format!("narr:{turn}:end"))
        .execute(w.h.pool())
        .await
        .unwrap();
    assert!(kinds(format!("narr:{turn}:77")).await, "an update is dropped");
    assert!(!kinds(format!("narr:{turn}:end")).await, "the close is retried");
}
