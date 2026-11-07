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
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
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
    sqlx::query_scalar(sql)
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

    let (s, t) = call(
        &h,
        Method::POST,
        &format!("/v1/turns/{turn}/send-now"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!((s, t["send_now_requested"].clone()), (202, json!(true)));

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
    assert_eq!(t["abort_requested"], true);
    assert!(!t["finished_at"].is_null());
    let (_, ev) = call(
        &h,
        Method::GET,
        &format!("/v1/conversations/{conv}/events"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    let last = ev["items"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["type"], "turn.finished");
    assert_eq!(last["payload"]["status"], "aborted");
    assert_eq!(last["payload"]["turn_id"], turn);

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
    let (s, _) = call(
        &h,
        Method::POST,
        &format!("/v1/turns/{turn}/send-now"),
        ("cookie", &f.cookie),
        None,
    )
    .await;
    assert_eq!(s, 409);
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
