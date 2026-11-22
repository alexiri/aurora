#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-225 (D-469, D-470, C-1152..C-1156, SEC-152): a follow-up waits for a person; Steer, Edit and
//! Cancel through the API with the authority and race rules of SEC-152. A fake runner (`hosted.rs`'s
//! rig) drives a real hosted turn on the `fake` harness, which steers.

use crate::common::*;
use crate::hosted::{Live, Rig, rig};
use aurora_api::domains::conversations::service;
use aurora_db::repo::conversations as convs;
use aurora_db::repo::turns;
use http::Method;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

struct Who {
    id: Uuid,
    cookie: String,
}

async fn person(r: &Rig, email: &str, ws_role: Option<&str>) -> Who {
    let id = r.h.user(email, false).await;
    r.h.member(r.org, id, "member", "active").await;
    if let Some(role) = ws_role {
        r.h.bind_ws(r.ws, id, role).await;
    }
    let (_, tok) = r.h.good_session(id).await;
    Who {
        id,
        cookie: cookie(&tok),
    }
}

struct W {
    r: Rig,
    lv: Live,
    /// The follow-ups' author: the rig's own member.
    a: Who,
    /// Another member of the workspace.
    b: Who,
    /// An owner of the workspace.
    owner: Who,
    /// In the organization, but not in the workspace.
    stranger: Who,
    /// The conversation whose turn is running.
    conv: Uuid,
    turn: Uuid,
}

fn ev(n: u64, kind: &str, data: Value) -> Value {
    let mut e = data;
    e["kind"] = json!(kind);
    json!({"n": n, "event": e})
}

/// A hosted agent on the `fake` harness (it steers), a running turn, and the people around it.
/// `shared`: the turn runs in an `api` conversation every member sees; otherwise in `a`'s direct one.
async fn world(shared: bool) -> W {
    let r = rig().await;
    let lv = r.boot("fup").await;
    let a_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'm@x.io'")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let a = Who {
        id: a_id,
        cookie: r.cookie.clone(),
    };
    let b = person(&r, "b@x.io", Some("member")).await;
    let owner = person(&r, "owner@x.io", Some("owner")).await;
    let stranger = person(&r, "stranger@x.io", None).await;
    let mut conv = lv.conv;
    let d = r.deliver().await;
    let mut turn: Uuid = d["turn_id"].as_str().unwrap().parse().unwrap();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    if shared {
        // End the direct conversation's first turn and run the next one in a shared conversation.
        r.report(
            &lv,
            json!({"kind": "turn.result", "turn_id": turn, "status": "done"}),
        )
        .await;
        let (_, key) = r.h.api_key(a.id, r.org, Some(r.ws), false).await;
        let (s, _, v) =
            r.h.send(
                Method::POST,
                &format!("/v1/agents/{}/conversations", lv.agent),
                &[("authorization", &bearer(&key)), ("origin", ORIGIN)],
                Some(json!({"external_ref": "fup-shared"})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        conv = v["id"].as_str().unwrap().parse().unwrap();
        let (s, v) =
            r.h.call(
                &a.cookie,
                Method::POST,
                &format!("/v1/conversations/{conv}/messages"),
                Some(json!({"text": "go"})),
            )
            .await;
        assert_eq!(s, 202, "{v}");
        let d = r.deliver().await;
        turn = d["turn_id"].as_str().unwrap().parse().unwrap();
        r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
            .await;
    }
    W {
        r,
        lv,
        a,
        b,
        owner,
        stranger,
        conv,
        turn,
    }
}

impl W {
    async fn call(&self, who: &Who, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        self.r.h.call(&who.cookie, m, uri, body).await
    }

    /// `who` sends a message while the turn runs: a follow-up. Returns its id.
    async fn followup(&self, who: &Who, text: &str) -> Uuid {
        let (s, v) = self
            .call(
                who,
                Method::POST,
                &format!("/v1/conversations/{}/messages", self.conv),
                Some(json!({"text": text})),
            )
            .await;
        assert_eq!(s, 202, "{v}");
        assert_eq!(v["followup"]["status"], "joining", "{v}");
        v["followup"]["id"].as_str().unwrap().parse().unwrap()
    }

    async fn steer(&self, who: &Who, id: Uuid) -> (u16, Value) {
        self.call(
            who,
            Method::POST,
            &format!("/v1/followups/{id}/steer"),
            None,
        )
        .await
    }

    async fn cancel(&self, who: &Who, id: Uuid) -> (u16, Value) {
        self.call(
            who,
            Method::POST,
            &format!("/v1/followups/{id}/cancel"),
            None,
        )
        .await
    }

    async fn edit(&self, who: &Who, id: Uuid, text: &str) -> (u16, Value) {
        self.call(
            who,
            Method::PATCH,
            &format!("/v1/followups/{id}"),
            Some(json!({"text": text})),
        )
        .await
    }

    async fn list(&self, who: &Who) -> (u16, Value) {
        self.call(
            who,
            Method::GET,
            &format!("/v1/conversations/{}/followups", self.conv),
            None,
        )
        .await
    }

    /// What the runner is handed: the `turn.message` items of one poll.
    async fn steered(&self) -> Vec<Value> {
        self.r
            .work()
            .await
            .into_iter()
            .filter(|i| i["kind"] == "turn.message")
            .collect()
    }

    async fn row(&self, id: Uuid) -> (String, String, bool, bool, Option<String>) {
        sqlx::query_as(
            "SELECT status, text, steer_requested_at IS NOT NULL, steer_delivered_at IS NOT NULL, steer_error
             FROM turns WHERE id = $1",
        )
        .bind(id)
        .fetch_one(self.r.h.pool())
        .await
        .unwrap()
    }

    async fn count_events(&self, ty: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM transcript_events WHERE conversation_id = $1 AND type = $2",
        )
        .bind(self.conv)
        .bind(ty)
        .fetch_one(self.r.h.pool())
        .await
        .unwrap()
    }

    async fn finish(&self) -> String {
        self.r
            .report(
                &self.lv,
                json!({"kind": "turn.result", "turn_id": self.turn, "status": "done"}),
            )
            .await
    }

    async fn promoted_turns(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM turns WHERE conversation_id = $1 AND role = 'turn' AND status IN ('queued','dispatched')",
        )
        .bind(self.conv)
        .fetch_one(self.r.h.pool())
        .await
        .unwrap()
    }
}

fn flags(c: &Value) -> (bool, bool, bool, bool) {
    (
        c["steer"].as_bool().unwrap(),
        c["send_now"].as_bool().unwrap(),
        c["edit"].as_bool().unwrap(),
        c["cancel"].as_bool().unwrap(),
    )
}

// ---- C-1153: a follow-up waits -------------------------------------------------------------

#[tokio::test]
async fn a_follow_up_waits_until_steer_is_clicked_then_joins_and_is_handed_over_once() {
    let w = world(false).await;
    let f = w.followup(&w.a, "also this").await;
    // Nobody asked: it is never handed to the harness, however often the engine looks (arm 7).
    for _ in 0..3 {
        assert!(
            w.steered().await.is_empty(),
            "a waiting follow-up is not steered"
        );
    }
    assert_eq!(
        w.row(f).await,
        ("joining".into(), "also this".into(), false, false, None)
    );

    let (s, v) = w.steer(&w.a, f).await;
    assert_eq!(s, 202, "{v}");
    assert_eq!(v["steer_requested"], true);
    // Asked, not yet handed over: still waiting, so still editable and cancellable.
    assert_eq!(flags(&v["controls"]), (false, false, true, true));
    let items = w.steered().await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["followup_id"], json!(f));
    assert_eq!(items[0]["turn_id"], json!(w.turn));
    assert_eq!(items[0]["text"], "also this");
    assert!(w.steered().await.is_empty(), "handed over once");
    assert_eq!(
        w.row(f).await,
        ("joining".into(), "also this".into(), true, true, None)
    );
    // Durable: asked once.
    assert_eq!(w.count_events("followup.steer_requested").await, 1);

    // Handed over: nothing may change it (409), neither author nor anyone else.
    for who in [&w.a, &w.owner] {
        for (s, v) in [
            w.steer(who, f).await,
            w.cancel(who, f).await,
            w.edit(who, f, "too late").await,
        ] {
            if who.id == w.owner.id && v["error"]["code"] == "followup_forbidden" {
                continue; // the owner is not the author: the edit is refused before the state
            }
            assert_eq!(
                (s, v["error"]["code"].as_str()),
                (409, Some("followup_not_waiting")),
                "{v}"
            );
        }
    }
    assert_eq!(
        w.row(f).await.1,
        "also this",
        "what the agent got is what is stored"
    );

    // The harness takes it: joined.
    let r = &w.r;
    r.report(
        &w.lv,
        json!({"kind": "turn.events", "turn_id": w.turn, "events": [
            ev(1, "message.joined", json!({"followup_id": f}))]}),
    )
    .await;
    assert_eq!(w.row(f).await.0, "joined");
    let (_, v) = w.list(&w.a).await;
    assert!(v["items"].as_array().unwrap().is_empty(), "{v}");
}

#[tokio::test]
async fn a_repeated_steer_is_a_no_op_and_a_double_click_asks_once() {
    let w = world(false).await;
    let f = w.followup(&w.a, "once").await;
    let ((s1, v1), (s2, v2)) = tokio::join!(w.steer(&w.a, f), w.steer(&w.a, f));
    assert_eq!((s1, s2), (202, 202), "{v1} {v2}");
    assert_eq!(w.count_events("followup.steer_requested").await, 1);
    let (s3, _) = w.steer(&w.a, f).await;
    assert_eq!(s3, 202);
    assert_eq!(w.count_events("followup.steer_requested").await, 1);
    assert_eq!(w.steered().await.len(), 1, "one delivery for one follow-up");
    assert!(w.steered().await.is_empty());
}

#[tokio::test]
async fn approval_resume_follow_ups_still_steer_by_themselves() {
    let w = world(false).await;
    // A platform-made follow-up (door `system`, C-824): no person asked, the engine hands it over.
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, parent_turn_id, door, speaker_kind, speaker_label, text)
         VALUES ($1,$2,$3,'followup','joining',$4,'{\"kind\":\"system\",\"ref\":\"approval:x\"}','system','','The request was approved.')",
    )
    .bind(id)
    .bind(w.conv)
    .bind(w.lv.agent)
    .bind(w.turn)
    .execute(w.r.h.pool())
    .await
    .unwrap();
    let items = w.steered().await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["followup_id"], json!(id));
    // …while a person's follow-up beside it waits.
    let f = w.followup(&w.a, "waits").await;
    assert!(w.steered().await.is_empty());
    assert!(!w.row(f).await.2);
}

// ---- C-1154: list and per-caller controls ----------------------------------------------------

#[tokio::test]
async fn the_list_shows_waiting_follow_ups_oldest_first_with_controls_for_the_caller() {
    let w = world(true).await;
    let f1 = w.followup(&w.a, "first").await;
    let f2 = w.followup(&w.b, "second").await;
    let (s, v) = w.list(&w.a).await;
    assert_eq!(s, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], json!(f1));
    assert_eq!(items[1]["id"], json!(f2));
    assert_eq!(items[0]["speaker_label"], "U");
    // The author may steer, edit and cancel; for someone else's message: steer and cancel, not edit.
    assert_eq!(flags(&items[0]["controls"]), (true, false, true, true));
    assert_eq!(flags(&items[1]["controls"]), (true, false, false, true));
    let (_, v) = w.list(&w.b).await;
    let items = v["items"].as_array().unwrap();
    assert_eq!(flags(&items[0]["controls"]), (true, false, false, true));
    assert_eq!(flags(&items[1]["controls"]), (true, false, true, true));
    // A person who cannot see the conversation gets 404, not an empty list.
    let (s, v) = w.list(&w.stranger).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (404, Some("not_found")),
        "{v}"
    );
    // Cancelled and finished-parent follow-ups are not listed.
    w.cancel(&w.a, f1).await;
    let (_, v) = w.list(&w.a).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn the_post_answer_carries_the_controls_for_the_sender() {
    let w = world(false).await;
    let (s, v) = w
        .call(
            &w.a,
            Method::POST,
            &format!("/v1/conversations/{}/messages", w.conv),
            Some(json!({"text": "hi"})),
        )
        .await;
    assert_eq!(s, 202, "{v}");
    assert_eq!(flags(&v["followup"]["controls"]), (true, false, true, true));
    assert_eq!(v["followup"]["steer_requested"], false);
    assert!(v["followup"]["steer_error"].is_null());
}

// ---- C-1155 / SEC-152: who may do what -------------------------------------------------------

#[tokio::test]
async fn only_the_author_edits_and_anyone_who_may_stop_cancels() {
    let w = world(true).await;
    let f = w.followup(&w.a, "mine").await;
    // Another member sees it but may not edit it: 403 followup_forbidden, and nothing changes.
    let (s, v) = w.edit(&w.b, f, "hijacked").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("followup_forbidden")),
        "{v}"
    );
    assert_eq!(w.row(f).await.1, "mine");
    assert_eq!(w.count_events("followup.edited").await, 0);
    // The owner is not the author either.
    let (s, v) = w.edit(&w.owner, f, "also no").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("followup_forbidden")),
        "{v}"
    );
    // Someone who cannot see the conversation learns nothing: 404 on every action (visibility first).
    for (s, v) in [
        w.edit(&w.stranger, f, "x").await,
        w.steer(&w.stranger, f).await,
        w.cancel(&w.stranger, f).await,
    ] {
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (404, Some("not_found")),
            "{v}"
        );
    }
    assert_eq!(w.row(f).await.0, "joining");
    // The author edits…
    let (s, v) = w.edit(&w.a, f, "mine, corrected").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["text"], "mine, corrected");
    assert!(!v["edited_at"].is_null());
    // …through their API key too (same person).
    let (_, key) = w.r.h.api_key(w.a.id, w.r.org, Some(w.r.ws), false).await;
    let (s, _, v) =
        w.r.h
            .send(
                Method::PATCH,
                &format!("/v1/followups/{f}"),
                &[("authorization", &bearer(&key)), ("origin", ORIGIN)],
                Some(json!({"text": "mine, via key"})),
            )
            .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    // Another member with Stop authority cancels it.
    let (s, v) = w.cancel(&w.b, f).await;
    assert_eq!(s, 202, "{v}");
    assert_eq!(v["status"], "cancelled");
    assert_eq!(flags(&v["controls"]), (false, false, false, false));
    let cancelled = sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM transcript_events WHERE conversation_id = $1 AND type = 'followup.cancelled'",
    )
    .bind(w.conv)
    .fetch_one(w.r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        cancelled,
        json!({"followup_id": f, "by": {"id": w.b.id, "display_name": "U"}})
    );
}

#[tokio::test]
async fn in_a_direct_conversation_an_owner_may_cancel_and_steer_but_a_member_gets_404() {
    let w = world(false).await;
    let f = w.followup(&w.a, "private").await;
    // A member who is not the direct person cannot even see the conversation.
    for (s, v) in [
        w.cancel(&w.b, f).await,
        w.steer(&w.b, f).await,
        w.edit(&w.b, f, "x").await,
    ] {
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (404, Some("not_found")),
            "{v}"
        );
    }
    let (s, _) = w.list(&w.b).await;
    assert_eq!(s, 404);
    // An owner can see a direct conversation: Stop authority. They cancel, not edit.
    let (s, v) = w.edit(&w.owner, f, "x").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("followup_forbidden")),
        "{v}"
    );
    let (s, v) = w.cancel(&w.owner, f).await;
    assert_eq!(s, 202, "{v}");
    assert_eq!(w.row(f).await.0, "cancelled");
    let g = w.followup(&w.a, "another").await;
    let (s, v) = w.steer(&w.owner, g).await;
    assert_eq!(s, 202, "{v}");
}

#[tokio::test]
async fn an_agent_token_cannot_reach_a_follow_up_route() {
    let w = world(false).await;
    let f = w.followup(&w.a, "x").await;
    // A BYO agent's own token is not a person: 401 on every follow-up route.
    let byo = w.r.h.byo_agent(w.r.org, w.r.ws, "byo-token", "Byo").await;
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &format!("/v1/agents/{byo}/token"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let tok = bearer(v["token"].as_str().unwrap());
    for (m, uri, body) in [
        (
            Method::GET,
            format!("/v1/conversations/{}/followups", w.conv),
            None,
        ),
        (Method::POST, format!("/v1/followups/{f}/steer"), None),
        (Method::POST, format!("/v1/followups/{f}/cancel"), None),
        (
            Method::PATCH,
            format!("/v1/followups/{f}"),
            Some(json!({"text": "y"})),
        ),
    ] {
        let (s, _, v) =
            w.r.h
                .send(
                    m,
                    &uri,
                    &[("authorization", &tok), ("origin", ORIGIN)],
                    body,
                )
                .await;
        assert_eq!(s.as_u16(), 401, "{uri} {v}");
    }
    assert_eq!(w.row(f).await.1, "x");
}

#[tokio::test]
async fn edited_text_has_the_limits_of_a_new_message_and_an_edit_after_promotion_is_refused() {
    let w = world(false).await;
    let f = w.followup(&w.a, "short").await;
    let (s, v) = w.edit(&w.a, f, "   ").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_request")),
        "{v}"
    );
    let (s, v) = w
        .edit(&w.a, f, &"x".repeat(aurora_core::limits::TURN_TEXT_MAX + 1))
        .await;
    assert_eq!(s, 413, "{v}");
    let ok = "y".repeat(aurora_core::limits::TURN_TEXT_MAX);
    let (s, _) = w.edit(&w.a, f, &ok).await;
    assert_eq!(s, 200);
    assert_eq!(w.row(f).await.1, ok);
    // Unknown fields are refused, as for a new message.
    let (s, _) = w
        .call(
            &w.a,
            Method::PATCH,
            &format!("/v1/followups/{f}"),
            Some(json!({"text": "z", "status": "joined"})),
        )
        .await;
    assert!(matches!(s, 400 | 422), "{s}");
    // The turn ends: the follow-up is promoted; now an edit is refused and the new turn keeps the text.
    w.finish().await;
    assert_eq!(w.row(f).await.0, "promoted");
    let (s, v) = w.edit(&w.a, f, "late").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("followup_not_waiting")),
        "{v}"
    );
    let (s, v) = w.cancel(&w.a, f).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("followup_not_waiting")),
        "{v}"
    );
    let d = w.r.deliver().await;
    assert_eq!(delivered_text(&d), ok);
}

#[tokio::test]
async fn an_edit_is_what_gets_steered_and_what_gets_promoted() {
    let w = world(false).await;
    let f = w.followup(&w.a, "draft").await;
    let (s, _) = w.edit(&w.a, f, "final words").await;
    assert_eq!(s, 200);
    assert_eq!(w.count_events("followup.edited").await, 1);
    w.steer(&w.a, f).await;
    let items = w.steered().await;
    assert_eq!(items[0]["text"], "final words");
    // The harness takes it into the run: joined, so it is not run again at the turn's end.
    w.r.report(
        &w.lv,
        json!({"kind": "turn.events", "turn_id": w.turn, "events": [
            ev(1, "message.joined", json!({"followup_id": f}))]}),
    )
    .await;

    let g = w.followup(&w.a, "second draft").await;
    w.edit(&w.a, g, "second final").await;
    w.finish().await;
    let d = w.r.deliver().await;
    assert_eq!(delivered_text(&d), "second final");
}

#[tokio::test]
async fn a_cancelled_follow_up_is_never_promoted_or_steered() {
    let w = world(false).await;
    let f = w.followup(&w.a, "never mind").await;
    let g = w.followup(&w.a, "keep").await;
    let (s, _) = w.cancel(&w.a, f).await;
    assert_eq!(s, 202);
    let (s, v) = w.cancel(&w.a, f).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("followup_not_waiting")),
        "{v}"
    );
    let (s, v) = w.steer(&w.a, f).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("followup_not_waiting")),
        "{v}"
    );
    assert!(w.steered().await.is_empty());
    w.finish().await;
    assert_eq!(w.row(f).await.0, "cancelled");
    assert_eq!(w.row(g).await.0, "promoted");
    assert_eq!(
        w.promoted_turns().await,
        1,
        "only the one that was not cancelled runs"
    );
}

// ---- C-1153: a missed steer is shown and runs next -------------------------------------------

#[tokio::test]
async fn a_steer_the_harness_missed_is_recorded_shown_and_the_follow_up_runs_as_the_next_turn() {
    for reason in ["missed", "refused"] {
        let w = world(false).await;
        let f = w.followup(&w.a, "try").await;
        w.steer(&w.a, f).await;
        assert_eq!(w.steered().await.len(), 1);
        assert_eq!(
            w.r.report(
                &w.lv,
                json!({"kind": "steer.result", "followup_id": f, "outcome": reason})
            )
            .await,
            "applied"
        );
        let (st, _, requested, delivered, err) = w.row(f).await;
        assert_eq!(
            (st.as_str(), requested, delivered, err.as_deref()),
            ("joining", true, true, Some(reason))
        );
        let evs = w.r.events(w.conv).await;
        let failed: Vec<_> = evs
            .iter()
            .filter(|e| e.0 == "followup.steer_failed")
            .collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].1, json!({"followup_id": f, "reason": reason}));
        // Not re-sent; still on the list, with Edit and Cancel but no Steer.
        assert!(w.steered().await.is_empty());
        let (_, v) = w.list(&w.a).await;
        let it = &v["items"][0];
        assert_eq!(it["steer_error"], reason);
        assert_eq!(flags(&it["controls"]), (false, false, true, true));
        // Editable again (the harness never had the text), then it runs next.
        let (s, _) = w.edit(&w.a, f, "try again").await;
        assert_eq!(s, 200);
        w.finish().await;
        assert_eq!(w.row(f).await.0, "promoted");
        assert_eq!(delivered_text(&w.r.deliver().await), "try again");
    }
}

#[tokio::test]
async fn a_delivered_report_changes_nothing_and_a_late_miss_after_joining_is_ignored() {
    let w = world(false).await;
    let f = w.followup(&w.a, "x").await;
    w.steer(&w.a, f).await;
    w.steered().await;
    w.r.report(
        &w.lv,
        json!({"kind": "turn.events", "turn_id": w.turn, "events": [ev(1, "message.joined", json!({"followup_id": f}))]}),
    )
    .await;
    for outcome in ["delivered", "missed"] {
        w.r.report(
            &w.lv,
            json!({"kind": "steer.result", "followup_id": f, "outcome": outcome}),
        )
        .await;
    }
    assert_eq!(w.row(f).await.0, "joined");
    assert_eq!(w.count_events("followup.steer_failed").await, 0);
}

// ---- frozen agents ---------------------------------------------------------------------------

#[tokio::test]
async fn a_frozen_agent_takes_no_steer_but_a_message_can_still_be_withdrawn() {
    let w = world(false).await;
    let f = w.followup(&w.a, "x").await;
    let (s, v) = w
        .call(
            &w.owner,
            Method::POST,
            &format!("/v1/agents/{}/freeze", w.lv.agent),
            Some(json!({"reason": "test"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w.steer(&w.a, f).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (423, Some("frozen")),
        "{v}"
    );
    // Nothing was asked: the harness is never handed it. (Freezing also ends the running turn, so
    // the message is promoted and waits for the unfreeze; the per-control rules for a frozen agent
    // with a live turn are the unit tests of `followups::controls`.)
    assert!(w.steered().await.is_empty());
    assert!(!w.row(f).await.2);
}

// ---- C-1156: send now ------------------------------------------------------------------------

#[tokio::test]
async fn send_now_is_refused_up_front_when_the_harness_steers_or_cannot_background() {
    let w = world(false).await;
    // `fake` steers: Send now never applies (Steer does).
    let (s, v) = w
        .call(
            &w.a,
            Method::POST,
            &format!("/v1/turns/{}/send-now", w.turn),
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("send_now_unsupported")),
        "{v}"
    );
    let sent: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT send_now_requested_at FROM turns WHERE id = $1")
            .bind(w.turn)
            .fetch_one(w.r.h.pool())
            .await
            .unwrap();
    assert!(
        sent.is_none(),
        "nothing was queued for a harness that cannot do it"
    );
}

#[tokio::test]
async fn a_send_now_the_supervisor_could_not_do_is_a_durable_event_and_moved_is_not() {
    let w = world(false).await;
    for outcome in ["nothing_running", "unsupported"] {
        assert_eq!(
            w.r.report(
                &w.lv,
                json!({"kind": "send_now.result", "turn_id": w.turn, "outcome": outcome})
            )
            .await,
            "applied"
        );
    }
    let evs = w.r.events(w.conv).await;
    let failed: Vec<&Value> = evs
        .iter()
        .filter(|e| e.0 == "turn.send_now_failed")
        .map(|e| &e.1)
        .collect();
    assert_eq!(
        failed,
        [
            &json!({"turn_id": w.turn, "outcome": "nothing_running"}),
            &json!({"turn_id": w.turn, "outcome": "unsupported"})
        ]
    );
    w.r.report(
        &w.lv,
        json!({"kind": "send_now.result", "turn_id": w.turn, "outcome": "moved"}),
    )
    .await;
    assert_eq!(w.count_events("turn.send_now_failed").await, 2);
}

// ---- Part B §4: races, two connections -------------------------------------------------------
// Every action takes the conversation lock, then the follow-up row FOR UPDATE; arm 7's claim and
// the promotion take the same conversation lock and re-check the row. The tests hold the lock in a
// second connection, start the competing operation, release, and look for exactly one outcome.

const HOLD: Duration = Duration::from_millis(400);

async fn locked(w: &W, f: Uuid) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = w.r.h.pool().begin().await.unwrap();
    assert!(convs::lock(&mut tx, w.conv).await.unwrap());
    turns::lock_followup(&mut tx, f).await.unwrap().unwrap();
    tx
}

#[tokio::test]
async fn race_cancel_vs_promotion_cancel_first_means_never_promoted() {
    let w = world(false).await;
    let f = w.followup(&w.a, "race").await;
    let mut tx = locked(&w, f).await;
    // The cancel's critical section is open; the turn ends meanwhile and waits for the lock.
    turns::cancel_followup(&mut tx, f).await.unwrap().unwrap();
    let release = async {
        tokio::time::sleep(HOLD).await;
        tx.commit().await.unwrap();
    };
    let (st, ()) = tokio::join!(w.finish(), release);
    assert_eq!(st, "applied");
    assert_eq!(w.row(f).await.0, "cancelled");
    assert_eq!(
        w.promoted_turns().await,
        0,
        "a cancelled message is never run"
    );
}

#[tokio::test]
async fn race_cancel_vs_promotion_promotion_first_means_cancel_is_refused() {
    let w = world(false).await;
    let f = w.followup(&w.a, "race").await;
    let mut tx = w.r.h.pool().begin().await.unwrap();
    // The turn ends (and promotes) in a transaction that has not committed yet.
    service::finish_turn(&mut tx, w.turn, "done", None, None, None)
        .await
        .unwrap()
        .unwrap();
    let release = async {
        tokio::time::sleep(HOLD).await;
        tx.commit().await.unwrap();
    };
    let ((s, v), ()) = tokio::join!(w.cancel(&w.a, f), release);
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("followup_not_waiting")),
        "{v}"
    );
    assert_eq!(w.row(f).await.0, "promoted");
    assert_eq!(
        w.promoted_turns().await,
        1,
        "run once, as the author sent it"
    );
}

#[tokio::test]
async fn race_edit_vs_delivery_edit_first_delivers_the_edited_text() {
    let w = world(false).await;
    let f = w.followup(&w.a, "old text").await;
    w.steer(&w.a, f).await;
    // Arm 7 is about to hand it over (it has read the candidate); the edit commits first.
    let mut tx = locked(&w, f).await;
    turns::edit_followup(&mut tx, f, "new text")
        .await
        .unwrap()
        .unwrap();
    let release = async {
        tokio::time::sleep(HOLD).await;
        tx.commit().await.unwrap();
    };
    let (items, ()) = tokio::join!(w.steered(), release);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(
        items[0]["text"], "new text",
        "what the agent got is what is stored"
    );
    assert_eq!(w.row(f).await.1, "new text");
}

#[tokio::test]
async fn race_edit_vs_delivery_delivery_first_refuses_the_edit() {
    let w = world(false).await;
    let f = w.followup(&w.a, "old text").await;
    w.steer(&w.a, f).await;
    // The engine's claim commits first; the edit that was waiting for the lock is refused.
    let mut tx = w.r.h.pool().begin().await.unwrap();
    assert!(convs::lock(&mut tx, w.conv).await.unwrap());
    let row = turns::claim_steer(&mut tx, f).await.unwrap().unwrap();
    assert_eq!(row.text, "old text");
    let release = async {
        tokio::time::sleep(HOLD).await;
        tx.commit().await.unwrap();
    };
    let ((s, v), ()) = tokio::join!(w.edit(&w.a, f, "new text"), release);
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("followup_not_waiting")),
        "{v}"
    );
    assert_eq!(w.row(f).await.1, "old text", "stored = delivered");
}

#[tokio::test]
async fn race_cancel_vs_delivery_cancel_first_hands_nothing_over() {
    let w = world(false).await;
    let f = w.followup(&w.a, "x").await;
    w.steer(&w.a, f).await;
    let mut tx = locked(&w, f).await;
    turns::cancel_followup(&mut tx, f).await.unwrap().unwrap();
    let release = async {
        tokio::time::sleep(HOLD).await;
        tx.commit().await.unwrap();
    };
    let (items, ()) = tokio::join!(w.steered(), release);
    assert!(
        items.is_empty(),
        "a cancelled message is not delivered: {items:?}"
    );
    assert_eq!(
        w.row(f).await,
        ("cancelled".into(), "x".into(), true, false, None)
    );
}

#[tokio::test]
async fn race_cancel_vs_delivery_delivery_first_refuses_the_cancel() {
    let w = world(false).await;
    let f = w.followup(&w.a, "x").await;
    w.steer(&w.a, f).await;
    let mut tx = w.r.h.pool().begin().await.unwrap();
    assert!(convs::lock(&mut tx, w.conv).await.unwrap());
    turns::claim_steer(&mut tx, f).await.unwrap().unwrap();
    let release = async {
        tokio::time::sleep(HOLD).await;
        tx.commit().await.unwrap();
    };
    let ((s, v), ()) = tokio::join!(w.cancel(&w.a, f), release);
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("followup_not_waiting")),
        "{v}"
    );
    assert_eq!(w.row(f).await.0, "joining");
}

#[tokio::test]
async fn race_edit_vs_steer_request_both_apply_in_lock_order() {
    let w = world(false).await;
    let f = w.followup(&w.a, "old").await;
    // The edit holds the lock; the steer request queues behind it, then both are in effect and the
    // delivery carries the edit.
    let mut tx = locked(&w, f).await;
    turns::edit_followup(&mut tx, f, "edited")
        .await
        .unwrap()
        .unwrap();
    let release = async {
        tokio::time::sleep(HOLD).await;
        tx.commit().await.unwrap();
    };
    let ((s, v), ()) = tokio::join!(w.steer(&w.a, f), release);
    assert_eq!(s, 202, "{v}");
    assert_eq!(v["text"], "edited");
    let items = w.steered().await;
    assert_eq!(items[0]["text"], "edited");
}

#[tokio::test]
async fn race_steer_vs_the_turn_ending_one_outcome_never_both() {
    // The turn ends first: the follow-up is promoted and the steer is refused; nothing is handed over.
    let w = world(false).await;
    let f = w.followup(&w.a, "late steer").await;
    let mut tx = w.r.h.pool().begin().await.unwrap();
    service::finish_turn(&mut tx, w.turn, "done", None, None, None)
        .await
        .unwrap()
        .unwrap();
    let release = async {
        tokio::time::sleep(HOLD).await;
        tx.commit().await.unwrap();
    };
    let ((s, v), ()) = tokio::join!(w.steer(&w.a, f), release);
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("followup_not_waiting")),
        "{v}"
    );
    assert!(w.steered().await.is_empty());
    assert_eq!(w.row(f).await.0, "promoted");

    // The steer is requested first, then the turn ends before the engine delivers it: promoted,
    // never handed over, so it runs once (as the next turn).
    let w = world(false).await;
    let f = w.followup(&w.a, "early steer").await;
    let (s, _) = w.steer(&w.a, f).await;
    assert_eq!(s, 202);
    w.finish().await;
    assert_eq!(w.row(f).await.0, "promoted");
    assert!(
        w.steered().await.is_empty(),
        "not delivered into a finished turn"
    );
    assert_eq!(w.promoted_turns().await, 1);
}

#[tokio::test]
async fn double_click_steer_across_connections_hands_over_exactly_one_message() {
    let w = world(false).await;
    let f = w.followup(&w.a, "once only").await;
    let (a, b, c) = tokio::join!(w.steer(&w.a, f), w.steer(&w.a, f), w.steered());
    // A click that lands after the engine already handed the message over is refused (409): one outcome
    // per click, never an error of another kind, and at least the first click asked.
    let ok = |s: u16| s == 202 || s == 409;
    assert!(
        ok(a.0) && ok(b.0) && (a.0 == 202 || b.0 == 202),
        "{} {}",
        a.0,
        b.0
    );
    let rest = w.steered().await;
    assert_eq!(
        c.len() + rest.len(),
        1,
        "exactly one delivery: {c:?} {rest:?}"
    );
}

// ---- C-1152: the harness list says what each one can do --------------------------------------

#[tokio::test]
async fn the_harness_list_carries_the_turn_controls_from_the_one_table() {
    let w = world(false).await;
    let (s, v) = w.call(&w.a, Method::GET, "/v1/harnesses", None).await;
    assert_eq!(s, 200, "{v}");
    for h in v["items"].as_array().unwrap() {
        let id = h["id"].as_str().unwrap();
        let want = aurora_protocol::runner::harness_turn_controls(id);
        assert_eq!(h["steer"], want.steer, "{id}");
        assert_eq!(h["background_tool"], want.background_tool, "{id}");
    }
}
