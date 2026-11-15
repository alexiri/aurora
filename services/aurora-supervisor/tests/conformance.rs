#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Harness conformance suite (`docs/12 §Harness interface`): the same cases run against every
//! adapter. Always against `fake`; against `claude-code` only with `AURORA_LIVE_CLAUDE=1`
//! (plus `ANTHROPIC_BASE_URL`, optional `AURORA_LIVE_MODEL`, `AURORA_LIVE_MCP_BRIDGE`), never in CI.
//!
//! Mock mode (decision I-30, `scripts/e2e/llm.sh`): the endpoint is the real LiteLLM proxy in front
//! of VidaiMock. `AURORA_LIVE_SCENARIO_MODELS=say=scn-pong,long=scn-count,tool=scn-memory-tool,
//! fail=scn-ratelimit,steer=scn-steer` picks the model (= VidaiMock scenario) per case, and adds a
//! provider-error case. `AURORA_LIVE_CUSTOM_HEADERS` is passed as `ANTHROPIC_CUSTOM_HEADERS` (the
//! supervisor suite has no gateway to inject the LiteLLM key). Without the variable nothing changes.

use std::time::Duration;

use aurora_protocol::runner::{ModelChoice, Recall};
use aurora_supervisor::adapters::claude_code::ClaudeCodeAdapter;
use aurora_supervisor::fake::FakeHarness;
use aurora_supervisor::harness::{
    HarnessAdapter, HarnessEvent, HarnessEventStream, McpBridgeSpec, SessionOptions, SteerAck,
    SteerInput, TurnInput,
};
use tokio::time::timeout;
use uuid::Uuid;

const T: Duration = Duration::from_secs(60);

/// Prompts that drive an adapter into each scenario. `None` = scenario not drivable (skipped).
struct Profile {
    say: &'static str,
    long_running: &'static str,
    /// `(prompt, expected harness error code)`.
    fail: Option<(&'static str, &'static str)>,
    tool: Option<&'static str>,
    steerable: Option<&'static str>,
}

const FAKE: Profile = Profile {
    say: "[[say: hello]] [[say: world]]",
    long_running: "[[say: start]] [[sleep: 30s]]",
    fail: Some(("[[fail: provider_quota]]", "provider_quota")),
    tool: Some("[[say: saving]] [[tool: memory_save {\"text\":\"x\"}]]"),
    steerable: Some("[[say: base]] [[steer-accept]]"),
};

const LIVE: Profile = Profile {
    say: "Reply with exactly the word: pong",
    long_running: "Count slowly from 1 to 500, one number per line, with a short comment each.",
    fail: None,
    tool: Some("Use the memory_save platform tool to save the note 'conformance', then say done."),
    steerable: None,
};

/// Mock mode: scenario model for a case (`say`, `long`, `tool`, `fail`, `steer`) from
/// `AURORA_LIVE_SCENARIO_MODELS`. `None` outside mock mode.
fn scenario_model(case: &str) -> Option<String> {
    let map = std::env::var("AURORA_LIVE_SCENARIO_MODELS").ok()?;
    let found = map
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| k.trim() == case)
        .map(|(_, v)| v.trim().to_owned());
    Some(found.unwrap_or_else(|| panic!("AURORA_LIVE_SCENARIO_MODELS has no `{case}=` entry")))
}

fn opts(case: &str, resume: Option<String>) -> SessionOptions {
    let endpoint = std::env::var("ANTHROPIC_BASE_URL").unwrap_or_default();
    SessionOptions {
        conversation_id: Uuid::now_v7(),
        working_dir: std::env::temp_dir(),
        resume_ref: resume,
        model: ModelChoice {
            provider: "litellm".into(),
            endpoint_id: "live".into(),
            base_url: endpoint,
            dialect: "anthropic_messages".into(),
            model: scenario_model(case).unwrap_or_else(|| {
                std::env::var("AURORA_LIVE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".into())
            }),
            effort: "low".into(),
            client_env: vec![],
        },
        proxy_url: "http://127.0.0.1:9".into(),
        mcp_bridge: McpBridgeSpec {
            command: std::env::var("AURORA_LIVE_MCP_BRIDGE")
                .map(|c| c.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default(),
        },
        env: mock_env(case),
    }
}

/// Extra child environment in mock mode only (empty otherwise).
fn mock_env(case: &str) -> Vec<(String, String)> {
    if scenario_model(case).is_none() {
        return vec![];
    }
    // The adapter points HTTP(S)_PROXY at a dead port (no gateway here); the mock is on loopback.
    let mut env = vec![("NO_PROXY".to_owned(), "127.0.0.1,localhost".to_owned())];
    if let Ok(h) = std::env::var("AURORA_LIVE_CUSTOM_HEADERS") {
        env.push(("ANTHROPIC_CUSTOM_HEADERS".to_owned(), h));
    }
    if case == "fail" {
        // The CLI retries 429/5xx with backoff for minutes; fail fast so the code is classified.
        env.push(("CLAUDE_CODE_MAX_RETRIES".to_owned(), "0".to_owned()));
    }
    env
}

fn turn(text: &str) -> TurnInput {
    TurnInput {
        turn_id: Uuid::now_v7(),
        text: text.into(),
        speaker_label: String::new(),
        door: String::new(),
        recall: Recall {
            index: String::new(),
            snippets: vec![],
        },
        attachment_paths: vec![],
        notices: vec![],
    }
}

async fn drain(mut rx: HarnessEventStream) -> Vec<HarnessEvent> {
    let mut v = Vec::new();
    while let Some(e) = timeout(T, rx.recv()).await.expect("stream stalled") {
        v.push(e);
    }
    v
}

fn assert_one_terminal(ev: &[HarnessEvent]) -> &HarnessEvent {
    let n = ev.iter().filter(|e| e.is_terminal()).count();
    assert_eq!(n, 1, "exactly one terminal event, got {ev:?}");
    let last = ev.last().unwrap();
    assert!(last.is_terminal(), "terminal event must be last: {ev:?}");
    last
}

async fn suite(a: &dyn HarnessAdapter, p: &Profile) {
    suite_with(a, p, &opts).await;
}

async fn suite_with(
    a: &dyn HarnessAdapter,
    p: &Profile,
    mk: &dyn Fn(&str, Option<String>) -> SessionOptions,
) {
    let opts = mk;
    let t0 = std::time::Instant::now();
    let caps = a.capabilities();

    eprintln!("conformance case: say (+{:?})", t0.elapsed());
    // answer rule + terminal-event guarantee: text before exactly one Done.
    let mut s = a.start_session(opts("say", None)).await.unwrap();
    let ev = drain(s.run_turn(turn(p.say)).await.unwrap()).await;
    assert!(
        matches!(assert_one_terminal(&ev), HarnessEvent::Done { .. }),
        "{ev:?}"
    );
    assert!(
        ev.iter()
            .any(|e| matches!(e, HarnessEvent::TextDelta { delta, .. } if !delta.is_empty())),
        "a successful turn produces answer text: {ev:?}"
    );

    eprintln!(
        "conformance case: steer-between-turns (+{:?})",
        t0.elapsed()
    );
    // steer refuses between turns (never queued).
    if caps.steer {
        let st = s
            .steer(SteerInput {
                followup_id: Uuid::now_v7(),
                text: "late".into(),
                speaker_label: String::new(),
            })
            .await
            .unwrap();
        assert_eq!(st, SteerAck::Refused, "steer between turns must refuse");
    }
    let st = s
        .steer(SteerInput {
            followup_id: Uuid::now_v7(),
            text: "late".into(),
            speaker_label: String::new(),
        })
        .await
        .unwrap();
    assert_eq!(st, SteerAck::Refused);

    eprintln!("conformance case: resume (+{:?})", t0.elapsed());
    // resume: release keeps the ref; a new session with it reports the same ref.
    let r = s.session_ref();
    assert!(
        r.is_some() || !caps.resume,
        "resume-capable sessions expose a ref after a turn"
    );
    s.release().await.unwrap();
    if caps.resume {
        let s2 = a.start_session(opts("say", r.clone())).await.unwrap();
        assert_eq!(s2.session_ref(), r);
        s2.release().await.unwrap();
    }

    eprintln!("conformance case: abort (+{:?})", t0.elapsed());
    // abort: terminal event still arrives, exactly once.
    let mut s = a.start_session(opts("long", None)).await.unwrap();
    let rx = s.run_turn(turn(p.long_running)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.abort().await.unwrap();
    let ev = drain(rx).await;
    assert_one_terminal(&ev);
    s.release().await.unwrap();

    eprintln!("conformance case: fail (+{:?})", t0.elapsed());
    // failure classification passthrough: scripted failure ends in Error with that code.
    if let Some((f, want)) = p.fail {
        let mut s = a.start_session(opts("fail", None)).await.unwrap();
        let ev = drain(s.run_turn(turn(f)).await.unwrap()).await;
        assert!(
            matches!(assert_one_terminal(&ev), HarnessEvent::Error { code, .. } if code == want),
            "{ev:?}"
        );
    }

    eprintln!("conformance case: tool (+{:?})", t0.elapsed());
    // tool round-trip: every started call finishes, with matching ids.
    if let (Some(t), true) = (p.tool, caps.tool_events) {
        let mut s = a.start_session(opts("tool", None)).await.unwrap();
        let ev = drain(s.run_turn(turn(t)).await.unwrap()).await;
        assert_one_terminal(&ev);
        for e in &ev {
            if let HarnessEvent::ToolStarted { call_id, .. } = e {
                assert!(
                    ev.iter().any(|f| matches!(f, HarnessEvent::ToolFinished { call_id: c, .. } if c == call_id)),
                    "tool {call_id} never finished: {ev:?}"
                );
            }
        }
        assert!(
            ev.iter()
                .any(|e| matches!(e, HarnessEvent::ToolStarted { .. }))
        );
    }

    eprintln!("conformance case: steer-running (+{:?})", t0.elapsed());
    // steer while running: accepted -> joined; and a steer to a run that cannot take it is refused.
    if let (Some(sp), true) = (p.steerable, caps.steer) {
        let mut s = a.start_session(opts("steer", None)).await.unwrap();
        let rx = s.run_turn(turn(sp)).await.unwrap();
        let id = Uuid::now_v7();
        let st = s
            .steer(SteerInput {
                followup_id: id,
                text: "more".into(),
                speaker_label: String::new(),
            })
            .await
            .unwrap();
        assert_eq!(st, SteerAck::Accepted);
        let ev = drain(rx).await;
        assert!(
            ev.contains(&HarnessEvent::MessageJoined { followup_id: id }),
            "{ev:?}"
        );
        assert_one_terminal(&ev);

        // missed steer: no steer-accept in the script, so the harness refuses to take it.
        let mut s = a.start_session(opts("steer", None)).await.unwrap();
        let rx = s.run_turn(turn("[[say: x]] [[sleep: 1s]]")).await.unwrap();
        let st = s
            .steer(SteerInput {
                followup_id: Uuid::now_v7(),
                text: "m".into(),
                speaker_label: String::new(),
            })
            .await
            .unwrap();
        assert_eq!(st, SteerAck::Refused);
        assert_one_terminal(&drain(rx).await);
    }

    eprintln!("conformance case: correction (+{:?})", t0.elapsed());
    // correction handling: any correction must refer to a message already streamed.
    let mut s = a.start_session(opts("say", None)).await.unwrap();
    let ev = drain(s.run_turn(turn(p.say)).await.unwrap()).await;
    let mut seen = Vec::new();
    for e in &ev {
        match e {
            HarnessEvent::TextDelta { message_id, .. } => seen.push(message_id.clone()),
            HarnessEvent::Correction { message_id, .. } => {
                assert!(
                    seen.contains(message_id),
                    "correction of unknown message {message_id}"
                );
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn fake_conforms() {
    suite(&FakeHarness, &FAKE).await;
}

#[tokio::test]
async fn claude_code_live_conforms_when_enabled() {
    if std::env::var("AURORA_LIVE_CLAUDE").as_deref() != Ok("1") {
        eprintln!("skipped: set AURORA_LIVE_CLAUDE=1 (+ ANTHROPIC_BASE_URL) to run the live suite");
        return;
    }
    assert!(
        std::env::var("ANTHROPIC_BASE_URL").is_ok(),
        "AURORA_LIVE_CLAUDE=1 requires ANTHROPIC_BASE_URL"
    );
    // Mock mode adds the provider-error case (the real-model profile has none).
    let profile = if std::env::var("AURORA_LIVE_SCENARIO_MODELS").is_ok() {
        Profile {
            fail: Some((LIVE.say, "provider_rate_limited")),
            ..LIVE
        }
    } else {
        LIVE
    };
    suite(&ClaudeCodeAdapter::default(), &profile).await;
}

// ---- codex (phase 4, P4-HAR, D-122): the same cases against the mock Responses upstream of
// `aurora-testkit`, run with the real pinned CLI. Skipped when the binary is not installed
// (`AURORA_CODEX_BIN`, default `/opt/aurora/harness/codex/codex`); the env-gated variant
// `AURORA_TEST_HARNESS_CODEX_LIVE=1` (+ `OPENAI_BASE_URL`, `AURORA_LIVE_MODEL`) runs against a
// real OpenAI endpoint instead.

use aurora_supervisor::adapters::codex::CodexAdapter;
use aurora_supervisor::adapters::codex::launcher::{ProcessLauncher, codex_bin};

const CODEX: Profile = Profile {
    say: "Reply with exactly the word: pong",
    long_running: "Count slowly from 1 to 500.",
    fail: Some(("fail please", "provider_quota")),
    tool: Some("Run `echo hi` with your shell tool, then say done."),
    // `steer = false`: a follow-up becomes a separate turn, so nothing is steerable.
    steerable: None,
};

fn codex_opts(base: &str, case: &str, resume: Option<String>) -> SessionOptions {
    let dir = std::env::temp_dir().join(format!("p4-har-conf-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = match case {
        "long" => "long",
        "tool" => "tool",
        "fail" => "fail-quota",
        _ => "say",
    };
    SessionOptions {
        conversation_id: Uuid::now_v7(),
        working_dir: dir,
        resume_ref: resume,
        model: ModelChoice {
            provider: "litellm".into(),
            endpoint_id: "mock".into(),
            base_url: format!("{base}/v1"),
            dialect: "openai_responses".into(),
            model: model.into(),
            effort: "low".into(),
            client_env: vec![],
        },
        // No gateway here: the proxy points at a dead port and the mock is on loopback.
        proxy_url: "http://127.0.0.1:9".into(),
        mcp_bridge: McpBridgeSpec {
            command: vec!["/bin/true".into()],
        },
        env: vec![("NO_PROXY".into(), "127.0.0.1,localhost".into())],
    }
}

#[tokio::test]
async fn codex_conforms_against_the_mock_responses_upstream() {
    // The test (not the release supervisor) may point at another binary.
    let bin = std::env::var_os("AURORA_CODEX_BIN").map_or_else(codex_bin, Into::into);
    if !bin.exists() {
        eprintln!(
            "skipped: {} not installed (set AURORA_CODEX_BIN)",
            bin.display()
        );
        return;
    }
    let up = aurora_testkit::responses::MockResponses::start().await;
    let base = up.base().to_owned();
    // Resume needs the same CODEX_HOME across sessions of one conversation.
    let shared = std::env::temp_dir().join(format!("p4-har-conf-home-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&shared).unwrap();
    let adapter = CodexAdapter::with_launcher(std::sync::Arc::new(ProcessLauncher {
        program: bin.clone(),
    }))
    .codex_home(shared.join(".codex"));
    let mk = move |case: &str, resume: Option<String>| codex_opts(&base, case, resume);
    suite_with(&adapter, &CODEX, &mk).await;

    // A second turn of one session resumes the Codex thread (real `exec resume`) and ends Done.
    let mut s = adapter.start_session(mk("say", None)).await.unwrap();
    for _ in 0..2 {
        let ev = drain(s.run_turn(turn(CODEX.say)).await.unwrap()).await;
        assert!(
            matches!(assert_one_terminal(&ev), HarnessEvent::Done { .. }),
            "{ev:?}"
        );
    }
    assert!(s.session_ref().is_some());

    // SEC-66 at the process level: every request that reached the upstream carried the
    // placeholder and nothing else as its credential.
    let seen = up.requests();
    assert!(!seen.is_empty());
    for r in &seen {
        assert_eq!(
            r.authorization.as_deref(),
            Some("Bearer aurora-placeholder"),
            "{r:?}"
        );
    }
    assert!(!shared.join(".codex/auth.json").exists());
    let cfg = std::fs::read_to_string(shared.join(".codex/config.toml")).unwrap();
    assert!(cfg.contains("approval_policy = \"never\""));
}

#[tokio::test]
async fn codex_live_conforms_when_enabled() {
    if std::env::var("AURORA_TEST_HARNESS_CODEX_LIVE").as_deref() != Ok("1") {
        eprintln!(
            "skipped: set AURORA_TEST_HARNESS_CODEX_LIVE=1 (+ OPENAI_BASE_URL) for the live suite"
        );
        return;
    }
    let base = std::env::var("OPENAI_BASE_URL")
        .expect("AURORA_TEST_HARNESS_CODEX_LIVE needs OPENAI_BASE_URL");
    let model = std::env::var("AURORA_LIVE_MODEL").unwrap_or_else(|_| "gpt-5.1-codex".into());
    let mk = move |case: &str, resume: Option<String>| {
        let mut o = codex_opts("", case, resume);
        o.model.base_url = base.clone();
        o.model.model = model.clone();
        o.env = vec![];
        o
    };
    let profile = Profile {
        fail: None,
        ..CODEX
    };
    suite_with(&CodexAdapter::default(), &profile, &mk).await;
}
