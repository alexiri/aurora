#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P2-010 (RFE-010, D-53, SEC-47): conversation archive, delete, transcript redaction, retention
//! and export. Authorization is asserted per principal and conversation kind; delete, redaction
//! and retention are audited atomically and never record content.

use crate::common::*;
use aurora_db::repo::transcript;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const SECRET: &str = "hunter2-super-secret-token";

pub(crate) struct Fx {
    pub h: Harness,
    pub org: Uuid,
    pub ws: Uuid,
    pub agent: Uuid,
    /// Workspace owner (org member bound as `owner`).
    pub owner: (Uuid, String),
    /// Plain workspace members.
    pub alice: (Uuid, String),
    pub bob: (Uuid, String),
    /// Org admin (retention).
    pub admin: (Uuid, String),
    /// Service-less member API key of alice (workspace key).
    pub alice_key: String,
}

pub(crate) async fn fixture_with(mutate: impl FnOnce(&mut aurora_config::ApiConfig)) -> Fx {
    let h = Harness::with_config(mutate).await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    async fn person(
        h: &Harness,
        org: Uuid,
        ws: Uuid,
        email: &str,
        org_role: &str,
        bind: Option<&str>,
    ) -> (Uuid, String) {
        let u = h.user(email, false).await;
        h.member(org, u, org_role, "active").await;
        if let Some(b) = bind {
            h.bind_ws(ws, u, b).await;
        }
        let (_, t) = h.good_session(u).await;
        (u, cookie(&t))
    }
    let owner = person(&h, org, ws, "owner@x.io", "member", Some("owner")).await;
    let alice = person(&h, org, ws, "alice@x.io", "member", Some("member")).await;
    let bob = person(&h, org, ws, "bob@x.io", "member", Some("member")).await;
    let admin = person(&h, org, ws, "admin@x.io", "admin", None).await;
    let (_, k) = h.api_key(alice.0, org, Some(ws), false).await;
    Fx {
        h,
        org,
        ws,
        agent,
        owner,
        alice,
        bob,
        admin,
        alice_key: bearer(&k),
    }
}

pub(crate) async fn fixture() -> Fx {
    fixture_with(|_| {}).await
}

impl Fx {
    pub async fn call(&self, m: Method, uri: &str, who: &str, body: Option<Value>) -> (u16, Value) {
        let hd: Vec<(&str, &str)> = if who.starts_with("Bearer ") {
            vec![("authorization", who), ("origin", ORIGIN)]
        } else {
            vec![("cookie", who), ("origin", ORIGIN)]
        };
        let (s, _, v) = self.h.send(m, uri, &hd, body).await;
        (s.as_u16(), v)
    }

    pub async fn conv(&self, kind: &str, direct: Option<Uuid>) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id, last_activity_at) VALUES ($1,$2,$3,$4,$5,$6, now())")
            .bind(id).bind(self.agent).bind(self.ws).bind(kind).bind(id.to_string()).bind(direct)
            .execute(self.h.pool()).await.unwrap();
        id
    }

    /// A finished turn by `author` with its durable events; returns `(turn id, queued seq, text seq)`.
    pub async fn turn(&self, conv: Uuid, author: Option<Uuid>, text: &str) -> (Uuid, i64, i64) {
        let t = Uuid::now_v7();
        sqlx::query("INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_user_id, speaker_label, text) VALUES ($1,$2,$3,'turn','done','{\"kind\":\"web\"}','person',$4,'Someone',$5)")
            .bind(t).bind(conv).bind(self.agent).bind(author).bind(text)
            .execute(self.h.pool()).await.unwrap();
        let mut c = self.h.pool().acquire().await.unwrap();
        let q = transcript::append_durable_event(&mut c, conv, Some(t), "turn.queued",
            &json!({"turn_id": t, "door": "dashboard", "speaker_label": "Someone", "text": text, "attachments": []})).await.unwrap();
        let a = transcript::append_durable_event(
            &mut c,
            conv,
            Some(t),
            "text",
            &json!({"turn_id": t, "message_id": "m1", "text": format!("agent saw {text}")}),
        )
        .await
        .unwrap();
        (t, q, a)
    }

    pub async fn count(&self, sql: &str, id: Uuid) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    pub async fn audit(&self, action: &str) -> Vec<Value> {
        sqlx::query_scalar("SELECT metadata FROM audit_log WHERE action = $1 ORDER BY created_at")
            .bind(action)
            .fetch_all(self.h.pool())
            .await
            .unwrap()
    }
}

fn del(id: Uuid) -> String {
    format!("/v1/conversations/{id}")
}

#[tokio::test]
async fn delete_authorization_matrix_per_kind() {
    let f = fixture().await;
    // direct conversation of alice: alice, the workspace owner, nobody else.
    let d = f.conv("direct", Some(f.alice.0)).await;
    assert_eq!(
        f.call(Method::DELETE, &del(d), &f.bob.1, None).await.0,
        404,
        "bob cannot even see it"
    );
    assert_eq!(
        f.call(Method::DELETE, &del(d), &f.alice.1, None).await.0,
        204
    );
    assert_eq!(
        f.count("SELECT count(*) FROM conversations WHERE id = $1", d)
            .await,
        0
    );
    let d2 = f.conv("direct", Some(f.alice.0)).await;
    assert_eq!(
        f.call(Method::DELETE, &del(d2), &f.owner.1, None).await.0,
        204,
        "owner (can_own, can see)"
    );
    // alice's own workspace key deletes her direct conversation too (principal = alice)
    let d3 = f.conv("direct", Some(f.alice.0)).await;
    assert_eq!(
        f.call(Method::DELETE, &del(d3), &f.alice_key, None).await.0,
        204
    );
    // group conversation: a member may see it but not delete it; the owner can.
    let g = f.conv("group", None).await;
    let (s, v) = f.call(Method::DELETE, &del(g), &f.alice.1, None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("forbidden")),
        "{v}"
    );
    assert_eq!(
        f.call(Method::DELETE, &del(g), &f.alice_key, None).await.0,
        403
    );
    assert_eq!(
        f.call(Method::DELETE, &del(g), &f.owner.1, None).await.0,
        204
    );
    // org admin is an implicit workspace owner
    let g2 = f.conv("api", None).await;
    assert_eq!(
        f.call(Method::DELETE, &del(g2), &f.admin.1, None).await.0,
        204
    );
    // unknown id
    assert_eq!(
        f.call(Method::DELETE, &del(Uuid::now_v7()), &f.owner.1, None)
            .await
            .0,
        404
    );
}

#[tokio::test]
async fn delete_is_refused_while_a_turn_is_active() {
    let f = fixture().await;
    let c = f.conv("group", None).await;
    f.turn(c, Some(f.alice.0), "hello").await;
    sqlx::query("INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text) VALUES ($1,$2,$3,'turn','queued','{\"kind\":\"web\"}','person','x','pending')")
        .bind(Uuid::now_v7()).bind(c).bind(f.agent).execute(f.h.pool()).await.unwrap();
    let (s, v) = f.call(Method::DELETE, &del(c), &f.owner.1, None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("conversation_active")),
        "{v}"
    );
    assert_eq!(
        f.count("SELECT count(*) FROM conversations WHERE id = $1", c)
            .await,
        1
    );
    assert!(f.audit("conversation.delete").await.is_empty());
    sqlx::query(
        "UPDATE turns SET status = 'aborted' WHERE conversation_id = $1 AND status = 'queued'",
    )
    .bind(c)
    .execute(f.h.pool())
    .await
    .unwrap();
    assert_eq!(
        f.call(Method::DELETE, &del(c), &f.owner.1, None).await.0,
        204
    );
}

#[tokio::test]
async fn delete_cascades_atomically_with_a_content_free_audit_row() {
    let f = fixture().await;
    let c = f.conv("group", None).await;
    let (t, _, _) = f.turn(c, Some(f.alice.0), SECRET).await;
    sqlx::query("INSERT INTO turn_answer_buffers (turn_id, msgs) VALUES ($1, $2)")
        .bind(t)
        .bind(json!([{"id": "m", "text": SECRET}]))
        .execute(f.h.pool())
        .await
        .unwrap();
    // attachment row + blob bytes
    let (att, key) = (Uuid::now_v7(), Uuid::now_v7().to_string());
    sqlx::query("INSERT INTO blobs (key, content) VALUES ($1, $2)")
        .bind(&key)
        .bind(SECRET.as_bytes())
        .execute(f.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO attachments (id, workspace_id, conversation_id, turn_id, direction, status, filename, content_type, size_bytes, sha256, blob_backend, blob_key) VALUES ($1,$2,$3,$4,'in','bound','secret.txt','text/plain',5,'x','postgres',$5)")
        .bind(att).bind(f.ws).bind(c).bind(t).bind(&key).execute(f.h.pool()).await.unwrap();
    // gateway approval + action approval keep their rows with the reference nulled
    let (ap, aap) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO approvals (id, org_id, workspace_id, agent_id, conversation_id, turn_id, gateway_instance_id, method, host, path, summary, status, deadline) VALUES ($1,$2,$3,$4,$5,$6,$7,'GET','h','/','{}','denied', now())")
        .bind(ap).bind(f.org).bind(f.ws).bind(f.agent).bind(c).bind(t).bind(Uuid::now_v7()).execute(f.h.pool()).await.unwrap();
    sqlx::query("INSERT INTO action_approvals (id, workspace_id, agent_id, conversation_id, turn_id, action_key, payload, summary, status, expires_at) VALUES ($1,$2,$3,$4,$5,'k','{}','s','rejected', now())")
        .bind(aap).bind(f.ws).bind(f.agent).bind(c).bind(t).execute(f.h.pool()).await.unwrap();
    // context credential
    sqlx::query("INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id, conversation_id, turn_id) VALUES ($1,'context',$2,'auc_x',$3,$4,$5)")
        .bind(Uuid::now_v7()).bind(vec![7u8; 32]).bind(f.agent).bind(c).bind(t).execute(f.h.pool()).await.unwrap();
    // schedule references: run + origin conversation become null
    let sched = Uuid::now_v7();
    sqlx::query("INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone, origin_conversation_id, run_conversation_id) VALUES ($1,$2,'s','p','0 * * * *','UTC',$3,$3)")
        .bind(sched).bind(f.agent).bind(c).execute(f.h.pool()).await.unwrap();
    // a stored harness session
    sqlx::query("UPDATE conversations SET harness_session_ref = 'sess-1' WHERE id = $1")
        .bind(c)
        .execute(f.h.pool())
        .await
        .unwrap();

    assert_eq!(
        f.call(Method::DELETE, &del(c), &f.owner.1, None).await.0,
        204
    );
    for sql in [
        "SELECT count(*) FROM conversations WHERE id = $1",
        "SELECT count(*) FROM turns WHERE conversation_id = $1",
        "SELECT count(*) FROM transcript_events WHERE conversation_id = $1",
        "SELECT count(*) FROM attachments WHERE conversation_id = $1",
        "SELECT count(*) FROM gateway_credentials WHERE conversation_id = $1",
    ] {
        assert_eq!(f.count(sql, c).await, 0, "{sql}");
    }
    assert_eq!(
        f.count(
            "SELECT count(*) FROM turn_answer_buffers WHERE turn_id = $1",
            t
        )
        .await,
        0
    );
    let blobs: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs WHERE key = $1")
        .bind(&key)
        .fetch_one(f.h.pool())
        .await
        .unwrap();
    assert_eq!(blobs, 0, "blob bytes removed after commit");
    let (cid, tid): (Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT conversation_id, turn_id FROM approvals WHERE id = $1")
            .bind(ap)
            .fetch_one(f.h.pool())
            .await
            .unwrap();
    assert_eq!((cid, tid), (None, None));
    let acid: Option<Uuid> =
        sqlx::query_scalar("SELECT conversation_id FROM action_approvals WHERE id = $1")
            .bind(aap)
            .fetch_one(f.h.pool())
            .await
            .unwrap();
    assert_eq!(acid, None);
    let (o, r): (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT origin_conversation_id, run_conversation_id FROM schedules WHERE id = $1",
    )
    .bind(sched)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!((o, r), (None, None));
    // session purge queued for the agent
    let (n, sref): (i64, String) = sqlx::query_as("SELECT count(*), max(session_ref) FROM session_purges WHERE agent_id = $1 AND conversation_id = $2 AND done_at IS NULL")
        .bind(f.agent).bind(c).fetch_one(f.h.pool()).await.unwrap();
    assert_eq!((n, sref.as_str()), (1, "sess-1"));
    // audit: one critical row with ids and counts, never content
    let rows = f.audit("conversation.delete").await;
    assert_eq!(rows.len(), 1);
    let m = rows[0].to_string();
    assert!(!m.contains(SECRET) && !m.contains("secret.txt"), "{m}");
    assert_eq!(rows[0]["conversation_ids"][0], json!(c));
    assert_eq!(
        (
            rows[0]["turns"].as_i64(),
            rows[0]["events"].as_i64(),
            rows[0]["attachments"].as_i64()
        ),
        (Some(1), Some(2), Some(1))
    );
}

#[tokio::test]
async fn redaction_replaces_payload_scrubs_text_and_is_authorized_audited_idempotent() {
    let f = fixture().await;
    let c = f.conv("group", None).await;
    let (t, q, a) = f.turn(c, Some(f.alice.0), SECRET).await;
    sqlx::query("INSERT INTO turn_answer_buffers (turn_id, msgs) VALUES ($1, $2)")
        .bind(t)
        .bind(json!([{"id": "m", "text": SECRET}]))
        .execute(f.h.pool())
        .await
        .unwrap();
    let uri = format!("/v1/conversations/{c}/events/{q}/redact");
    // not the turn author, not an owner: 403 (bob can see the group conversation)
    assert_eq!(
        f.call(Method::POST, &uri, &f.bob.1, Some(json!({})))
            .await
            .0,
        403
    );
    assert_eq!(f.count("SELECT count(*) FROM transcript_events WHERE conversation_id = $1 AND redacted_at IS NOT NULL", c).await, 0);
    // unknown event
    assert_eq!(
        f.call(
            Method::POST,
            &format!("/v1/conversations/{c}/events/999/redact"),
            &f.alice.1,
            Some(json!({}))
        )
        .await
        .0,
        404
    );
    // the author may redact her own event
    let (s, v) = f
        .call(
            Method::POST,
            &uri,
            &f.alice.1,
            Some(json!({"reason": "pasted a password"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["redacted_seqs"], json!([q]));
    let p: Value = sqlx::query_scalar(
        "SELECT payload FROM transcript_events WHERE conversation_id = $1 AND seq = $2",
    )
    .bind(c)
    .bind(q)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(p["redacted"], true);
    assert_eq!(p["turn_id"], json!(t), "structural ids survive");
    assert_eq!(p["reason"], "pasted a password");
    assert!(
        p.get("text").is_none() && !p.to_string().contains(SECRET),
        "{p}"
    );
    let ty: String = sqlx::query_scalar(
        "SELECT type FROM transcript_events WHERE conversation_id = $1 AND seq = $2",
    )
    .bind(c)
    .bind(q)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(ty, "turn.queued");
    assert_eq!(f.count("SELECT count(*) FROM transcript_events WHERE conversation_id = $1 AND redacted_by_user_id IS NOT NULL AND redacted_at IS NOT NULL", c).await, 1);
    let text: String = sqlx::query_scalar("SELECT text FROM turns WHERE id = $1")
        .bind(t)
        .fetch_one(f.h.pool())
        .await
        .unwrap();
    assert_eq!(text, "", "turns.text scrubbed");
    let msgs: Value = sqlx::query_scalar("SELECT msgs FROM turn_answer_buffers WHERE turn_id = $1")
        .bind(t)
        .fetch_one(f.h.pool())
        .await
        .unwrap();
    assert_eq!(msgs, json!([]), "answer buffer scrubbed");
    // the reader sees the marker, not the content
    let (_, ev) = f
        .call(
            Method::GET,
            &format!("/v1/conversations/{c}/events"),
            &f.bob.1,
            None,
        )
        .await;
    assert!(
        !ev.to_string().contains(&format!("\"text\":\"{SECRET}\"")),
        "{ev}"
    );
    // idempotent: second call changes nothing and writes no second audit row
    let (s, v) = f
        .call(Method::POST, &uri, &f.alice.1, Some(json!({})))
        .await;
    assert_eq!((s, &v["redacted_seqs"]), (200, &json!([])));
    assert_eq!(f.audit("transcript_event.redact").await.len(), 1);
    // an owner redacts the rest of the turn through the turn route
    let (s, v) = f
        .call(
            Method::POST,
            &format!("/v1/turns/{t}/redact"),
            &f.owner.1,
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["redacted_seqs"], json!([a]));
    // bob (neither author nor owner) cannot redact the turn
    assert_eq!(
        f.call(
            Method::POST,
            &format!("/v1/turns/{t}/redact"),
            &f.bob.1,
            Some(json!({}))
        )
        .await
        .0,
        403
    );
    for m in f.audit("transcript_event.redact").await {
        let s = m.to_string();
        assert!(
            !s.contains(SECRET) && !s.contains("agent saw"),
            "audit leaks content: {s}"
        );
    }
    assert_eq!(f.audit("transcript_event.redact").await.len(), 2);
}

#[tokio::test]
async fn redaction_authority_in_direct_conversations_and_event_without_turn() {
    let f = fixture().await;
    let d = f.conv("direct", Some(f.alice.0)).await;
    let (_, q, _) = f.turn(d, Some(f.alice.0), "mine").await;
    let uri = format!("/v1/conversations/{d}/events/{q}/redact");
    assert_eq!(
        f.call(Method::POST, &uri, &f.bob.1, Some(json!({})))
            .await
            .0,
        404,
        "invisible to bob"
    );
    assert_eq!(
        f.call(Method::POST, &uri, &f.alice.1, Some(json!({})))
            .await
            .0,
        200
    );
    // an event without a turn: only the direct user / can_own
    let mut c = f.h.pool().acquire().await.unwrap();
    let n = transcript::append_durable_event(
        &mut c,
        d,
        None,
        "notice",
        &json!({"code": "x", "text": SECRET}),
    )
    .await
    .unwrap();
    drop(c);
    let g = f.conv("group", None).await;
    let mut c = f.h.pool().acquire().await.unwrap();
    let gn = transcript::append_durable_event(&mut c, g, None, "notice", &json!({"text": SECRET}))
        .await
        .unwrap();
    drop(c);
    assert_eq!(
        f.call(
            Method::POST,
            &format!("/v1/conversations/{g}/events/{gn}/redact"),
            &f.alice.1,
            Some(json!({}))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        f.call(
            Method::POST,
            &format!("/v1/conversations/{g}/events/{gn}/redact"),
            &f.owner.1,
            Some(json!({}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        f.call(
            Method::POST,
            &format!("/v1/conversations/{d}/events/{n}/redact"),
            &f.alice_key,
            Some(json!({"reason": "x".repeat(501)}))
        )
        .await
        .0,
        400
    );
    assert_eq!(
        f.call(
            Method::POST,
            &format!("/v1/conversations/{d}/events/{n}/redact"),
            &f.alice_key,
            Some(json!({}))
        )
        .await
        .0,
        200
    );
}

#[tokio::test]
async fn reset_session_bumps_the_epoch_clears_the_ref_and_queues_a_purge() {
    let f = fixture().await;
    let c = f.conv("group", None).await;
    let (_, q, _) = f.turn(c, Some(f.alice.0), SECRET).await;
    sqlx::query("UPDATE conversations SET harness_session_ref = 'sess-9', harness_session_epoch = 3 WHERE id = $1").bind(c).execute(f.h.pool()).await.unwrap();
    let uri = format!("/v1/conversations/{c}/events/{q}/redact");
    let (s, v) = f
        .call(
            Method::POST,
            &uri,
            &f.alice.1,
            Some(json!({"reset_session": true})),
        )
        .await;
    assert_eq!((s, &v["session_reset"]), (200, &json!(true)), "{v}");
    let (r, e): (Option<String>, i64) = sqlx::query_as(
        "SELECT harness_session_ref, harness_session_epoch FROM conversations WHERE id = $1",
    )
    .bind(c)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!((r, e), (None, 4));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM session_purges WHERE conversation_id = $1 AND session_ref = 'sess-9' AND done_at IS NULL").bind(c).fetch_one(f.h.pool()).await.unwrap();
    assert_eq!(n, 1);
    let rows = f.audit("transcript_event.redact").await;
    assert_eq!(rows[0]["reset_session"], true);
    // without the flag the session is untouched
    let c2 = f.conv("group", None).await;
    let (_, q2, _) = f.turn(c2, Some(f.alice.0), "x").await;
    sqlx::query("UPDATE conversations SET harness_session_ref = 'sess-2' WHERE id = $1")
        .bind(c2)
        .execute(f.h.pool())
        .await
        .unwrap();
    f.call(
        Method::POST,
        &format!("/v1/conversations/{c2}/events/{q2}/redact"),
        &f.alice.1,
        Some(json!({})),
    )
    .await;
    let (r, e): (Option<String>, i64) = sqlx::query_as(
        "SELECT harness_session_ref, harness_session_epoch FROM conversations WHERE id = $1",
    )
    .bind(c2)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!((r.as_deref(), e), (Some("sess-2"), 0));
}

#[tokio::test]
async fn archive_hides_from_default_lists_and_a_person_message_unarchives() {
    let f = fixture().await;
    let (_, v) = f
        .call(
            Method::POST,
            &format!("/v1/agents/{}/conversations/direct", f.agent),
            &f.alice.1,
            None,
        )
        .await;
    let c: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let list = |archived: Option<bool>| {
        let q = archived.map_or(String::new(), |a| format!("?archived={a}"));
        format!("/v1/agents/{}/conversations{q}", f.agent)
    };
    let ids = |v: &Value| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&f.call(Method::GET, &list(None), &f.alice.1, None).await.1),
        [c.to_string()]
    );
    // bob cannot see (and so cannot archive) alice's direct conversation
    assert_eq!(
        f.call(
            Method::POST,
            &format!("/v1/conversations/{c}/archive"),
            &f.bob.1,
            None
        )
        .await
        .0,
        404
    );
    let (s, v) = f
        .call(
            Method::POST,
            &format!("/v1/conversations/{c}/archive"),
            &f.alice.1,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["archived_at"].is_string());
    assert!(ids(&f.call(Method::GET, &list(None), &f.alice.1, None).await.1).is_empty());
    assert!(
        ids(&f
            .call(Method::GET, &list(Some(false)), &f.alice.1, None)
            .await
            .1)
        .is_empty()
    );
    assert_eq!(
        ids(&f
            .call(Method::GET, &list(Some(true)), &f.alice.1, None)
            .await
            .1),
        [c.to_string()]
    );
    // archiving is a tidy-up, not a lock: reading still works, and a message unarchives
    assert_eq!(
        f.call(
            Method::GET,
            &format!("/v1/conversations/{c}"),
            &f.alice.1,
            None
        )
        .await
        .0,
        200
    );
    let (s, _) = f
        .call(
            Method::POST,
            &format!("/v1/conversations/{c}/messages"),
            &f.alice.1,
            Some(json!({"text": "hi again"})),
        )
        .await;
    assert_eq!(s, 202);
    assert_eq!(
        ids(&f.call(Method::GET, &list(None), &f.alice.1, None).await.1),
        [c.to_string()]
    );
    // explicit unarchive
    f.call(
        Method::POST,
        &format!("/v1/conversations/{c}/archive"),
        &f.alice.1,
        None,
    )
    .await;
    let (s, v) = f
        .call(
            Method::POST,
            &format!("/v1/conversations/{c}/unarchive"),
            &f.alice.1,
            None,
        )
        .await;
    assert_eq!((s, v["archived_at"].is_null()), (200, true));
}

#[tokio::test]
async fn retention_setting_validation_authority_and_audit() {
    let f = fixture_with(|c| c.retention.conversation_days_max = Some(365)).await;
    let uri = format!("/v1/orgs/{}/retention", f.org);
    let (s, v) = f.call(Method::GET, &uri, &f.admin.1, None).await;
    assert_eq!(
        (s, &v["conversations_days"], &v["conversations_days_max"]),
        (200, &Value::Null, &json!(365)),
        "{v}"
    );
    for who in [&f.alice.1, &f.owner.1] {
        // workspace roles are not org roles
        assert_eq!(
            f.call(
                Method::PUT,
                &uri,
                who,
                Some(json!({"conversations_days": 30}))
            )
            .await
            .0,
            403
        );
        assert_eq!(f.call(Method::GET, &uri, who, None).await.0, 403);
    }
    for (d, code) in [
        (6, "retention_invalid"),
        (3651, "retention_invalid"),
        (0, "retention_invalid"),
        (366, "retention_above_instance_maximum"),
    ] {
        let (s, v) = f
            .call(
                Method::PUT,
                &uri,
                &f.admin.1,
                Some(json!({"conversations_days": d})),
            )
            .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (422, Some(code)),
            "{d}: {v}"
        );
    }
    assert_eq!(f.audit("retention.update").await.len(), 0);
    for d in [7, 365] {
        let (s, v) = f
            .call(
                Method::PUT,
                &uri,
                &f.admin.1,
                Some(json!({"conversations_days": d})),
            )
            .await;
        assert_eq!((s, &v["conversations_days"]), (200, &json!(d)), "{v}");
    }
    let stored: Option<i64> = sqlx::query_scalar("SELECT (settings #>> '{retention,conversations_days}')::bigint FROM organizations WHERE id = $1").bind(f.org).fetch_one(f.h.pool()).await.unwrap();
    assert_eq!(stored, Some(365));
    // absent key = unchanged, null = none
    let (_, v) = f.call(Method::PUT, &uri, &f.admin.1, Some(json!({}))).await;
    assert_eq!(v["conversations_days"], 365);
    let (_, v) = f
        .call(
            Method::PUT,
            &uri,
            &f.admin.1,
            Some(json!({"conversations_days": null})),
        )
        .await;
    assert_eq!(v["conversations_days"], Value::Null);
    let rows = f.audit("retention.update").await;
    assert_eq!(rows.len(), 3, "7, 365 and the clear");
    assert_eq!(
        rows[0]["conversations_days"],
        json!({"from": null, "to": 7})
    );
}

async fn age(f: &Fx, c: Uuid, days: i64) {
    sqlx::query("UPDATE conversations SET last_activity_at = now() - make_interval(days => $2::int) WHERE id = $1")
        .bind(c).bind(days).execute(f.h.pool()).await.unwrap();
}

#[tokio::test]
async fn retention_sweep_deletes_only_idle_conversations_through_the_delete_path() {
    let f = fixture().await;
    // disabled by default: nothing is deleted
    let old = f.conv("group", None).await;
    f.turn(old, Some(f.alice.0), "old").await;
    age(&f, old, 100).await;
    assert_eq!(
        aurora_api::domains::conversations::lifecycle::sweep_retention(f.h.pool(), None)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        f.count("SELECT count(*) FROM conversations WHERE id = $1", old)
            .await,
        1
    );
    sqlx::query("UPDATE organizations SET settings = jsonb_set(settings, '{retention}', '{\"conversations_days\": 30}') WHERE id = $1").bind(f.org).execute(f.h.pool()).await.unwrap();
    let fresh = f.conv("group", None).await;
    f.turn(fresh, Some(f.alice.0), "new").await;
    let busy = f.conv("group", None).await;
    sqlx::query("INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text) VALUES ($1,$2,$3,'turn','running','{\"kind\":\"web\"}','person','x','p')").bind(Uuid::now_v7()).bind(busy).bind(f.agent).execute(f.h.pool()).await.unwrap();
    age(&f, busy, 100).await;
    let pending = f.conv("group", None).await;
    sqlx::query("INSERT INTO action_approvals (id, workspace_id, agent_id, conversation_id, action_key, payload, summary, status, expires_at) VALUES ($1,$2,$3,$4,'k','{}','s','pending', now() + interval '1 day')").bind(Uuid::now_v7()).bind(f.ws).bind(f.agent).bind(pending).execute(f.h.pool()).await.unwrap();
    age(&f, pending, 100).await;
    let direct = f.conv("direct", Some(f.bob.0)).await; // direct conversations follow the same rule
    f.turn(direct, Some(f.bob.0), SECRET).await;
    sqlx::query("UPDATE conversations SET harness_session_ref = 's-direct' WHERE id = $1")
        .bind(direct)
        .execute(f.h.pool())
        .await
        .unwrap();
    age(&f, direct, 31).await;
    // another org is untouched by this org's window
    let other_org = f.h.org().await;
    let other_ws = f.h.workspace(other_org).await;
    let oa = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'o','O','byo','medium','')").bind(oa).bind(other_org).bind(other_ws).execute(f.h.pool()).await.unwrap();
    let oc = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) VALUES ($1,$2,$3,'group','x', now() - interval '900 days')").bind(oc).bind(oa).bind(other_ws).execute(f.h.pool()).await.unwrap();

    let n = aurora_api::domains::conversations::lifecycle::sweep_retention(f.h.pool(), None)
        .await
        .unwrap();
    assert_eq!(n, 2, "old + direct");
    for (c, kept) in [
        (old, false),
        (direct, false),
        (fresh, true),
        (busy, true),
        (pending, true),
        (oc, true),
    ] {
        assert_eq!(
            f.count("SELECT count(*) FROM conversations WHERE id = $1", c)
                .await,
            i64::from(kept),
            "{c}"
        );
    }
    let rows = f.audit("conversation.delete").await;
    assert_eq!(rows.len(), 1, "one audit row per batch");
    assert_eq!(rows[0]["reason"], "retention");
    assert_eq!(rows[0]["conversation_ids"].as_array().unwrap().len(), 2);
    assert!(!rows[0].to_string().contains(SECRET));
    let actor: (String, Option<Uuid>) = sqlx::query_as(
        "SELECT actor_kind, actor_id FROM audit_log WHERE action = 'conversation.delete'",
    )
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(actor, ("system".to_owned(), None));
    let purge: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM session_purges WHERE session_ref = 's-direct' AND done_at IS NULL",
    )
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(
        purge, 1,
        "the harness session of a swept conversation is purged too"
    );
    // the instance maximum tightens an org window that is longer than it
    sqlx::query("UPDATE organizations SET settings = jsonb_set(settings, '{retention}', '{\"conversations_days\": 3000}') WHERE id = $1").bind(f.org).execute(f.h.pool()).await.unwrap();
    age(&f, fresh, 40).await;
    assert_eq!(
        aurora_api::domains::conversations::lifecycle::sweep_retention(f.h.pool(), None)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        aurora_api::domains::conversations::lifecycle::sweep_retention(f.h.pool(), Some(30))
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn retention_sweep_batches_conversations() {
    let f = fixture().await;
    sqlx::query("UPDATE organizations SET settings = jsonb_set(settings, '{retention}', '{\"conversations_days\": 7}') WHERE id = $1").bind(f.org).execute(f.h.pool()).await.unwrap();
    let n = aurora_core::limits::RETENTION_SWEEP_BATCH + 5;
    sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) SELECT gen_random_uuid(), $1, $2, 'group', 'r' || g, now() - interval '30 days' FROM generate_series(1, $3) g")
        .bind(f.agent).bind(f.ws).bind(n).execute(f.h.pool()).await.unwrap();
    let got = aurora_api::domains::conversations::lifecycle::sweep_retention(f.h.pool(), None)
        .await
        .unwrap();
    assert_eq!(got as i64, n);
    assert_eq!(f.audit("conversation.delete").await.len(), 2, "200 + 5");
}

#[tokio::test]
async fn export_format_authorization_and_redacted_markers() {
    let f = fixture().await;
    let c = f.conv("group", None).await;
    let (t, q, _) = f.turn(c, Some(f.alice.0), SECRET).await;
    f.call(
        Method::POST,
        &format!("/v1/conversations/{c}/events/{q}/redact"),
        &f.alice.1,
        Some(json!({})),
    )
    .await;
    let (key, att) = (Uuid::now_v7().to_string(), Uuid::now_v7());
    sqlx::query("INSERT INTO blobs (key, content) VALUES ($1, $2)")
        .bind(&key)
        .bind(b"file-bytes".as_slice())
        .execute(f.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO attachments (id, workspace_id, conversation_id, turn_id, direction, status, filename, content_type, size_bytes, sha256, blob_backend, blob_key) VALUES ($1,$2,$3,$4,'in','bound','../report.txt','text/plain',10,'abc','postgres',$5)")
        .bind(att).bind(f.ws).bind(c).bind(t).bind(&key).execute(f.h.pool()).await.unwrap();
    let uri = format!("/v1/conversations/{c}/export");
    // the turn author alone may not export; a plain member neither
    assert_eq!(f.call(Method::GET, &uri, &f.alice.1, None).await.0, 403);
    assert_eq!(f.call(Method::GET, &uri, &f.bob.1, None).await.0, 403);
    let (s, v) = f.call(Method::GET, &uri, &f.owner.1, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["format"], "aurora.conversation.v1");
    assert_eq!(v["conversation"]["id"], json!(c));
    assert_eq!(v["turns"].as_array().unwrap().len(), 1);
    let evs = v["events"].as_array().unwrap();
    assert_eq!(evs.len(), 2);
    assert_eq!(
        evs[0]["payload"]["redacted"], true,
        "redacted events export as their marker"
    );
    assert!(
        !v.to_string().contains(&format!("\"{SECRET}\"")) && !evs[0].to_string().contains(SECRET)
    );
    let a = &v["attachments"][0];
    assert_eq!(
        (
            a["id"].as_str(),
            a["filename"].as_str(),
            a["size"].as_i64(),
            a["sha256"].as_str(),
            a["direction"].as_str(),
            a["purged"].as_bool()
        ),
        (
            Some(att.to_string().as_str()),
            Some("../report.txt"),
            Some(10),
            Some("abc"),
            Some("in"),
            Some(false)
        )
    );
    // after_seq pages
    let (_, v) = f
        .call(
            Method::GET,
            &format!("{uri}?after_seq={q}"),
            &f.owner.1,
            None,
        )
        .await;
    assert_eq!(v["events"].as_array().unwrap().len(), 1);
    assert_eq!(
        f.call(
            Method::GET,
            &format!("{uri}?attachments=tar"),
            &f.owner.1,
            None
        )
        .await
        .0,
        400
    );
    // an org admin can export a user's direct conversation (subject-access), audited
    let d = f.conv("direct", Some(f.alice.0)).await;
    f.turn(d, Some(f.alice.0), "private").await;
    assert_eq!(
        f.call(
            Method::GET,
            &format!("/v1/conversations/{d}/export"),
            &f.bob.1,
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(
        f.call(
            Method::GET,
            &format!("/v1/conversations/{d}/export"),
            &f.alice.1,
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        f.call(
            Method::GET,
            &format!("/v1/conversations/{d}/export"),
            &f.admin.1,
            None
        )
        .await
        .0,
        200
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(f.audit("conversation.export").await.len() >= 3);
}

#[tokio::test]
async fn export_zip_contains_the_json_and_the_files_and_too_large_is_refused() {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let f = fixture().await;
    let c = f.conv("group", None).await;
    let (t, _, _) = f.turn(c, Some(f.alice.0), "x").await;
    let key = Uuid::now_v7().to_string();
    sqlx::query("INSERT INTO blobs (key, content) VALUES ($1, $2)")
        .bind(&key)
        .bind(b"zip-me-please".as_slice())
        .execute(f.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO attachments (id, workspace_id, conversation_id, turn_id, direction, status, filename, content_type, size_bytes, sha256, blob_backend, blob_key) VALUES ($1,$2,$3,$4,'out','bound','a b.txt','text/plain',13,'abc','postgres',$5)")
        .bind(Uuid::now_v7()).bind(f.ws).bind(c).bind(t).bind(&key).execute(f.h.pool()).await.unwrap();
    let req = http::Request::builder()
        .uri(format!("/v1/conversations/{c}/export?attachments=zip"))
        .header("cookie", &f.owner.1)
        .body(Body::empty())
        .unwrap();
    let res = f.h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "application/zip");
    assert!(
        res.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
    let b = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&b[..4], b"PK\x03\x04");
    let has = |needle: &[u8]| b.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"conversation.json") && has(b"zip-me-please") && has(b"a_b.txt"));
    assert!(has(b"aurora.conversation.v1"));
    // too many events: 422 export_too_large, paged exports below the cap still work
    let big = f.conv("group", None).await;
    sqlx::query("INSERT INTO transcript_events (conversation_id, seq, type, payload) SELECT $1, g, 'notice', '{}' FROM generate_series(1, $2) g")
        .bind(big).bind(aurora_core::limits::EXPORT_MAX_EVENTS + 1).execute(f.h.pool()).await.unwrap();
    let (s, v) = f
        .call(
            Method::GET,
            &format!("/v1/conversations/{big}/export"),
            &f.owner.1,
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("export_too_large")),
        "{v}"
    );
    let (s, v) = f
        .call(
            Method::GET,
            &format!("/v1/conversations/{big}/export?after_seq=2"),
            &f.owner.1,
            None,
        )
        .await;
    assert_eq!(
        (s, v["events"].as_array().map(Vec::len)),
        (200, Some(199_999))
    );
}

#[tokio::test]
async fn session_purge_is_delivered_live_and_in_the_home_sync_and_completed_by_the_report() {
    let r = crate::hosted::rig().await;
    let lv = r.boot("daneel").await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(
        r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
            .await,
        "applied"
    );
    assert_eq!(
        r.report(&lv, json!({"kind": "session", "conversation_id": lv.conv, "session_ref": "sess-live", "epoch": 0})).await,
        "applied"
    );
    // a live turn blocks the delete (and so any purge)
    let (s, v) = r
        .user_call(
            Method::DELETE,
            &format!("/v1/conversations/{}", lv.conv),
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("conversation_active")),
        "{v}"
    );
    assert_eq!(
        r.report(
            &lv,
            json!({"kind": "turn.result", "turn_id": turn, "status": "done"})
        )
        .await,
        "applied"
    );

    let (s, _) = r
        .user_call(
            Method::DELETE,
            &format!("/v1/conversations/{}", lv.conv),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let pending = |r: &crate::hosted::Rig| {
        let pool = r.h.pool().clone();
        let agent = lv.agent;
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM session_purges WHERE agent_id = $1 AND done_at IS NULL",
            )
            .bind(agent)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(pending(&r).await, 1);

    // live delivery: a `session.purge` work item for the running sandbox, once per lease
    let items = r.work().await;
    let p = items
        .iter()
        .find(|i| i["kind"] == "session.purge")
        .unwrap_or_else(|| panic!("{items:?}"));
    assert_eq!(p["conversation_id"], json!(lv.conv));
    assert_eq!(p["session_ref"], "sess-live");
    assert_eq!(
        (p["sandbox_id"].as_str(), p["boot_id"].as_str()),
        (Some(lv.sb.as_str()), Some(lv.boot.as_str()))
    );
    assert!(
        r.work().await.iter().all(|i| i["kind"] != "session.purge"),
        "leased"
    );

    // a (re)booting sandbox gets the pending purge in the render_inputs of its home sync
    sqlx::query("UPDATE sandboxes SET home_applied_generation = 0, lifecycle_lease_until = NULL WHERE agent_id = $1")
        .bind(lv.agent).execute(r.h.pool()).await.unwrap();
    let items = r.work().await;
    let ri = items
        .iter()
        .find(|i| i["kind"] == "home.sync" && i["part"]["kind"] == "render_inputs")
        .unwrap();
    assert_eq!(
        ri["part"]["entries"][0]["purges"],
        json!([{"conversation_id": lv.conv, "session_ref": "sess-live"}])
    );

    let generation = items.iter().rfind(|i| i["kind"] == "home.sync").unwrap()["generation"]
        .as_u64()
        .unwrap();
    assert_eq!(
        r.report(
            &lv,
            json!({"kind": "home.synced", "generation": generation})
        )
        .await,
        "applied"
    );

    // a failed purge stays pending and is re-sent after the lease is released
    assert_eq!(
        r.report(&lv, json!({"kind": "session.purged", "conversation_id": lv.conv, "session_ref": "sess-live", "ok": false})).await,
        "applied"
    );
    assert_eq!(pending(&r).await, 1);
    assert!(
        r.work().await.iter().any(|i| i["kind"] == "session.purge"),
        "re-sent after a failure"
    );
    assert_eq!(
        r.report(&lv, json!({"kind": "session.purged", "conversation_id": lv.conv, "session_ref": "sess-live", "ok": true})).await,
        "applied"
    );
    assert_eq!(pending(&r).await, 0);
    assert_eq!(
        r.report(&lv, json!({"kind": "session.purged", "conversation_id": lv.conv, "session_ref": "sess-live", "ok": true})).await,
        "duplicate"
    );
    assert!(r.work().await.iter().all(|i| i["kind"] != "session.purge"));
}
