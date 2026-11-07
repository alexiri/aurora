#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! BE-062: schedules - API, cron, arm 9, gates, authority, tools.

use crate::common::*;
use axum::body::Body;
use base64::Engine;
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
        "max_sandboxes": 4, "harnesses": ["fake"], "max_attachment_bytes": 1048576
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

impl Rig {
    async fn sched(&self, agent: Uuid, body: Value) -> Value {
        let (s, v) = self
            .user_call(
                Method::POST,
                &format!("/v1/agents/{agent}/schedules"),
                Some(body),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v
    }

    async fn due(&self, id: &str) {
        sqlx::query(
            "UPDATE schedules SET next_fire_at = now() - interval '2 days' WHERE id = $1::uuid",
        )
        .bind(id)
        .execute(self.h.pool())
        .await
        .unwrap();
    }

    async fn runs(&self, id: &str) -> Vec<(String, Option<Uuid>)> {
        sqlx::query_as(
            "SELECT outcome, turn_id FROM schedule_runs WHERE schedule_id = $1::uuid ORDER BY created_at, id",
        )
        .bind(id)
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }

    /// Deliver and finish the initial "hello" turn so the sandbox is idle.
    async fn finish_hello(&self, lv: &Live) {
        let d = self.deliver().await;
        let t = d["turn_id"].as_str().unwrap().to_owned();
        self.report(lv, json!({"kind": "turn.started", "turn_id": t}))
            .await;
        self.report(
            lv,
            json!({"kind": "turn.result", "turn_id": t, "status": "done"}),
        )
        .await;
    }

    /// Fire `id` and return the delivery of its run turn (needs a booted sandbox).
    async fn fire_and_deliver(&self, id: &str) -> Value {
        self.due(id).await;
        self.work().await; // arm 9 creates the turn
        // Creating/changing a schedule bumps the home-sync generation (the schedules summary is
        // projected, FIX-005); dispatch waits for the sync, so apply it as the supervisor would.
        sqlx::query(
            "UPDATE sandboxes s SET home_applied_generation =
                 a.home_sync_generation + w.home_sync_generation + 1
             FROM agents a JOIN workspaces w ON w.id = a.workspace_id WHERE a.id = s.agent_id",
        )
        .execute(self.h.pool())
        .await
        .unwrap();
        self.deliver().await
    }

    async fn gate(&self, lv: &Live, turn: &str, outcome: &str, output: &str) -> String {
        self.report(
            lv,
            json!({"kind": "gate.result", "turn_id": turn, "outcome": outcome, "output": output}),
        )
        .await
    }

    async fn sched_row(&self, id: &str) -> (bool, Option<String>, i32) {
        sqlx::query_as(
            "SELECT enabled, disabled_reason, consecutive_failures FROM schedules WHERE id = $1::uuid",
        )
        .bind(id)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn api_validation_crud_and_run_now() {
    let r = rig().await;
    let agent = r.agent("sched1", true).await;
    for (cron, tz) in [
        ("0 0 9 * * *", "UTC"),
        ("* * * *", "UTC"),
        ("*/2 * * * *", "UTC"),
        ("0 9 * * *", "Not/AZone"),
    ] {
        let (s, v) = r
            .user_call(
                Method::POST,
                &format!("/v1/agents/{agent}/schedules"),
                Some(json!({"name": "n", "prompt": "p", "cron": cron, "timezone": tz})),
            )
            .await;
        assert_eq!(s, 400, "{cron} {tz}: {v}");
    }
    let v = r
        .sched(
            agent,
            json!({"name": "daily", "prompt": "do it", "cron": "0 9 * * *"}),
        )
        .await;
    assert_eq!(v["timezone"], "UTC");
    assert_eq!(v["enabled"], true);
    assert!(v["next_fire_at"].is_string());
    assert!(v["created_by"]["display_name"].is_string());
    let id = v["id"].as_str().unwrap().to_owned();

    let (s, v) = r
        .user_call(Method::GET, &format!("/v1/agents/{agent}/schedules"), None)
        .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 1));
    let (s, v) = r
        .user_call(
            Method::PATCH,
            &format!("/v1/schedules/{id}"),
            Some(json!({"enabled": false, "gate_command": "echo hi"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["disabled_reason"], "user");
    assert!(v["next_fire_at"].is_null());
    assert_eq!(v["gate_command"], "echo hi");
    let (_, v) = r
        .user_call(
            Method::PATCH,
            &format!("/v1/schedules/{id}"),
            Some(json!({"enabled": true, "gate_command": null})),
        )
        .await;
    assert!(v["gate_command"].is_null() && v["next_fire_at"].is_string());

    // run-now: creates a turn, a second call while it is active is busy.
    let (s, run) = r
        .user_call(Method::POST, &format!("/v1/schedules/{id}/run-now"), None)
        .await;
    assert_eq!(s, 201, "{run}");
    assert_eq!(run["outcome"], "fired");
    let (s, v) = r
        .user_call(Method::POST, &format!("/v1/schedules/{id}/run-now"), None)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("schedule_busy"))
    );
    let (s, v) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}/runs"), None)
        .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 1));
    let text: String = sqlx::query_scalar("SELECT text FROM turns WHERE id = $1")
        .bind(Uuid::parse_str(run["turn_id"].as_str().unwrap()).unwrap())
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    assert!(text.starts_with("Scheduled task: daily"));

    let (s, _) = r
        .user_call(Method::DELETE, &format!("/v1/schedules/{id}"), None)
        .await;
    assert_eq!(s, 204);
    let (s, _) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}"), None)
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn missed_fires_coalesce_and_pollers_fire_once() {
    let r = rig().await;
    let agent = r.agent("sched2", true).await;
    let v = r
        .sched(
            agent,
            json!({"name": "d", "prompt": "p", "cron": "0 * * * *"}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    r.due(&id).await;
    let (a, b) = tokio::join!(r.work(), r.work());
    let _ = (a, b);
    let runs = r.runs(&id).await;
    let outcomes: Vec<&str> = runs.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(
        outcomes,
        ["coalesced", "fired"],
        "one fire, one coalesce marker"
    );
    let (_, v) = r
        .user_call(Method::GET, &format!("/v1/schedules/{id}"), None)
        .await;
    assert!(v["last_fired_at"].is_string());
    let future: bool = sqlx::query_scalar(
        "SELECT next_fire_at > now() AND fire_lease_until IS NULL FROM schedules WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert!(future);
    // Not due again.
    r.work().await;
    assert_eq!(r.runs(&id).await.len(), 2);
}

#[tokio::test]
async fn creator_losing_access_disables_with_authorization_and_notice() {
    let r = rig().await;
    let agent = r.agent("sched3", true).await;
    let origin = r.direct(agent).await;
    let v = r
        .sched(
            agent,
            json!({"name": "n", "prompt": "p", "cron": "0 * * * *"}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE schedules SET origin_conversation_id = $2 WHERE id = $1::uuid")
        .bind(&id)
        .bind(origin)
        .execute(r.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE org_id = $1")
        .bind(r.org)
        .execute(r.h.pool())
        .await
        .unwrap();
    r.due(&id).await;
    r.work().await;
    assert_eq!(r.runs(&id).await[0].0, "skipped_unauthorized");
    let (en, why, _) = r.sched_row(&id).await;
    assert_eq!((en, why.as_deref()), (false, Some("authorization")));
    let ev = r.events(origin).await;
    assert!(
        ev.iter()
            .any(|e| e.0 == "notice" && e.1["code"] == "schedule_disabled"),
        "{ev:?}"
    );
}

#[tokio::test]
async fn gates_quiet_wake_fail_and_failure_disable() {
    let r = rig().await;
    let lv = r.boot("sched4").await;
    r.finish_hello(&lv).await;
    let origin = lv.conv;
    let v = r
        .sched(
            lv.agent,
            json!({"name": "g", "prompt": "check", "cron": "0 * * * *", "gate_command": "./check.sh"}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE schedules SET origin_conversation_id = $2 WHERE id = $1::uuid")
        .bind(&id)
        .bind(origin)
        .execute(r.h.pool())
        .await
        .unwrap();

    // Quiet: run gated_quiet, no turn attached, turn closed.
    let d = r.fire_and_deliver(&id).await;
    assert_eq!(d["gate"]["command"], "./check.sh");
    assert_eq!(d["gate"]["timeout_secs"], 30);
    let t1 = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(r.gate(&lv, &t1, "quiet", "").await, "applied");
    assert_eq!(r.gate(&lv, &t1, "quiet", "").await, "duplicate");
    let runs = r.runs(&id).await;
    assert_eq!(runs.last().unwrap(), &("gated_quiet".to_owned(), None));
    assert_eq!(r.turn_status(&t1).await.0, "done");

    // Wake: output appended as a notice in the run conversation, turn continues.
    let d = r.fire_and_deliver(&id).await;
    let t2 = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(r.gate(&lv, &t2, "wake", "3 new items").await, "applied");
    assert_eq!(r.gate(&lv, &t2, "wake", "3 new items").await, "duplicate");
    let rc: Uuid = sqlx::query_scalar("SELECT conversation_id FROM turns WHERE id = $1::uuid")
        .bind(&t2)
        .fetch_one(r.h.pool())
        .await
        .unwrap();
    let notes: Vec<_> = r
        .events(rc)
        .await
        .into_iter()
        .filter(|e| e.0 == "notice" && e.1["code"] == "gate_output")
        .collect();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].1["text"], "3 new items");
    assert_eq!(r.runs(&id).await.last().unwrap().0, "fired");
    r.report(&lv, json!({"kind": "turn.started", "turn_id": t2}))
        .await;
    r.report(
        &lv,
        json!({"kind": "turn.result", "turn_id": t2, "status": "done"}),
    )
    .await;
    let origin_ev = r.events(origin).await;
    assert!(origin_ev.iter().any(|e| e.1["code"] == "schedule_run"));

    // Failed: counts toward the failure disable; replay on a second AppState is a no-op.
    let d = r.fire_and_deliver(&id).await;
    let t3 = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(r.gate(&lv, &t3, "failed", "exit 2").await, "applied");
    assert_eq!(r.runs(&id).await.last().unwrap().0, "gate_failed");
    assert_eq!(
        r.turn_status(&t3).await,
        ("failed".into(), Some("gate_failed".into()))
    );
    assert_eq!(r.sched_row(&id).await, (true, None, 1));
    let state2 = aurora_api::app::AppState::new(r.h.pool().clone(), config(|_| {}));
    let app2 = test_router(state2);
    let st = r
        .report_on(
            &app2,
            &lv,
            json!({"kind": "gate.result", "turn_id": t3, "outcome": "failed", "output": "exit 2"}),
        )
        .await;
    assert_ne!(st, "applied");
    assert_eq!(r.sched_row(&id).await.2, 1);

    // The fifth consecutive failure disables.
    sqlx::query("UPDATE schedules SET consecutive_failures = 4 WHERE id = $1::uuid")
        .bind(&id)
        .execute(r.h.pool())
        .await
        .unwrap();
    let d = r.fire_and_deliver(&id).await;
    let t4 = d["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(r.gate(&lv, &t4, "failed", "boom").await, "applied");
    let (en, why, n) = r.sched_row(&id).await;
    assert_eq!((en, why.as_deref(), n), (false, Some("failures"), 5));
    assert!(
        r.events(origin)
            .await
            .iter()
            .any(|e| e.1["code"] == "schedule_disabled")
    );
}

#[tokio::test]
async fn schedule_tools_via_tool_calls() {
    let r = rig().await;
    let lv = r.boot("sched5").await;
    let d = r.deliver().await;
    let turn = d["turn_id"].as_str().unwrap().to_owned();
    r.report(&lv, json!({"kind": "turn.started", "turn_id": turn}))
        .await;
    let call = |tool: &'static str, args: Value| {
        let (r, lv, turn) = (&r, &lv, turn.clone());
        async move {
            let (s, v) = r
                .runner(
                    Some(r.rep),
                    "/v1/runner/tool-calls",
                    json!({"sandbox_id": lv.sb, "boot_id": lv.boot, "call_id": "c1",
                           "conversation_id": lv.conv, "turn_id": turn, "tool": tool, "args": args}),
                )
                .await;
            assert_eq!(s, 200, "{v}");
            v
        }
    };
    let v = call(
        "schedule_create",
        json!({"name": "t", "prompt": "p", "cron": "30 8 * * 1-5", "timezone": "Europe/Madrid"}),
    )
    .await;
    assert_eq!(v["ok"], true, "{v}");
    let id = v["content"][0]["value"]["id"].as_str().unwrap().to_owned();
    let v = call(
        "schedule_create",
        json!({"name": "t", "prompt": "p", "cron": "* * * * *"}),
    )
    .await;
    assert_eq!(v["ok"], false);
    let (origin, creator): (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT origin_conversation_id, creator_user_id FROM schedules WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(r.h.pool())
    .await
    .unwrap();
    assert_eq!(origin, Some(lv.conv));
    assert!(creator.is_some());
    let v = call("schedule_list", json!({})).await;
    assert_eq!(
        v["content"][0]["value"]["schedules"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let v = call("schedule_update", json!({"id": id, "enabled": false})).await;
    assert_eq!(v["content"][0]["value"]["disabled_reason"], "user");
    // Another agent's schedule is invisible to the tool.
    let other = r.agent("sched5b", false).await;
    let o = r
        .sched(
            other,
            json!({"name": "o", "prompt": "p", "cron": "0 * * * *"}),
        )
        .await;
    let v = call("schedule_delete", json!({"id": o["id"]})).await;
    assert_eq!(v["ok"], false);
    let v = call("schedule_delete", json!({"id": id})).await;
    assert_eq!(v["ok"], true, "{v}");
}
