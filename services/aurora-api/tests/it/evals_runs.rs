#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-EVAL-2 (D-289..D-293, C-860, C-862, C-864, C-865, C-867, C-868, C-870, C-871, SEC-114,
//! SEC-116; docs/18 §Evals): eval runs as real turns, one `eval` conversation per question,
//! scoring, side-effect refusal, schedules, regression and retention, over the real router, the
//! due-work arm and the database. The harness is played by the test: it ends the turns the arm
//! creates, exactly as a supervisor's `turn.result` would.

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use aurora_api::domains::conversations::service as conv;
use aurora_api::domains::hosted::lifecycle;
use aurora_api::domains::tools::{ToolCallContext, ToolContext, registry};
use http::Method;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

const CANARY: &str = "canary-answer-91c4ab";

struct W {
    r: Rig,
    agent: Uuid,
    member: String,
    owner: String,
    stranger: String,
}

async fn world() -> W {
    let r = rig_with(|c| {
        c.governance.allowed_key_scopes = vec!["organization".into()];
    })
    .await;
    // A granted LLM key, so queued turns are not failed with `no_model_key`.
    let agent = r.agent("evaluee", true).await;
    let u = r.h.user("owner@x.io", false).await;
    r.h.member(r.org, u, "member", "active").await;
    r.h.bind_ws(r.ws, u, "owner").await;
    let owner = cookie(&r.h.good_session(u).await.1);
    let s = r.h.user("stranger@x.io", false).await;
    r.h.member(r.org, s, "member", "active").await;
    let stranger = cookie(&r.h.good_session(s).await.1);
    let member = r.cookie.clone();
    W {
        r,
        agent,
        member,
        owner,
        stranger,
    }
}

impl W {
    async fn call(&self, ck: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .r
            .h
            .send(m, uri, &[("cookie", ck), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    async fn q(&self, body: Value) -> Value {
        let (s, v) = self
            .call(
                &self.member,
                Method::POST,
                &format!("/v1/agents/{}/eval-questions", self.agent),
                Some(body),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v
    }

    async fn numeric(&self, text: &str, n: f64) -> Value {
        self.q(json!({"question": text, "expected_kind": "numeric", "expected_number": n}))
            .await
    }

    async fn start(&self) -> (u16, Value) {
        self.call(
            &self.member,
            Method::POST,
            &format!("/v1/agents/{}/eval-runs", self.agent),
            Some(json!({})),
        )
        .await
    }

    async fn run(&self, id: &str) -> Value {
        let (s, v) = self
            .call(
                &self.member,
                Method::GET,
                &format!("/v1/eval-runs/{id}"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn status(&self, id: &str) -> String {
        self.run(id).await["status"].as_str().unwrap().to_owned()
    }

    /// One poll of the due-work engine (arm 10 advances the runs).
    async fn tick(&self) {
        self.r.work().await;
    }

    /// The harness answers the oldest active eval turn: durable text, then the terminal state.
    async fn answer(&self, text: Option<&str>, status: &str, code: Option<&str>) {
        let (turn, conversation): (Uuid, Uuid) = sqlx::query_as(
            "SELECT id, conversation_id FROM turns WHERE source_event_id LIKE 'eval:%'
              AND status IN ('queued','dispatched','running') ORDER BY created_at, id LIMIT 1",
        )
        .fetch_one(self.r.h.pool())
        .await
        .expect("an active eval turn");
        let mut conn = self.r.h.pool().acquire().await.unwrap();
        if let Some(t) = text {
            conv::append_event(
                &mut conn,
                conversation,
                Some(turn),
                "text",
                &json!({"turn_id": turn, "text": t}),
            )
            .await
            .unwrap();
        }
        lifecycle::finish(&mut conn, turn, status, code, None, None)
            .await
            .unwrap()
            .expect("turn finished");
    }

    /// Tick, answer, tick … until the run is no longer active; the request-log wait that
    /// precedes `done` is skipped by backdating the results (the log is flushed every 5 s).
    async fn drive(&self, id: &str, answers: &[&str]) {
        for a in answers {
            self.tick().await;
            self.answer(Some(a), "done", None).await;
        }
        self.tick().await;
        self.settle(id).await;
    }

    async fn settle(&self, id: &str) {
        sqlx::query(
            "UPDATE eval_results SET finished_at = finished_at - interval '1 minute'
              WHERE run_id = $1::uuid AND finished_at IS NOT NULL",
        )
        .bind(id)
        .execute(self.r.h.pool())
        .await
        .unwrap();
        for _ in 0..20 {
            self.tick().await;
            if !matches!(self.status(id).await.as_str(), "queued" | "running") {
                return;
            }
        }
        panic!("run {id} did not end");
    }

    async fn scalar(&self, sql: &'static str, id: &str) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(self.r.h.pool())
            .await
            .unwrap()
    }
}

fn results(v: &Value) -> Vec<Value> {
    v["results"].as_array().unwrap().clone()
}

#[tokio::test]
async fn a_run_makes_one_eval_conversation_and_turn_per_question_and_scores_them() {
    let w = world().await;
    w.numeric("What is 6 times 7?", 42.0).await;
    w.q(json!({"question": "Capital of France?", "expected_kind": "text", "expected_text": "Paris"}))
        .await;
    w.numeric("What is 2 plus 2?", 4.0).await;
    let (s, run) = w.start().await;
    assert_eq!(s, 202, "{run}");
    assert_eq!(run["status"], "queued");
    assert_eq!(run["counts"]["total"], 3);
    let id = run["id"].as_str().unwrap().to_owned();

    // A replayed claim (two ticks before the answer) creates the turn once.
    w.tick().await;
    w.tick().await;
    let n: i64 = w
        .scalar(
            "SELECT count(*) FROM turns WHERE source_event_id LIKE 'eval:' || $1 || ':%'",
            &id,
        )
        .await;
    assert_eq!(n, 1);
    let (kind, ext, text): (String, String, String) = sqlx::query_as(
        "SELECT c.kind, c.external_ref, t.text FROM turns t JOIN conversations c ON c.id = t.conversation_id
         WHERE t.source_event_id = 'eval:' || $1 || ':1'",
    )
    .bind(&id)
    .fetch_one(w.r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (kind.as_str(), ext.as_str()),
        ("eval", format!("eval:{id}:1").as_str())
    );
    assert!(text.starts_with("What is 6 times 7?"), "{text}");
    assert!(
        text.contains("ANSWER:"),
        "the instruction is appended: {text}"
    );

    w.answer(Some("Six times seven is 42.\nANSWER: 42"), "done", None)
        .await;
    w.tick().await;
    w.answer(Some("Paris is the capital.\nANSWER: paris"), "done", None)
        .await;
    w.tick().await;
    // The third answer never gives the final line.
    w.answer(Some(&format!("It is 4. {CANARY}")), "done", None)
        .await;
    w.tick().await;
    w.settle(&id).await;

    let v = w.run(&id).await;
    assert_eq!(v["status"], "done", "{v}");
    assert_eq!(v["counts"]["passed"], 2);
    assert_eq!(v["counts"]["failed"], 1);
    let r = results(&v);
    assert_eq!(r.len(), 3);
    let outcomes: Vec<_> = r.iter().map(|x| x["outcome"].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["pass", "pass", "fail"]);
    assert_eq!(r[2]["reason_code"], "no_answer_line");
    assert_eq!(r[0]["extracted_value"], "42");
    assert_eq!(r[0]["expected"]["number"], 42.0);
    assert!(r[2]["answer"].as_str().unwrap().contains(CANARY));
    assert!((v["pass_rate"].as_f64().unwrap() - 66.666).abs() < 0.01);
    // Three conversations, three turns, no turn answered twice by later ticks.
    w.tick().await;
    assert_eq!(
        w.scalar(
            "SELECT count(*) FROM turns WHERE source_event_id LIKE 'eval:' || $1 || ':%'",
            &id
        )
        .await,
        3
    );
    assert_eq!(
        w.scalar("SELECT count(*) FROM conversations WHERE kind = 'eval' AND external_ref LIKE 'eval:' || $1 || ':%'", &id).await,
        3
    );

    // Hidden from the default chat list; shown with ?kind=eval.
    let (_, def) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/agents/{}/conversations", w.agent),
            None,
        )
        .await;
    assert!(
        def["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["kind"] != "eval"),
        "{def}"
    );
    let (_, ev) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/agents/{}/conversations?kind=eval", w.agent),
            None,
        )
        .await;
    assert_eq!(ev["items"].as_array().unwrap().len(), 3);
    // A stranger sees nothing of it.
    let (s, _) = w
        .call(
            &w.stranger,
            Method::GET,
            &format!("/v1/eval-runs/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 404);
    // SEC-116: no audit row carries a question, an answer or an expected value; no thread link
    // (nothing to mirror to) exists for an eval conversation.
    wait_audit_rows(&w, "eval.run.start").await;
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE metadata::text ILIKE ANY (ARRAY['%canary%', '%times 7%', '%paris%'])",
    )
    .fetch_one(w.r.h.pool())
    .await
    .unwrap();
    assert_eq!(leaked, 0);
    let links: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_links l JOIN conversations c ON c.id = l.conversation_id WHERE c.kind = 'eval'",
    )
    .fetch_one(w.r.h.pool())
    .await
    .unwrap();
    assert_eq!(links, 0);
}

async fn wait_audit_rows(w: &W, action: &str) {
    for _ in 0..200 {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
            .bind(action)
            .fetch_one(w.r.h.pool())
            .await
            .unwrap();
        if n > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no audit row {action}");
}

#[tokio::test]
async fn start_refusals_and_the_one_active_run_rule() {
    let w = world().await;
    let (s, v) = w.start().await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("eval_no_questions")),
        "{v}"
    );
    w.numeric("1+1", 2.0).await;
    let (s, run) = w.start().await;
    assert_eq!(s, 202);
    let (s, v) = w.start().await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("eval_run_active")),
        "{v}"
    );
    // Delete of an active run is refused too; a stranger cannot start.
    let id = run["id"].as_str().unwrap();
    let (s, v) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/eval-runs/{id}"),
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("eval_run_active"))
    );
    let (s, _) = w
        .call(
            &w.stranger,
            Method::POST,
            &format!("/v1/agents/{}/eval-runs", w.agent),
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 404);
    // Cancel ends it; a second cancel is 409 `eval_run_finished`.
    let (s, _) = w
        .call(
            &w.member,
            Method::POST,
            &format!("/v1/eval-runs/{id}/cancel"),
            None,
        )
        .await;
    assert_eq!(s, 202);
    w.tick().await;
    assert_eq!(w.status(id).await, "cancelled");
    let (s, v) = w
        .call(
            &w.member,
            Method::POST,
            &format!("/v1/eval-runs/{id}/cancel"),
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("eval_run_finished"))
    );
    // The daily cap: the twentieth start of the UTC day is the last.
    for _ in 0..19 {
        sqlx::query(
            "INSERT INTO eval_runs (id, agent_id, workspace_id, trigger, status, question_set_version, fingerprint, fingerprint_hash, finished_at)
             SELECT gen_random_uuid(), agent_id, workspace_id, 'manual', 'done', 'v', '{}', 'h', now() FROM eval_runs WHERE id = $1::uuid",
        )
        .bind(id)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    }
    let (s, h, v) =
        w.r.h
            .send(
                Method::POST,
                &format!("/v1/agents/{}/eval-runs", w.agent),
                &[("cookie", w.member.as_str()), ("origin", ORIGIN)],
                Some(json!({})),
            )
            .await;
    assert_eq!(
        (s.as_u16(), v["error"]["code"].as_str()),
        (429, Some("eval_runs_limited")),
        "{v}"
    );
    assert!(h.get("retry-after").is_some());
    // A frozen agent: 423.
    sqlx::query("UPDATE agents SET frozen_at = now(), frozen_reason = 'test', frozen_authority = 'workspace_owner' WHERE id = $1")
        .bind(w.agent)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    let (s, v) = w.start().await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (423, Some("frozen")),
        "{v}"
    );
}

#[tokio::test]
async fn a_freeze_or_lost_access_cancels_a_run_mid_way() {
    let w = world().await;
    w.numeric("a", 1.0).await;
    w.numeric("b", 2.0).await;
    let (_, run) = w.start().await;
    let id = run["id"].as_str().unwrap().to_owned();
    w.tick().await;
    w.answer(Some("ANSWER: 1"), "done", None).await;
    sqlx::query("UPDATE agents SET frozen_at = now(), frozen_reason = 'test', frozen_authority = 'workspace_owner' WHERE id = $1")
        .bind(w.agent)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    w.tick().await;
    let v = w.run(&id).await;
    assert_eq!(
        (v["status"].as_str(), v["error_code"].as_str()),
        (Some("cancelled"), Some("frozen")),
        "{v}"
    );
    let r = results(&v);
    assert_eq!(r[0]["outcome"], "pass");
    assert_eq!(
        (r[1]["outcome"].as_str(), r[1]["reason_code"].as_str()),
        (Some("not_scored"), Some("frozen"))
    );

    // Lost access: the starter is removed from the workspace before the second question.
    sqlx::query("UPDATE agents SET frozen_at = NULL, frozen_reason = NULL, frozen_authority = NULL WHERE id = $1")
        .bind(w.agent)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    let (_, run) = w.start().await;
    let id = run["id"].as_str().unwrap().to_owned();
    w.tick().await;
    w.answer(Some("ANSWER: 1"), "done", None).await;
    sqlx::query("UPDATE eval_runs SET started_by_user_id = NULL WHERE id = $1::uuid")
        .bind(&id)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    w.tick().await;
    let v = w.run(&id).await;
    assert_eq!(
        (v["status"].as_str(), v["error_code"].as_str()),
        (Some("cancelled"), Some("access_lost")),
        "{v}"
    );
}

#[tokio::test]
async fn failed_turns_timeouts_and_cancel_are_recorded_not_scored_as_passes() {
    let w = world().await;
    for t in ["a", "b", "c", "d"] {
        w.numeric(t, 1.0).await;
    }
    let (_, run) = w.start().await;
    let id = run["id"].as_str().unwrap().to_owned();
    w.tick().await;
    w.answer(None, "failed", Some("harness_error")).await;
    w.tick().await;
    // The second question hangs past EVAL_TURN_TIMEOUT.
    sqlx::query("UPDATE turns SET created_at = now() - interval '2 hours' WHERE source_event_id = 'eval:' || $1 || ':2'")
        .bind(&id)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    w.tick().await;
    let aborted: bool = sqlx::query_scalar(
        "SELECT abort_requested_at IS NOT NULL OR status = 'aborted' FROM turns WHERE source_event_id = 'eval:' || $1 || ':2'",
    )
    .bind(&id)
    .fetch_one(w.r.h.pool())
    .await
    .unwrap();
    assert!(aborted, "the timed-out turn is aborted");
    // The third is in flight when the run is cancelled; the fourth is never reached.
    w.tick().await;
    let (s, _) = w
        .call(
            &w.member,
            Method::POST,
            &format!("/v1/eval-runs/{id}/cancel"),
            None,
        )
        .await;
    assert_eq!(s, 202);
    w.tick().await;
    let v = w.run(&id).await;
    assert_eq!(v["status"], "cancelled", "{v}");
    let r = results(&v);
    let got: Vec<_> = r
        .iter()
        .map(|x| {
            (
                x["outcome"].as_str().unwrap(),
                x["reason_code"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("error", "harness_error"),
            ("error", "timeout"),
            ("not_scored", "cancelled"),
            ("not_scored", "cancelled")
        ]
    );
    assert_eq!(v["counts"]["errored"], 2);
    assert_eq!(v["counts"]["not_scored"], 2);
    assert!(v["pass_rate"].is_null());
    // A judged question cannot be scored until the judge exists.
    sqlx::query("DELETE FROM eval_questions WHERE agent_id = $1")
        .bind(w.agent)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    w.q(json!({"question": "Explain.", "expected_kind": "judged", "criteria": "mentions x"}))
        .await;
    let (_, run) = w.start().await;
    let id = run["id"].as_str().unwrap().to_owned();
    w.drive(&id, &["something"]).await;
    let r = results(&w.run(&id).await);
    assert_eq!(
        (r[0]["outcome"].as_str(), r[0]["reason_code"].as_str()),
        (Some("error"), Some("judge_not_available"))
    );
}

fn eval_ctx(w: &W, conversation: Uuid, turn: Uuid) -> ToolCallContext {
    ToolCallContext {
        agent: ToolContext {
            pool: w.r.h.pool().clone(),
            agent_id: w.agent,
            workspace_id: w.r.ws,
            org_id: w.r.org,
        },
        conversation_id: conversation,
        turn_id: Some(turn),
        call_id: "c1".into(),
        state: w.r.h.state.clone(),
    }
}

#[tokio::test]
async fn platform_writes_in_an_eval_turn_are_refused_and_counted() {
    let w = world().await;
    w.numeric("a", 1.0).await;
    let (_, run) = w.start().await;
    let id = run["id"].as_str().unwrap().to_owned();
    w.tick().await;
    let (turn, c): (Uuid, Uuid) = sqlx::query_as(
        "SELECT id, conversation_id FROM turns WHERE source_event_id = 'eval:' || $1 || ':1'",
    )
    .bind(&id)
    .fetch_one(w.r.h.pool())
    .await
    .unwrap();
    let ctx = eval_ctx(&w, c, turn);
    let reg = registry();
    let writes: [(&str, Value); 6] = [
        ("memory_save", json!({"title": "t", "content": CANARY})),
        ("memory_delete", json!({"id": Uuid::now_v7()})),
        (
            "skill_create",
            json!({"name": "s", "description": "d", "body": CANARY}),
        ),
        (
            "schedule_create",
            json!({"name": "n", "prompt": "p", "cron": "0 9 * * *"}),
        ),
        ("send_message", json!({"text": CANARY})),
        ("message_agent", json!({"text": CANARY})),
    ];
    for (tool, args) in writes {
        let res = reg.call(&ctx, tool, args, Duration::from_secs(5)).await;
        assert!(!res.ok, "{tool}");
        assert_eq!(res.error.unwrap().code.as_str(), "eval_read_only", "{tool}");
    }
    let mem: i64 = sqlx::query_scalar("SELECT count(*) FROM memories WHERE agent_id = $1")
        .bind(w.agent)
        .fetch_one(w.r.h.pool())
        .await
        .unwrap();
    assert_eq!(mem, 0);
    let approvals: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals WHERE agent_id = $1")
        .bind(w.agent)
        .fetch_one(w.r.h.pool())
        .await
        .unwrap();
    assert_eq!(approvals, 0, "a refusal creates no approval");
    // The refusals are counted on the result, then on the run once it ends.
    w.answer(Some("ANSWER: 1"), "done", None).await;
    w.tick().await;
    w.settle(&id).await;
    let v = w.run(&id).await;
    assert_eq!(results(&v)[0]["refused_writes"], 6, "{v}");
    assert_eq!(v["counts"]["refused_writes"], 6);

    // The same write in a normal conversation is unchanged.
    let direct = w.r.direct(w.agent).await;
    let (dturn,): (Uuid,) = {
        w.r.say(direct, "hi").await;
        sqlx::query_as("SELECT id FROM turns WHERE conversation_id = $1")
            .bind(direct)
            .fetch_one(w.r.h.pool())
            .await
            .unwrap()
    };
    let nctx = eval_ctx(&w, direct, dturn);
    let res = reg
        .call(
            &nctx,
            "memory_save",
            json!({"title": "t", "content": "plain"}),
            Duration::from_secs(5),
        )
        .await;
    assert_ne!(
        res.error
            .as_ref()
            .map(|e| e.code.as_str().to_owned())
            .as_deref(),
        Some("eval_read_only"),
        "{res:?}"
    );
}

#[tokio::test]
async fn regression_needs_a_ten_point_drop_and_the_setting() {
    let w = world().await;
    for i in 0..10 {
        w.numeric(&format!("q{i}"), 1.0).await;
    }
    let all = ["ANSWER: 1"; 10];
    let (_, r1) = w.start().await;
    let r1 = r1["id"].as_str().unwrap().to_owned();
    w.drive(&r1, &all).await;
    assert!(
        w.run(&r1).await["regression"].is_null(),
        "no earlier run to compare with"
    );
    // One wrong answer: 90 against 100 is a drop of exactly ten points.
    let mut nine = all;
    nine[3] = "ANSWER: 2";
    let (_, r2) = w.start().await;
    let r2 = r2["id"].as_str().unwrap().to_owned();
    w.drive(&r2, &nine).await;
    let v = w.run(&r2).await;
    assert_eq!(v["regression"]["from_run_id"], r1.as_str(), "{v}");
    assert!((v["regression"]["drop_points"].as_f64().unwrap() - 10.0).abs() < 0.01);
    // Notice off: the same drop sets nothing.
    let (s, _) = w
        .call(
            &w.owner,
            Method::PUT,
            &format!("/v1/agents/{}/eval-settings", w.agent),
            Some(json!({"judge": null, "regression_notify": false})),
        )
        .await;
    assert_eq!(s, 200);
    let (_, r3) = w.start().await;
    let r3 = r3["id"].as_str().unwrap().to_owned();
    w.drive(&r3, &nine).await;
    assert!(w.run(&r3).await["regression"].is_null());
}

#[tokio::test]
async fn schedules_validate_fire_skip_and_disable() {
    let w = world().await;
    let uri = format!("/v1/agents/{}/eval-schedules", w.agent);
    let (s, v) = w
        .call(
            &w.member,
            Method::POST,
            &uri,
            Some(json!({"name": "n", "cron": "*/30 * * * *", "timezone": "UTC"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("invalid")),
        "{v}"
    );
    assert_eq!(v["error"]["details"]["field"], "cron");
    let (s, v) = w
        .call(
            &w.member,
            Method::POST,
            &uri,
            Some(json!({"name": "n", "cron": "0 * * * *", "timezone": "Not/AZone"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["details"]["field"].as_str()),
        (422, Some("timezone")),
        "{v}"
    );
    // `anchor = workspace` is for owners only.
    let (s, _) = w
        .call(
            &w.member,
            Method::POST,
            &uri,
            Some(
                json!({"name": "n", "cron": "0 * * * *", "timezone": "UTC", "anchor": "workspace"}),
            ),
        )
        .await;
    assert_eq!(s, 403);
    let mut ids = Vec::new();
    for i in 0..5 {
        let (s, v) = w
            .call(
                &w.member,
                Method::POST,
                &uri,
                Some(json!({"name": format!("s{i}"), "cron": "0 * * * *", "timezone": "UTC"})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        assert!(v["next_fire_at"].is_string());
        ids.push(v["id"].as_str().unwrap().to_owned());
    }
    let (s, v) = w
        .call(
            &w.member,
            Method::POST,
            &uri,
            Some(json!({"name": "six", "cron": "0 * * * *", "timezone": "UTC"})),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("eval_schedule_limit")),
        "{v}"
    );

    let due = |id: &str| {
        let id = id.to_owned();
        let pool = w.r.h.pool().clone();
        async move {
            sqlx::query("UPDATE eval_schedules SET next_fire_at = now() - interval '1 minute', fire_lease_until = NULL WHERE id = $1::uuid")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    let outcome = |id: &str| {
        let id = id.to_owned();
        let pool = w.r.h.pool().clone();
        async move {
            sqlx::query_as::<_, (Option<String>, bool, Option<String>)>(
                "SELECT last_outcome, enabled, disabled_reason FROM eval_schedules WHERE id = $1::uuid",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    // No questions: a failed fire (counts toward the streak), no run.
    due(&ids[0]).await;
    w.tick().await;
    assert_eq!(outcome(&ids[0]).await.0.as_deref(), Some("failed"));
    w.numeric("q", 1.0).await;
    // Fires: a run with trigger `schedule` and no starter.
    due(&ids[0]).await;
    w.tick().await;
    let (trig, by, sched): (String, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT trigger, started_by_user_id, schedule_id FROM eval_runs WHERE agent_id = $1",
    )
    .bind(w.agent)
    .fetch_one(w.r.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (trig.as_str(), by, sched.map(|s| s.to_string())),
        ("schedule", None, Some(ids[0].clone()))
    );
    assert_eq!(outcome(&ids[0]).await.0.as_deref(), Some("ok"));
    // The run is active: another schedule's fire is skipped as busy.
    due(&ids[1]).await;
    w.tick().await;
    assert_eq!(outcome(&ids[1]).await.0.as_deref(), Some("skipped_busy"));
    // Frozen: skipped, no run.
    sqlx::query("UPDATE agents SET frozen_at = now(), frozen_reason = 'test', frozen_authority = 'workspace_owner' WHERE id = $1").bind(w.agent).execute(w.r.h.pool()).await.unwrap();
    due(&ids[2]).await;
    w.tick().await;
    assert_eq!(outcome(&ids[2]).await.0.as_deref(), Some("skipped_frozen"));
    sqlx::query("UPDATE agents SET frozen_at = NULL, frozen_reason = NULL, frozen_authority = NULL WHERE id = $1").bind(w.agent).execute(w.r.h.pool()).await.unwrap();
    // The creator loses the workspace: the anchor rule disables with `authorization`.
    sqlx::query("DELETE FROM workspace_access WHERE workspace_id = $1")
        .bind(w.r.ws)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE eval_runs SET status = 'done' WHERE agent_id = $1")
        .bind(w.agent)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    due(&ids[3]).await;
    w.tick().await;
    let (o, enabled, reason) = outcome(&ids[3]).await;
    assert_eq!(
        (o.as_deref(), enabled, reason.as_deref()),
        (Some("failed"), false, Some("authorization"))
    );
}

#[tokio::test]
async fn five_failed_fires_disable_a_schedule_with_failures() {
    let w = world().await;
    // No questions: every fire fails (nothing to run) and counts toward the streak.
    let (_, sch) = w
        .call(
            &w.member,
            Method::POST,
            &format!("/v1/agents/{}/eval-schedules", w.agent),
            Some(json!({"name": "n", "cron": "0 * * * *", "timezone": "UTC"})),
        )
        .await;
    let sid: Uuid = sch["id"].as_str().unwrap().parse().unwrap();
    for n in 1..=5 {
        sqlx::query("UPDATE eval_schedules SET next_fire_at = now() - interval '1 minute', fire_lease_until = NULL WHERE id = $1")
            .bind(sid)
            .execute(w.r.h.pool())
            .await
            .unwrap();
        w.tick().await;
        let (enabled, reason, k): (bool, Option<String>, i32) = sqlx::query_as(
            "SELECT enabled, disabled_reason, consecutive_failures FROM eval_schedules WHERE id = $1",
        )
        .bind(sid)
        .fetch_one(w.r.h.pool())
        .await
        .unwrap();
        if n < 5 {
            assert_eq!((enabled, reason.as_deref(), k), (true, None, n), "fire {n}");
        } else {
            assert_eq!((enabled, reason.as_deref()), (false, Some("failures")));
        }
    }
}

#[tokio::test]
async fn retention_removes_old_runs_and_their_eval_conversations() {
    let w = world().await;
    w.numeric("q", 1.0).await;
    let (_, run) = w.start().await;
    let id = run["id"].as_str().unwrap().to_owned();
    w.drive(&id, &["ANSWER: 1"]).await;
    let c: i64 = w.scalar("SELECT count(*) FROM conversations WHERE kind = 'eval' AND external_ref LIKE 'eval:' || $1 || ':%'", &id).await;
    assert_eq!(c, 1);
    // Younger than the window: kept.
    let n = aurora_api::jobs::sweep_eval_retention(w.r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, Some(0));
    sqlx::query(
        "UPDATE eval_runs SET finished_at = now() - interval '91 days' WHERE id = $1::uuid",
    )
    .bind(&id)
    .execute(w.r.h.pool())
    .await
    .unwrap();
    let n = aurora_api::jobs::sweep_eval_retention(w.r.h.pool())
        .await
        .unwrap();
    assert_eq!(n, Some(1));
    assert_eq!(
        w.scalar("SELECT count(*) FROM eval_runs WHERE id = $1::uuid", &id)
            .await,
        0
    );
    assert_eq!(w.scalar("SELECT count(*) FROM conversations WHERE kind = 'eval' AND external_ref LIKE 'eval:' || $1 || ':%'", &id).await, 0);
    assert_eq!(
        w.scalar(
            "SELECT count(*) FROM turns WHERE source_event_id LIKE 'eval:' || $1 || ':%'",
            &id
        )
        .await,
        0
    );
}

#[tokio::test]
async fn deleting_a_run_needs_an_owner_and_removes_its_conversations() {
    let w = world().await;
    w.numeric("q", 1.0).await;
    let (_, run) = w.start().await;
    let id = run["id"].as_str().unwrap().to_owned();
    w.drive(&id, &["ANSWER: 1"]).await;
    let (s, _) = w
        .call(
            &w.member,
            Method::DELETE,
            &format!("/v1/eval-runs/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 403);
    let (s, _) = w
        .call(
            &w.owner,
            Method::DELETE,
            &format!("/v1/eval-runs/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_eq!(w.scalar("SELECT count(*) FROM conversations WHERE kind = 'eval' AND external_ref LIKE 'eval:' || $1 || ':%'", &id).await, 0);
    let (s, _) = w
        .call(&w.member, Method::GET, &format!("/v1/eval-runs/{id}"), None)
        .await;
    assert_eq!(s, 404);
}
