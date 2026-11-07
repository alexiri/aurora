#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-052: a fake runner drives the HTTP plane through a whole hosted turn.
mod common;

use axum::body::Body;
use base64::Engine;
use common::*;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Rig {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    tok: String,
    cookie: String,
    rep: Uuid,
}

struct Live {
    agent: Uuid,
    conv: Uuid,
    sb: String,
    boot: String,
}

fn caps() -> Value {
    json!({
        "backend": "fake", "home_durability": "resident",
        "resources": {"cpu": {"min": 250, "max": 4000}, "memory_mib": {"min": 256, "max": 8192},
                      "pids": {"min": 64, "max": 4096}, "home_gib": {"min": 1, "max": 100}},
        "max_sandboxes": 4, "harnesses": ["fake"], "max_attachment_bytes": 1048576,
        "image": "registry.example/aurora-sandbox:9.9.9"
    })
}

async fn rig() -> Rig {
    let dir = std::env::temp_dir().join(format!("aurora-ca-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ca.pem"), "-----BEGIN CERTIFICATE-----\nX\n").unwrap();
    let d = dir.to_string_lossy().into_owned();
    let h = Harness::with_config(move |c| {
        c.gateway_ca_dir = d;
        c.llm.endpoints.push(aurora_config::LlmEndpoint {
            id: "anthropic".into(),
            provider: "anthropic".into(),
            default_model: Some("claude-test".into()),
            ..Default::default()
        });
    })
    .await;
    let admin = h.user("admin@x.io", true).await;
    let (_, sess) = h.good_session(admin).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/runners",
            &[("cookie", &cookie(&sess)), ("origin", ORIGIN)],
            Some(json!({"name": "r1"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let tok = v["token"].as_str().unwrap().to_owned();
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let user = h.user("m@x.io", false).await;
    h.member(org, user, "member", "active").await;
    h.bind_ws(ws, user, "member").await;
    let (_, st) = h.good_session(user).await;
    let mut r = Rig {
        h,
        org,
        ws,
        tok,
        cookie: cookie(&st),
        rep: Uuid::nil(),
    };
    let (s, v) = r
        .runner(
            None,
            "/v1/runner/register",
            json!({"version": "0.1", "backend": "fake", "capabilities": caps(), "advertised_host": "runner-a"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    r.rep = v["replica_id"].as_str().unwrap().parse().unwrap();
    r
}

impl Rig {
    async fn runner(&self, replica: Option<Uuid>, uri: &str, body: Value) -> (u16, Value) {
        let b = bearer(&self.tok);
        let rid = replica.map(|r| r.to_string());
        let mut hd = vec![("authorization", b.as_str())];
        if let Some(r) = &rid {
            hd.push(("x-aurora-replica", r.as_str()));
        }
        let (s, _, v) = self.h.send(Method::POST, uri, &hd, Some(body)).await;
        (s.as_u16(), v)
    }

    async fn work(&self) -> Vec<Value> {
        let (s, v) = self
            .runner(
                Some(self.rep),
                "/v1/runner/work",
                json!({"wait_secs": 0, "capacity": {"running": 0, "max": 4}}),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["items"].as_array().unwrap().clone()
    }

    async fn report(&self, lv: &Live, body: Value) -> String {
        self.report_on(&self.h.app, lv, body).await
    }

    async fn report_on(&self, app: &axum::Router, lv: &Live, mut body: Value) -> String {
        body["report_id"] = json!(Uuid::now_v7());
        body["sandbox_id"] = json!(lv.sb);
        body["boot_id"] = json!(lv.boot);
        let rid = self.rep.to_string();
        let b = bearer(&self.tok);
        let req = Request::builder()
            .method(Method::POST)
            .uri("/v1/runner/reports")
            .header("authorization", b)
            .header("x-aurora-replica", rid)
            .header("content-type", "application/json")
            .body(Body::from(json!({"reports": [body]}).to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        v["results"][0]["status"]
            .as_str()
            .unwrap_or_else(|| panic!("{v}"))
            .to_owned()
    }

    async fn user_call(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(
                m,
                uri,
                &[("cookie", &self.cookie), ("origin", ORIGIN)],
                body,
            )
            .await;
        (s.as_u16(), v)
    }

    async fn agent(&self, handle: &str, grant: bool) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness) VALUES ($1,$2,$3,$4,$4,'hosted','fake')")
            .bind(id).bind(self.org).bind(self.ws).bind(handle).execute(self.h.pool()).await.unwrap();
        if grant {
            let sec = Uuid::now_v7();
            sqlx::query("INSERT INTO secrets (id, org_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id, host_pattern, injection, value_source, value_ref) VALUES ($1,$2,$3,'llm','anthropic','api_key','anthropic','api.anthropic.com','{}','managed','x')")
                .bind(sec).bind(self.org).bind(format!("k-{handle}")).execute(self.h.pool()).await.unwrap();
            sqlx::query(
                "INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)",
            )
            .bind(Uuid::now_v7())
            .bind(self.ws)
            .bind(id)
            .bind(sec)
            .execute(self.h.pool())
            .await
            .unwrap();
        }
        id
    }

    async fn direct(&self, agent: Uuid) -> Uuid {
        let (s, v) = self
            .user_call(
                Method::POST,
                &format!("/v1/agents/{agent}/conversations/direct"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["id"].as_str().unwrap().parse().unwrap()
    }

    async fn say(&self, conv: Uuid, text: &str) -> Value {
        let (s, v) = self
            .user_call(
                Method::POST,
                &format!("/v1/conversations/{conv}/messages"),
                Some(json!({"text": text})),
            )
            .await;
        assert_eq!(s, 202, "{v}");
        v
    }

    /// Message, start, running, channel, ready, home sync; returns after `home.synced`.
    async fn boot(&self, handle: &str) -> Live {
        let agent = self.agent(handle, true).await;
        let conv = self.direct(agent).await;
        self.say(conv, "hello").await;
        let items = self.work().await;
        let start = items.iter().find(|i| i["kind"] == "sandbox.start").unwrap();
        let (sb, boot) = (
            start["sandbox_id"].as_str().unwrap().to_owned(),
            start["boot_id"].as_str().unwrap().to_owned(),
        );
        assert_eq!(
            start["spec"]["image"],
            "registry.example/aurora-sandbox:9.9.9"
        );
        let raw = start["spec"]["boot_secret"].as_str().unwrap();
        let doc: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(raw)
                .unwrap(),
        )
        .unwrap();
        let lv = Live {
            agent,
            conv,
            sb,
            boot,
        };
        assert_eq!(
            self.report(
                &lv,
                json!({"kind": "sandbox.status", "state": "running", "container_ref": "c"})
            )
            .await,
            "applied"
        );
        let (s, _) = self
            .runner(
                Some(self.rep),
                "/v1/runner/channel/claim",
                json!({"bootstrap_token": doc["bootstrap_token"]}),
            )
            .await;
        assert_eq!(s, 200);
        let (s, _) = self
            .runner(
                Some(self.rep),
                "/v1/runner/channel/hold",
                json!({"sandbox_id": lv.sb, "boot_id": lv.boot}),
            )
            .await;
        assert_eq!(s, 200);
        // No dispatch before ready + home sync.
        assert!(
            self.work()
                .await
                .iter()
                .all(|i| i["kind"] != "turn.deliver")
        );
        self.report(
            &lv,
            json!({"kind": "supervisor.ready", "version": "0.1",
                "harness": {"id": "fake", "capabilities": ["steer", "resume"]}}),
        )
        .await;
        let items = self.work().await;
        let parts: Vec<&Value> = items.iter().filter(|i| i["kind"] == "home.sync").collect();
        assert!(!parts.is_empty());
        assert!(items.iter().all(|i| i["kind"] != "turn.deliver"));
        let last = parts.last().unwrap();
        assert_eq!(last["part"]["kind"], "final");
        assert_eq!(last["part_index"].as_u64().unwrap() + 1, last["part_count"]);
        let generation_n = last["generation"].as_u64().unwrap();
        assert!(!serde_json::to_string(&items).unwrap().contains("auc_"));
        assert_eq!(
            self.report(
                &lv,
                json!({"kind": "home.synced", "generation": generation_n})
            )
            .await,
            "applied"
        );
        lv
    }

    async fn deliver(&self) -> Value {
        let items = self.work().await;
        items
            .into_iter()
            .find(|i| i["kind"] == "turn.deliver")
            .expect("turn.deliver")
    }

    async fn events(&self, conv: Uuid) -> Vec<(String, Value)> {
        let rows: Vec<(String, Value)> = sqlx::query_as(
            "SELECT type, payload FROM transcript_events WHERE conversation_id = $1 ORDER BY seq",
        )
        .bind(conv)
        .fetch_all(self.h.pool())
        .await
        .unwrap();
        rows
    }

    async fn turn_status(&self, id: &str) -> (String, Option<String>) {
        sqlx::query_as("SELECT status, error_code FROM turns WHERE id = $1::uuid")
            .bind(id)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

fn types(ev: &[(String, Value)]) -> Vec<&str> {
    ev.iter().map(|e| e.0.as_str()).collect()
}

fn ev(n: u64, kind: &str, data: Value) -> Value {
    let mut e = data;
    e["kind"] = json!(kind);
    json!({"n": n, "event": e})
}

#[tokio::test]
async fn end_to_end_turn_with_steer_credential_and_replay() {
    let r = rig().await;
    let lv = r.boot("daneel").await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(d["input"]["text"], "hello");
    assert_eq!(d["model"]["dialect"], "anthropic_messages");
    assert!(!d["model"]["model"].as_str().unwrap().is_empty());
    let cred = d["context_credential"].as_str().unwrap();
    assert!(cred.starts_with("auc_"));
    let (kind, tconv, exp_ok): (String, Option<Uuid>, bool) = sqlx::query_as(
        "SELECT kind, conversation_id, expires_at > now() + interval '5 hours' FROM gateway_credentials WHERE token_hash = $1",
    )
    .bind(&aurora_core::token::hash(cred)[..])
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (kind.as_str(), tconv, exp_ok),
        ("context", Some(lv.conv), true)
    );
    assert_eq!(r.turn_status(&turn).await.0, "dispatched");

    assert_eq!(
        r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
            .await,
        "applied"
    );
    assert_eq!(
        r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
            .await,
        "duplicate"
    );
    let batch = json!({"kind": "turn.events", "turn_id": turn, "events": [
        ev(1, "text.delta", json!({"message_id": "m0", "delta": "thinking aloud"})),
        ev(2, "tool.started", json!({"call_id": "c1", "name": "memory_list", "summary": "list"})),
        ev(3, "tool.finished", json!({"call_id": "c1", "ok": true})),
        ev(4, "text.delta", json!({"message_id": "m1", "delta": "Hel"})),
        ev(5, "text.delta", json!({"message_id": "m1", "delta": "lo"})),
    ]});
    assert_eq!(r.report(&lv, batch.clone()).await, "applied");
    assert_eq!(r.report(&lv, batch.clone()).await, "duplicate");

    // Follow-up while running is steered.
    let f = r.say(lv.conv, "also this").await;
    let fid = f["followup"]["id"].as_str().unwrap().to_owned();
    let items = r.work().await;
    let msg = items.iter().find(|i| i["kind"] == "turn.message").unwrap();
    assert_eq!(msg["followup_id"], fid);
    assert_eq!(msg["turn_id"], turn);
    assert!(
        r.work().await.iter().all(|i| i["kind"] != "turn.message"),
        "steered once"
    );
    r.report(
        &lv,
        json!({"kind": "turn.events", "turn_id": turn, "events": [
            ev(6, "message.joined", json!({"followup_id": fid}))]}),
    )
    .await;
    let st: String = sqlx::query_scalar("SELECT status FROM turns WHERE id = $1::uuid")
        .bind(&fid)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(st, "joined");

    let res = json!({"kind": "turn.result", "turn_id": turn, "status": "done",
        "usage": {"input_tokens": 10, "output_tokens": 5}});
    assert_eq!(r.report(&lv, res.clone()).await, "applied");
    assert_eq!(r.report(&lv, res).await, "duplicate");
    let (status, usage): (String, Value) =
        sqlx::query_as("SELECT status, usage FROM turns WHERE id = $1::uuid")
            .bind(&turn)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(status, "done");
    assert_eq!(usage["output_tokens"], 5);
    let evs = r.events(lv.conv).await;
    assert_eq!(
        types(&evs),
        [
            "turn.queued",
            "turn.started",
            "tool.started",
            "tool.finished",
            "turn.queued",
            "message.joined",
            "text",
            "turn.finished"
        ]
    );
    let text = evs.iter().find(|e| e.0 == "text").unwrap();
    assert_eq!(
        text.1["text"], "Hello",
        "only the post-tool answer is durable"
    );
    // Context credential now expires within the grace window.
    let soon: bool = sqlx::query_scalar(
        "SELECT expires_at <= now() + interval '301 seconds' FROM gateway_credentials WHERE token_hash = $1",
    )
    .bind(&aurora_core::token::hash(cred)[..])
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(soon);

    // Replay of the whole event report on a second AppState writes nothing new.
    let state2 = aurora_api::app::AppState::new(r.h.pool().clone(), config(|_| {}));
    let app2 = test_router(state2);
    let before = r.events(lv.conv).await.len();
    for body in [
        batch,
        json!({"kind": "turn.result", "turn_id": turn, "status": "done"}),
    ] {
        let st = r.report_on(&app2, &lv, body).await;
        assert_ne!(st, "applied");
    }
    assert_eq!(r.events(lv.conv).await.len(), before);
}

#[tokio::test]
async fn followup_is_promoted_when_the_turn_ends_unjoined() {
    let r = rig().await;
    let lv = r.boot("promo").await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    let f = r.say(lv.conv, "second").await;
    let fid = f["followup"]["id"].as_str().unwrap().to_owned();
    r.report(
        &lv,
        json!({"kind": "turn.result", "turn_id": turn, "status": "done"}),
    )
    .await;
    let evs = r.events(lv.conv).await;
    let promoted: Vec<_> = evs
        .iter()
        .filter(|e| e.0 == "turn.queued" && e.1.get("promoted_from").is_some())
        .collect();
    assert_eq!(promoted.len(), 1);
    assert_eq!(promoted[0].1["promoted_from"], fid);
    let (st, n): (String, i64) = sqlx::query_as(
        "SELECT (SELECT status FROM turns WHERE id = $1::uuid),
                (SELECT count(*) FROM turns WHERE conversation_id = $2 AND role = 'turn' AND status = 'queued')",
    )
    .bind(&fid)
    .bind(lv.conv)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!((st.as_str(), n), ("promoted", 1));
    // The promoted turn is dispatched next.
    let d2 = r.deliver().await;
    assert_eq!(d2["input"]["text"], "second");
}

#[tokio::test]
async fn abort_running_turn_is_delivered_and_ends_aborted() {
    let r = rig().await;
    let lv = r.boot("abortme").await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    let (s, _) = r
        .user_call(Method::POST, &format!("/v1/turns/{turn}/abort"), None)
        .await;
    assert_eq!(s, 202);
    let items = r.work().await;
    assert_eq!(
        items.iter().filter(|i| i["kind"] == "turn.abort").count(),
        1
    );
    assert!(
        r.work().await.iter().all(|i| i["kind"] != "turn.abort"),
        "leased"
    );
    r.report(
        &lv,
        json!({"kind": "turn.result", "turn_id": turn, "status": "aborted"}),
    )
    .await;
    assert_eq!(r.turn_status(&turn).await.0, "aborted");
}

#[tokio::test]
async fn never_started_is_revived_once_then_failed() {
    let r = rig().await;
    let lv = r.boot("slowboot").await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    let age = || async {
        sqlx::query(
            "UPDATE turns SET dispatched_at = now() - interval '200 seconds' WHERE id = $1::uuid",
        )
        .bind(&turn)
        .execute(r.h.pool())
        .await
        .unwrap();
    };
    age().await;
    r.work().await;
    assert_eq!(r.turn_status(&turn).await.0, "queued", "revived");
    let d2 = r.deliver().await;
    assert_eq!(d2["turn_id"], turn.as_str());
    age().await;
    r.work().await;
    assert_eq!(
        r.turn_status(&turn).await,
        ("failed".into(), Some("never_started".into()))
    );
    let evs = r.events(lv.conv).await;
    assert_eq!(types(&evs).iter().filter(|t| **t == "error").count(), 1);
    assert_eq!(*types(&evs).last().unwrap(), "turn.finished");
    let _ = lv.agent;
}

#[tokio::test]
async fn stalled_turn_fails_and_ceiling_warns_once_then_times_out() {
    let r = rig().await;
    let lv = r.boot("stally").await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    // Warning window (ceiling 6h, warning 15m).
    sqlx::query("UPDATE turns SET dispatched_at = now() - interval '5 hours 50 minutes' WHERE id = $1::uuid")
        .bind(&turn).execute(r.h.pool()).await.unwrap();
    let items = r.work().await;
    assert_eq!(
        items.iter().filter(|i| i["kind"] == "turn.notice").count(),
        1
    );
    assert!(r.work().await.iter().all(|i| i["kind"] != "turn.notice"));
    let n = r
        .events(lv.conv)
        .await
        .iter()
        .filter(|e| e.0 == "notice")
        .count();
    assert_eq!(n, 1);
    // Ceiling.
    sqlx::query("UPDATE turns SET dispatched_at = now() - interval '7 hours' WHERE id = $1::uuid")
        .bind(&turn)
        .execute(r.h.pool())
        .await
        .unwrap();
    let items = r.work().await;
    assert!(items.iter().any(|i| i["kind"] == "turn.abort"));
    assert_eq!(
        r.turn_status(&turn).await,
        ("failed".into(), Some("time_limit".into()))
    );

    // Stall on the next turn.
    r.say(lv.conv, "again").await;
    let d = r.deliver().await;
    let t2 = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": t2}))
        .await;
    sqlx::query(
        "UPDATE turns SET last_progress_at = now() - interval '11 minutes' WHERE id = $1::uuid",
    )
    .bind(&t2)
    .execute(r.h.pool())
    .await
    .unwrap();
    r.work().await;
    assert_eq!(
        r.turn_status(&t2).await,
        ("failed".into(), Some("turn_stalled".into()))
    );
}

#[tokio::test]
async fn no_model_key_fails_before_any_sandbox_start() {
    let r = rig().await;
    let agent = r.agent("keyless", false).await;
    let conv = r.direct(agent).await;
    let v = r.say(conv, "hi").await;
    let turn = v["turn"]["id"].as_str().unwrap().to_owned();
    let items = r.work().await;
    assert!(items.iter().all(|i| i["kind"] != "sandbox.start"));
    assert_eq!(
        r.turn_status(&turn).await,
        ("failed".into(), Some("no_model_key".into()))
    );
}

#[tokio::test]
async fn sandbox_restart_and_models_routes() {
    let r = rig().await;
    let lv = r.boot("routes").await;
    let (s, v) = r
        .user_call(
            Method::GET,
            &format!("/v1/agents/{}/sandbox", lv.agent),
            None,
        )
        .await;
    assert_eq!((s, v["state"].as_str()), (200, Some("running")));
    // The runner-advertised image is the single source of truth (`runner.image`).
    assert_eq!(v["image"], "registry.example/aurora-sandbox:9.9.9", "{v}");
    assert_eq!(v["keep_awake_reasons"][0], "turn");
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    let (s, v) = r
        .user_call(
            Method::POST,
            &format!("/v1/agents/{}/restart", lv.agent),
            None,
        )
        .await;
    assert_eq!(s, 202, "{v}");
    assert_eq!(
        r.turn_status(&turn).await,
        ("failed".into(), Some("agent_restarted".into()))
    );
    let items = r.work().await;
    assert_eq!(
        items.iter().find(|i| i["kind"] == "sandbox.stop").unwrap()["reason"],
        "restart"
    );
    let (s, v) = r
        .user_call(
            Method::GET,
            &format!("/v1/agents/{}/models", lv.agent),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["items"].is_array());
}

/// FUP-M4.1: the answer buffer is durable. Events land through one AppState, the result through
/// a fresh one (an API replica restart between `turn.events` and `turn.result`); the durable
/// `text` event is complete and the buffer row is consumed.
#[tokio::test]
async fn answer_text_survives_api_restart_between_events_and_result() {
    let r = rig().await;
    let lv = r.boot("daneel").await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(
        r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
            .await,
        "applied"
    );
    let batch1 = json!({"kind": "turn.events", "turn_id": turn, "events": [
        ev(1, "text.delta", json!({"message_id": "m0", "delta": "narration"})),
        ev(2, "tool.started", json!({"call_id": "c1", "name": "memory_list", "summary": "list"})),
        ev(3, "tool.finished", json!({"call_id": "c1", "ok": true})),
        ev(4, "text.delta", json!({"message_id": "m1", "delta": "The "})),
    ]});
    assert_eq!(r.report(&lv, batch1).await, "applied");
    // Buffer is in the database, not in process memory.
    let buf: Value =
        sqlx::query_scalar("SELECT msgs FROM turn_answer_buffers WHERE turn_id = $1::uuid")
            .bind(&turn)
            .fetch_one(r.h.pool())
            .await
            .unwrap();
    assert_eq!(buf[0]["text"], "The ");

    // "Restart": a brand-new AppState/router continues the turn.
    let app2 = test_router(aurora_api::app::AppState::new(
        r.h.pool().clone(),
        config(|_| {}),
    ));
    let batch2 = json!({"kind": "turn.events", "turn_id": turn, "events": [
        ev(5, "text.delta", json!({"message_id": "m1", "delta": "full "})),
        ev(6, "text.delta", json!({"message_id": "m1", "delta": "answer"})),
    ]});
    assert_eq!(r.report_on(&app2, &lv, batch2).await, "applied");
    let app3 = test_router(aurora_api::app::AppState::new(
        r.h.pool().clone(),
        config(|_| {}),
    ));
    assert_eq!(
        r.report_on(
            &app3,
            &lv,
            json!({"kind": "turn.result", "turn_id": turn, "status": "done"})
        )
        .await,
        "applied"
    );
    let evs = r.events(lv.conv).await;
    let texts: Vec<&Value> = evs.iter().filter(|e| e.0 == "text").map(|e| &e.1).collect();
    assert_eq!(texts.len(), 1, "{evs:?}");
    assert_eq!(texts[0]["text"], "The full answer");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM turn_answer_buffers")
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert_eq!(left, 0, "buffer consumed by the result");
}
