#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-CHAT-1 (D-228, C-725, C-726): quote-reply and files-only messages.

use crate::common::*;
use crate::hosted::{Live, Rig, rig};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

fn ev(n: u64, kind: &str, data: Value) -> Value {
    let mut e = data;
    e["kind"] = json!(kind);
    json!({"n": n, "event": e})
}

async fn post(r: &Rig, conv: Uuid, body: Value, key: Option<&str>) -> (u16, Value) {
    let mut hs: Vec<(&str, &str)> = vec![("cookie", &r.cookie), ("origin", ORIGIN)];
    if let Some(k) = key {
        hs.push(("idempotency-key", k));
    }
    let (s, _, v) =
        r.h.send(
            Method::POST,
            &format!("/v1/conversations/{conv}/messages"),
            &hs,
            Some(body),
        )
        .await;
    (s.as_u16(), v)
}

async fn seq_of(r: &Rig, conv: Uuid, ty: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT seq FROM transcript_events WHERE conversation_id = $1 AND type = $2 ORDER BY seq LIMIT 1",
    )
    .bind(conv)
    .bind(ty)
    .fetch_one(r.h.pool())
    .await
    .unwrap()
}

async fn queued_payload(r: &Rig, turn: &str) -> Value {
    sqlx::query_scalar(
        "SELECT payload FROM transcript_events WHERE turn_id = $1::uuid AND type = 'turn.queued'",
    )
    .bind(turn)
    .fetch_one(r.h.pool())
    .await
    .unwrap()
}

async fn finish(r: &Rig, lv: &Live, turn: &str) {
    r.report(
        lv,
        json!({"kind": "turn.result", "turn_id": turn, "status": "done"}),
    )
    .await;
}

fn err_reason(v: &Value) -> (&str, &str) {
    (
        v["error"]["code"].as_str().unwrap_or_default(),
        v["error"]["details"]["reason"].as_str().unwrap_or_default(),
    )
}

#[tokio::test]
async fn reply_targets_payload_steer_delivery_and_redaction_cascade() {
    let r = rig().await;
    let lv = r.boot("quoter").await;
    let conv = lv.conv;
    // The boot turn ("hello", seq 1) is still queued: a reply now is a follow-up row.
    let (s, v) = post(
        &r,
        conv,
        json!({"text": "re hello", "reply_to": {"seq": 1}}),
        Some("k-1"),
    )
    .await;
    assert_eq!(s, 202, "{v}");
    let fid = v["followup"]["id"].as_str().unwrap().to_owned();
    // The same key and body replays; a different quote under the key is a reuse.
    let (s, v2) = post(
        &r,
        conv,
        json!({"text": "re hello", "reply_to": {"seq": 1}}),
        Some("k-1"),
    )
    .await;
    assert_eq!(
        (s, v2["followup"]["id"].as_str()),
        (202, Some(fid.as_str()))
    );
    let (s, v3) = post(&r, conv, json!({"text": "re hello"}), Some("k-1")).await;
    assert_eq!(s, 422, "{v3}");
    assert_eq!(v3["error"]["code"], "idempotency_key_reused");

    let q = queued_payload(&r, &fid).await;
    assert_eq!(q["reply_to"]["seq"], 1);
    assert_eq!(q["reply_to"]["author_kind"], "person");
    assert_eq!(q["reply_to"]["excerpt"], "hello");
    assert!(q["reply_to"]["at"].as_str().unwrap().contains('T'));
    assert!(!q["reply_to"]["author_label"].as_str().unwrap().is_empty());

    // Delivery: the turn itself is unquoted; the steered follow-up carries the quote, its stored
    // text does not.
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(delivered_text(&d), "hello");
    r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    let items = r.work().await;
    let msg = items.iter().find(|i| i["kind"] == "turn.message").unwrap();
    let steered = msg["text"].as_str().unwrap();
    assert!(
        steered.starts_with("> In reply to ") && steered.ends_with("\n> hello\n\nre hello"),
        "{steered}"
    );
    let stored: String = sqlx::query_scalar("SELECT text FROM turns WHERE id = $1::uuid")
        .bind(&fid)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(stored, "re hello");

    let batch = json!({"kind": "turn.events", "turn_id": turn, "events": [
        ev(1, "tool.started", json!({"call_id": "c1", "name": "memory_list", "summary": "list"})),
        ev(2, "tool.finished", json!({"call_id": "c1", "ok": true})),
        ev(3, "text.delta", json!({"message_id": "m1", "delta": "Hi there"})),
        ev(4, "message.joined", json!({"followup_id": fid})),
    ]});
    assert_eq!(r.report(&lv, batch).await, "applied");
    finish(&r, &lv, &turn).await;

    // Targets: a tool event is not replyable; unknown and other conversations' seqs are not found.
    let tool = seq_of(&r, conv, "tool.started").await;
    let answer = seq_of(&r, conv, "text").await;
    let (s, v) = post(
        &r,
        conv,
        json!({"text": "x", "reply_to": {"seq": tool}}),
        None,
    )
    .await;
    assert_eq!(
        (s, err_reason(&v)),
        (422, ("reply_target_invalid", "not_replyable"))
    );
    let (s, v) = post(
        &r,
        conv,
        json!({"text": "x", "reply_to": {"seq": 9999}}),
        None,
    )
    .await;
    assert_eq!(
        (s, err_reason(&v)),
        (422, ("reply_target_invalid", "not_found"))
    );
    let other = r.agent("other", true).await;
    let oc = r.direct(other).await;
    r.say(oc, "secret").await;
    sqlx::query(
        "INSERT INTO transcript_events (conversation_id, seq, type, payload) VALUES ($1, 500, 'text', '{\"text\":\"secret\"}')",
    )
    .bind(oc)
    .execute(r.h.pool())
    .await
    .unwrap();
    let (s, v) = post(
        &r,
        conv,
        json!({"text": "x", "reply_to": {"seq": 500}}),
        None,
    )
    .await;
    assert_eq!(
        (s, err_reason(&v)),
        (422, ("reply_target_invalid", "not_found"))
    );
    assert!(!v.to_string().contains("secret"));

    // A reply to the agent's answer (a new turn), redacted before delivery.
    let (s, v) = post(
        &r,
        conv,
        json!({"text": "thanks", "reply_to": {"seq": answer}}),
        None,
    )
    .await;
    assert_eq!(s, 202, "{v}");
    let reply_turn = v["turn"]["id"].as_str().unwrap().to_owned();
    let q = queued_payload(&r, &reply_turn).await;
    assert_eq!(q["reply_to"]["author_kind"], "agent");
    assert_eq!(q["reply_to"]["author_label"], "quoter");
    assert_eq!(q["reply_to"]["excerpt"], "Hi there");

    let (s, v) = r
        .user_call(
            Method::POST,
            &format!("/v1/conversations/{conv}/events/{answer}/redact"),
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let q = queued_payload(&r, &reply_turn).await;
    assert_eq!(q["reply_to"], json!({"seq": answer, "redacted": true}));
    let (s, v) = post(
        &r,
        conv,
        json!({"text": "again", "reply_to": {"seq": answer}}),
        None,
    )
    .await;
    assert_eq!(
        (s, err_reason(&v)),
        (422, ("reply_target_invalid", "redacted"))
    );

    let d2 = r.deliver().await;
    assert_eq!(
        delivered_text(&d2),
        "> In reply to a message that was redacted.\n\nthanks"
    );
}

#[tokio::test]
async fn promoted_followup_and_files_only_messages() {
    let r = rig().await;
    let lv = r.boot("filer").await;
    let conv = lv.conv;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    // A follow-up that is not steered (no `work` poll) is promoted when the turn ends.
    let (s, v) = post(
        &r,
        conv,
        json!({"text": "second", "reply_to": {"seq": 1}}),
        None,
    )
    .await;
    assert_eq!(s, 202, "{v}");
    finish(&r, &lv, &turn).await;
    let d2 = r.deliver().await;
    let t2 = d2["turn_id"].as_str().unwrap().to_owned();
    let text = delivered_text(&d2);
    assert!(
        text.starts_with("> In reply to ") && text.ends_with("\n> hello\n\nsecond"),
        "{text}"
    );
    let stored: String = sqlx::query_scalar("SELECT text FROM turns WHERE id = $1::uuid")
        .bind(&t2)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(stored, "second");
    assert_eq!(queued_payload(&r, &t2).await["reply_to"]["seq"], 1);
    r.report(&lv, json!({"kind": "turn.started", "turn_id": t2}))
        .await;
    finish(&r, &lv, &t2).await;

    // Nothing at all is still a 400; files only is accepted.
    let (s, v) = post(&r, conv, json!({"text": ""}), None).await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(v["error"]["details"]["fields"][0]["path"], "/text");
    let (s, _) = post(&r, conv, json!({}), None).await;
    assert_eq!(s, 400);
    let mut ids = Vec::new();
    for name in ["a.txt", "b.txt"] {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO attachments (id, workspace_id, conversation_id, direction, status, filename, content_type, size_bytes, sha256, blob_backend, blob_key)
             VALUES ($1,$2,$3,'in','pending',$4,'text/plain',1,'00','postgres',$5)",
        )
        .bind(id)
        .bind(r.ws)
        .bind(conv)
        .bind(name)
        .bind(format!("k-{id}"))
        .execute(r.h.pool())
        .await
        .unwrap();
        ids.push(id);
    }
    let (s, v) = post(&r, conv, json!({"attachment_ids": ids}), None).await;
    assert_eq!(s, 202, "{v}");
    let t3 = v["turn"]["id"].as_str().unwrap().to_owned();
    let stored: String = sqlx::query_scalar("SELECT text FROM turns WHERE id = $1::uuid")
        .bind(&t3)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(stored, "");
    let d3 = r.deliver().await;
    assert_eq!(delivered_text(&d3), "(sent 2 files)");
    assert_eq!(d3["attachments"].as_array().unwrap().len(), 2);
}
