#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! TEST-017: a hosted `claude-code` agent answers through the gateway LLM lane. Gated by
//! `AURORA_TEST_PODMAN=1` and `AURORA_TEST_LLM=1`; run via `scripts/e2e/llm.sh`.
//!
//! The model endpoint is the real LiteLLM proxy in front of VidaiMock (decision I-30): a scripted
//! stand-in, not a model. The scenario is picked by the model name (`scn-<id>`).
mod common;

use common::*;
use serde_json::{Value, json};

fn llm_enabled() -> bool {
    let on = std::env::var("AURORA_TEST_LLM").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: AURORA_TEST_LLM != 1");
    }
    on && enabled()
}

fn var(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} not set: run scripts/e2e/llm.sh"))
}

const ENDPOINT: &str = "litellm-mock";

/// Stack with one LiteLLM endpoint; the owner holds an org-less workspace LLM key granted to Daneel,
/// whose harness is `claude-code`. `model` is the agent's model override (= VidaiMock scenario).
async fn fx(model: &str) -> (Stack, Api) {
    Stack::wipe();
    let base = var("AURORA_LLM_BASE_URL");
    let toml = format!(
        r#"
[llm]
enabled_providers = ["litellm"]
provider_preference = ["litellm"]

[[llm.endpoints]]
id = "{ENDPOINT}"
provider = "litellm"
display_name = "LiteLLM (VidaiMock)"
base_url = "{base}"
dialects = ["anthropic_messages"]
default_model = "scn-pong"
"#
    );
    let st = Stack::start_with(Opts::default(), &toml).await;
    let api = st.signup("owner@e2e.test").await;
    sqlx::query(
        "UPDATE agents SET harness = 'claude-code', model_provider = 'litellm', \
         model_endpoint_id = $2, model_id = $3 WHERE id = $1::uuid",
    )
    .bind(&api.daneel)
    .bind(ENDPOINT)
    .bind(model)
    .execute(st.pool())
    .await
    .unwrap();
    let (s, v) = api
        .post(
            &format!("/v1/workspaces/{}/secrets", api.ws),
            json!({"name": "litellm-mock-key", "kind": "llm",
                   "llm": {"endpoint_id": ENDPOINT, "auth_mode": "api_key"},
                   "value": var("AURORA_LLM_KEY")}),
        )
        .await;
    assert_eq!(s, 201, "llm key: {v}\n{}", st.log_tails());
    let secret = v["id"].as_str().unwrap().to_owned();
    api.grant(&api.daneel, &secret).await;
    (st, api)
}

async fn run_turn(st: &Stack, api: &Api, conv: &str, text: &str) -> (String, Vec<Value>) {
    let v = api.say(conv, text).await;
    let turn = v["turn"]["id"].as_str().unwrap().to_owned();
    let (status, err) = st.wait_turn(&turn, &["done", "failed"], 240).await;
    if status != "done" {
        eprintln!("{}", diagnostics(st).await);
    }
    assert_eq!(
        (status.as_str(), err),
        ("done", None),
        "{}\n{}",
        st.log_tails(),
        podman_logs()
    );
    (turn, api.events(conv).await)
}

/// What went wrong, for a failed turn: the turn's error, the gateway's request log, sandbox logs.
async fn diagnostics(st: &Stack) -> String {
    let turns: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT status, error_code, error_message FROM turns")
            .fetch_all(st.pool())
            .await
            .unwrap_or_default();
    #[allow(clippy::type_complexity)]
    let reqs: Vec<(String, String, String, Option<i32>, i32, Option<String>)> = sqlx::query_as(
        "SELECT host, path, decision, upstream_status, status, error_code FROM request_log ORDER BY ts",
    )
    .fetch_all(st.pool())
    .await
    .unwrap_or_default();
    format!(
        "DIAG turns={turns:?}\nDIAG request_log={reqs:?}\nDIAG sandbox logs:\n{}",
        podman_logs()
    )
}

fn podman_logs() -> String {
    containers()
        .iter()
        .map(|c| {
            let (_, l) = podman_rc(&["logs", "--tail", "40", c]);
            format!("--- {c} ---\n{l}")
        })
        .collect()
}

#[tokio::test]
async fn l1_claude_code_answers_through_gateway_llm_lane() {
    if !llm_enabled() {
        return;
    }
    let (st, api) = fx("scn-pong").await;
    let conv = api.direct().await;
    let (_turn, ev) = run_turn(&st, &api, &conv, "Reply with exactly the word: pong").await;
    let text = transcript_text(&ev);
    assert!(
        text.contains("pong"),
        "answer missing from transcript: {text}"
    );

    // The key was injected by the gateway; the sandbox only ever held a placeholder.
    let key = var("AURORA_LLM_KEY");
    assert!(!text.contains(&key));
    for c in containers() {
        let (_, env) = podman_rc(&["inspect", "--format", "{{json .Config.Env}}", &c]);
        assert!(!env.contains(&key), "key leaked into container env");
    }
    // Gateway attribution: the request log (written asynchronously) names the LLM key and this
    // conversation for the proxied model call.
    let host = var("AURORA_LLM_BASE_URL")
        .trim_start_matches("http://")
        .to_owned();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let rows = loop {
        #[allow(clippy::type_complexity)]
        let rows: Vec<(String, Option<String>, Option<String>, i32)> = sqlx::query_as(
            "SELECT path, conversation_id::text, secret_id::text, status FROM request_log \
             WHERE host LIKE $1 AND path LIKE '/v1/messages%'",
        )
        .bind(format!("{}%", host.split(':').next().unwrap()))
        .fetch_all(st.pool())
        .await
        .unwrap();
        if !rows.is_empty() || std::time::Instant::now() > end {
            break rows;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    };
    assert!(
        rows.iter()
            .any(|(_, c, s, status)| c.as_deref() == Some(conv.as_str())
                && s.is_some()
                && *status == 200),
        "no attributed 200 /v1/messages row in request_log: {rows:?}\n{}\n{}",
        diagnostics(&st).await,
        st.log_tails()
    );
    st.finish().await;
}

#[tokio::test]
async fn l2_tool_call_with_real_arguments_through_the_platform_bridge() {
    if !llm_enabled() {
        return;
    }
    let (st, api) = fx("scn-memory-tool").await;
    let conv = api.direct().await;
    let (_turn, ev) = run_turn(
        &st,
        &api,
        &conv,
        "Use the memory_save platform tool to save the note 'conformance', then say done.",
    )
    .await;
    let text = transcript_text(&ev);
    assert!(text.contains("memory_save"), "no tool event: {text}");
    assert!(text.contains("done"), "final answer missing: {text}");
    // The real bridge -> supervisor -> runner -> API path executed the call with the scenario's arguments.
    let content: String = sqlx::query_scalar(
        "SELECT content FROM memories WHERE agent_id = $1::uuid AND key = 'conformance'",
    )
    .bind(&api.daneel)
    .fetch_one(st.pool())
    .await
    .expect("memory_save ran with the scripted arguments");
    assert_eq!(content, "Saved by the conformance run.");
    st.finish().await;
}
