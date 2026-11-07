#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_db::bus::{BusEvent, Channel, Listener};
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

pub struct Fx {
    pub org: Uuid,
    pub ws: Uuid,
    pub agent: Uuid,
    pub gw: Uuid,
}

pub async fn fixture(h: &Harness) -> Fx {
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = agent(h, org, ws, "bot").await;
    let gw = Uuid::now_v7();
    sqlx::query("INSERT INTO gateway_instances (id, hostname, version, started_at, last_seen_at) VALUES ($1,'h','0',now(),now())")
        .bind(gw).execute(h.pool()).await.unwrap();
    Fx { org, ws, agent, gw }
}

pub async fn agent(h: &Harness, org: Uuid, ws: Uuid, handle: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,$4,'Bot','byo','medium','')")
        .bind(id).bind(org).bind(ws).bind(handle).execute(h.pool()).await.unwrap();
    id
}

pub async fn approval(h: &Harness, f: &Fx, ws: Uuid, agent: Uuid, batch: Option<&str>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, gateway_instance_id, method, host, path,
            summary, batch_id, status, deadline)
         VALUES ($1,$2,$3,$4,$5,'POST','api.example.com','/x',
            '{\"title\":\"T\",\"fields\":[{\"label\":\"a\",\"value\":\"b\"}],\"text\":null,\"link\":null,\"group\":null,\"source\":\"generic\"}',
            $6,'pending', now() + interval '5 minutes')",
    )
    .bind(id).bind(f.org).bind(ws).bind(agent).bind(f.gw).bind(batch)
    .execute(h.pool()).await.unwrap();
    id
}

async fn notify(h: &Harness, id: Uuid, ws: Uuid, ev: &str) {
    aurora_db::bus::notify(
        h.pool(),
        Channel::Approval,
        &json!({"id": id, "workspace_id": ws, "event": ev}),
    )
    .await
    .unwrap();
}

async fn post(h: &Harness, uri: &str, auth: (&str, &str), body: Value) -> (u16, Value) {
    let (s, _, v) = h
        .send(
            Method::POST,
            uri,
            &[(auth.0, auth.1), ("origin", ORIGIN)],
            Some(body),
        )
        .await;
    (s.as_u16(), v)
}

#[tokio::test]
async fn decide_first_wins_authz_and_notify() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let member = h.user("m@x.io", false).await;
    let outsider = h.user("o@x.io", false).await;
    let unbound = h.user("u@x.io", false).await;
    h.member(f.org, member, "member", "active").await;
    h.member(f.org, unbound, "member", "active").await;
    h.bind_ws(f.ws, member, "approver").await;
    let (_, mt) = h.good_session(member).await;
    let (_, ot) = h.good_session(outsider).await;
    let (_, ut) = h.good_session(unbound).await;
    let (_, key) = h.api_key(member, f.org, Some(f.ws), false).await;
    let a1 = approval(&h, &f, f.ws, f.agent, None).await;
    let a2 = approval(&h, &f, f.ws, f.agent, None).await;

    let mut l = Listener::connect(h.pool(), &[Channel::Approval])
        .await
        .unwrap();
    let c = cookie(&mt);
    let uri = format!("/v1/approvals/{a1}/decision");
    let (s, v) = post(&h, &uri, ("cookie", &c), json!({"decision":"approve"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "approved");
    assert_eq!(v["decided_via"], "dashboard");
    assert_eq!(v["decided_by"]["id"], json!(member));
    assert_eq!(v["agent"]["handle"], "bot");
    let got = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let BusEvent::Notification { payload, .. } = l.next().await {
                break payload;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(got["event"], "decided");
    assert_eq!(got["id"], json!(a1));

    let (s, v) = post(&h, &uri, ("cookie", &c), json!({"decision":"deny"})).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("already_decided"))
    );

    // non-member and unbound member: 404
    let oc = cookie(&ot);
    let uri2 = format!("/v1/approvals/{a2}/decision");
    let (s, _) = post(&h, &uri2, ("cookie", &oc), json!({"decision":"deny"})).await;
    assert_eq!(s, 404);
    let uc = cookie(&ut);
    let (s, _) = post(&h, &uri2, ("cookie", &uc), json!({"decision":"deny"})).await;
    assert_eq!(s, 404);
    let (s, _) = post(
        &h,
        "/v1/approvals/00000000-0000-7000-8000-000000000000/decision",
        ("cookie", &c),
        json!({"decision":"deny"}),
    )
    .await;
    assert_eq!(s, 404);

    // bearer key of the member
    let b = bearer(&key);
    let (s, v) = post(&h, &uri2, ("authorization", &b), json!({"decision":"deny"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["status"].as_str(), v["decided_via"].as_str()),
        (Some("denied"), Some("api"))
    );
}

#[tokio::test]
async fn batch_decides_only_agents_rows_caller_may_decide() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let ws2 = h.workspace(f.org).await;
    let other_agent = agent(&h, f.org, f.ws, "other").await;
    let agent2 = agent(&h, f.org, ws2, "bot").await;
    let member = h.user("m@x.io", false).await;
    h.member(f.org, member, "member", "active").await;
    h.bind_ws(f.ws, member, "approver").await;
    let (_, mt) = h.good_session(member).await;
    let mine1 = approval(&h, &f, f.ws, f.agent, Some("b1")).await;
    let mine2 = approval(&h, &f, f.ws, f.agent, Some("b1")).await;
    let not_mine = approval(&h, &f, ws2, agent2, Some("b1")).await;
    let other = approval(&h, &f, f.ws, other_agent, Some("b1")).await;
    // Same agent id, ws2 unbound: not decidable.
    sqlx::query("UPDATE approvals SET agent_id = $2 WHERE id = $1")
        .bind(not_mine)
        .bind(f.agent)
        .execute(h.pool())
        .await
        .unwrap();

    let c = cookie(&mt);
    let (s, v) = post(
        &h,
        "/v1/approvals/batches/b1/decision",
        ("cookie", &c),
        json!({"decision":"approve","agent_id":f.agent}),
    )
    .await;
    assert_eq!((s, v["decided"].as_u64()), (200, Some(2)), "{v}");
    let status = |id: Uuid| {
        let p = h.pool().clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT status FROM approvals WHERE id=$1")
                .bind(id)
                .fetch_one(&p)
                .await
                .unwrap()
        }
    };
    assert_eq!(status(mine1).await, "approved");
    assert_eq!(status(mine2).await, "approved");
    assert_eq!(status(not_mine).await, "pending");
    assert_eq!(status(other).await, "pending");
    // Repeat: nothing pending left -> 0
    let (s, v) = post(
        &h,
        "/v1/approvals/batches/b1/decision",
        ("cookie", &c),
        json!({"decision":"deny","agent_id":f.agent}),
    )
    .await;
    assert_eq!((s, v["decided"].as_u64()), (200, Some(0)));
    let (s, _) = post(
        &h,
        "/v1/approvals/batches/nope/decision",
        ("cookie", &c),
        json!({"decision":"deny","agent_id":f.agent}),
    )
    .await;
    assert_eq!(s, 404);
    let (s, _) = post(
        &h,
        "/v1/approvals/batches/b1/decision",
        ("cookie", &c),
        json!({"decision":"deny"}),
    )
    .await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn list_filters_and_pagination() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let member = h.user("m@x.io", false).await;
    h.member(f.org, member, "member", "active").await;
    h.bind_ws(f.ws, member, "approver").await;
    let (_, mt) = h.good_session(member).await;
    let c = cookie(&mt);
    let mut ids = vec![];
    for _ in 0..3 {
        ids.push(approval(&h, &f, f.ws, f.agent, None).await);
    }
    sqlx::query("UPDATE approvals SET status='expired' WHERE id=$1")
        .bind(ids[0])
        .execute(h.pool())
        .await
        .unwrap();
    let (s, v) = h
        .get(
            &format!("/v1/workspaces/{}/approvals?status=pending", f.ws),
            &[("cookie", &c)],
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let (_, v) = h
        .get(
            &format!("/v1/workspaces/{}/approvals?limit=2", f.ws),
            &[("cookie", &c)],
        )
        .await;
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let cur = v["next_cursor"].as_str().unwrap().to_owned();
    let (_, v2) = h
        .get(
            &format!("/v1/workspaces/{}/approvals?limit=2&cursor={cur}", f.ws),
            &[("cookie", &c)],
        )
        .await;
    assert_eq!(v2["items"].as_array().unwrap().len(), 1);
    assert!(v2["next_cursor"].is_null());
    let (s, _) = h
        .get(
            &format!("/v1/workspaces/{}/approvals?status=bogus", f.ws),
            &[("cookie", &c)],
        )
        .await;
    assert_eq!(s, 400);
    let (_, ot) = h.good_session(h.user("z@x.io", false).await).await;
    let oc = cookie(&ot);
    let (s, _) = h
        .get(
            &format!("/v1/workspaces/{}/approvals", f.ws),
            &[("cookie", &oc)],
        )
        .await;
    assert_eq!(s, 404);
    // pending (no wait) only for decidable
    let (_, v) = h.get("/v1/approvals/pending", &[("cookie", &c)]).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let (_, v) = h.get("/v1/approvals/pending", &[("cookie", &oc)]).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn long_poll_returns_early_and_times_out_empty() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let member = h.user("m@x.io", false).await;
    h.member(f.org, member, "member", "active").await;
    h.bind_ws(f.ws, member, "approver").await;
    let (_, mt) = h.good_session(member).await;
    let c = cookie(&mt);

    let started = std::time::Instant::now();
    let (s, v) = h
        .get("/v1/approvals/pending?wait=1", &[("cookie", &c)])
        .await;
    assert_eq!(s, 200);
    assert!(v["items"].as_array().unwrap().is_empty());
    assert!(started.elapsed() >= Duration::from_millis(900));

    let started = std::time::Instant::now();
    let hd = [("cookie", c.as_str())];
    let poll = h.get("/v1/approvals/pending?wait=20", &hd);
    let creator = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let id = approval(&h, &f, f.ws, f.agent, None).await;
        notify(&h, id, f.ws, "created").await;
        id
    };
    let ((s, v), id) = tokio::join!(poll, creator);
    assert_eq!(s, 200);
    assert_eq!(v["items"][0]["id"], json!(id));
    assert!(started.elapsed() < Duration::from_secs(10));
}

async fn next_text(body: &mut axum::body::Body, secs: u64) -> String {
    let frame = tokio::time::timeout(Duration::from_secs(secs), body.frame())
        .await
        .expect("frame in time")
        .expect("body open")
        .unwrap();
    String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap()
}

#[tokio::test]
async fn sse_emits_created_decided_and_keepalive() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let member = h.user("m@x.io", false).await;
    let stranger = h.user("s@x.io", false).await;
    h.member(f.org, member, "member", "active").await;
    h.bind_ws(f.ws, member, "approver").await;
    let (_, mt) = h.good_session(member).await;
    let (_, st) = h.good_session(stranger).await;
    let (_, key) = h.api_key(member, f.org, Some(f.ws), false).await;
    let state = h
        .state
        .clone()
        .with_sse_keepalive(Duration::from_millis(200));
    let app = test_router(state);
    let req = |tok: &str| {
        Request::builder()
            .uri("/v1/approvals/stream")
            .header("cookie", cookie(tok))
            .body(axum::body::Body::empty())
            .unwrap()
    };
    // API keys are refused (session only)
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/approvals/stream")
                .header("authorization", bearer(&key))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    let res = app.clone().oneshot(req(&mt)).await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(
        res.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let mut body = res.into_body();
    let res2 = app.clone().oneshot(req(&st)).await.unwrap();
    let mut stranger_body = res2.into_body();

    let id = approval(&h, &f, f.ws, f.agent, None).await;
    notify(&h, id, f.ws, "created").await;
    let mut seen = String::new();
    while !seen.contains("event: approval") {
        seen.push_str(&next_text(&mut body, 5).await);
    }
    assert!(seen.contains("\"event\":\"created\""), "{seen}");
    assert!(!seen.contains("\nid:"), "no SSE ids (C-10)");

    let c = cookie(&mt);
    let (s, _) = post(
        &h,
        &format!("/v1/approvals/{id}/decision"),
        ("cookie", &c),
        json!({"decision":"deny"}),
    )
    .await;
    assert_eq!(s, 200);
    let mut seen = String::new();
    while !seen.contains("\"event\":\"decided\"") {
        seen.push_str(&next_text(&mut body, 5).await);
    }
    assert!(seen.contains("\"status\":\"denied\""), "{seen}");

    // Keepalive comment arrives on an idle stream.
    let mut seen = String::new();
    while !seen.contains(':') {
        seen.push_str(&next_text(&mut body, 5).await);
    }
    // The stranger never sees approval events, only keepalives.
    let mut txt = String::new();
    for _ in 0..3 {
        txt.push_str(&next_text(&mut stranger_body, 5).await);
    }
    assert!(!txt.contains("event: approval"), "{txt}");
}

/// SEC-32: an approval decision is audited.
#[tokio::test]
async fn sec32_approval_decision_is_audited() {
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let member = h.user("m@x.io", false).await;
    h.member(f.org, member, "member", "active").await;
    h.bind_ws(f.ws, member, "approver").await;
    let (_, mt) = h.good_session(member).await;
    let a1 = approval(&h, &f, f.ws, f.agent, None).await;
    let c = cookie(&mt);
    let (s, v) = post(
        &h,
        &format!("/v1/approvals/{a1}/decision"),
        ("cookie", &c),
        json!({"decision":"approve"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let mut n = 0i64;
    for _ in 0..100 {
        n = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='approval.decide'")
            .fetch_one(h.pool())
            .await
            .unwrap();
        if n > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(n, 1);
}

#[tokio::test]
async fn member_cannot_decide_or_see_approvals_approver_can() {
    // RFE-012 (D-55, SEC-28): `can_use` is not enough; `can_approve` is.
    let h = Harness::new().await;
    let f = fixture(&h).await;
    let member = h.user("m@x.io", false).await;
    let approver = h.user("a@x.io", false).await;
    let owner = h.user("o@x.io", false).await;
    let admin = h.user("ad@x.io", false).await;
    for (u, role) in [(member, "member"), (approver, "member"), (owner, "member")] {
        h.member(f.org, u, role, "active").await;
    }
    h.member(f.org, admin, "admin", "active").await;
    h.bind_ws(f.ws, member, "member").await;
    h.bind_ws(f.ws, approver, "approver").await;
    h.bind_ws(f.ws, owner, "owner").await;
    let ck = |u: Uuid| {
        let h = &h;
        async move { cookie(&h.good_session(u).await.1) }
    };
    let (mc, ac, oc, adc) = (
        ck(member).await,
        ck(approver).await,
        ck(owner).await,
        ck(admin).await,
    );
    let batch1 = approval(&h, &f, f.ws, f.agent, Some("bb")).await;
    let single = approval(&h, &f, f.ws, f.agent, None).await;

    // Member: 403 approver_required on decide, list and batch; feed is empty.
    let (s, v) = post(
        &h,
        &format!("/v1/approvals/{single}/decision"),
        ("cookie", &mc),
        json!({"decision":"approve"}),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("approver_required")),
        "{v}"
    );
    let (s, v) = h
        .get(
            &format!("/v1/workspaces/{}/approvals", f.ws),
            &[("cookie", &mc)],
        )
        .await;
    assert_eq!(
        (s.as_u16(), v["error"]["code"].as_str()),
        (403, Some("approver_required"))
    );
    let (s, v) = post(
        &h,
        "/v1/approvals/batches/bb/decision",
        ("cookie", &mc),
        json!({"decision":"approve","agent_id":f.agent}),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (403, Some("approver_required")),
        "{v}"
    );
    let (s, v) = h.get("/v1/approvals/pending", &[("cookie", &mc)]).await;
    assert_eq!(
        (s.as_u16(), v["items"].as_array().map(Vec::len)),
        (200, Some(0))
    );
    let st = |id: Uuid| {
        let p = h.pool().clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT status FROM approvals WHERE id=$1")
                .bind(id)
                .fetch_one(&p)
                .await
                .unwrap()
        }
    };
    assert_eq!(st(single).await, "pending");
    assert_eq!(st(batch1).await, "pending");

    // Approver, owner and org admin see the feed and can decide.
    for c in [&ac, &oc, &adc] {
        let (s, v) = h.get("/v1/approvals/pending", &[("cookie", c)]).await;
        assert_eq!(s.as_u16(), 200);
        assert_eq!(v["items"].as_array().unwrap().len(), 2, "{v}");
    }
    let (s, v) = post(
        &h,
        &format!("/v1/approvals/{single}/decision"),
        ("cookie", &ac),
        json!({"decision":"approve"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");

    // Role change applies on the next request: promote the member.
    sqlx::query(
        "UPDATE workspace_access SET role = 'approver' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(f.ws)
    .bind(member)
    .execute(h.pool())
    .await
    .unwrap();
    let (s, v) = post(
        &h,
        "/v1/approvals/batches/bb/decision",
        ("cookie", &mc),
        json!({"decision":"deny","agent_id":f.agent}),
    )
    .await;
    assert_eq!((s, v["decided"].as_u64()), (200, Some(1)), "{v}");
}
