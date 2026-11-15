#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-CHAT-4 parts A and C (D-229, D-230, C-727, C-728): message search — the index, its
//! visibility, removal and backfill, the query syntax, and the agent tools' scope.

use crate::common::*;
use aurora_api::domains::conversations::service;
use aurora_api::domains::tools::{ToolCallContext, ToolContext, registry};
use aurora_db::repo::search;
use aurora_protocol::tools::ToolContent;
use http::Method;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::Duration;
use uuid::Uuid;

struct Who {
    id: Uuid,
    cookie: String,
}

struct Fx {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    other_agent: Uuid,
    a: Who,
    b: Who,
    o: Who,
}

async fn who(h: &Harness, org: Uuid, ws: Uuid, email: &str, bind: &str) -> Who {
    let id = h.user(email, false).await;
    h.member(org, id, "member", "active").await;
    h.bind_ws(ws, id, bind).await;
    let (_, tok) = h.good_session(id).await;
    Who {
        id,
        cookie: cookie(&tok),
    }
}

async fn fx() -> Fx {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let mut agents = Vec::new();
    for (handle, name) in [("bot", "Bot"), ("other", "Other")] {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,$4,$5,'byo','medium','')")
            .bind(id).bind(org).bind(ws).bind(handle).bind(name).execute(h.pool()).await.unwrap();
        agents.push(id);
    }
    let a = who(&h, org, ws, "a@x.io", "approver").await;
    let b = who(&h, org, ws, "b@x.io", "approver").await;
    let o = who(&h, org, ws, "o@x.io", "owner").await;
    Fx {
        h,
        org,
        ws,
        agent: agents[0],
        other_agent: agents[1],
        a,
        b,
        o,
    }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

impl Fx {
    async fn conv(&self, agent: Uuid, kind: &str, direct: Option<Uuid>) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id, last_activity_at) VALUES ($1,$2,$3,$4,$5,$6, now())")
            .bind(id).bind(agent).bind(self.ws).bind(kind).bind(id.to_string()).bind(direct)
            .execute(self.h.pool()).await.unwrap();
        id
    }

    /// A finished person turn with its durable `turn.queued`; returns `(turn id, seq)`.
    async fn say(&self, conv: Uuid, by: Option<&Who>, label: &str, text: &str) -> (Uuid, i64) {
        let agent: Uuid = sqlx::query_scalar("SELECT agent_id FROM conversations WHERE id = $1")
            .bind(conv)
            .fetch_one(self.h.pool())
            .await
            .unwrap();
        let turn = Uuid::now_v7();
        sqlx::query("INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_user_id, speaker_label, text) VALUES ($1,$2,$3,'turn','done','{\"kind\":\"web\"}',$4,$5,$6,$7)")
            .bind(turn).bind(conv).bind(agent)
            .bind(if by.is_some() { "person" } else { "system" })
            .bind(by.map(|w| w.id)).bind(label).bind(text)
            .execute(self.h.pool()).await.unwrap();
        let seq = self
            .event(
                conv,
                Some(turn),
                "turn.queued",
                json!({"turn_id": turn, "door": "web", "speaker_label": label, "text": text, "attachments": []}),
            )
            .await;
        (turn, seq)
    }

    async fn event(&self, conv: Uuid, turn: Option<Uuid>, ty: &str, payload: Value) -> i64 {
        let mut tx = self.h.pool().begin().await.unwrap();
        let seq = service::append_event(&mut tx, conv, turn, ty, &payload)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        seq
    }

    async fn answer(&self, conv: Uuid, turn: Uuid, text: &str) -> i64 {
        self.event(
            conv,
            Some(turn),
            "text",
            json!({"turn_id": turn, "text": text}),
        )
        .await
    }

    async fn complete_index(&self) {
        while !search::backfill_step(self.h.pool(), 1000)
            .await
            .unwrap()
            .complete
        {}
    }

    async fn search(&self, who: &Who, qs: &str) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(
                Method::GET,
                &format!("/v1/workspaces/{}/messages/search?{qs}", self.ws),
                &[("cookie", &who.cookie), ("origin", ORIGIN)],
                None,
            )
            .await;
        (s.as_u16(), v)
    }

    async fn hits(&self, who: &Who, qs: &str) -> Vec<Value> {
        let (s, v) = self.search(who, qs).await;
        assert_eq!(s, 200, "{qs}: {v}");
        v["items"].as_array().unwrap().clone()
    }

    async fn count_rows(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM transcript_search")
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

fn convs(hits: &[Value]) -> HashSet<Uuid> {
    hits.iter()
        .map(|h| h["conversation"]["id"].as_str().unwrap().parse().unwrap())
        .collect()
}

fn highlighted(item: &Value) -> Vec<String> {
    let units: Vec<u16> = item["snippet"].as_str().unwrap().encode_utf16().collect();
    item["highlights"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let (a, b) = (
                r[0].as_u64().unwrap() as usize,
                r[1].as_u64().unwrap() as usize,
            );
            String::from_utf16(&units[a..b]).unwrap()
        })
        .collect()
}

#[tokio::test]
async fn visibility_query_syntax_filters_sorts_and_cursor() {
    let f = fx().await;
    let ca = f.conv(f.agent, "direct", Some(f.a.id)).await;
    let cb = f.conv(f.agent, "direct", Some(f.b.id)).await;
    let cg = f.conv(f.agent, "group", None).await;
    f.say(ca, Some(&f.a), "A", "deploy the staging cluster")
        .await;
    f.say(cb, Some(&f.b), "B", "deploy secret plans").await;
    let (t1, _) = f
        .say(
            cg,
            Some(&f.a),
            "A",
            "deployment notes for the blue green rollout",
        )
        .await;
    f.answer(
        cg,
        t1,
        "Deploying now: **blue** green `kubectl apply` [docs](https://x.io/d)",
    )
    .await;
    f.say(
        cg,
        Some(&f.o),
        "O",
        "we were running tests and the häuser are groß",
    )
    .await;
    f.complete_index().await;

    // Visibility is inside the query: a member never finds another member's direct conversation,
    // an owner does (as the list shows).
    let a = f.hits(&f.a, "q=deploy").await;
    assert_eq!(convs(&a), HashSet::from([ca, cg]));
    assert!(!serde_json::to_string(&a).unwrap().contains("secret"));
    let o = f.hits(&f.o, "q=deploy").await;
    assert_eq!(convs(&o), HashSet::from([ca, cb, cg]));
    assert!(f.hits(&f.a, "q=secret").await.is_empty());
    assert_eq!(f.hits(&f.b, "q=secret").await.len(), 1);
    let (s, _) = f
        .search(&f.a, &format!("q=deploy&conversation_id={cb}"))
        .await;
    assert_eq!(s, 404);

    // Snippet: plain text, highlight ranges are UTF-16 offsets of the matched words.
    let one = f.hits(&f.a, "q=staging").await;
    assert_eq!(highlighted(&one[0]), ["staging"]);
    assert_eq!(one[0]["author"]["kind"], "person");
    assert_eq!(one[0]["type"], "turn.queued");
    let md = f.hits(&f.a, "q=kubectl").await;
    let snip = md[0]["snippet"].as_str().unwrap();
    assert!(
        !snip.contains('*') && !snip.contains('`') && snip.contains("https://x.io"),
        "{snip}"
    );
    assert_eq!(md[0]["author"]["label"], "Bot");

    // Syntax: prefix on the last word, phrase, exclusion, OR, stemming and exact foreign words.
    assert_eq!(f.hits(&f.a, "q=deplo").await.len(), 3);
    assert_eq!(
        f.hits(&f.a, &format!("q={}", enc("\"blue green\"")))
            .await
            .len(),
        2
    );
    assert_eq!(
        f.hits(&f.a, &format!("q={}", enc("deploy -rollout")))
            .await
            .len(),
        2
    );
    assert_eq!(
        f.hits(&f.a, &format!("q={}", enc("staging OR kubectl")))
            .await
            .len(),
        2
    );
    assert_eq!(
        f.hits(&f.a, &format!("q={}", enc("runs"))).await.len(),
        1,
        "stemmed"
    );
    assert_eq!(
        f.hits(&f.a, &format!("q={}", enc("häuser"))).await.len(),
        1,
        "exact"
    );

    // Filters.
    assert_eq!(f.hits(&f.a, "q=deploy&author=me").await.len(), 2);
    assert_eq!(f.hits(&f.a, "q=deploy&author=agent").await.len(), 1);
    assert_eq!(f.hits(&f.a, "q=deploy&author=others").await.len(), 0);
    assert_eq!(f.hits(&f.a, "q=runs&author=others").await.len(), 1);
    assert_eq!(
        f.hits(&f.a, &format!("q=deploy&conversation_id={cg}"))
            .await
            .len(),
        2
    );
    assert!(f.hits(&f.a, "q=deploy&after=2099-01-01").await.is_empty());
    assert_eq!(f.hits(&f.a, "q=deploy&before=2099-01-01").await.len(), 3);
    assert!(
        f.hits(&f.a, &format!("q=deploy&agent_id={}", f.other_agent))
            .await
            .is_empty()
    );
    let (s, _) = f.search(&f.a, "q=deploy&author=nobody").await;
    assert_eq!(s, 400);
    f.event(
        cg,
        Some(t1),
        "attachment",
        json!({"turn_id": t1, "attachment_id": Uuid::now_v7(), "filename": "report-q3.pdf", "caption": "Quarterly"}),
    )
    .await;
    let files = f.hits(&f.a, "q=report&has=file").await;
    assert_eq!(
        (files.len(), files[0]["type"].as_str()),
        (1, Some("attachment"))
    );
    sqlx::query("UPDATE conversations SET archived_at = now() WHERE id = $1")
        .bind(cg)
        .execute(f.h.pool())
        .await
        .unwrap();
    assert_eq!(f.hits(&f.a, "q=deploy&archived=exclude").await.len(), 1);
    assert_eq!(f.hits(&f.a, "q=deploy&archived=only").await.len(), 2);
    assert!(f.hits(&f.a, "q=deploy").await[0]["conversation"]["archived"].is_boolean());

    // Both sorts page without gaps or repeats, even when a row is inserted between pages.
    for sort in ["recent", "relevance"] {
        let mut seen: Vec<(String, i64)> = Vec::new();
        let mut cursor = String::new();
        for page in 0..10 {
            let (s, v) = f
                .search(&f.o, &format!("q=deploy&limit=1&sort={sort}{cursor}"))
                .await;
            assert_eq!(s, 200, "{v}");
            for i in v["items"].as_array().unwrap() {
                seen.push((
                    i["conversation"]["id"].as_str().unwrap().into(),
                    i["seq"].as_i64().unwrap(),
                ));
            }
            if page == 0 {
                f.say(cg, Some(&f.o), "O", "deploy newer message").await;
            }
            match v["next_cursor"].as_str() {
                Some(c) => cursor = format!("&cursor={}", enc(c)),
                None => break,
            }
        }
        let unique: HashSet<_> = seen.iter().cloned().collect();
        assert_eq!(unique.len(), seen.len(), "{sort}: no repeats");
        assert!(seen.len() >= 4, "{sort}: {seen:?}");
    }

    // Hostile and invalid queries are 422 or handled, never 500.
    for q in [
        "a & b | !c",
        "'; DROP TABLE x; --",
        "\\\\ \\",
        "(((",
        "a:*b <-> c",
        "\"unterminated",
        "-",
        "!!!",
    ] {
        let (s, v) = f.search(&f.a, &format!("q={}", enc(q))).await;
        assert!(s == 200 || s == 422, "{q}: {s} {v}");
    }
    let (s, v) = f
        .search(&f.a, &format!("q={}", enc(&"x".repeat(10_000))))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("search_query_invalid"))
    );
    assert_eq!(v["error"]["details"]["reason"], "too_long");
    let (s, v) = f.search(&f.a, "q=%20").await;
    assert_eq!(
        (s, v["error"]["details"]["reason"].as_str()),
        (422, Some("empty"))
    );
    let many = (0..13)
        .map(|i| format!("w{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let (s, v) = f.search(&f.a, &format!("q={}", enc(&many))).await;
    assert_eq!(
        (s, v["error"]["details"]["reason"].as_str()),
        (422, Some("too_many_terms"))
    );
}

#[tokio::test]
async fn redaction_delete_and_attachment_names() {
    let f = fx().await;
    let cg = f.conv(f.agent, "group", None).await;
    let cd = f.conv(f.agent, "direct", Some(f.a.id)).await;
    let (t, seq) = f
        .say(cg, Some(&f.a), "A", "the launch codes are 1234")
        .await;
    let ans = f.answer(cg, t, "noted the launch plan").await;
    f.say(cd, Some(&f.a), "A", "launch tomorrow").await;
    f.complete_index().await;
    assert_eq!(f.hits(&f.a, "q=launch").await.len(), 3);
    // A turn.queued's attachment names are indexed too.
    let att = Uuid::now_v7();
    sqlx::query("INSERT INTO attachments (id, workspace_id, conversation_id, direction, status, filename, content_type, size_bytes, sha256, blob_backend, blob_key) VALUES ($1,$2,$3,'in','bound','budget-2027.xlsx','x/y',1,'00','postgres',$4)")
        .bind(att).bind(f.ws).bind(cg).bind(format!("k{att}")).execute(f.h.pool()).await.unwrap();
    f.event(cg, Some(t), "turn.queued", json!({"turn_id": t, "door": "web", "speaker_label": "A", "text": "", "attachments": [att]})).await;
    assert_eq!(f.hits(&f.a, "q=budget&has=file").await.len(), 1);

    // Redacting removes the row in the redaction's transaction; the text never matches again.
    let (s, _, v) =
        f.h.send(
            Method::POST,
            &format!("/v1/conversations/{cg}/events/{seq}/redact"),
            &[("cookie", &f.a.cookie), ("origin", ORIGIN)],
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(f.hits(&f.a, "q=codes").await.is_empty());
    assert_eq!(f.hits(&f.a, "q=launch").await.len(), 2);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM transcript_search WHERE conversation_id = $1 AND seq = $2",
    )
    .bind(cg)
    .bind(seq)
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0);
    let _ = ans;

    // Deleting a conversation cascades.
    let before = f.count_rows().await;
    let (s, _, v) =
        f.h.send(
            Method::DELETE,
            &format!("/v1/conversations/{cd}"),
            &[("cookie", &f.a.cookie), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert!(s.is_success(), "{s} {v}");
    assert_eq!(f.count_rows().await, before - 1);
    assert_eq!(f.hits(&f.a, "q=tomorrow").await.len(), 0);
}

#[tokio::test]
async fn backfill_resumes_flips_index_complete_and_skips_redacted() {
    let f = fx().await;
    let cg = f.conv(f.agent, "group", None).await;
    let (t, _) = f.say(cg, Some(&f.a), "A", "first old message").await;
    f.complete_index().await;
    // Pre-existing events written without the index (as before the migration).
    let mut raw = Vec::new();
    for i in 0..5 {
        let seq: i64 = sqlx::query_scalar(
            "UPDATE conversations SET last_seq = last_seq + 1 WHERE id = $1 RETURNING last_seq",
        )
        .bind(cg)
        .fetch_one(f.h.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO transcript_events (conversation_id, seq, turn_id, type, payload) VALUES ($1,$2,$3,'text',$4)")
            .bind(cg).bind(seq).bind(t).bind(json!({"turn_id": t, "text": format!("legacy message {i}")}))
            .execute(f.h.pool()).await.unwrap();
        raw.push(seq);
    }
    sqlx::query("UPDATE transcript_events SET redacted_at = now(), payload = '{\"redacted\":true}' WHERE conversation_id = $1 AND seq = $2")
        .bind(cg).bind(raw[4]).execute(f.h.pool()).await.unwrap();
    sqlx::query("UPDATE transcript_search_backfill SET complete = false, cursor_conversation = NULL, cursor_seq = NULL, indexed = 0")
        .execute(f.h.pool()).await.unwrap();
    let (s, v) = f.search(&f.a, "q=legacy").await;
    assert_eq!(
        (
            s,
            &v["index_complete"],
            v["items"].as_array().unwrap().len()
        ),
        (200, &json!(false), 0)
    );

    // Step by step (each call re-reads the persisted cursor: a restart resumes there).
    let s1 = search::backfill_step(f.h.pool(), 2).await.unwrap();
    assert_eq!((s1.scanned, s1.complete), (2, false));
    let cur: (Option<Uuid>, Option<i64>) =
        sqlx::query_as("SELECT cursor_conversation, cursor_seq FROM transcript_search_backfill")
            .fetch_one(f.h.pool())
            .await
            .unwrap();
    assert_eq!(cur.0, Some(cg));
    // An event written (and indexed) by the write path meanwhile is not indexed twice.
    f.say(cg, Some(&f.a), "A", "legacy message during backfill")
        .await;
    let mut steps = 0;
    loop {
        let st = search::backfill_step(f.h.pool(), 2).await.unwrap();
        steps += 1;
        assert!(steps < 20);
        if st.complete {
            break;
        }
    }
    let (s, v) = f.search(&f.a, "q=legacy&limit=50").await;
    assert_eq!((s, &v["index_complete"]), (200, &json!(true)));
    assert_eq!(
        v["items"].as_array().unwrap().len(),
        5,
        "4 legacy + 1 during; the redacted one is skipped"
    );
    let dup: i64 = sqlx::query_scalar(
        "SELECT count(*) - count(DISTINCT (conversation_id, seq)) FROM transcript_search",
    )
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(dup, 0);
}

// ------------------------------------------------------------------ agent tools (C-728)

fn ctx(f: &Fx, conv: Uuid, turn: Uuid, agent: Uuid) -> ToolCallContext {
    ToolCallContext {
        agent: ToolContext {
            pool: f.h.pool().clone(),
            agent_id: agent,
            workspace_id: f.ws,
            org_id: f.org,
        },
        conversation_id: conv,
        turn_id: Some(turn),
        call_id: "c1".into(),
        state: f.h.state.clone(),
    }
}

async fn call(c: &ToolCallContext, tool: &str, args: Value) -> Result<Value, String> {
    let r = registry()
        .call(c, tool, args, Duration::from_secs(10))
        .await;
    if !r.ok {
        return Err(r.error.unwrap().message);
    }
    match r.content.unwrap().remove(0) {
        ToolContent::Json(j) => Ok(j.value),
        other => panic!("{other:?}"),
    }
}

fn hit_convs(v: &Value) -> HashSet<Uuid> {
    v["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["conversation_id"].as_str().unwrap().parse().unwrap())
        .collect()
}

#[tokio::test]
async fn agent_tools_scope_matrix_wrapper_and_access_change() {
    let f = fx().await;
    let ca = f.conv(f.agent, "direct", Some(f.a.id)).await;
    let cb = f.conv(f.agent, "direct", Some(f.b.id)).await;
    let co = f.conv(f.agent, "direct", Some(f.o.id)).await;
    let cg = f.conv(f.agent, "group", None).await;
    let cx = f.conv(f.other_agent, "group", None).await;
    let (ta, _) = f.say(ca, Some(&f.a), "A", "needle in A direct").await;
    f.say(cb, Some(&f.b), "B", "needle in B direct").await;
    let (to, _) = f.say(co, Some(&f.o), "O", "needle in O direct").await;
    let (_, gseq) = f.say(cg, Some(&f.a), "A", "needle in the group").await;
    let (_, redacted) = f
        .say(cg, Some(&f.a), "A", "needle that gets redacted")
        .await;
    f.say(cx, Some(&f.a), "A", "needle of another agent").await;
    // A schedule turn has no speaker.
    let (ts, _) = f.say(cg, None, "Schedule", "needle from a schedule").await;
    f.complete_index().await;
    sqlx::query("UPDATE transcript_events SET redacted_at = now(), payload = '{\"redacted\":true}' WHERE conversation_id = $1 AND seq = $2")
        .bind(cg).bind(redacted).execute(f.h.pool()).await.unwrap();
    sqlx::query("DELETE FROM transcript_search WHERE conversation_id = $1 AND seq = $2")
        .bind(cg)
        .bind(redacted)
        .execute(f.h.pool())
        .await
        .unwrap();

    let args = json!({"query": "needle"});
    // Member speaker A: own direct + non-direct, never B's, never another agent's.
    let v = call(
        &ctx(&f, ca, ta, f.agent),
        "conversation_search",
        args.clone(),
    )
    .await
    .unwrap();
    assert_eq!(hit_convs(&v), HashSet::from([ca, cg]));
    assert_eq!(v["untrusted"], true);
    assert!(v["notice"].as_str().unwrap().contains("untrusted"));
    assert!(!v.to_string().contains("redacted"));
    // Owner speaker O: also does not find B's (or A's) direct conversation.
    let v = call(
        &ctx(&f, cg, to, f.agent),
        "conversation_search",
        args.clone(),
    )
    .await
    .unwrap();
    assert_eq!(hit_convs(&v), HashSet::from([co, cg]));
    // A speakerless (schedule) turn: the current conversation and the non-direct ones.
    let v = call(
        &ctx(&f, cg, ts, f.agent),
        "conversation_search",
        args.clone(),
    )
    .await
    .unwrap();
    assert_eq!(hit_convs(&v), HashSet::from([cg]));
    // this_conversation narrows.
    let v = call(
        &ctx(&f, ca, ta, f.agent),
        "conversation_search",
        json!({"query": "needle", "where": "this_conversation"}),
    )
    .await
    .unwrap();
    assert_eq!(hit_convs(&v), HashSet::from([ca]));
    // Bad queries are tool errors.
    let e = call(
        &ctx(&f, ca, ta, f.agent),
        "conversation_search",
        json!({"query": "  "}),
    )
    .await
    .unwrap_err();
    assert!(e.starts_with("search_query_invalid"), "{e}");

    // conversation_read: in scope works; out of scope reads exactly like an unknown id.
    let c = ctx(&f, ca, ta, f.agent);
    let v = call(
        &c,
        "conversation_read",
        json!({"conversation_id": cg, "seq": gseq}),
    )
    .await
    .unwrap();
    assert_eq!(v["untrusted"], true);
    let msgs = v["messages"].as_array().unwrap();
    assert!(msgs.iter().any(|m| m["text"] == "needle in the group"));
    assert!(
        msgs.iter().any(|m| m["redacted"] == true),
        "redacted events are shown as redacted"
    );
    assert!(!v.to_string().contains("gets redacted"));
    let out_of_scope = call(
        &c,
        "conversation_read",
        json!({"conversation_id": cb, "seq": 1}),
    )
    .await
    .unwrap_err();
    let unknown = call(
        &c,
        "conversation_read",
        json!({"conversation_id": Uuid::now_v7(), "seq": 1}),
    )
    .await
    .unwrap_err();
    let other_agent = call(
        &c,
        "conversation_read",
        json!({"conversation_id": cx, "seq": 1}),
    )
    .await
    .unwrap_err();
    assert_eq!(out_of_scope, unknown);
    assert_eq!(other_agent, unknown);

    // index_complete passes through.
    sqlx::query("UPDATE transcript_search_backfill SET complete = false")
        .execute(f.h.pool())
        .await
        .unwrap();
    let v = call(&c, "conversation_search", args.clone()).await.unwrap();
    assert_eq!(v["index_complete"], false);

    // Access change: the speaker loses the workspace; non-direct conversations disappear.
    sqlx::query("DELETE FROM workspace_access WHERE workspace_id = $1 AND user_id = $2")
        .bind(f.ws)
        .bind(f.a.id)
        .execute(f.h.pool())
        .await
        .unwrap();
    let v = call(&ctx(&f, ca, ta, f.agent), "conversation_search", args)
        .await
        .unwrap();
    assert!(!hit_convs(&v).contains(&cg), "{v}");
    let e = call(
        &ctx(&f, ca, ta, f.agent),
        "conversation_read",
        json!({"conversation_id": cg, "seq": gseq}),
    )
    .await
    .unwrap_err();
    assert_eq!(e, unknown);
}
