#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-EVAL-3 (D-292, C-862, C-869, SEC-115; docs/18 §Evals): judged questions scored through
//! `PlatformLlm` with the agent's granted judge key. The provider is a wiremock LiteLLM endpoint
//! speaking Anthropic Messages (the dialect the `fake`/`claude-code` harness shares); the harness
//! is played by the test as in `evals_runs.rs`.

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use aurora_api::domains::conversations::service as conv;
use aurora_api::domains::hosted::lifecycle;
use aurora_api::domains::llm::platform::{self, Failure, Purpose, Request};
use aurora_config::LlmEndpoint;
use http::Method;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

const KEY: &str = "sk-judge-secret-4d2f";
const CANARY: &str = "canary-other-77e1";

/// Answers each judge call with the next text of the queue, with a fixed usage.
struct Seq(Mutex<VecDeque<String>>);

impl Respond for Seq {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        let t = self.0.lock().unwrap().pop_front().unwrap_or_default();
        ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": t}],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }))
    }
}

fn verdict(v: &str, why: &str) -> String {
    json!({"verdict": v, "rationale": why}).to_string()
}

struct W {
    r: Rig,
    agent: Uuid,
    member: String,
    owner: String,
    admin: String,
    judge: MockServer,
    other: MockServer,
}

fn endpoint(id: &str, base: &str) -> LlmEndpoint {
    LlmEndpoint {
        id: id.into(),
        provider: "litellm".into(),
        base_url: Some(base.into()),
        dialects: vec!["anthropic_messages".into()],
        default_model: Some("judge-1".into()),
        ..LlmEndpoint::default()
    }
}

async fn models(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "judge-1"}]})),
        )
        .mount(server)
        .await;
}

async fn world() -> W {
    let judge = MockServer::start().await;
    let other = MockServer::start().await;
    models(&judge).await;
    models(&other).await;
    let (ju, ou) = (judge.uri(), other.uri());
    let r = rig_with(move |c| {
        c.governance.allowed_key_scopes = vec!["organization".into()];
        c.llm.endpoints.push(endpoint("judge1", &ju));
        c.llm.endpoints.push(endpoint("judge2", &ou));
    })
    .await;
    let agent = r.agent("evaluee", true).await;
    let u = r.h.user("owner@x.io", false).await;
    r.h.member(r.org, u, "member", "active").await;
    r.h.bind_ws(r.ws, u, "owner").await;
    let owner = cookie(&r.h.good_session(u).await.1);
    let a = r.h.user("orgadmin@x.io", false).await;
    r.h.member(r.org, a, "admin", "active").await;
    let admin = cookie(&r.h.good_session(a).await.1);
    let member = r.cookie.clone();
    W {
        r,
        agent,
        member,
        owner,
        admin,
        judge,
        other,
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

    /// An org LLM key on `endpoint` (created through the API, so its value is really sealed),
    /// granted to the agent and set as the agent's judge.
    async fn judge_key(&self, endpoint: &str) -> Uuid {
        let (s, v) = self
            .call(
                &self.admin,
                Method::POST,
                &format!("/v1/orgs/{}/secrets", self.r.org),
                Some(json!({"name": format!("k-{endpoint}"), "kind": "llm",
                            "llm": {"endpoint_id": endpoint}, "value": KEY})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
        sqlx::query(
            "INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(Uuid::now_v7())
        .bind(self.r.ws)
        .bind(self.agent)
        .bind(id)
        .execute(self.r.h.pool())
        .await
        .unwrap();
        let (s, v) = self
            .call(
                &self.owner,
                Method::PUT,
                &format!("/v1/agents/{}/eval-settings", self.agent),
                Some(json!({"judge": {"secret_id": id, "model_id": "judge-1"}, "regression_notify": false})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        id
    }

    async fn judged(&self, q: &str, criteria: &str) {
        let (s, v) = self
            .call(
                &self.member,
                Method::POST,
                &format!("/v1/agents/{}/eval-questions", self.agent),
                Some(json!({"question": q, "expected_kind": "judged", "criteria": criteria})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
    }

    async fn start(&self) -> String {
        let (s, v) = self
            .call(
                &self.member,
                Method::POST,
                &format!("/v1/agents/{}/eval-runs", self.agent),
                Some(json!({})),
            )
            .await;
        assert_eq!(s, 202, "{v}");
        v["id"].as_str().unwrap().to_owned()
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

    async fn tick(&self) {
        self.r.work().await;
    }

    /// The harness answers the oldest active eval turn (an optional tool call first).
    async fn answer(&self, text: &str, tool_args: Option<&str>) {
        let (turn, conversation): (Uuid, Uuid) = sqlx::query_as(
            "SELECT id, conversation_id FROM turns WHERE source_event_id LIKE 'eval:%'
              AND status IN ('queued','dispatched','running') ORDER BY created_at, id LIMIT 1",
        )
        .fetch_one(self.r.h.pool())
        .await
        .expect("an active eval turn");
        let mut conn = self.r.h.pool().acquire().await.unwrap();
        if let Some(a) = tool_args {
            conv::append_event(
                &mut conn,
                conversation,
                Some(turn),
                "tool_call",
                &json!({"turn_id": turn, "call_id": "c1", "tool": "t", "arguments": {"x": a}}),
            )
            .await
            .unwrap();
        }
        conv::append_event(
            &mut conn,
            conversation,
            Some(turn),
            "text",
            &json!({"turn_id": turn, "text": text}),
        )
        .await
        .unwrap();
        lifecycle::finish(&mut conn, turn, "done", None, None, None)
            .await
            .unwrap()
            .expect("turn finished");
    }

    /// Answer every question in turn, waiting (with a deadline) for each judge task to write its
    /// result, then end the run (the request-log wait is skipped by backdating the results).
    async fn drive(&self, id: &str, answers: &[(&str, Option<&str>)]) {
        for (i, (text, tool)) in answers.iter().enumerate() {
            self.tick().await;
            self.answer(text, *tool).await;
            self.tick().await;
            let pos = i32::try_from(i + 1).unwrap();
            let mut done = false;
            for _ in 0..400 {
                let f: Option<bool> = sqlx::query_scalar(
                    "SELECT finished_at IS NOT NULL FROM eval_results WHERE run_id = $1::uuid AND position = $2",
                )
                .bind(id)
                .bind(pos)
                .fetch_optional(self.r.h.pool())
                .await
                .unwrap();
                if f == Some(true) {
                    done = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(done, "question {pos} was not scored in time");
        }
        sqlx::query(
            "UPDATE eval_results SET finished_at = finished_at - interval '1 minute' WHERE run_id = $1::uuid",
        )
        .bind(id)
        .execute(self.r.h.pool())
        .await
        .unwrap();
        for _ in 0..20 {
            self.tick().await;
            if self.run(id).await["status"] == "done" {
                return;
            }
        }
        panic!("run {id} did not end: {}", self.run(id).await);
    }

    /// `(outcome, reason_code)` per question.
    async fn outcomes(&self, id: &str) -> Vec<(String, Option<String>)> {
        sqlx::query_as(
            "SELECT outcome, reason_code FROM eval_results WHERE run_id = $1::uuid ORDER BY position",
        )
        .bind(id)
        .fetch_all(self.r.h.pool())
        .await
        .unwrap()
    }

    async fn posts(&self, s: &MockServer) -> Vec<wiremock::Request> {
        s.received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.method == http::Method::POST)
            .collect()
    }
}

fn is(got: &[(String, Option<String>)], want: &[(&str, Option<&str>)]) {
    let g: Vec<(&str, Option<&str>)> = got
        .iter()
        .map(|(o, r)| (o.as_str(), r.as_deref()))
        .collect();
    assert_eq!(g, want);
}

#[tokio::test]
async fn verdicts_prose_extra_keys_rationale_cap_and_usage() {
    let w = world().await;
    w.judge_key("judge1").await;
    let long = "r".repeat(1000);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(Seq(Mutex::new(VecDeque::from([
            verdict("pass", "meets the criteria"),
            verdict("fail", "line one\nline two"),
            "The answer looks fine to me.".to_owned(),
            json!({"verdict": "pass", "rationale": "x", "extra": 1}).to_string(),
            verdict("pass", &long),
        ]))))
        .mount(&w.judge)
        .await;
    for i in 1..=5 {
        w.judged(&format!("Question {i}?"), "mentions x").await;
    }
    let id = w.start().await;
    let a: Vec<(&str, Option<&str>)> = (1..=5).map(|_| ("an answer", None)).collect();
    w.drive(&id, &a).await;
    is(
        &w.outcomes(&id).await,
        &[
            ("pass", None),
            ("fail", None),
            ("error", Some("judge_unparseable")),
            ("error", Some("judge_unparseable")),
            ("pass", None),
        ],
    );
    let rat: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT judge_rationale FROM eval_results WHERE run_id = $1::uuid ORDER BY position",
    )
    .bind(&id)
    .fetch_all(w.r.h.pool())
    .await
    .unwrap();
    assert_eq!(rat[0].as_deref(), Some("meets the criteria"));
    assert_eq!(rat[1].as_deref(), Some("line one line two"), "one line");
    assert_eq!(rat[4].as_ref().map(|r| r.chars().count()), Some(300));
    // Usage: five calls, summed on the run; never on a turn.
    let v = w.run(&id).await;
    assert_eq!(
        v["judge_usage"],
        json!({"input_tokens": 50, "output_tokens": 25}),
        "{v}"
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM turns WHERE source_event_id LIKE 'eval:%' AND usage IS NOT NULL",
    )
    .fetch_one(w.r.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 0, "judge usage is not a turn's usage");
    // The pass rate counts the two verdicts and the unparseable ones are errors.
    assert_eq!(v["counts"]["errored"], 2);
}

#[tokio::test]
async fn the_judge_sees_only_its_question_criteria_and_own_answer_under_the_cap() {
    let w = world().await;
    w.judge_key("judge1").await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(Seq(Mutex::new(VecDeque::from([
            verdict("pass", "a"),
            verdict("pass", "b"),
        ]))))
        .mount(&w.judge)
        .await;
    w.judged("First question?", "criteria-one").await;
    w.judged("Second question?", "criteria-two").await;
    let id = w.start().await;
    let big = format!("{}{}", "a".repeat(9000), "TAIL-BEYOND-THE-CAP");
    w.drive(
        &id,
        &[
            (&big, Some(CANARY)),
            (&format!("second answer {CANARY}-answer"), None),
        ],
    )
    .await;
    let posts = w.posts(&w.judge).await;
    assert_eq!(posts.len(), 2);
    let b: Value = serde_json::from_slice(&posts[0].body).unwrap();
    assert_eq!(b["max_tokens"], 512);
    assert_eq!(b["stream"], false);
    assert!(b.get("tools").is_none(), "{b}");
    assert_eq!(b["messages"].as_array().unwrap().len(), 1);
    assert_eq!(b["messages"][0]["role"], "user");
    let m: Value = serde_json::from_str(b["messages"][0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        m.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        ["answer", "criteria", "question"].map(String::from).into()
    );
    assert_eq!(m["question"], "First question?");
    assert_eq!(m["criteria"], "criteria-one");
    assert!(m["answer"].as_str().unwrap().len() <= 8 * 1024);
    let raw = String::from_utf8(posts[0].body.clone()).unwrap();
    for absent in [
        CANARY,
        "TAIL-BEYOND-THE-CAP",
        "Second question?",
        "criteria-two",
        "second answer",
    ] {
        assert!(
            !raw.contains(absent),
            "{absent} reached the first judge request"
        );
    }
    // The key is the provider's header only; it is in no row of ours.
    assert_eq!(
        posts[0]
            .headers
            .get("authorization")
            .map(|v| v.to_str().unwrap().to_owned()),
        Some(format!("Bearer {KEY}"))
    );
    for sql in [
        "SELECT count(*) FROM audit_log WHERE metadata::text LIKE '%' || $1 || '%'",
        "SELECT count(*) FROM eval_results WHERE to_jsonb(eval_results)::text LIKE '%' || $1 || '%'",
        "SELECT count(*) FROM eval_runs WHERE to_jsonb(eval_runs)::text LIKE '%' || $1 || '%'",
        "SELECT count(*) FROM request_log WHERE to_jsonb(request_log)::text LIKE '%' || $1 || '%'",
    ] {
        let n: i64 = sqlx::query_scalar(sql)
            .bind(KEY)
            .fetch_one(w.r.h.pool())
            .await
            .unwrap();
        assert_eq!(n, 0, "{sql}");
    }
    assert!(!w.run(&id).await.to_string().contains(KEY));
}

#[tokio::test]
async fn a_judge_that_cannot_be_used_is_an_error_and_no_request_is_sent() {
    let w = world().await;
    let key = w.judge_key("judge1").await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(Seq(Mutex::new(VecDeque::from([verdict("pass", "ok")]))))
        .mount(&w.judge)
        .await;
    for i in 1..=4 {
        w.judged(&format!("Question {i}?"), "c").await;
    }
    let id = w.start().await;
    // Question 1 is judged normally; then each way the key can stop being usable, one question each.
    w.tick().await;
    w.answer("a1", None).await;
    w.tick().await;
    for _ in 0..400 {
        if w.outcomes(&id).await[0].0 != "not_scored" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // 2: the grant is revoked after the settings were saved.
    sqlx::query("DELETE FROM grants WHERE secret_id = $1")
        .bind(key)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    w.tick().await;
    w.answer("a2", None).await;
    w.tick().await;
    w.tick().await;
    // 3: granted again but the key is no longer active.
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)")
        .bind(Uuid::now_v7())
        .bind(w.r.ws)
        .bind(w.agent)
        .bind(key)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE secrets SET status = 'rejected' WHERE id = $1")
        .bind(key)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    w.tick().await;
    w.answer("a3", None).await;
    w.tick().await;
    w.tick().await;
    // 4: active again but governance no longer admits its scope (a workspace-scoped key).
    sqlx::query("UPDATE secrets SET status = 'active', workspace_id = $2 WHERE id = $1")
        .bind(key)
        .bind(w.r.ws)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    w.tick().await;
    w.answer("a4", None).await;
    w.tick().await;
    w.tick().await;
    for _ in 0..400 {
        if w.outcomes(&id).await.iter().all(|o| o.0 != "not_scored") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    is(
        &w.outcomes(&id).await,
        &[
            ("pass", None),
            ("error", Some("judge_not_granted")),
            ("error", Some("judge_not_usable")),
            ("error", Some("judge_not_permitted")),
        ],
    );
    assert_eq!(
        w.posts(&w.judge).await.len(),
        1,
        "only question 1 reached the provider"
    );
}

#[tokio::test]
async fn a_failing_judge_is_judge_unavailable_with_no_fallback_and_the_run_continues() {
    let w = world().await;
    w.judge_key("judge1").await;
    // A second, healthy provider with a granted key must never be called.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(Seq(Mutex::new(VecDeque::from([verdict(
            "pass",
            "from the wrong provider",
        )]))))
        .mount(&w.other)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&w.judge)
        .await;
    let (s, v) = w
        .call(&w.admin, Method::POST, &format!("/v1/orgs/{}/secrets", w.r.org),
            Some(json!({"name": "k-other", "kind": "llm", "llm": {"endpoint_id": "judge2"}, "value": "sk-other"})))
        .await;
    assert_eq!(s, 201, "{v}");
    let other: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)")
        .bind(Uuid::now_v7())
        .bind(w.r.ws)
        .bind(w.agent)
        .bind(other)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    w.judged("Question 1?", "c").await;
    w.judged("Question 2?", "c").await;
    let id = w.start().await;
    w.drive(&id, &[("a1", None), ("a2", None)]).await;
    is(
        &w.outcomes(&id).await,
        &[
            ("error", Some("judge_unavailable")),
            ("error", Some("judge_unavailable")),
        ],
    );
    assert_eq!(w.posts(&w.judge).await.len(), 2, "no retry");
    assert!(w.posts(&w.other).await.is_empty(), "no fallback provider");
}

#[tokio::test]
async fn a_judged_question_without_a_judge_is_judge_not_configured() {
    let w = world().await;
    w.judged("Question?", "c").await;
    let id = w.start().await;
    w.tick().await;
    w.answer("a", None).await;
    w.tick().await;
    for _ in 0..20 {
        w.tick().await;
    }
    is(
        &w.outcomes(&id).await,
        &[("error", Some("judge_not_configured"))],
    );
    assert!(w.posts(&w.judge).await.is_empty());
}

/// The call itself: a hanging provider ends at the request's timeout as `Timeout`; the answer
/// carries the output cap; a refusal sends nothing.
#[tokio::test]
async fn platform_llm_times_out_and_refuses_before_any_request() {
    let w = world().await;
    let key = w.judge_key("judge1").await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
        .mount(&w.judge)
        .await;
    let agent = aurora_db::repo::agents::get(w.r.h.pool(), w.agent)
        .await
        .unwrap()
        .unwrap();
    let req = |t| Request {
        model: "judge-1".into(),
        system: "s",
        user: "u".into(),
        max_tokens: 512,
        timeout: t,
    };
    let t = std::time::Instant::now();
    let r = platform::complete(
        &w.r.h.state,
        Purpose::EvalJudge,
        &agent,
        key,
        req(Duration::from_millis(300)),
    )
    .await;
    assert_eq!(r.err(), Some(Failure::Timeout));
    assert!(t.elapsed() < Duration::from_secs(5));
    // A key that is not the agent's own is refused before any request.
    let before = w.posts(&w.judge).await.len();
    let r = platform::complete(
        &w.r.h.state,
        Purpose::EvalJudge,
        &agent,
        Uuid::now_v7(),
        req(Duration::from_secs(1)),
    )
    .await;
    assert_eq!(r.err(), Some(Failure::NotGranted));
    assert_eq!(w.posts(&w.judge).await.len(), before);
}

/// FIX-165: the LLM client does not follow redirects, so a provider answering 3xx cannot move the
/// key to another host: neither on key verification nor on the judge's completion call.
#[tokio::test]
async fn a_redirecting_provider_never_gets_the_key_to_another_host() {
    let w = world().await;
    let key = w.judge_key("judge1").await;
    // From here on the judge endpoint redirects everything to the other server.
    w.judge.reset().await;
    w.other.reset().await;
    let to = |p: &str| {
        ResponseTemplate::new(307).insert_header("location", format!("{}{p}", w.other.uri()))
    };
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(to("/v1/messages"))
        .mount(&w.judge)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(to("/v1/models"))
        .mount(&w.judge)
        .await;
    // The judge call: an upstream failure, nothing reaches the other server.
    let agent = aurora_db::repo::agents::get(w.r.h.pool(), w.agent)
        .await
        .unwrap()
        .unwrap();
    let r = platform::complete(
        &w.r.h.state,
        Purpose::EvalJudge,
        &agent,
        key,
        Request {
            model: "judge-1".into(),
            system: "s",
            user: "u".into(),
            max_tokens: 512,
            timeout: Duration::from_secs(5),
        },
    )
    .await;
    assert_eq!(r.err(), Some(Failure::Unavailable));
    assert_eq!(w.posts(&w.judge).await.len(), 1);
    // Key verification: a new key on the redirecting endpoint is not verified (no origin rule:
    // saved `pending`), and nothing is asked of the other server.
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/orgs/{}/secrets", w.r.org),
            Some(json!({"name": "k-redirected", "kind": "llm",
                        "llm": {"endpoint_id": "judge1"}, "value": KEY})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["origin"]["status"], "pending", "{v}");
    assert!(
        !w.judge.received_requests().await.unwrap().is_empty(),
        "the redirecting endpoint was asked"
    );
    let seen = w.other.received_requests().await.unwrap();
    assert!(
        seen.is_empty(),
        "the redirect target received {} request(s): {:?}",
        seen.len(),
        seen.iter()
            .map(|r| r.url.path().to_owned())
            .collect::<Vec<_>>()
    );
}
