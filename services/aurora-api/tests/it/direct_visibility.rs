//! SEC-43 (phase 1, RFE-009, D-47): a `direct` conversation, and everything that names it, is
//! visible only to its user (while they can use the workspace) and to workspace owners; only its
//! user writes. Docs: 04 §Conversation visibility, 05 §Conversation visibility, 10, 07.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

/// One identity: its session cookie and a workspace key and an org key of the same user.
struct Who {
    id: Uuid,
    cookie: String,
    ws_key: String,
    org_key: String,
}

struct Fx {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    gw: Uuid,
    /// The direct user.
    u: Who,
    /// Another plain member.
    m: Who,
    /// Workspace owner (binding `owner`).
    o: Who,
    /// Org admin (implicit owner, no binding).
    a: Who,
    /// The direct conversation of `u`.
    conv: Uuid,
}

async fn who(
    h: &Harness,
    org: Uuid,
    ws: Uuid,
    email: &str,
    org_role: &str,
    bind: Option<&str>,
) -> Who {
    let id = h.user(email, false).await;
    h.member(org, id, org_role, "active").await;
    if let Some(b) = bind {
        h.bind_ws(ws, id, b).await;
    }
    let (_, tok) = h.good_session(id).await;
    let (_, wk) = h.api_key(id, org, Some(ws), false).await;
    let (_, ok) = h.api_key(id, org, None, false).await;
    Who {
        id,
        cookie: cookie(&tok),
        ws_key: bearer(&wk),
        org_key: bearer(&ok),
    }
}

async fn fx() -> Fx {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    let gw = Uuid::now_v7();
    sqlx::query("INSERT INTO gateway_instances (id, hostname, version, started_at, last_seen_at) VALUES ($1,'h','0',now(),now())")
        .bind(gw).execute(h.pool()).await.unwrap();
    let u = who(&h, org, ws, "u@x.io", "member", Some("approver")).await;
    let m = who(&h, org, ws, "m@x.io", "member", Some("approver")).await;
    let o = who(&h, org, ws, "o@x.io", "member", Some("owner")).await;
    let a = who(&h, org, ws, "a@x.io", "admin", None).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            &format!("/v1/agents/{agent}/conversations/direct"),
            &[("cookie", &u.cookie), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let conv = v["id"].as_str().unwrap().parse().unwrap();
    Fx {
        h,
        org,
        ws,
        agent,
        gw,
        u,
        m,
        o,
        a,
        conv,
    }
}

impl Fx {
    async fn req(
        &self,
        m: Method,
        uri: &str,
        hdr: (&str, &str),
        body: Option<Value>,
    ) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[(hdr.0, hdr.1), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    /// Every credential of a person: session, workspace key, org key.
    fn creds(w: &Who) -> [(&'static str, &str); 3] {
        [
            ("cookie", w.cookie.as_str()),
            ("authorization", w.ws_key.as_str()),
            ("authorization", w.org_key.as_str()),
        ]
    }

    async fn group_conv(&self) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO conversations (id, workspace_id, agent_id, kind, external_ref, last_activity_at) VALUES ($1,$2,$3,'group',$4, now())")
            .bind(id).bind(self.ws).bind(self.agent).bind(format!("g:{id}"))
            .execute(self.h.pool()).await.unwrap();
        id
    }

    async fn approval(&self, conv: Option<Uuid>) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO approvals (id, org_id, workspace_id, agent_id, gateway_instance_id, conversation_id,
                method, host, path, summary, status, deadline)
             VALUES ($1,$2,$3,$4,$5,$6,'POST','api.example.com','/x',
                '{\"title\":\"T\",\"fields\":[],\"text\":null,\"link\":null,\"group\":null,\"source\":\"generic\"}',
                'pending', now() + interval '5 minutes')",
        )
        .bind(id).bind(self.org).bind(self.ws).bind(self.agent).bind(self.gw).bind(conv)
        .execute(self.h.pool()).await.unwrap();
        id
    }

    async fn event(&self) {
        let mut tx = self.h.pool().begin().await.unwrap();
        aurora_db::repo::transcript::append_durable_event(
            &mut tx,
            self.conv,
            None,
            "notice",
            &json!({"code": "x", "text": "secret dm text"}),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
}

fn ids(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect()
}

async fn stream_status(f: &Fx, conv: Uuid, hdr: (&str, &str)) -> u16 {
    let res =
        f.h.app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/v1/conversations/{conv}/stream"))
                    .header(hdr.0, hdr.1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
    let s = res.status().as_u16();
    if s == 200 {
        // Do not drain an open stream; just prove a frame-less body is fine to drop.
        let mut b = Body::new(res.into_body());
        let _ = tokio::time::timeout(Duration::from_millis(50), b.frame()).await;
    }
    s
}

#[tokio::test]
async fn read_routes_follow_visibility_for_every_credential() {
    let f = fx().await;
    f.event().await;
    let get = format!("/v1/conversations/{}", f.conv);
    let events = format!("/v1/conversations/{}/events", f.conv);
    // 200 for the direct user and for owners (workspace owner, org admin); 404 for a member.
    for (name, who, want) in [
        ("u", &f.u, 200),
        ("o", &f.o, 200),
        ("a", &f.a, 200),
        ("m", &f.m, 404),
    ] {
        for cred in Fx::creds(who) {
            let (s, v) = f.req(Method::GET, &get, cred, None).await;
            assert_eq!(s, want, "GET conversation as {name} {}: {v}", cred.0);
            let (s, v) = f.req(Method::GET, &events, cred, None).await;
            assert_eq!(s, want, "GET events as {name}: {v}");
            if want == 200 {
                assert_eq!(v["items"][0]["payload"]["text"], "secret dm text");
            } else {
                assert!(!v.to_string().contains("secret dm text"));
            }
            assert_eq!(
                stream_status(&f, f.conv, cred).await,
                want,
                "stream as {name}"
            );
        }
    }
}

#[tokio::test]
async fn other_kinds_stay_visible_to_every_member() {
    let f = fx().await;
    let g = f.group_conv().await;
    for who in [&f.u, &f.m, &f.o, &f.a] {
        let (s, _) = f
            .req(
                Method::GET,
                &format!("/v1/conversations/{g}"),
                ("cookie", &who.cookie),
                None,
            )
            .await;
        assert_eq!(s, 200);
    }
}

#[tokio::test]
async fn owners_cannot_post_or_upload_but_member_gets_404() {
    let f = fx().await;
    let post = format!("/v1/conversations/{}/messages", f.conv);
    let up = format!("/v1/conversations/{}/attachments", f.conv);
    for who in [&f.o, &f.a] {
        for cred in Fx::creds(who) {
            let (s, v) = f
                .req(
                    Method::POST,
                    &post,
                    cred,
                    Some(json!({"text": "hi as the user"})),
                )
                .await;
            assert_eq!(s, 403, "{v}");
            assert_eq!(v["error"]["code"], "direct_conversation_forbidden", "{v}");
        }
        let res = upload(&f, &up, &who.cookie).await;
        assert_eq!(res, 403);
    }
    for cred in Fx::creds(&f.m) {
        let (s, _) = f
            .req(Method::POST, &post, cred, Some(json!({"text": "hi"})))
            .await;
        assert_eq!(s, 404);
    }
    assert_eq!(upload(&f, &up, &f.m.cookie).await, 404);
    // Nothing was written.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE conversation_id = $1")
        .bind(f.conv)
        .fetch_one(f.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    // The direct user still can.
    let (s, v) = f
        .req(
            Method::POST,
            &post,
            ("cookie", &f.u.cookie),
            Some(json!({"text": "mine"})),
        )
        .await;
    assert_eq!(s, 202, "{v}");
}

async fn upload(f: &Fx, uri: &str, cookie: &str) -> u16 {
    let b = "XBOUNDARYX";
    let mut body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(b"x");
    body.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    let res =
        f.h.app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header("cookie", cookie)
                    .header("origin", ORIGIN)
                    .header("content-type", format!("multipart/form-data; boundary={b}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
    let s = res.status().as_u16();
    let _ = res.into_body().collect().await;
    s
}

#[tokio::test]
async fn owners_can_abort_and_send_now_but_members_get_404() {
    let f = fx().await;
    // A queued turn written by the direct user.
    let (s, v) = f
        .req(
            Method::POST,
            &format!("/v1/conversations/{}/messages", f.conv),
            ("cookie", &f.u.cookie),
            Some(json!({"text": "work"})),
        )
        .await;
    assert_eq!(s, 202, "{v}");
    let turn = v["turn"]["id"].as_str().unwrap().to_owned();
    let (s, _) = f
        .req(
            Method::POST,
            &format!("/v1/turns/{turn}/send-now"),
            ("cookie", &f.m.cookie),
            None,
        )
        .await;
    assert_eq!(s, 404);
    let (s, _) = f
        .req(
            Method::POST,
            &format!("/v1/turns/{turn}/abort"),
            ("cookie", &f.m.cookie),
            None,
        )
        .await;
    assert_eq!(s, 404);
    let (s, v) = f
        .req(
            Method::POST,
            &format!("/v1/turns/{turn}/send-now"),
            ("cookie", &f.o.cookie),
            None,
        )
        .await;
    assert_eq!(s, 202, "{v}");
    let (s, v) = f
        .req(
            Method::POST,
            &format!("/v1/turns/{turn}/abort"),
            ("cookie", &f.a.cookie),
            None,
        )
        .await;
    assert_eq!(s, 202, "{v}");
}

#[tokio::test]
async fn conversation_list_shows_owners_every_direct_conversation() {
    let f = fx().await;
    // The member has a direct conversation of their own too.
    let (s, v) = f
        .req(
            Method::POST,
            &format!("/v1/agents/{}/conversations/direct", f.agent),
            ("cookie", &f.m.cookie),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let mine = v["id"].as_str().unwrap().to_owned();
    let list = format!("/v1/agents/{}/conversations", f.agent);
    let (s, v) = f
        .req(Method::GET, &list, ("cookie", &f.m.cookie), None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(ids(&v), vec![mine.clone()], "a member sees only their own");
    assert!(v["items"][0]["direct_user"].is_null());
    let (_, v) = f
        .req(Method::GET, &list, ("cookie", &f.o.cookie), None)
        .await;
    let got = ids(&v);
    assert_eq!(got.len(), 2);
    for i in v["items"].as_array().unwrap() {
        assert_eq!(i["direct_user"]["id"], i["direct_user_id"], "{i}");
    }
    let (_, v) = f
        .req(Method::GET, &list, ("authorization", &f.a.org_key), None)
        .await;
    assert_eq!(ids(&v).len(), 2);
}

#[tokio::test]
async fn direct_user_who_lost_workspace_access_sees_nothing() {
    let f = fx().await;
    sqlx::query("DELETE FROM workspace_access WHERE workspace_id = $1 AND user_id = $2")
        .bind(f.ws)
        .bind(f.u.id)
        .execute(f.h.pool())
        .await
        .unwrap();
    for cred in Fx::creds(&f.u) {
        let (s, _) = f
            .req(
                Method::GET,
                &format!("/v1/conversations/{}", f.conv),
                cred,
                None,
            )
            .await;
        assert_eq!(s, 404);
    }
    let (s, _) = f
        .req(
            Method::GET,
            &format!("/v1/conversations/{}/events", f.conv),
            ("cookie", &f.u.cookie),
            None,
        )
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn approvals_naming_a_direct_conversation_are_hidden_from_other_members() {
    let f = fx().await;
    let dm = f.approval(Some(f.conv)).await;
    let plain = f.approval(None).await;
    let list = format!("/v1/workspaces/{}/approvals", f.ws);
    // History.
    for (who, want) in [(&f.u, 2), (&f.o, 2), (&f.a, 2), (&f.m, 1)] {
        let (s, v) = f
            .req(Method::GET, &list, ("cookie", &who.cookie), None)
            .await;
        assert_eq!(s, 200);
        let got = ids(&v);
        assert_eq!(got.len(), want, "{v}");
        assert!(got.contains(&plain.to_string()));
        assert_eq!(got.contains(&dm.to_string()), want == 2);
    }
    // Pending long-poll (wait=0) with every credential kind.
    for cred in Fx::creds(&f.m) {
        let (s, v) = f
            .req(Method::GET, "/v1/approvals/pending", cred, None)
            .await;
        assert_eq!(s, 200);
        let got = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(got, vec![plain.to_string()]);
    }
    for cred in Fx::creds(&f.o) {
        let (_, v) = f
            .req(Method::GET, "/v1/approvals/pending", cred, None)
            .await;
        assert_eq!(v["items"].as_array().unwrap().len(), 2);
    }
    // The long-poll of a member does not wake for, or return, a DM approval created meanwhile.
    sqlx::query("UPDATE approvals SET status = 'denied' WHERE id = $1")
        .bind(plain)
        .execute(f.h.pool())
        .await
        .unwrap();
    let (s, v) = f
        .req(
            Method::GET,
            "/v1/approvals/pending?wait=1",
            ("cookie", &f.m.cookie),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert!(v["items"].as_array().unwrap().is_empty(), "{v}");
    // Decide: 404 for a member, 200 for the direct user.
    let dec = format!("/v1/approvals/{dm}/decision");
    let (s, _) = f
        .req(
            Method::POST,
            &dec,
            ("cookie", &f.m.cookie),
            Some(json!({"decision": "approve"})),
        )
        .await;
    assert_eq!(s, 404);
    let (s, _) = f
        .req(
            Method::POST,
            &dec,
            ("authorization", &f.m.ws_key),
            Some(json!({"decision": "approve"})),
        )
        .await;
    assert_eq!(s, 404);
    let (s, v) = f
        .req(
            Method::POST,
            &dec,
            ("cookie", &f.o.cookie),
            Some(json!({"decision": "approve"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn approval_batch_decision_skips_invisible_direct_approvals() {
    let f = fx().await;
    let dm = f.approval(Some(f.conv)).await;
    sqlx::query("UPDATE approvals SET batch_id = 'b1' WHERE id = $1")
        .bind(dm)
        .execute(f.h.pool())
        .await
        .unwrap();
    let body = json!({"agent_id": f.agent, "decision": "deny"});
    let uri = "/v1/approvals/batches/b1/decision";
    let (s, _) = f
        .req(
            Method::POST,
            uri,
            ("cookie", &f.m.cookie),
            Some(body.clone()),
        )
        .await;
    assert_eq!(s, 404);
    let status: String = sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
        .bind(dm)
        .fetch_one(f.h.pool())
        .await
        .unwrap();
    assert_eq!(status, "pending");
    let (s, v) = f
        .req(Method::POST, uri, ("cookie", &f.u.cookie), Some(body))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["decided"], 1);
}

#[tokio::test]
async fn approvals_sse_omits_invisible_direct_approvals() {
    let f = fx().await;
    let open = |cookie: String| {
        let app = f.h.app.clone();
        async move {
            let res = app
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/v1/approvals/stream")
                        .header("cookie", cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), 200);
            Body::new(res.into_body())
        }
    };
    let mut member = open(f.m.cookie.clone()).await;
    let mut owner = open(f.o.cookie.clone()).await;
    let dm = f.approval(Some(f.conv)).await;
    aurora_db::bus::notify(
        f.h.pool(),
        aurora_db::bus::Channel::Approval,
        &json!({"id": dm, "workspace_id": f.ws, "event": "created"}),
    )
    .await
    .unwrap();
    async fn text_within(b: &mut Body, wait: Duration) -> String {
        let mut out = String::new();
        let deadline = tokio::time::Instant::now() + wait;
        while let Ok(Some(Ok(fr))) = tokio::time::timeout_at(deadline, b.frame()).await {
            if let Some(d) = fr.data_ref() {
                out.push_str(&String::from_utf8_lossy(d));
            }
            if out.contains("approval") {
                break;
            }
        }
        out
    }
    let o = text_within(&mut owner, Duration::from_secs(5)).await;
    assert!(o.contains(&dm.to_string()), "owner must receive it: {o}");
    let m = text_within(&mut member, Duration::from_millis(800)).await;
    assert!(!m.contains(&dm.to_string()), "member must not: {m}");
}

#[tokio::test]
async fn activity_rows_of_invisible_direct_conversations_are_omitted() {
    let f = fx().await;
    for conv in [Some(f.conv), None] {
        sqlx::query(
            "INSERT INTO request_log (id, ts, org_id, workspace_id, agent_id, conversation_id, credential_kind,
                lane, method, host, path, decision, status, latency_ms)
             VALUES ($1, now(), $2,$3,$4,$5,'agent','mitm','GET','a.com','/p','allow',200,5)",
        )
        .bind(Uuid::now_v7()).bind(f.org).bind(f.ws).bind(f.agent).bind(conv)
        .execute(f.h.pool()).await.unwrap();
    }
    let uri = format!("/v1/workspaces/{}/activity", f.ws);
    for (who, want) in [(&f.u, 2), (&f.o, 2), (&f.a, 2), (&f.m, 1)] {
        for cred in Fx::creds(who) {
            let (s, v) = f.req(Method::GET, &uri, cred, None).await;
            assert_eq!(s, 200);
            assert_eq!(v["items"].as_array().unwrap().len(), want, "{v}");
        }
    }
    // Filtering by the conversation cannot reveal it either.
    let (_, v) = f
        .req(
            Method::GET,
            &format!("{uri}?conversation_id={}", f.conv),
            ("cookie", &f.m.cookie),
            None,
        )
        .await;
    assert!(v["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn schedule_and_peer_task_references_are_redacted() {
    let f = fx().await;
    let sched = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone, enabled,
            origin_conversation_id)
         VALUES ($1,$2,'s','p','0 * * * *','UTC',true,$3)",
    )
    .bind(sched)
    .bind(f.agent)
    .bind(f.conv)
    .execute(f.h.pool())
    .await
    .unwrap();
    let uri = format!("/v1/schedules/{sched}");
    let (s, v) = f
        .req(Method::GET, &uri, ("cookie", &f.m.cookie), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["origin_conversation_id"].is_null());
    assert_eq!(v["conversation_hidden"], true);
    let (_, v) = f
        .req(Method::GET, &uri, ("cookie", &f.o.cookie), None)
        .await;
    assert_eq!(v["origin_conversation_id"], f.conv.to_string());
    assert_eq!(v["conversation_hidden"], false);
    let (_, v) = f
        .req(
            Method::GET,
            &format!("/v1/agents/{}/schedules", f.agent),
            ("cookie", &f.m.cookie),
            None,
        )
        .await;
    assert_eq!(v["items"][0]["conversation_hidden"], true);
}

#[tokio::test]
async fn owner_reads_write_one_audit_row_per_hour_and_own_reads_none() {
    let f = fx().await;
    let uri = format!("/v1/conversations/{}", f.conv);
    for _ in 0..3 {
        let (s, _) = f
            .req(Method::GET, &uri, ("cookie", &f.o.cookie), None)
            .await;
        assert_eq!(s, 200);
        let (s, _) = f
            .req(
                Method::GET,
                &format!("{uri}/events"),
                ("cookie", &f.o.cookie),
                None,
            )
            .await;
        assert_eq!(s, 200);
        let (s, _) = f
            .req(Method::GET, &uri, ("cookie", &f.u.cookie), None)
            .await;
        assert_eq!(s, 200);
        let (s, _) = f
            .req(Method::GET, &uri, ("cookie", &f.m.cookie), None)
            .await;
        assert_eq!(s, 404);
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let rows: Vec<(Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT actor_id, target_id FROM audit_log WHERE action = 'conversation.owner_view'",
    )
    .fetch_all(f.h.pool())
    .await
    .unwrap();
    assert_eq!(rows, vec![(Some(f.o.id), Some(f.conv.to_string()))]);
    // A different owner gets their own row.
    let (s, _) = f
        .req(Method::GET, &uri, ("cookie", &f.a.cookie), None)
        .await;
    assert_eq!(s, 200);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'conversation.owner_view'",
    )
    .fetch_one(f.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 2);
}

#[tokio::test]
async fn can_see_conversation_predicate_matches_the_sql_fragment() {
    use aurora_db::authz;
    let f = fx().await;
    let g = f.group_conv().await;
    for (who, dm) in [(&f.u, true), (&f.m, false), (&f.o, true), (&f.a, true)] {
        assert_eq!(
            authz::can_see_conversation_id(f.h.pool(), who.id, f.conv)
                .await
                .unwrap(),
            dm
        );
        assert!(
            authz::can_see_conversation_id(f.h.pool(), who.id, g)
                .await
                .unwrap()
        );
    }
    let hidden = authz::hidden_conversations(f.h.pool(), f.m.id, &[f.conv, g])
        .await
        .unwrap();
    assert_eq!(hidden, vec![f.conv]);
    // A suspended owner sees nothing.
    f.h.set_member_status(f.org, f.o.id, "suspended").await;
    assert!(
        !authz::can_see_conversation_id(f.h.pool(), f.o.id, f.conv)
            .await
            .unwrap()
    );
}

/// Clarification C of D-47: an action approval in another workspace than its (direct)
/// conversation is decided under that workspace's own law, but its decider never learns the
/// conversation id, the turn id or the frozen payload text.
#[tokio::test]
async fn cross_workspace_decider_cannot_read_the_dm_id_or_payload() {
    let f = fx().await;
    let ws2 = f.h.workspace(f.org).await;
    let q = who(&f.h, f.org, ws2, "q@x.io", "member", Some("approver")).await;
    let turn = Uuid::now_v7();
    let id = Uuid::now_v7();
    let secret = "SECRET-DM-TEXT-42";
    sqlx::query(
        "INSERT INTO action_approvals (id, workspace_id, agent_id, conversation_id, action_key, payload,
            summary, status, expires_at)
         VALUES ($1,$2,$3,$4,'no_such_action',$5,'Agent alpha wants to talk','pending', now() + interval '1 hour')",
    )
    .bind(id)
    .bind(ws2)
    .bind(f.agent)
    .bind(f.conv)
    .bind(json!({"text": secret, "turn": turn}))
    .execute(f.h.pool())
    .await
    .unwrap();
    let conv = f.conv.to_string();
    let clean = |what: &str, v: &Value| {
        let t = v.to_string();
        assert!(!t.contains(&conv), "{what} leaks the conversation id: {t}");
        assert!(!t.contains(secret), "{what} leaks the payload: {t}");
    };
    for cred in Fx::creds(&q) {
        let (s, v) = f
            .req(
                Method::GET,
                &format!("/v1/workspaces/{ws2}/action-approvals"),
                cred,
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(
            v["items"].as_array().unwrap().len(),
            1,
            "decidable side stays visible: {v}"
        );
        assert!(v["items"][0]["conversation_id"].is_null());
        assert_eq!(v["items"][0]["conversation_hidden"], true);
        clean("list", &v);
        let (_, v) = f
            .req(
                Method::GET,
                &format!("/v1/workspaces/{ws2}/activity"),
                cred,
                None,
            )
            .await;
        clean("activity", &v);
        let (s, v) = f
            .req(
                Method::GET,
                &format!("/v1/conversations/{conv}"),
                cred,
                None,
            )
            .await;
        // 404, or 403 `workspace_mismatch` for a workspace key confined to ws2.
        assert!(s == 404 || s == 403, "{s}");
        clean("conversation", &v);
    }
    // The summary is the only content; deciding works and its response is clean too.
    let (s, v) = f
        .req(
            Method::POST,
            &format!("/v1/action-approvals/{id}/decision"),
            ("cookie", &q.cookie),
            Some(json!({"decision": "deny"})),
        )
        .await;
    assert_ne!(s, 404, "{v}");
    clean("decision", &v);
    let (_, v) = f
        .req(
            Method::GET,
            &format!("/v1/workspaces/{ws2}/action-approvals"),
            ("cookie", &q.cookie),
            None,
        )
        .await;
    clean("list after decision", &v);
}
