#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-146 Part B (D-232 (7), C-732 (3), SEC-88): a turn's trace context crosses the queue.
//! `turns.trace_context` is format-checked; the runner work items of the turn carry it (read from
//! the database); it is copied onto the turn's context credential at mint (the gateway role has
//! no grant on `turns`); a sandbox or supervisor report can never write it.

use crate::hosted::rig;
use serde_json::json;
use uuid::Uuid;

const TP: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

#[tokio::test]
async fn turn_context_reaches_work_items_and_the_context_credential() {
    let r = rig().await;
    let lv = r.boot("tracer").await;
    let conv = lv.conv;

    // The format CHECK is enforced on both columns.
    for table in ["turns", "gateway_credentials"] {
        for bad in [
            "garbage",
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01 ",
        ] {
            let res = sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {table} SET trace_context = $1 WHERE id = (SELECT id FROM {table} LIMIT 1)"
            )))
            .bind(bad)
            .execute(r.h.pool())
            .await;
            assert!(res.is_err(), "{table} accepted {bad:?}");
        }
    }

    // The boot turn is queued; the API stored what its span carried (nothing in this harness).
    let turn: Uuid = sqlx::query_scalar(
        "SELECT id FROM turns WHERE conversation_id = $1 AND role = 'turn' AND status = 'queued'",
    )
    .bind(conv)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE turns SET trace_context = $2 WHERE id = $1")
        .bind(turn)
        .bind(TP)
        .execute(r.h.pool())
        .await
        .unwrap();

    // The delivery work item carries the turn's context at the item level.
    let d = r.deliver().await;
    assert_eq!(d["turn_id"], json!(turn.to_string()));
    assert_eq!(d["trace_context"], json!(TP), "{d}");

    // The context credential minted for the turn carries a copy.
    let copied: Option<String> = sqlx::query_scalar(
        "SELECT trace_context FROM gateway_credentials
          WHERE turn_id = $1 AND kind = 'context' AND revoked_at IS NULL",
    )
    .bind(turn)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(copied.as_deref(), Some(TP));

    // Later work items of the same turn carry it too (abort signal).
    sqlx::query("UPDATE turns SET abort_requested_at = now() WHERE id = $1")
        .bind(turn)
        .execute(r.h.pool())
        .await
        .unwrap();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    let items = r.work().await;
    if let Some(a) = items.iter().find(|i| i["kind"] == "turn.abort") {
        assert_eq!(a["trace_context"], json!(TP), "{a}");
    }
}

#[tokio::test]
async fn items_of_a_turn_without_context_have_none_and_reports_cannot_set_it() {
    let r = rig().await;
    let lv = r.boot("untraced").await;
    let d = r.deliver().await;
    assert!(d.get("trace_context").is_none(), "{d}");
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    // A report that smuggles a context field is ignored: the column stays NULL.
    r.report(
        &lv,
        json!({"kind": "turn.started", "turn_id": turn, "trace_context": TP}),
    )
    .await;
    let stored: Option<String> =
        sqlx::query_scalar("SELECT trace_context FROM turns WHERE id = $1::uuid")
            .bind(&turn)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(stored, None);
}
