#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P3-018 (RFE-018, SEC-55): `Idempotency-Key` on `POST /v1/conversations/{id}/messages`
//! (C-320..C-326; `18 §Message idempotency`).

use crate::common::*;
use aurora_api::domains::conversations::service;
use aurora_config::RateClassOverride;
use http::{HeaderMap, Method};
use serde_json::{Value, json};
use sqlx::postgres::PgListener;
use std::time::Duration;
use uuid::Uuid;

struct Fx {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    user: Uuid,
    cookie: String,
    key: String,
}

async fn fixture_with(mutate: impl FnOnce(&mut aurora_config::ApiConfig)) -> Fx {
    let h = Harness::with_config(mutate).await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    let user = h.user("m@x.io", false).await;
    h.member(org, user, "member", "active").await;
    h.bind_ws(ws, user, "member").await;
    let (_, tok) = h.good_session(user).await;
    let (_, key) = h.api_key(user, org, Some(ws), false).await;
    Fx {
        h,
        org,
        ws,
        agent,
        user,
        cookie: cookie(&tok),
        key: bearer(&key),
    }
}

async fn fixture() -> Fx {
    fixture_with(|_| {}).await
}

impl Fx {
    async fn conv(&self, kind: &str, direct: Option<Uuid>) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id, last_activity_at) VALUES ($1,$2,$3,$4,$5,$6, now())")
            .bind(id).bind(self.agent).bind(self.ws).bind(kind).bind(id.to_string()).bind(direct)
            .execute(self.h.pool()).await.unwrap();
        id
    }

    /// A message with the given raw `Idempotency-Key` header values (none = no header).
    async fn post_as(
        &self,
        auth: (&str, &str),
        conv: Uuid,
        keys: &[&str],
        body: Value,
    ) -> (u16, HeaderMap, Value) {
        let mut hs: Vec<(&str, &str)> = vec![auth, ("origin", ORIGIN)];
        for k in keys {
            hs.push(("idempotency-key", k));
        }
        let (s, h, v) = self
            .h
            .send(
                Method::POST,
                &format!("/v1/conversations/{conv}/messages"),
                &hs,
                Some(body),
            )
            .await;
        (s.as_u16(), h, v)
    }

    async fn post(&self, conv: Uuid, keys: &[&str], text: &str) -> (u16, HeaderMap, Value) {
        self.post_as(("cookie", &self.cookie), conv, keys, json!({"text": text}))
            .await
    }

    async fn count(&self, sql: &str, id: Uuid) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn turns(&self, conv: Uuid) -> i64 {
        self.count(
            "SELECT count(*) FROM turns WHERE conversation_id = $1 AND role = 'turn'",
            conv,
        )
        .await
    }

    async fn followups(&self, conv: Uuid) -> i64 {
        self.count(
            "SELECT count(*) FROM turns WHERE conversation_id = $1 AND role = 'followup'",
            conv,
        )
        .await
    }
}

fn replayed(h: &HeaderMap) -> Option<&str> {
    h.get("idempotent-replayed").map(|v| v.to_str().unwrap())
}

fn id_of(v: &Value, k: &str) -> Uuid {
    v[k]["id"].as_str().unwrap().parse().unwrap()
}

/// Key validation (length, charset, repeated header), no header = phase-0 behaviour, ignored on
/// other routes.
#[tokio::test]
async fn key_validation_absent_key_and_other_routes() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    let long = "k".repeat(129);
    let max = "k".repeat(128);
    for bad in ["", "has space", "slash/key", "ünï", long.as_str()] {
        let (s, _, v) = f.post(conv, &[bad], "hi").await;
        assert_eq!(s, 422, "{bad:?}: {v}");
        assert_eq!(v["error"]["code"], "idempotency_key_invalid");
    }
    let (s, _, v) = f.post(conv, &["a", "b"], "hi").await;
    assert_eq!(
        (s, &v["error"]["code"]),
        (422, &json!("idempotency_key_invalid"))
    );
    assert_eq!(f.turns(conv).await, 0, "nothing created by an invalid key");
    for ok in ["a", max.as_str(), "A.b_c:d~e-1"] {
        let c2 = f.conv("group", None).await;
        let (s, h, v) = f.post(c2, &[ok], "hi").await;
        assert_eq!(s, 202, "{ok:?}: {v}");
        assert_eq!(replayed(&h), None, "first use carries no replay header");
    }
    // No header: two posts, a turn and a follow-up (phase 0).
    let c3 = f.conv("group", None).await;
    let (_, h1, a) = f.post(c3, &[], "same").await;
    let (_, _, b) = f.post(c3, &[], "same").await;
    assert_eq!(replayed(&h1), None);
    assert!(!a["turn"].is_null() && a["followup"].is_null());
    assert!(!b["followup"].is_null() && b["turn"].is_null());
    assert_eq!((f.turns(c3).await, f.followups(c3).await), (1, 1));
    // Ignored (even if invalid) on another route.
    let (s, _, _) =
        f.h.send(
            Method::GET,
            &format!("/v1/conversations/{c3}"),
            &[
                ("cookie", &f.cookie),
                ("origin", ORIGIN),
                ("idempotency-key", "bad key!"),
            ],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200);
}

/// Replay answers with the original status and shape for `{turn}` and `{followup}`, reflects the
/// current row, sets `Idempotent-Replayed`, and creates nothing (no second turn, no second NOTIFY).
#[tokio::test]
async fn replay_turn_then_followup_with_current_state_and_no_second_notify() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    let mut l = PgListener::connect_with(f.h.pool()).await.unwrap();
    l.listen("aurora_work").await.unwrap();

    let (s, h, first) = f.post(conv, &["k-turn"], "hello").await;
    assert_eq!(s, 202);
    assert_eq!(replayed(&h), None);
    let t1 = id_of(&first, "turn");
    tokio::time::timeout(Duration::from_secs(5), l.recv())
        .await
        .expect("first use notifies")
        .unwrap();

    let (s, h, again) = f.post(conv, &["k-turn"], "hello").await;
    assert_eq!(s, 202);
    assert_eq!(replayed(&h), Some("true"));
    assert_eq!(id_of(&again, "turn"), t1);
    assert!(again["followup"].is_null());
    assert_eq!((f.turns(conv).await, f.followups(conv).await), (1, 0));
    assert!(
        tokio::time::timeout(Duration::from_millis(400), l.recv())
            .await
            .is_err(),
        "a replay sends no NOTIFY aurora_work"
    );
    assert_eq!(
        f.count(
            "SELECT count(*) FROM transcript_events WHERE conversation_id=$1 AND type='turn.queued'",
            conv
        )
        .await,
        1
    );

    // A key that collides with the active-turn index is a follow-up, and replays as one.
    let (s, h, fu) = f.post(conv, &["k-fu"], "later").await;
    assert_eq!((s, replayed(&h)), (202, None));
    let fid = id_of(&fu, "followup");
    assert_eq!(fu["followup"]["status"], "joining");
    assert_eq!(fu["followup"]["promoted_turn_id"], Value::Null);
    let (s, h, fu2) = f.post(conv, &["k-fu"], "later").await;
    assert_eq!((s, replayed(&h)), (202, Some("true")));
    assert_eq!(id_of(&fu2, "followup"), fid);
    assert!(fu2["turn"].is_null());
    assert_eq!((f.turns(conv).await, f.followups(conv).await), (1, 1));

    // The replay holds the resource as it is now: running, then done (turn); promoted (follow-up).
    sqlx::query("UPDATE turns SET status='dispatched' WHERE id=$1")
        .bind(t1)
        .execute(f.h.pool())
        .await
        .unwrap();
    let (_, _, r) = f.post(conv, &["k-turn"], "hello").await;
    assert_eq!(r["turn"]["status"], "dispatched");
    let mut tx = f.h.pool().begin().await.unwrap();
    service::start_turn(&mut tx, t1, None).await.unwrap();
    service::finish_turn(&mut tx, t1, "done", None, None, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (s, h, r) = f.post(conv, &["k-turn"], "hello").await;
    assert_eq!((s, replayed(&h)), (202, Some("true")));
    assert_eq!(r["turn"]["status"], "done");
    let (s, h, r) = f.post(conv, &["k-fu"], "later").await;
    assert_eq!((s, replayed(&h)), (202, Some("true")));
    assert_eq!(r["followup"]["status"], "promoted");
    let promoted: Uuid = r["followup"]["promoted_turn_id"]
        .as_str()
        .expect("promoted_turn_id set")
        .parse()
        .unwrap();
    let (txt, role): (String, String) = sqlx::query_as("SELECT text, role FROM turns WHERE id=$1")
        .bind(promoted)
        .fetch_one(f.h.pool())
        .await
        .unwrap();
    assert_eq!((txt.as_str(), role.as_str()), ("later", "turn"));
    // The promoted turn carries no key: the key stays on the follow-up row (C-325).
    assert_eq!(
        f.count(
            "SELECT count(*) FROM turns WHERE id = $1 AND source_event_id IS NOT NULL",
            promoted
        )
        .await,
        0
    );
    // A failed and an aborted turn replay their state too.
    sqlx::query("UPDATE turns SET status='failed', error_code='x', finished_at=now() WHERE id=$1")
        .bind(t1)
        .execute(f.h.pool())
        .await
        .unwrap();
    let (_, _, r) = f.post(conv, &["k-turn"], "hello").await;
    assert_eq!(r["turn"]["status"], "failed");
    sqlx::query("UPDATE turns SET status='aborted' WHERE id=$1")
        .bind(t1)
        .execute(f.h.pool())
        .await
        .unwrap();
    let (_, _, r) = f.post(conv, &["k-turn"], "hello").await;
    assert_eq!(r["turn"]["status"], "aborted");
}

/// N parallel identical requests → exactly one row, N identical responses.
#[tokio::test]
async fn concurrent_identical_requests_create_exactly_one_turn() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    let rs = futures::future::join_all((0..8).map(|_| f.post(conv, &["race-1"], "once"))).await;
    for (s, _, v) in &rs {
        assert_eq!(*s, 202, "{v}");
    }
    let firsts = rs.iter().filter(|(_, h, _)| replayed(h).is_none()).count();
    assert_eq!(firsts, 1, "exactly one original, the rest replays");
    let id = id_of(&rs[0].2, "turn");
    assert!(
        rs.iter()
            .all(|(_, _, v)| id_of(v, "turn") == id && v["followup"].is_null())
    );
    assert_eq!((f.turns(conv).await, f.followups(conv).await), (1, 0));
    assert_eq!(
        f.count(
            "SELECT count(*) FROM transcript_events WHERE conversation_id=$1 AND type='turn.queued'",
            conv
        )
        .await,
        1
    );

    // With an active turn the same race yields exactly one follow-up.
    let rs = futures::future::join_all((0..8).map(|_| f.post(conv, &["race-2"], "queued"))).await;
    assert!(rs.iter().all(|(s, _, _)| *s == 202));
    assert_eq!(
        rs.iter().filter(|(_, h, _)| replayed(h).is_none()).count(),
        1
    );
    assert_eq!((f.turns(conv).await, f.followups(conv).await), (1, 1));
}

/// Scope: per (conversation, user); session and API key of one person share a key space; a
/// replay after loss of access is refused; header validation comes after authorization.
#[tokio::test]
async fn scope_users_session_vs_api_key_and_access_loss() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    let (s, _, a) = f.post(conv, &["shared"], "from session").await;
    assert_eq!(s, 202);
    // Same person through an API key: the same key space → replay.
    let (s, h, b) = f
        .post_as(
            ("authorization", &f.key),
            conv,
            &["shared"],
            json!({"text": "from session"}),
        )
        .await;
    assert_eq!((s, replayed(&h)), (202, Some("true")));
    assert_eq!(id_of(&b, "turn"), id_of(&a, "turn"));
    assert_eq!(f.turns(conv).await, 1);

    // Another user, same key, same conversation: an independent key space (a follow-up).
    let other = f.h.user("o@x.io", false).await;
    f.h.member(f.org, other, "member", "active").await;
    f.h.bind_ws(f.ws, other, "member").await;
    let (_, tok) = f.h.good_session(other).await;
    let oc = cookie(&tok);
    let (s, h, c) = f
        .post_as(("cookie", &oc), conv, &["shared"], json!({"text": "other"}))
        .await;
    assert_eq!((s, replayed(&h)), (202, None), "{c}");
    assert!(!c["followup"].is_null());
    assert_eq!(f.followups(conv).await, 1);
    let stored: Vec<String> = sqlx::query_scalar(
        "SELECT source_event_id FROM turns WHERE conversation_id=$1 ORDER BY created_at, id",
    )
    .bind(conv)
    .fetch_all(f.h.pool())
    .await
    .unwrap();
    assert_eq!(
        stored,
        [
            format!("api:{}:shared", f.user),
            format!("api:{other}:shared")
        ]
    );
    // The stored key is never returned.
    assert!(!a.to_string().contains("api:"));

    // Authorization first: a user who cannot see a direct conversation learns nothing from an
    // invalid header (404, not 422), nor gets a stored result (404) with a valid one.
    let dc = f.conv("direct", Some(f.user)).await;
    let (s, _, _) = f.post(dc, &["d1"], "mine").await;
    assert_eq!(s, 202);
    let (s, _, v) = f
        .post_as(("cookie", &oc), dc, &["bad key"], json!({"text": "mine"}))
        .await;
    assert_eq!(s, 404, "{v}");
    let (s, _, v) = f
        .post_as(("cookie", &oc), dc, &["d1"], json!({"text": "mine"}))
        .await;
    assert_eq!(s, 404, "{v}");
    // Loss of access: the stored result is no longer served.
    sqlx::query("DELETE FROM workspace_access WHERE workspace_id=$1 AND user_id=$2")
        .bind(f.ws)
        .bind(f.user)
        .execute(f.h.pool())
        .await
        .unwrap();
    let (s, _, v) = f.post(conv, &["shared"], "from session").await;
    assert!(matches!(s, 403 | 404), "{s} {v}");
    assert!(v["turn"].is_null());
}

/// A replay returns the stored result while a new message is 423; a changed limit does not refuse
/// a replay either.
#[tokio::test]
async fn replay_while_frozen_returns_stored_new_message_is_423() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    let (s, _, a) = f.post(conv, &["fz"], "before").await;
    assert_eq!(s, 202);
    sqlx::query("UPDATE agents SET frozen_at=now(), frozen_reason='incident', frozen_authority='instance_admin' WHERE id=$1")
        .bind(f.agent)
        .execute(f.h.pool())
        .await
        .unwrap();
    let (s, h, b) = f.post(conv, &["fz"], "before").await;
    assert_eq!((s, replayed(&h)), (202, Some("true")), "{b}");
    assert_eq!(id_of(&b, "turn"), id_of(&a, "turn"));
    let (s, _, v) = f.post(conv, &["fz-new"], "after").await;
    assert_eq!(s, 423, "{v}");
    assert_eq!(v["error"]["code"], "frozen");
    assert_eq!(f.followups(conv).await, 0);
    assert_eq!(f.turns(conv).await, 1);
}

/// A replay consumes one message-bucket token (C-52).
#[tokio::test]
async fn replay_consumes_one_message_token() {
    let f = fixture_with(|c| {
        c.ratelimit.api.multiplier = 1.0;
        c.ratelimit.api.classes.insert(
            "message".into(),
            RateClassOverride {
                per_minute: Some(1),
                burst: Some(3),
                ..Default::default()
            },
        );
    })
    .await;
    let conv = f.conv("group", None).await;
    for i in 0..3 {
        let (s, h, v) = f.post(conv, &["rl"], "x").await;
        assert_eq!(s, 202, "request {i}: {v}");
        assert_eq!(replayed(&h).is_some(), i > 0);
    }
    let (s, _, v) = f.post(conv, &["rl"], "x").await;
    assert_eq!(s, 429, "{v}");
    assert_eq!(f.turns(conv).await, 1);
}

/// Mismatch on `text` or `attachment_ids` → 422 `idempotency_key_reused` naming the stored row.
#[tokio::test]
async fn mismatch_text_and_attachments_name_the_stored_row() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    let (_, _, a) = f.post(conv, &["mm"], "original").await;
    let t1 = id_of(&a, "turn");
    let (s, _, v) = f.post(conv, &["mm"], "changed").await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "idempotency_key_reused");
    assert_eq!(v["error"]["details"]["turn_id"], json!(t1));
    assert_eq!(v["error"]["details"]["followup_id"], Value::Null);
    let (s, _, v) = f
        .post_as(
            ("cookie", &f.cookie),
            conv,
            &["mm"],
            json!({"text": "original", "attachment_ids": [Uuid::now_v7()]}),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "idempotency_key_reused");
    // An empty-attachments list equals none: still a replay.
    let (s, h, _) = f
        .post_as(
            ("cookie", &f.cookie),
            conv,
            &["mm"],
            json!({"text": "original", "attachment_ids": []}),
        )
        .await;
    assert_eq!((s, replayed(&h)), (202, Some("true")));

    // Stored follow-up: the details name `followup_id`.
    let (_, _, fu) = f.post(conv, &["mm-fu"], "queued").await;
    let (s, _, v) = f.post(conv, &["mm-fu"], "different").await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(
        v["error"]["details"]["followup_id"],
        json!(id_of(&fu, "followup"))
    );
    assert_eq!(v["error"]["details"]["turn_id"], Value::Null);
    assert_eq!((f.turns(conv).await, f.followups(conv).await), (1, 1));
}

/// Deleting the conversation (and so retention, which uses the same path) frees its keys.
#[tokio::test]
async fn conversation_delete_frees_the_key() {
    let f = fixture().await;
    let conv = f.conv("direct", Some(f.user)).await;
    let (s, _, _) = f.post(conv, &["gone"], "x").await;
    assert_eq!(s, 202);
    sqlx::query("UPDATE turns SET status='done', finished_at=now() WHERE conversation_id=$1")
        .bind(conv)
        .execute(f.h.pool())
        .await
        .unwrap();
    assert_eq!(
        f.count(
            "SELECT count(*) FROM turns WHERE conversation_id=$1 AND source_event_id IS NOT NULL",
            conv
        )
        .await,
        1
    );
    let (s, _, v) =
        f.h.send(
            Method::DELETE,
            &format!("/v1/conversations/{conv}"),
            &[("cookie", &f.cookie), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 204, "{v}");
    assert_eq!(
        f.count("SELECT count(*) FROM turns WHERE conversation_id=$1", conv)
            .await,
        0
    );
    // The key is free again in a fresh conversation (a new message, not a replay).
    let c2 = f.conv("direct", Some(f.user)).await;
    let (s, h, _) = f.post(c2, &["gone"], "x").await;
    assert_eq!((s, replayed(&h)), (202, None));
}
