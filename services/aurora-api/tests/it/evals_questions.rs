#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-EVAL-1 (D-289, D-291, D-292, C-861, C-863, C-866, C-883, SEC-115, SEC-116; docs/18 §Evals):
//! eval questions, the agent's eval settings and the fingerprint route, over the real router and
//! database. Expected answers are agent-audience data: a canary expected answer must appear in no
//! audit row, no request-log row and nothing a sandbox is given.

use crate::common::*;
use crate::hosted::{Rig, rig_with};
use base64::Engine as _;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const CANARY: &str = "canary-expected-answer-7d3f19";

struct W {
    r: Rig,
    agent: Uuid,
    /// A plain workspace member (`can_use`).
    member: String,
    owner: String,
    /// In the org, not in the workspace.
    stranger: String,
}

async fn world() -> W {
    let r = rig_with(|c| {
        // A workspace-scoped key is refused by governance (`scope_not_permitted`).
        c.governance.allowed_key_scopes = vec!["organization".into()];
    })
    .await;
    let agent = r.agent("evaluee", false).await;
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

    async fn post_q(&self, body: Value) -> (u16, Value) {
        self.call(
            &self.member,
            Method::POST,
            &format!("/v1/agents/{}/eval-questions", self.agent),
            Some(body),
        )
        .await
    }

    async fn list(&self) -> Vec<Value> {
        let (s, v) = self
            .call(
                &self.member,
                Method::GET,
                &format!("/v1/agents/{}/eval-questions", self.agent),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["questions"].as_array().unwrap().clone()
    }

    async fn settings(&self, ck: &str, body: Value) -> (u16, Value) {
        self.call(
            ck,
            Method::PUT,
            &format!("/v1/agents/{}/eval-settings", self.agent),
            Some(body),
        )
        .await
    }

    /// An LLM secret; `grant` it to the agent; `status` and `workspace` shape its readiness.
    async fn key(&self, name: &str, grant: bool, status: &str, workspace: bool) -> Uuid {
        let sec = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO secrets (id, org_id, workspace_id, name, kind, llm_provider, llm_auth_mode,
                llm_endpoint_id, host_pattern, injection, value_source, value_ref, status)
             VALUES ($1,$2,$3,$4,'llm','anthropic','api_key','anthropic','api.anthropic.com','{}','managed','x',$5)",
        )
        .bind(sec)
        .bind(self.r.org)
        .bind(workspace.then_some(self.r.ws))
        .bind(name)
        .bind(status)
        .execute(self.r.h.pool())
        .await
        .unwrap();
        if grant {
            sqlx::query(
                "INSERT INTO grants (id, workspace_id, agent_id, secret_id) VALUES ($1,$2,$3,$4)",
            )
            .bind(Uuid::now_v7())
            .bind(self.r.ws)
            .bind(self.agent)
            .bind(sec)
            .execute(self.r.h.pool())
            .await
            .unwrap();
        }
        sec
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

fn numeric(q: &str) -> Value {
    json!({"question": q, "expected_kind": "numeric", "expected_number": 42.5, "tolerance": 0.5})
}

#[tokio::test]
async fn one_question_of_each_kind_is_created_in_order() {
    let w = world().await;
    let (s, n) = w.post_q(numeric("How many?")).await;
    assert_eq!(s, 201, "{n}");
    assert_eq!(n["position"], 1);
    assert_eq!(n["version"], 1);
    assert_eq!(n["expected_number"], 42.5);
    assert_eq!(n["tolerance"], 0.5);
    assert_eq!(n["agent"]["id"], w.agent.to_string());
    let (s, t) = w
        .post_q(json!({"question": "Name?", "expected_kind": "text", "expected_text": "Ada"}))
        .await;
    assert_eq!(s, 201, "{t}");
    assert_eq!(t["position"], 2);
    assert_eq!(t["expected_text"], "Ada");
    assert!(t["expected_number"].is_null() && t["criteria"].is_null());
    let (s, j) = w
        .post_q(
            json!({"question": "Explain.", "expected_kind": "judged", "criteria": "Mentions X"}),
        )
        .await;
    assert_eq!(s, 201, "{j}");
    assert_eq!(j["position"], 3);
    assert_eq!(j["enabled"], true);
    // A numeric question with no tolerance has 0.
    let (_, z) = w
        .post_q(json!({"question": "Z", "expected_kind": "numeric", "expected_number": 1}))
        .await;
    assert_eq!(z["tolerance"], 0.0);
    let l = w.list().await;
    assert_eq!(
        l.iter()
            .map(|q| q["position"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
}

#[tokio::test]
async fn every_invalid_shape_is_422_invalid_with_its_field() {
    let w = world().await;
    let long = |n: usize| "x".repeat(n);
    let cases: Vec<(Value, &str)> = vec![
        (
            json!({"question": "q", "expected_kind": "judged"}),
            "criteria",
        ),
        (
            json!({"question": "q", "expected_kind": "numeric"}),
            "expected_number",
        ),
        (
            json!({"question": "q", "expected_kind": "numeric", "expected_number": 1, "tolerance": -0.1}),
            "tolerance",
        ),
        (
            json!({"question": "q", "expected_kind": "text", "expected_text": long(1001)}),
            "expected_text",
        ),
        (
            json!({"question": "q", "expected_kind": "judged", "criteria": long(1001)}),
            "criteria",
        ),
        (
            json!({"question": long(2001), "expected_kind": "judged", "criteria": "c"}),
            "question",
        ),
        (
            json!({"question": "  ", "expected_kind": "judged", "criteria": "c"}),
            "question",
        ),
        (
            json!({"question": "q", "expected_kind": "text", "expected_text": "a", "criteria": "c"}),
            "criteria",
        ),
        (
            json!({"question": "q", "expected_kind": "numeric", "expected_number": 1, "expected_text": "a"}),
            "expected_text",
        ),
        (
            json!({"question": "q", "expected_kind": "judged", "criteria": "c", "expected_number": 1}),
            "expected_number",
        ),
        (
            json!({"question": "q", "expected_kind": "essay"}),
            "expected_kind",
        ),
    ];
    for (body, field) in cases {
        let (s, v) = w.post_q(body.clone()).await;
        assert_eq!((s, code(&v)), (422, "invalid"), "{body} → {v}");
        assert_eq!(v["error"]["details"]["field"], field, "{body} → {v}");
    }
    // The limits themselves are accepted.
    let (s, v) = w
        .post_q(
            json!({"question": long(2000), "expected_kind": "text", "expected_text": long(1000)}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(w.list().await.len(), 1);
}

#[tokio::test]
async fn the_101st_question_is_refused() {
    let w = world().await;
    sqlx::query(
        "INSERT INTO eval_questions (id, agent_id, workspace_id, position, question, expected_kind, criteria)
         SELECT gen_random_uuid(), $1, $2, g, 'q' || g, 'judged', 'c' FROM generate_series(1, 100) g",
    )
    .bind(w.agent)
    .bind(w.r.ws)
    .execute(w.r.h.pool())
    .await
    .unwrap();
    let (s, v) = w.post_q(numeric("one too many")).await;
    assert_eq!((s, code(&v)), (422, "eval_question_limit"), "{v}");
    assert_eq!(w.list().await.len(), 100);
    // Deleting one makes room.
    let id = w.list().await[0]["id"].as_str().unwrap().to_owned();
    let (s, _) = w
        .call(
            &w.member,
            Method::DELETE,
            &format!("/v1/eval-questions/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_eq!(w.post_q(numeric("fits")).await.0, 201);
}

#[tokio::test]
async fn patch_bumps_version_and_positions_stay_unique_and_contiguous() {
    let w = world().await;
    let (_, a) = w.post_q(numeric("A")).await;
    let (_, b) = w
        .post_q(json!({"question": "B", "expected_kind": "text", "expected_text": "b"}))
        .await;
    let (_, c) = w
        .post_q(json!({"question": "C", "expected_kind": "judged", "criteria": "c"}))
        .await;
    let (a, b, c) = (
        a["id"].as_str().unwrap(),
        b["id"].as_str().unwrap(),
        c["id"].as_str().unwrap(),
    );
    let q = |id: &str| format!("/v1/eval-questions/{id}");
    let (s, v) = w
        .call(
            &w.member,
            Method::PATCH,
            &q(a),
            Some(json!({"question": "A2", "tolerance": 2})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (
            v["version"].clone(),
            v["question"].clone(),
            v["tolerance"].clone()
        ),
        (json!(2), json!("A2"), json!(2.0))
    );
    // An unchanged PATCH (only a move) leaves the version alone.
    let (s, v) = w
        .call(
            &w.member,
            Method::PATCH,
            &q(c),
            Some(json!({"position": 1})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["position"].clone(), v["version"].clone()),
        (json!(1), json!(1))
    );
    let order = |l: &[Value]| {
        l.iter()
            .map(|x| x["question"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let l = w.list().await;
    assert_eq!(order(&l), ["C", "A2", "B"]);
    assert_eq!(
        l.iter()
            .map(|x| x["position"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    // Beyond the end clamps to the last place.
    let (_, v) = w
        .call(
            &w.member,
            Method::PATCH,
            &q(c),
            Some(json!({"position": 99})),
        )
        .await;
    assert_eq!(v["position"], 3);
    assert_eq!(order(&w.list().await), ["A2", "B", "C"]);
    // A field of another kind is refused; so is a bad value.
    for (body, field) in [
        (json!({"expected_text": "x"}), "expected_text"),
        (json!({"tolerance": -1}), "tolerance"),
        (json!({"question": ""}), "question"),
    ] {
        let (s, v) = w
            .call(&w.member, Method::PATCH, &q(a), Some(body.clone()))
            .await;
        assert_eq!((s, code(&v)), (422, "invalid"), "{body} → {v}");
        assert_eq!(v["error"]["details"]["field"], field);
    }
    let (s, v) = w
        .call(
            &w.member,
            Method::PATCH,
            &q(b),
            Some(json!({"tolerance": 1})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "invalid"), "{v}");
    // Delete closes the gap and never leaves a duplicate position.
    let (s, _) = w.call(&w.member, Method::DELETE, &q(a), None).await;
    assert_eq!(s, 204);
    let l = w.list().await;
    assert_eq!(order(&l), ["B", "C"]);
    assert_eq!(
        l.iter()
            .map(|x| x["position"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2]
    );
    let (s, v) = w.call(&w.member, Method::GET, &q(a), None).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
}

#[tokio::test]
async fn a_caller_without_can_use_gets_404_everywhere_and_a_member_sees_the_expected_answers() {
    let w = world().await;
    let (_, qv) = w
        .post_q(json!({"question": "Secret?", "expected_kind": "text", "expected_text": CANARY}))
        .await;
    let id = qv["id"].as_str().unwrap().to_owned();
    let a = w.agent;
    let routes: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, format!("/v1/agents/{a}/eval-questions"), None),
        (
            Method::POST,
            format!("/v1/agents/{a}/eval-questions"),
            Some(numeric("q")),
        ),
        (Method::GET, format!("/v1/eval-questions/{id}"), None),
        (
            Method::PATCH,
            format!("/v1/eval-questions/{id}"),
            Some(json!({"enabled": false})),
        ),
        (Method::DELETE, format!("/v1/eval-questions/{id}"), None),
        (Method::GET, format!("/v1/agents/{a}/eval-settings"), None),
        (
            Method::PUT,
            format!("/v1/agents/{a}/eval-settings"),
            Some(json!({"judge": null, "regression_notify": false})),
        ),
        (
            Method::GET,
            format!("/v1/agents/{a}/eval-fingerprint"),
            None,
        ),
    ];
    for (m, uri, body) in routes {
        let (s, v) = w.call(&w.stranger, m.clone(), &uri, body).await;
        assert_eq!((s, code(&v)), (404, "not_found"), "{m} {uri} → {v}");
        assert!(!v.to_string().contains(CANARY));
    }
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/eval-questions/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["expected_text"], CANARY);
    assert_eq!(w.list().await[0]["expected_text"], CANARY);
}

#[tokio::test]
async fn a_byo_agent_has_no_evals() {
    let w = world().await;
    let byo = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'byo','byo','byo')")
        .bind(byo).bind(w.r.org).bind(w.r.ws)
        .execute(w.r.h.pool()).await.unwrap();
    for (m, suffix, body) in [
        (Method::GET, "eval-questions", None),
        (Method::POST, "eval-questions", Some(numeric("q"))),
        (Method::GET, "eval-settings", None),
        (Method::GET, "eval-fingerprint", None),
    ] {
        let (s, v) = w
            .call(
                &w.member,
                m.clone(),
                &format!("/v1/agents/{byo}/{suffix}"),
                body,
            )
            .await;
        assert_eq!(
            (s, code(&v)),
            (422, "eval_not_supported"),
            "{m} {suffix} → {v}"
        );
    }
}

/// SEC-116: the expected answer is in the question's responses and rows only.
#[tokio::test]
async fn a_canary_expected_answer_reaches_no_audit_row_request_log_or_sandbox() {
    let w = world().await;
    let agent = w.r.agent("bootee", true).await;
    for body in [
        json!({"question": "Q?", "expected_kind": "text", "expected_text": CANARY}),
        json!({"question": "Q2?", "expected_kind": "judged", "criteria": CANARY}),
    ] {
        let (s, v) = w
            .call(
                &w.member,
                Method::POST,
                &format!("/v1/agents/{agent}/eval-questions"),
                Some(body),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        let id = v["id"].as_str().unwrap();
        let (s, _) = w
            .call(
                &w.member,
                Method::PATCH,
                &format!("/v1/eval-questions/{id}"),
                Some(json!({"enabled": false})),
            )
            .await;
        assert_eq!(s, 200);
    }
    // The three audit rows are best-effort: wait for them.
    for _ in 0..200 {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action LIKE 'eval.question.%'",
        )
        .fetch_one(w.r.h.pool())
        .await
        .unwrap();
        if n >= 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let audit: Vec<String> =
        sqlx::query_scalar("SELECT audit_log::text FROM audit_log WHERE action LIKE 'eval.%'")
            .fetch_all(w.r.h.pool())
            .await
            .unwrap();
    assert!(audit.len() >= 4, "{audit:?}");
    assert!(
        audit
            .iter()
            .all(|r| !r.contains(CANARY) && !r.contains("Q?")),
        "{audit:?}"
    );
    let log: Vec<String> = sqlx::query_scalar("SELECT request_log::text FROM request_log")
        .fetch_all(w.r.h.pool())
        .await
        .unwrap();
    assert!(log.iter().all(|r| !r.contains(CANARY)));
    // What the runner is given for this agent's sandbox: the start item with its boot document,
    // and the home sync parts after ready.
    let conv = w.r.direct(agent).await;
    w.r.say(conv, "hello").await;
    let items = w.r.work().await;
    let start = items.iter().find(|i| i["kind"] == "sandbox.start").unwrap();
    let raw = start["spec"]["boot_secret"].as_str().unwrap();
    let doc = String::from_utf8(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw)
            .unwrap(),
    )
    .unwrap();
    assert!(doc.contains("bootstrap_token"));
    assert!(!doc.contains(CANARY) && !start.to_string().contains(CANARY));
    let lv = crate::hosted::Live {
        agent,
        conv,
        sb: start["sandbox_id"].as_str().unwrap().to_owned(),
        boot: start["boot_id"].as_str().unwrap().to_owned(),
    };
    w.r.report(
        &lv,
        json!({"kind": "sandbox.status", "state": "running", "container_ref": "c"}),
    )
    .await;
    let tok: Value = serde_json::from_str(&doc).unwrap();
    let (s, _) =
        w.r.runner(
            Some(w.r.rep),
            "/v1/runner/channel/claim",
            json!({"bootstrap_token": tok["bootstrap_token"]}),
        )
        .await;
    assert_eq!(s, 200);
    let (s, _) =
        w.r.runner(
            Some(w.r.rep),
            "/v1/runner/channel/hold",
            json!({"sandbox_id": lv.sb, "boot_id": lv.boot}),
        )
        .await;
    assert_eq!(s, 200);
    w.r.report(&lv, json!({"kind": "supervisor.ready", "version": "0.1", "harness": {"id": "fake", "capabilities": ["steer", "resume"]}})).await;
    let sync = w.r.work().await;
    assert!(sync.iter().any(|i| i["kind"] == "home.sync"));
    assert!(!serde_json::to_string(&sync).unwrap().contains(CANARY));
}

#[tokio::test]
async fn the_judge_key_must_be_granted_usable_and_permitted_and_changing_it_needs_can_own() {
    let w = world().await;
    let judge = |s: Uuid| json!({"judge": {"secret_id": s, "model_id": "claude-judge"}, "regression_notify": true});
    // Defaults of an existing agent.
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/agents/{}/eval-settings", w.agent),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v, json!({"judge": null, "regression_notify": true}));
    let not_granted = w.key("k-ungranted", false, "active", false).await;
    let proposed = w.key("k-proposed", true, "proposed", false).await;
    let scoped = w.key("k-workspace", true, "active", true).await;
    let good = w.key("k-good", true, "active", false).await;
    // Even a can_own caller is refused for each reason.
    for (key, want) in [
        (not_granted, "eval_judge_not_granted"),
        (proposed, "eval_judge_not_usable"),
        (scoped, "eval_judge_not_permitted"),
        (Uuid::now_v7(), "eval_judge_not_granted"),
    ] {
        let (s, v) = w.settings(&w.owner, judge(key)).await;
        assert_eq!((s, code(&v)), (422, want), "{v}");
    }
    let (s, v) = w
        .settings(&w.owner, json!({"judge": {"secret_id": good, "model_id": "bad model!"}, "regression_notify": true}))
        .await;
    assert_eq!((s, code(&v)), (422, "invalid"), "{v}");
    // A plain member cannot set the judge.
    let (s, v) = w.settings(&w.member, judge(good)).await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    // The owner can; the answer names the key and says it is usable.
    let (s, v) = w.settings(&w.owner, judge(good)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["judge"]["secret_id"], good.to_string());
    assert_eq!(v["judge"]["secret_label"], "k-good");
    assert_eq!(v["judge"]["model_id"], "claude-judge");
    assert_eq!(v["judge"]["usable"], true);
    // A member may flip the notice while leaving the judge as it is, but not clear the judge.
    let (s, v) = w.settings(&w.member, json!({"judge": {"secret_id": good, "model_id": "claude-judge"}, "regression_notify": false})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["regression_notify"], false);
    let (s, v) = w
        .settings(
            &w.member,
            json!({"judge": null, "regression_notify": false}),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    // The grant is revoked: the stored judge reads as not usable.
    sqlx::query("DELETE FROM grants WHERE agent_id = $1 AND secret_id = $2")
        .bind(w.agent)
        .bind(good)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    let (_, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/agents/{}/eval-settings", w.agent),
            None,
        )
        .await;
    assert_eq!(v["judge"]["usable"], false);
    // `null` clears.
    let (s, v) = w
        .settings(&w.owner, json!({"judge": null, "regression_notify": true}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v, json!({"judge": null, "regression_notify": true}));
    // Audit: ids only, critical, written with the change.
    let rows: Vec<(Value, bool)> = sqlx::query_as("SELECT metadata, critical FROM audit_log WHERE action = 'eval.settings.update' ORDER BY created_at")
        .fetch_all(w.r.h.pool()).await.unwrap();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows.iter().all(|r| r.1));
    assert_eq!(rows[0].0["judge_changed"], true);
    assert_eq!(
        rows[0].0["change"]["judge_secret_id"]["to"],
        good.to_string()
    );
    assert_eq!(rows[1].0["judge_changed"], false);
}

#[tokio::test]
async fn the_fingerprint_route_serves_the_c866_object_and_follows_the_configuration() {
    let w = world().await;
    let uri = format!("/v1/agents/{}/eval-fingerprint", w.agent);
    let (s, v) = w.call(&w.member, Method::GET, &uri, None).await;
    assert_eq!(s, 200, "{v}");
    let fp = &v["fingerprint"];
    let mut keys: Vec<_> = fp.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    let mut want: Vec<_> = aurora_api::domains::evals::fingerprint::FIELDS
        .iter()
        .map(|k| (*k).to_owned())
        .collect();
    want.sort();
    assert_eq!(keys, want);
    assert_eq!(
        fp["model_roles"],
        json!({"large": null, "medium": null, "small": null})
    );
    assert_eq!(fp["subagent_model"], "inherit");
    assert_eq!(fp["harness"], "fake");
    assert_eq!(v["fingerprint_hash"].as_str().unwrap().len(), 64);
    assert_eq!(v["question_set_version"], fp["question_set_version"]);
    let hash0 = v["fingerprint_hash"].as_str().unwrap().to_owned();
    // Stable across calls.
    let (_, again) = w.call(&w.member, Method::GET, &uri, None).await;
    assert_eq!(again["fingerprint_hash"], hash0.as_str());
    // A question changes the set version, hence the hash; the text itself is nowhere in it.
    let (_, q) = w
        .post_q(json!({"question": "canary-question-text-5e1", "expected_kind": "text", "expected_text": CANARY}))
        .await;
    let (_, v1) = w.call(&w.member, Method::GET, &uri, None).await;
    assert_ne!(v1["fingerprint_hash"], hash0.as_str());
    assert!(!v1.to_string().contains("canary"));
    // A disabled question leaves the enabled set: the version returns to the first.
    let id = q["id"].as_str().unwrap();
    w.call(
        &w.member,
        Method::PATCH,
        &format!("/v1/eval-questions/{id}"),
        Some(json!({"enabled": false})),
    )
    .await;
    let (_, v2) = w.call(&w.member, Method::GET, &uri, None).await;
    assert_eq!(v2["fingerprint_hash"], hash0.as_str());
    // The brief, a skill and a granted rule each move it; the model and effort fields too.
    sqlx::query("UPDATE agents SET brief = 'be terse' WHERE id = $1")
        .bind(w.agent)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    let (_, v3) = w.call(&w.member, Method::GET, &uri, None).await;
    assert_ne!(v3["fingerprint_hash"], hash0.as_str());
    assert_eq!(v3["fingerprint"]["skills"], json!([]));
    sqlx::query("UPDATE agents SET effort = 'high', subagent_model = 'small' WHERE id = $1")
        .bind(w.agent)
        .execute(w.r.h.pool())
        .await
        .unwrap();
    let (_, v4) = w.call(&w.member, Method::GET, &uri, None).await;
    assert_eq!(v4["fingerprint"]["effort"], "high");
    assert_eq!(v4["fingerprint"]["subagent_model"], "small");
    assert_ne!(v4["fingerprint_hash"], v3["fingerprint_hash"]);
    let _: Uuid = w.agent;
}
