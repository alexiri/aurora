#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_api::domains::conversations::service;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

struct Fx {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    user: Uuid,
    cookie: String,
    key: String,
}

async fn fixture(h: &Harness) -> Fx {
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = h.byo_agent(org, ws, "bot", "Bot").await;
    let user = h.user("m@x.io", false).await;
    h.member(org, user, "member", "active").await;
    h.bind_ws(ws, user, "member").await;
    let (_, tok) = h.good_session(user).await;
    let (_, key) = h.api_key(user, org, Some(ws), false).await;
    Fx {
        org,
        ws,
        agent,
        user,
        cookie: cookie(&tok),
        key: bearer(&key),
    }
}

async fn call(
    h: &Harness,
    m: Method,
    uri: &str,
    auth: (&str, &str),
    body: Option<Value>,
) -> (u16, Value) {
    let (s, _, v) = h
        .send(m, uri, &[(auth.0, auth.1), ("origin", ORIGIN)], body)
        .await;
    (s.as_u16(), v)
}

async fn direct(h: &Harness, f: &Fx) -> Uuid {
    let (s, v) = call(
        h,
        Method::POST,
        &format!("/v1/agents/{}/conversations/direct", f.agent),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v["id"].as_str().unwrap().parse().unwrap()
}

async fn msg(h: &Harness, f: &Fx, conv: Uuid, text: &str) -> (u16, Value) {
    call(
        h,
        Method::POST,
        &format!("/v1/conversations/{conv}/messages"),
        ("cookie", &f.cookie),
        Some(json!({"text": text})),
    )
    .await
}

async fn count(h: &Harness, sql: &str, id: Uuid) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

/// Open the SSE stream; returns the body to read frames from.
async fn open(h: &Harness, f: &Fx, conv: Uuid, query: &str, last_id: Option<&str>) -> Body {
    let mut b = Request::builder()
        .method(Method::GET)
        .uri(format!("/v1/conversations/{conv}/stream{query}"))
        .header("cookie", &f.cookie);
    if let Some(l) = last_id {
        b = b.header("last-event-id", l);
    }
    let res = h
        .app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    Body::new(res.into_body())
}

/// Read SSE messages (blank-line separated, comments skipped) until `n` are collected.
async fn read_msgs(
    body: &mut Body,
    n: usize,
    wait: Duration,
) -> Vec<(String, Option<String>, Value)> {
    let mut buf = String::new();
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + wait;
    while out.len() < n {
        let frame = tokio::time::timeout_at(deadline, body.frame()).await;
        let Ok(Some(Ok(frame))) = frame else { break };
        if let Some(d) = frame.data_ref() {
            buf.push_str(std::str::from_utf8(d).unwrap());
        }
        while let Some(i) = buf.find("\n\n") {
            let block: String = buf.drain(..i + 2).collect();
            let (mut ev, mut id, mut data) = (None, None, None);
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    ev = Some(v.trim().to_owned());
                } else if let Some(v) = line.strip_prefix("id:") {
                    id = Some(v.trim().to_owned());
                } else if let Some(v) = line.strip_prefix("data:") {
                    data = Some(serde_json::from_str::<Value>(v.trim()).unwrap());
                }
            }
            if let (Some(ev), Some(data)) = (ev, data) {
                out.push((ev, id, data));
            }
        }
    }
    out
}

#[tokio::test]
async fn direct_get_or_create_is_idempotent_under_concurrency() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let uri = format!("/v1/agents/{}/conversations/direct", f.agent);
    let (a, b) = tokio::join!(
        call(&h, Method::POST, &uri, ("cookie", &f.cookie), None),
        call(&h, Method::POST, &uri, ("cookie", &f.cookie), None),
    );
    assert_eq!((a.0, b.0), (200, 200), "{a:?} {b:?}");
    assert_eq!(a.1["id"], b.1["id"]);
    assert_eq!(a.1["kind"], "direct");
    assert_eq!(a.1["external_ref"], format!("user:{}", f.user));
    let n: i64 = count(
        &h,
        "SELECT count(*) FROM conversations WHERE agent_id = $1",
        f.agent,
    )
    .await;
    assert_eq!(n, 1);
}

#[tokio::test]
async fn concurrent_messages_make_one_turn_and_one_followup() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let conv = direct(&h, &f).await;
    let (a, b) = tokio::join!(msg(&h, &f, conv, "one"), msg(&h, &f, conv, "two"));
    assert_eq!((a.0, b.0), (202, 202), "{a:?} {b:?}");
    let turns = [&a.1, &b.1].iter().filter(|v| !v["turn"].is_null()).count();
    let fups = [&a.1, &b.1]
        .iter()
        .filter(|v| !v["followup"].is_null())
        .count();
    assert_eq!((turns, fups), (1, 1));
    for v in [&a.1, &b.1] {
        assert_eq!(v["turn"].is_null(), !v["followup"].is_null());
    }
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM turns WHERE conversation_id = $1 AND role='turn'",
            conv
        )
        .await,
        1
    );
    assert_eq!(count(&h, "SELECT count(*) FROM turns WHERE conversation_id = $1 AND role='followup' AND status='joining'", conv).await, 1);

    let (_, c) = call(
        &h,
        Method::GET,
        &format!("/v1/conversations/{conv}"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(c["active_turn"]["status"], "queued");
    assert_eq!(c["last_seq"], 2);
}

#[tokio::test]
async fn events_paginate_and_turn_queued_payload() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let conv = direct(&h, &f).await;
    msg(&h, &f, conv, "hello").await;
    for i in 0..4 {
        let mut c = h.pool().acquire().await.unwrap();
        service::append_event(
            &mut c,
            conv,
            None,
            "notice",
            &json!({"code":"n","text":format!("t{i}")}),
        )
        .await
        .unwrap();
    }
    let base = format!("/v1/conversations/{conv}/events");
    let (s, v) = call(
        &h,
        Method::GET,
        &format!("{base}?limit=2"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(s, 200);
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["seq"], 1);
    assert_eq!(items[0]["type"], "turn.queued");
    let p = &items[0]["payload"];
    assert_eq!(p["door"], "dashboard");
    assert_eq!(p["text"], "hello");
    assert_eq!(p["speaker_label"], "U");
    assert_eq!(p["attachments"], json!([]));
    assert_eq!(v["last_seq"], 2);
    let (_, v) = call(
        &h,
        Method::GET,
        &format!("{base}?after_seq=2&limit=500"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    let seqs: Vec<i64> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["seq"].as_i64().unwrap())
        .collect();
    assert_eq!(seqs, vec![3, 4, 5]);
    assert_eq!(v["last_seq"], 5);
    let (s, _) = call(
        &h,
        Method::GET,
        &format!("{base}?limit=501"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(s, 400);
    // ephemeral types are refused by the repository
    let mut c = h.pool().acquire().await.unwrap();
    assert!(
        service::append_event(&mut c, conv, None, "text.delta", &json!({}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn sse_history_then_tail_resume_without_gaps_or_duplicates() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let conv = direct(&h, &f).await;
    msg(&h, &f, conv, "hi").await; // seq 1
    {
        let mut c = h.pool().acquire().await.unwrap();
        service::append_event(
            &mut c,
            conv,
            None,
            "notice",
            &json!({"code":"a","text":"a"}),
        )
        .await
        .unwrap(); // 2
    }
    let mut body = open(&h, &f, conv, "?after_seq=0", None).await;
    let got = read_msgs(&mut body, 2, Duration::from_secs(5)).await;
    assert_eq!(
        got.iter().map(|g| g.1.clone().unwrap()).collect::<Vec<_>>(),
        ["1", "2"]
    );
    assert_eq!(got[0].0, "turn.queued");
    assert_eq!(got[0].2["seq"], 1);
    // concurrent appends while tailing
    let h2 = &h;
    let appender = async move {
        for i in 0..5 {
            let mut tx = h2.pool().begin().await.unwrap();
            service::append_event(
                &mut tx,
                conv,
                None,
                "notice",
                &json!({"code":"b","text":i.to_string()}),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
    };
    let (_, tail) = tokio::join!(appender, read_msgs(&mut body, 5, Duration::from_secs(5)));
    let ids: Vec<String> = tail.iter().map(|g| g.1.clone().unwrap()).collect();
    assert_eq!(ids, ["3", "4", "5", "6", "7"]);
    // resume from Last-Event-ID 5 (header wins over after_seq): exactly 6, 7
    let mut body2 = open(&h, &f, conv, "?after_seq=0", Some("5")).await;
    let again = read_msgs(&mut body2, 2, Duration::from_secs(5)).await;
    assert_eq!(
        again
            .iter()
            .map(|g| g.1.clone().unwrap())
            .collect::<Vec<_>>(),
        ["6", "7"]
    );
    let extra = read_msgs(&mut body2, 1, Duration::from_millis(300)).await;
    assert!(extra.is_empty());
}

#[tokio::test]
async fn deltas_are_coalesced_ephemeral_and_not_persisted() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let conv = direct(&h, &f).await;
    let mut body = open(&h, &f, conv, "?after_seq=0", None).await;
    let turn = Uuid::now_v7();
    let bus = h.state.conversation_bus.clone();
    for _ in 0..30 {
        bus.publish_delta(
            h.pool(),
            conv,
            turn,
            "text.delta",
            (Some("m1"), Some("r1")),
            "ab",
        )
        .await;
    }
    bus.publish_delta(h.pool(), conv, turn, "activity", (None, None), "running ls")
        .await;
    bus.flush_turn(h.pool(), conv, turn).await;
    let got = read_msgs(&mut body, 40, Duration::from_millis(1500)).await;
    let text: Vec<_> = got.iter().filter(|g| g.0 == "text.delta").collect();
    assert!(
        !text.is_empty() && text.len() < 30,
        "coalesced: {}",
        text.len()
    );
    assert!(text.iter().all(|g| g.1.is_none() && g.2["seq"].is_null()));
    let joined: String = text
        .iter()
        .map(|g| g.2["payload"]["delta"].as_str().unwrap())
        .collect();
    assert_eq!(joined, "ab".repeat(30));
    assert_eq!(text[0].2["payload"]["message_id"], "m1");
    assert_eq!(text[0].2["payload"]["reply_to"], "r1");
    let act = got.iter().find(|g| g.0 == "activity").unwrap();
    assert_eq!(act.2["payload"]["line"], "running ls");
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM transcript_events WHERE conversation_id = $1",
            conv
        )
        .await,
        0
    );
}

#[tokio::test]
async fn abort_queued_send_now_and_finish_fencing() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let conv = direct(&h, &f).await;
    let (_, v) = msg(&h, &f, conv, "go").await;
    let turn = v["turn"]["id"].as_str().unwrap().to_owned();

    // (FIX-225, C-1156, D-470) A bring-your-own agent has no harness of ours: Send now is refused up
    // front with a reason (it used to answer 202 and do nothing), and nothing is queued.
    let (s, t) = call(
        &h,
        Method::POST,
        &format!("/v1/turns/{turn}/send-now"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(
        (s, t["error"]["code"].as_str()),
        (409, Some("send_now_unsupported")),
        "{t}"
    );
    let (_, t) = call(
        &h,
        Method::GET,
        &format!("/v1/conversations/{conv}"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(t["active_turn"]["send_now_requested"], json!(false), "{t}");

    let (s, t) = call(
        &h,
        Method::POST,
        &format!("/v1/turns/{turn}/abort"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(s, 202, "{t}");
    assert_eq!(t["status"], "aborted");
    // (FIX-253 review) The stop ends at the terminal transition: the queued turn was aborted in
    // the same call, so nothing is pending any more — `abort_requested` is already cleared.
    assert_eq!(t["abort_requested"], false);
    assert!(!t["finished_at"].is_null());
    let (_, ev) = call(
        &h,
        Method::GET,
        &format!("/v1/conversations/{conv}/events"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    let items = ev["items"].as_array().unwrap();
    let last = items.last().unwrap().clone();
    assert_eq!(last["type"], "turn.finished");
    assert_eq!(last["payload"]["status"], "aborted");
    assert_eq!(last["payload"]["turn_id"], turn);
    // (P5-REFINE-7, C-1075) Stop leaves a mark, in the transaction of `turn.finished`.
    let mark = &items[items.len() - 2];
    assert_eq!(mark["type"], "notice");
    assert_eq!(
        mark["payload"],
        json!({"code": "turn_stopped", "text": "You stopped this answer.", "turn_id": turn})
    );

    let (s, e) = call(
        &h,
        Method::POST,
        &format!("/v1/turns/{turn}/abort"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("turn_finished"))
    );
    // (FIX-225) The refusal for an agent that cannot do it comes before the finished-turn one.
    let (s, e) = call(
        &h,
        Method::POST,
        &format!("/v1/turns/{turn}/send-now"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (409, Some("send_now_unsupported"))
    );
}

#[tokio::test]
async fn finish_promotes_followups_and_replays_are_noops() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let conv = direct(&h, &f).await;
    let (_, v) = msg(&h, &f, conv, "first").await;
    let t1: Uuid = v["turn"]["id"].as_str().unwrap().parse().unwrap();
    let (_, v) = msg(&h, &f, conv, "second").await;
    assert_eq!(v["followup"]["status"], "joining");
    let (_, ev) = call(
        &h,
        Method::GET,
        &format!("/v1/conversations/{conv}/events?after_seq=1"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(ev["items"][0]["payload"]["followup_of"], json!(t1));
    assert_eq!(ev["items"][0]["payload"]["turn_id"], v["followup"]["id"]);
    let (_, v) = msg(&h, &f, conv, "third").await;
    assert_eq!(v["followup"]["parent_turn_id"], json!(t1));
    sqlx::query("UPDATE turns SET status='dispatched' WHERE id=$1")
        .bind(t1)
        .execute(h.pool())
        .await
        .unwrap();

    let mut tx = h.pool().begin().await.unwrap();
    assert!(
        service::start_turn(&mut tx, t1, None)
            .await
            .unwrap()
            .is_some()
    );
    let r = service::finish_turn(
        &mut tx,
        t1,
        "done",
        None,
        None,
        Some(&json!({"input_tokens":3,"output_tokens":4})),
    )
    .await
    .unwrap();
    assert_eq!(r.unwrap().status, "done");
    assert!(
        service::finish_turn(&mut tx, t1, "done", None, None, None)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();

    // first follow-up became the new queued turn; the other re-parented under it and is still joining
    let active: (String, String) = sqlx::query_as("SELECT status, text FROM turns WHERE conversation_id=$1 AND role='turn' AND status='queued'")
        .bind(conv).fetch_one(h.pool()).await.unwrap();
    assert_eq!(active.1, "second");
    assert_eq!(count(&h, "SELECT count(*) FROM turns WHERE conversation_id=$1 AND role='followup' AND status='promoted'", conv).await, 1);
    assert_eq!(count(&h, "SELECT count(*) FROM turns WHERE conversation_id=$1 AND role='followup' AND status='joining'", conv).await, 1);
    let (_, c) = call(
        &h,
        Method::GET,
        &format!("/v1/conversations/{conv}"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(c["active_turn"]["text"], "second");
    let (_, ev) = call(
        &h,
        Method::GET,
        &format!("/v1/conversations/{conv}/events"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    let fin: Vec<_> = ev["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "turn.finished")
        .collect();
    assert_eq!(fin.len(), 1);
    assert_eq!(fin[0]["payload"]["usage"]["input_tokens"], 3);
}

#[tokio::test]
async fn permissions_and_validation() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let conv = direct(&h, &f).await;
    let outsider = h.user("o@x.io", false).await;
    let (_, ot) = h.good_session(outsider).await;
    let oc = cookie(&ot);
    let get = format!("/v1/conversations/{conv}");
    assert_eq!(
        call(&h, Method::GET, &get, ("cookie", &oc), None).await.0,
        404
    );
    assert_eq!(
        call(
            &h,
            Method::GET,
            &format!("{get}/events"),
            ("cookie", &oc),
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(
        call(
            &h,
            Method::POST,
            &format!("{get}/messages"),
            ("cookie", &oc),
            Some(json!({"text":"x"}))
        )
        .await
        .0,
        404
    );
    assert_eq!(
        call(
            &h,
            Method::POST,
            &format!("/v1/agents/{}/conversations/direct", f.agent),
            ("cookie", &oc),
            None
        )
        .await
        .0,
        404
    );

    // another member of the same workspace cannot see this direct conversation
    let peer = h.user("p@x.io", false).await;
    h.member(f.org, peer, "member", "active").await;
    h.bind_ws(f.ws, peer, "member").await;
    let (_, pt) = h.good_session(peer).await;
    assert_eq!(
        call(&h, Method::GET, &get, ("cookie", &cookie(&pt)), None)
            .await
            .0,
        404
    );
    let (_, list) = call(
        &h,
        Method::GET,
        &format!("/v1/agents/{}/conversations", f.agent),
        ("cookie", &cookie(&pt)),
        None,
    )
    .await;
    assert_eq!(list["items"].as_array().unwrap().len(), 0);

    // key confined to another workspace
    let ws2 = h.workspace(f.org).await;
    let (_, k2) = h.api_key(f.user, f.org, Some(ws2), false).await;
    let (s, _) = call(&h, Method::GET, &get, ("authorization", &bearer(&k2)), None).await;
    assert_eq!(s, 403);

    // validation
    assert_eq!(msg(&h, &f, conv, "   ").await.0, 400);
    assert_eq!(msg(&h, &f, conv, &"x".repeat(100_001)).await.0, 413);
    assert_eq!(
        call(
            &h,
            Method::GET,
            &format!("/v1/agents/{}/conversations?kind=nope", f.agent),
            ("cookie", &f.cookie),
            None
        )
        .await
        .0,
        400
    );

    // api conversations: key only, get-or-create
    let uri = format!("/v1/agents/{}/conversations", f.agent);
    let body = json!({"external_ref": "ticket-1", "title": "T"});
    assert_eq!(
        call(
            &h,
            Method::POST,
            &uri,
            ("cookie", &f.cookie),
            Some(body.clone())
        )
        .await
        .0,
        403
    );
    let (s, a) = call(
        &h,
        Method::POST,
        &uri,
        ("authorization", &f.key),
        Some(body.clone()),
    )
    .await;
    assert_eq!((s, a["kind"].clone()), (201, json!("api")));
    let (s, b) = call(
        &h,
        Method::POST,
        &uri,
        ("authorization", &f.key),
        Some(body),
    )
    .await;
    assert_eq!((s, &b["id"]), (200, &a["id"]));
    let (s, _) = call(
        &h,
        Method::POST,
        &uri,
        ("authorization", &f.key),
        Some(json!({"external_ref": ""})),
    )
    .await;
    assert_eq!(s, 400);
    // key can message its api conversation; door is api
    let cid = a["id"].as_str().unwrap();
    let (s, m) = call(
        &h,
        Method::POST,
        &format!("/v1/conversations/{cid}/messages"),
        ("authorization", &f.key),
        Some(json!({"text":"hey"})),
    )
    .await;
    assert_eq!((s, m["turn"]["door"].clone()), (202, json!("api")));
    // list ordering by last activity includes both kinds
    let (_, l) = call(&h, Method::GET, &uri, ("cookie", &f.cookie), None).await;
    assert_eq!(l["items"].as_array().unwrap().len(), 2);
    assert_eq!(l["items"][0]["id"], a["id"]);
}

#[tokio::test]
async fn hosted_agent_disabled_is_422() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let conv = direct(&h, &f).await;
    sqlx::query("UPDATE agents SET kind='hosted', harness='claude_code' WHERE id=$1")
        .bind(f.agent)
        .execute(h.pool())
        .await
        .ok();
    let (s, e) = msg(&h, &f, conv, "x").await;
    if !h.state.config.hosted.enabled {
        assert_eq!(
            (s, e["error"]["code"].as_str()),
            (422, Some("hosted_disabled"))
        );
    }
}

// ------------------------------------------------------------------ P5-REFINE-7 (C-1073)

async fn new_direct(h: &Harness, f: &Fx) -> (u16, Value) {
    call(
        h,
        Method::POST,
        &format!("/v1/agents/{}/conversations/direct/new", f.agent),
        ("cookie", &f.cookie),
        None,
    )
    .await
}

/// Several direct chats per person: `direct` is the most recently active non-archived one,
/// `direct/new` is 201 a fresh one and 200 (the same) while that one is still empty; an
/// archived chat is never "the person's chat"; the limit is 500 (active and archived).
#[tokio::test]
async fn several_direct_chats_new_chat_archive_and_the_limit() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let first = direct(&h, &f).await;
    assert_eq!(direct(&h, &f).await, first, "get-or-create is stable");
    let ext: String = sqlx::query_scalar("SELECT external_ref FROM conversations WHERE id = $1")
        .bind(first)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(ext, format!("user:{}", f.user));
    // The first chat is still empty: New chat hands it back (no pile of empty chats).
    let (s, v) = new_direct(&h, &f).await;
    assert_eq!(
        (s, v["id"].as_str()),
        (200, Some(first.to_string().as_str()))
    );
    let (s, _) = msg(&h, &f, first, "hello").await;
    assert_eq!(s, 202);
    let (s, v) = new_direct(&h, &f).await;
    assert_eq!(s, 201, "{v}");
    let second: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    assert_ne!(second, first);
    assert_eq!(v["kind"], "direct");
    assert_eq!(v["direct_user_id"], f.user.to_string());
    assert_eq!(v["last_seq"], 0, "fresh history");
    assert!(
        v["external_ref"]
            .as_str()
            .unwrap()
            .starts_with(&format!("user:{}:", f.user))
    );
    let (s, v) = new_direct(&h, &f).await;
    assert_eq!(
        (s, v["id"].as_str()),
        (200, Some(second.to_string().as_str()))
    );
    // `direct` follows the most recent activity.
    assert_eq!(direct(&h, &f).await, second);
    let (s, _) = msg(&h, &f, first, "back to the first").await;
    assert_eq!(s, 202);
    assert_eq!(direct(&h, &f).await, first);
    // Archive the active one: `direct` returns the other, the archived one stays listed there.
    let (s, _) = call(
        &h,
        Method::POST,
        &format!("/v1/conversations/{first}/archive"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(direct(&h, &f).await, second);
    let (_, l) = call(
        &h,
        Method::GET,
        &format!("/v1/agents/{}/conversations?archived=true", f.agent),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    let archived: Vec<&str> = l["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(archived, [first.to_string().as_str()]);
    // Every chat archived: `direct` starts a new one rather than reopening an archived one.
    let (s, _) = call(
        &h,
        Method::POST,
        &format!("/v1/conversations/{second}/archive"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(s, 200);
    let third = direct(&h, &f).await;
    assert!(third != first && third != second);
    // The limit counts active and archived chats: at 500, a new one is 422.
    sqlx::query(
        "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id,
            last_activity_at, archived_at)
         SELECT gen_random_uuid(), $1, $2, 'direct', 'user:' || $3::text || ':' || g, $3,
                now() - interval '1 day', now()
         FROM generate_series(1, 497) g",
    )
    .bind(f.agent)
    .bind(f.ws)
    .bind(f.user)
    .execute(h.pool())
    .await
    .unwrap();
    let (s, _) = msg(&h, &f, third, "not empty").await;
    assert_eq!(s, 202);
    let (s, e) = new_direct(&h, &f).await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (422, Some("conversation_limit")),
        "{e}"
    );
    // Session only, like `direct`.
    let (s, _) = call(
        &h,
        Method::POST,
        &format!("/v1/agents/{}/conversations/direct/new", f.agent),
        ("authorization", &f.key),
        None,
    )
    .await;
    assert_eq!(s, 403);
}

/// Rename: the person renames their own direct chat; a workspace owner cannot rename someone
/// else's; group/api need `can_own`; the first person message titles an untitled chat (60
/// characters), a chosen title is kept, `null` re-derives; ids-only audit.
#[tokio::test]
async fn rename_rules_and_the_auto_title() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let owner = h.user("owner@x.io", false).await;
    h.member(f.org, owner, "member", "active").await;
    h.bind_ws(f.ws, owner, "owner").await;
    let (_, otok) = h.good_session(owner).await;
    let ocookie = cookie(&otok);
    let conv = direct(&h, &f).await;
    let long = format!("  **Plan** the `release` for @team\n{}", "x".repeat(10));
    let (s, _) = msg(
        &h,
        &f,
        conv,
        &format!("```\ncode only\n```\n{}", "word ".repeat(30)),
    )
    .await;
    assert_eq!(s, 202);
    let get = |c: Uuid, ck: String| {
        let h = &h;
        async move {
            call(
                h,
                Method::GET,
                &format!("/v1/conversations/{c}"),
                ("cookie", &ck),
                None,
            )
            .await
            .1
        }
    };
    let v = get(conv, f.cookie.clone()).await;
    // The first line after the fence: "code only".
    assert_eq!(v["title"], "code only");
    assert_eq!(v["title_source"], "auto");
    // A later message does not retitle.
    let (s, _) = msg(&h, &f, conv, &long).await;
    assert_eq!(s, 202);
    assert_eq!(get(conv, f.cookie.clone()).await["title"], "code only");

    // The person renames; an owner of someone else's direct chat cannot.
    let patch = |c: Uuid, ck: String, body: Value| {
        let h = &h;
        async move {
            call(
                h,
                Method::PATCH,
                &format!("/v1/conversations/{c}"),
                ("cookie", &ck),
                Some(body),
            )
            .await
        }
    };
    let (s, v) = patch(conv, f.cookie.clone(), json!({"title": "  Release plan "})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["title"].as_str(), v["title_source"].as_str()),
        (Some("Release plan"), Some("person"))
    );
    let (s, e) = patch(conv, ocookie.clone(), json!({"title": "mine"})).await;
    assert_eq!(
        (s, e["error"]["code"].as_str()),
        (403, Some("direct_conversation_forbidden"))
    );
    for bad in [
        json!({"title": ""}),
        json!({"title": "a\u{7}b"}),
        json!({"title": "x".repeat(201)}),
    ] {
        let (s, e) = patch(conv, f.cookie.clone(), bad).await;
        assert_eq!(
            (s, e["error"]["code"].as_str()),
            (422, Some("invalid_title"))
        );
    }
    // `null` goes back to the auto title (the first person message).
    let (s, v) = patch(conv, f.cookie.clone(), json!({"title": null})).await;
    assert_eq!(s, 200);
    assert_eq!(
        (v["title"].as_str(), v["title_source"].as_str()),
        (Some("code only"), Some("auto"))
    );

    // A fresh chat titled by a long first line: 60 characters with the ellipsis.
    let (_, n) = new_direct(&h, &f).await;
    let n: Uuid = n["id"].as_str().unwrap().parse().unwrap();
    let (s, _) = msg(&h, &f, n, &long).await;
    assert_eq!(s, 202);
    let t = get(n, f.cookie.clone()).await["title"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        t, "Plan the release for team",
        "first line, markup and mention removed"
    );
    let (_, n2) = new_direct(&h, &f).await;
    let n2: Uuid = n2["id"].as_str().unwrap().parse().unwrap();
    let (s, _) = msg(&h, &f, n2, &"z".repeat(100)).await;
    assert_eq!(s, 202);
    let t2 = get(n2, f.cookie.clone()).await["title"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(t2.chars().count(), 60);
    assert!(t2.ends_with('…'));

    // A group conversation: owners rename, members cannot.
    let g = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at)
         VALUES ($1, $2, $3, 'group', 'thread:x', now())",
    )
    .bind(g)
    .bind(f.agent)
    .bind(f.ws)
    .execute(h.pool())
    .await
    .unwrap();
    let (s, e) = patch(g, f.cookie.clone(), json!({"title": "Ops"})).await;
    assert_eq!((s, e["error"]["code"].as_str()), (403, Some("forbidden")));
    let (s, v) = patch(g, ocookie.clone(), json!({"title": "Ops"})).await;
    assert_eq!((s, v["title"].as_str()), (200, Some("Ops")));

    // Audited with ids only.
    let meta: Vec<Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'conversation.rename' ORDER BY created_at",
    )
    .fetch_all(h.pool())
    .await
    .unwrap();
    assert_eq!(meta.len(), 3);
    assert!(
        meta.iter()
            .all(|m| !m.to_string().contains("Release") && !m.to_string().contains("Ops"))
    );
}
