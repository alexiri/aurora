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
    /// The harness takes a steer only after it has acknowledged the turn's prompt (Pi): the first
    /// steer of a run is retried until accepted, within a deadline.
    steer_needs_ack: bool,
    /// A scripted run that cannot take a steer exists (the fake's script without `steer-accept`).
    /// A real harness cannot be made to miss one: a steer sent after the run is refused (covered by
    /// the between-turns case) and one queued during it joins.
    missed_steer: bool,
}

const FAKE: Profile = Profile {
    say: "[[say: hello]] [[say: world]]",
    long_running: "[[say: start]] [[sleep: 30s]]",
    fail: Some(("[[fail: provider_quota]]", "provider_quota")),
    tool: Some("[[say: saving]] [[tool: memory_save {\"text\":\"x\"}]]"),
    steerable: Some("[[say: base]] [[steer-accept]]"),
    steer_needs_ack: false,
    missed_steer: true,
};

const LIVE: Profile = Profile {
    say: "Reply with exactly the word: pong",
    long_running: "Count slowly from 1 to 500, one number per line, with a short comment each.",
    fail: None,
    tool: Some("Use the memory_save platform tool to save the note 'conformance', then say done."),
    steerable: None,
    steer_needs_ack: false,
    missed_steer: false,
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
            roles: None,
            subagent_role: None,
            model_meta: None,
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
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let st = loop {
            let st = s
                .steer(SteerInput {
                    followup_id: id,
                    text: "more".into(),
                    speaker_label: String::new(),
                })
                .await
                .unwrap();
            if st == SteerAck::Accepted
                || !p.steer_needs_ack
                || std::time::Instant::now() > deadline
            {
                break st;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert_eq!(st, SteerAck::Accepted);
        let ev = drain(rx).await;
        assert!(
            ev.contains(&HarnessEvent::MessageJoined { followup_id: id }),
            "{ev:?}"
        );
        assert_one_terminal(&ev);

        // missed steer: no steer-accept in the script, so the harness refuses to take it.
        if p.missed_steer {
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
    steer_needs_ack: false,
    missed_steer: false,
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
            roles: None,
            subagent_role: None,
            model_meta: None,
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

// ---- pi (phase 5, P5-HARN, D-361, D-362, SEC-133): the same cases against the mock Messages,
// Responses and Chat upstreams of `aurora-testkit`, run with the real pinned CLI. Skipped when the
// binary is not installed (`AURORA_PI_BIN`, default `/opt/aurora/harness/pi/node_modules/.bin/pi`);
// the env-gated variant `AURORA_TEST_HARNESS_PI_LIVE=1` runs the same cases through a real gateway
// and LiteLLM (`PI_LIVE_BASE_URL`, `PI_LIVE_DIALECT`, `AURORA_LIVE_MODEL`).

use aurora_supervisor::adapters::pi::PiAdapter;
use aurora_supervisor::adapters::pi::launcher::{ProcessLauncher as PiLauncher, pi_bin};
use aurora_testkit::harness_upstream::HarnessUpstream;

const PI: Profile = Profile {
    say: "Reply with exactly the word: pong",
    long_running: "Count slowly from 1 to 500.",
    fail: Some(("fail please", "provider_quota")),
    tool: Some("Run `echo hi` with your bash tool, then say done."),
    // Pi accepts a steer during a run (after it has taken the prompt), and joins it before the
    // next model call.
    steerable: Some("Run `sleep 2` with your bash tool, then say done."),
    steer_needs_ack: true,
    missed_steer: false,
};

/// Records every request line sent to the session proxy and refuses it: a turn that reaches any
/// host other than the (loopback, `NO_PROXY`) endpoint shows up here.
struct RefusingProxy {
    url: String,
    seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl RefusingProxy {
    async fn start() -> Self {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = l.accept().await else {
                    return;
                };
                let log = log.clone();
                tokio::spawn(async move {
                    let mut first = String::new();
                    let (r, mut w) = sock.split();
                    let _ = BufReader::new(r).read_line(&mut first).await;
                    log.lock().unwrap().push(first.trim().to_owned());
                    let _ = w
                        .write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n")
                        .await;
                });
            }
        });
        Self { url, seen }
    }

    fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

fn pi_model(case: &str) -> &'static str {
    match case {
        "long" => "long",
        "tool" => "tool",
        "fail" => "fail-quota",
        "steer" => "steer",
        "mcp" => "mcp",
        _ => "say",
    }
}

/// Where the mock serves `dialect` (Messages: the host; OpenAI: `…/v1`).
fn pi_base(base: &str, dialect: &str) -> String {
    if dialect == "anthropic_messages" {
        base.to_owned()
    } else {
        format!("{base}/v1")
    }
}

fn pi_opts(
    base: &str,
    proxy: &str,
    dialect: &str,
    case: &str,
    resume: Option<String>,
) -> SessionOptions {
    let dir = std::env::temp_dir().join(format!("p5-harn-pi-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    SessionOptions {
        conversation_id: Uuid::now_v7(),
        working_dir: dir,
        resume_ref: resume,
        model: ModelChoice {
            provider: "litellm".into(),
            endpoint_id: "mock".into(),
            base_url: pi_base(base, dialect),
            dialect: dialect.into(),
            model: pi_model(case).into(),
            effort: "low".into(),
            client_env: vec![],
            roles: None,
            subagent_role: None,
            model_meta: None,
        },
        proxy_url: proxy.to_owned(),
        mcp_bridge: McpBridgeSpec {
            command: vec!["/bin/true".into()],
        },
        env: vec![("NO_PROXY".into(), "127.0.0.1,localhost".into())],
    }
}

fn pi_adapter() -> Option<PiAdapter> {
    // The test (not the release supervisor) may point at another binary.
    let bin = std::env::var_os("AURORA_PI_BIN").map_or_else(pi_bin, Into::into);
    if !bin.exists() {
        eprintln!(
            "skipped: {} not installed (set AURORA_PI_BIN)",
            bin.display()
        );
        return None;
    }
    Some(PiAdapter::with_launcher(std::sync::Arc::new(PiLauncher {
        program: bin,
    })))
}

const PI_DIALECTS: [(&str, &str); 3] = [
    ("anthropic_messages", "/v1/messages"),
    ("openai_responses", "/v1/responses"),
    ("openai_chat", "/v1/chat/completions"),
];

#[tokio::test]
async fn pi_conforms_against_the_mock_upstream_of_every_dialect_it_declares() {
    let Some(adapter) = pi_adapter() else { return };
    assert_eq!(
        adapter.capabilities().dialects,
        PI_DIALECTS.map(|(d, _)| d.to_owned())
    );
    for (dialect, path) in PI_DIALECTS {
        eprintln!("== pi: {dialect}");
        let up = HarnessUpstream::start().await;
        let proxy = RefusingProxy::start().await;
        let (base, purl) = (up.base().to_owned(), proxy.url.clone());
        let mk =
            move |case: &str, resume: Option<String>| pi_opts(&base, &purl, dialect, case, resume);
        suite_with(&adapter, &PI, &mk).await;

        // SEC-66 / SEC-133 at the process level: every request that reached the upstream carried
        // the placeholder and nothing else as its credential, on the dialect's inference path only,
        // and nothing was sent to any other host (the proxy saw no request).
        let seen = up.requests();
        assert!(!seen.is_empty());
        for r in &seen {
            assert_eq!(r.path, path, "{dialect}: only the inference path");
            assert_eq!(
                r.credential().as_deref(),
                Some("aurora-placeholder"),
                "{dialect}: {r:?}"
            );
        }
        assert_eq!(
            proxy.requests(),
            Vec::<String>::new(),
            "{dialect}: vendor network"
        );
    }
}

/// `by_model` of a finished turn.
fn done_usage(ev: &[HarnessEvent]) -> serde_json::Value {
    match ev.last() {
        Some(HarnessEvent::Done { usage: Some(u) }) => u.clone(),
        other => panic!("expected a Done with usage, got {other:?}"),
    }
}

#[tokio::test]
async fn pi_reports_the_four_counters_per_model_and_the_totals_are_their_sum() {
    let Some(adapter) = pi_adapter() else { return };
    for (dialect, _) in PI_DIALECTS {
        let up = HarnessUpstream::start().await;
        let proxy = RefusingProxy::start().await;
        let mut s = adapter
            .start_session(pi_opts(up.base(), &proxy.url, dialect, "tool", None))
            .await
            .unwrap();
        // Two model requests (the tool call, then the answer): the mock's fixed usage twice.
        let ev = drain(s.run_turn(turn(PI.tool.unwrap())).await.unwrap()).await;
        let u = done_usage(&ev);
        let (cache_read, cache_write) = (
            6,
            if dialect == "anthropic_messages" {
                4
            } else {
                0
            },
        );
        // The mock's OpenAI usage has `prompt_tokens` 11 including 3 cached: Pi's `input` excludes them.
        let input = if dialect == "anthropic_messages" {
            22
        } else {
            16
        };
        assert_eq!(u["by_model"].as_array().unwrap().len(), 1, "{dialect}: {u}");
        assert_eq!(u["by_model"][0]["model"], "tool", "{dialect}");
        for (k, want) in [
            ("input_tokens", input),
            ("output_tokens", 8),
            ("cache_read_tokens", cache_read),
            ("cache_write_tokens", cache_write),
        ] {
            assert_eq!(u[k], want, "{dialect}: {k}: {u}");
            assert_eq!(u["by_model"][0][k], want, "{dialect}: by_model {k}: {u}");
        }
        s.release().await.unwrap();
    }
}

#[tokio::test]
async fn pi_platform_tool_calls_come_from_a_descendant_of_the_pi_process() {
    let Some(adapter) = pi_adapter() else { return };
    let up = HarnessUpstream::start_with_mcp_tool("mcp__aurora__ping").await;
    let proxy = RefusingProxy::start().await;
    let log = std::env::temp_dir().join(format!("p5-harn-pi-probe-{}.log", Uuid::now_v7()));
    let probe = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mcp-probe.js");
    let mut o = pi_opts(up.base(), &proxy.url, "anthropic_messages", "mcp", None);
    o.mcp_bridge = McpBridgeSpec {
        command: vec!["node".into(), probe.into()],
    };
    o.env
        .push(("MCP_PROBE_LOG".into(), log.display().to_string()));
    let mut s = adapter.start_session(o).await.unwrap();
    let ev = drain(s.run_turn(turn("call the ping tool")).await.unwrap()).await;
    assert!(
        matches!(assert_one_terminal(&ev), HarnessEvent::Done { .. }),
        "{ev:?}"
    );
    // The platform tool is exposed directly, under Pi's MCP name, and the call finished.
    let call = ev.iter().find_map(|e| match e {
        HarnessEvent::ToolStarted { call_id, name, .. } if name == "mcp__aurora__ping" => {
            Some(call_id.clone())
        }
        _ => None,
    });
    let call = call.unwrap_or_else(|| panic!("no mcp__aurora__ping call: {ev:?}"));
    assert!(
        ev.iter().any(|e| matches!(e, HarnessEvent::ToolFinished { call_id, ok: true, .. } if *call_id == call)),
        "{ev:?}"
    );
    // D-301: the bridge ran as a descendant of the Pi process the adapter started (never a shared or
    // background service): some ancestor of the MCP server is `pi`, and the server's parent is not
    // the top of the process tree.
    let probed = std::fs::read_to_string(&log).expect("the MCP server was never called");
    let first: serde_json::Value = serde_json::from_str(probed.lines().next().unwrap()).unwrap();
    let chain = first["chain"].as_array().unwrap();
    assert!(chain.len() >= 2, "{first}");
    assert!(
        chain.iter().skip(1).any(|p| {
            // `pi` (the process title Pi sets), `node …/.bin/pi …` or the package's own entry.
            let cmd = p["cmd"].as_str().unwrap_or_default();
            cmd.contains("pi-coding-agent")
                || cmd
                    .split_whitespace()
                    .any(|w| w.rsplit('/').next() == Some("pi"))
        }),
        "no ancestor of the MCP server is the pi process: {first}"
    );
    assert_eq!(probed.lines().count(), 1, "one call, one probe line");
    s.release().await.unwrap();
}

#[tokio::test]
async fn pi_live_conforms_when_enabled() {
    if std::env::var("AURORA_TEST_HARNESS_PI_LIVE").as_deref() != Ok("1") {
        eprintln!(
            "skipped: set AURORA_TEST_HARNESS_PI_LIVE=1 (+ PI_LIVE_BASE_URL, PI_LIVE_DIALECT) for the live suite"
        );
        return;
    }
    let base = std::env::var("PI_LIVE_BASE_URL").expect("PI_LIVE_BASE_URL");
    let dialect = std::env::var("PI_LIVE_DIALECT").unwrap_or_else(|_| "anthropic_messages".into());
    let model = std::env::var("AURORA_LIVE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".into());
    let mk = move |case: &str, resume: Option<String>| {
        let mut o = pi_opts("", "http://127.0.0.1:9", &dialect, case, resume);
        o.model.base_url = base.clone();
        o.model.model = model.clone();
        o.env = vec![];
        o
    };
    let profile = Profile {
        fail: None,
        steerable: None,
        ..PI
    };
    suite_with(&PiAdapter::default(), &profile, &mk).await;
}

// ---- goose (phase 5, P5-HARN, D-361, D-362, SEC-133): the same cases against the mock Messages and
// Chat upstreams, run with the real pinned CLI. Skipped when the binary is not installed
// (`AURORA_GOOSE_BIN`, default `/opt/aurora/harness/goose/goose`); the env-gated variant
// `AURORA_TEST_HARNESS_GOOSE_LIVE=1` (+ `GOOSE_LIVE_BASE_URL`, `GOOSE_LIVE_DIALECT`,
// `AURORA_LIVE_MODEL`) runs it through a real gateway and LiteLLM.

use aurora_supervisor::adapters::goose::GooseAdapter;
use aurora_supervisor::adapters::goose::launcher::{ProcessLauncher as GooseLauncher, goose_bin};

const GOOSE: Profile = Profile {
    say: "Reply with exactly the word: pong",
    long_running: "Count slowly from 1 to 500.",
    fail: Some(("fail please", "provider_quota")),
    tool: Some("Run `echo hi` with your shell tool, then say done."),
    // `steer = false`: a follow-up becomes a separate turn, so nothing is steerable.
    steerable: None,
    steer_needs_ack: false,
    missed_steer: false,
};

const GOOSE_DIALECTS: [(&str, &str); 2] = [
    ("anthropic_messages", "/v1/messages"),
    ("openai_chat", "/v1/chat/completions"),
];

fn goose_adapter() -> Option<GooseAdapter> {
    // The test (not the release supervisor) may point at another binary.
    let bin = std::env::var_os("AURORA_GOOSE_BIN").map_or_else(goose_bin, Into::into);
    if !bin.exists() {
        eprintln!(
            "skipped: {} not installed (set AURORA_GOOSE_BIN)",
            bin.display()
        );
        return None;
    }
    Some(GooseAdapter::with_launcher(std::sync::Arc::new(
        GooseLauncher { program: bin },
    )))
}

/// A goose session choice: the model is the mock scenario; `home` shares the sandbox home between
/// sessions of one conversation (resume needs the same state directory).
fn goose_opts(
    base: &str,
    proxy: &str,
    dialect: &str,
    model: &str,
    home: Option<&std::path::Path>,
    conversation: Uuid,
    resume: Option<String>,
) -> SessionOptions {
    let mut o = pi_opts(base, proxy, dialect, "say", resume);
    o.model.model = model.into();
    o.conversation_id = conversation;
    if let Some(h) = home {
        o.working_dir = h.to_path_buf();
    }
    o
}

#[tokio::test]
async fn goose_conforms_against_the_mock_upstream_of_every_dialect_it_declares() {
    let Some(adapter) = goose_adapter() else {
        return;
    };
    assert_eq!(
        adapter.capabilities().dialects,
        GOOSE_DIALECTS.map(|(d, _)| d.to_owned())
    );
    for (dialect, path) in GOOSE_DIALECTS {
        eprintln!("== goose: {dialect}");
        let up = HarnessUpstream::start_with_tools("aurora__ping", "shell").await;
        let proxy = RefusingProxy::start().await;
        let (base, purl) = (up.base().to_owned(), proxy.url.clone());
        let mk = move |case: &str, resume: Option<String>| {
            goose_opts(
                &base,
                &purl,
                dialect,
                pi_model(case),
                None,
                Uuid::now_v7(),
                resume,
            )
        };
        suite_with(&adapter, &GOOSE, &mk).await;

        // SEC-66 / SEC-133 at the process level: every inference request carried the placeholder
        // and nothing else; the only other request the endpoint saw is goose's discovery of the
        // model's context window through the LiteLLM provider (`GET /model/info`: the gateway
        // refuses it, goose falls back to the rendered limit); and **nothing went to any other
        // host**: the proxy saw no request, so the models.dev catalogue fetch is gone (the rendered
        // catalogue makes goose skip it).
        let seen = up.requests();
        assert!(!seen.is_empty());
        for r in &seen {
            if r.path == "/model/info" && dialect == "openai_chat" {
                continue;
            }
            assert_eq!(r.path, path, "{dialect}: only the inference path");
            assert_eq!(
                r.credential().as_deref(),
                Some("aurora-placeholder"),
                "{dialect}: {r:?}"
            );
        }
        assert_eq!(
            proxy.requests(),
            Vec::<String>::new(),
            "{dialect}: vendor network (models.dev)"
        );
    }
}

#[tokio::test]
async fn goose_resumes_its_session_and_a_model_change_between_turns_reaches_the_upstream() {
    let Some(adapter) = goose_adapter() else {
        return;
    };
    let up = HarnessUpstream::start_with_tools("aurora__ping", "shell").await;
    let proxy = RefusingProxy::start().await;
    let home = std::env::temp_dir().join(format!("p5-harn-goose-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&home).unwrap();
    let conv = Uuid::now_v7();
    let mut s = adapter
        .start_session(goose_opts(
            up.base(),
            &proxy.url,
            "anthropic_messages",
            "say",
            Some(&home),
            conv,
            None,
        ))
        .await
        .unwrap();
    let ev = drain(s.run_turn(turn("first")).await.unwrap()).await;
    assert!(
        matches!(assert_one_terminal(&ev), HarnessEvent::Done { .. }),
        "{ev:?}"
    );
    let r = s.session_ref().expect("a session ref after the first turn");
    // Second turn of the same session: `--resume`.
    let ev = drain(s.run_turn(turn("second")).await.unwrap()).await;
    assert!(
        matches!(assert_one_terminal(&ev), HarnessEvent::Done { .. }),
        "{ev:?}"
    );
    s.release().await.unwrap();
    // A new process (a restart) resumes the stored session with a **different model**: goose keeps
    // the stored model of a resumed session unless `--model` is passed, which the adapter always does.
    let before = up.requests().len();
    let mut s2 = adapter
        .start_session(goose_opts(
            up.base(),
            &proxy.url,
            "anthropic_messages",
            "tool",
            Some(&home),
            conv,
            Some(r.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(s2.session_ref().as_deref(), Some(r.as_str()));
    let ev = drain(s2.run_turn(turn("third")).await.unwrap()).await;
    assert!(
        matches!(assert_one_terminal(&ev), HarnessEvent::Done { .. }),
        "{ev:?}"
    );
    let seen = up.requests();
    assert_eq!(
        seen[before].body["model"], "tool",
        "the new model reached the upstream, not the stored one"
    );
    // The conversation's earlier turns are in the resumed context.
    assert!(
        seen[before].body.to_string().contains("first"),
        "resumed history is sent"
    );
    assert_eq!(proxy.requests(), Vec::<String>::new(), "vendor network");
}

#[tokio::test]
async fn goose_reports_the_four_counters_with_the_input_net_of_the_cache() {
    let Some(adapter) = goose_adapter() else {
        return;
    };
    for (dialect, _) in GOOSE_DIALECTS {
        let up = HarnessUpstream::start_with_tools("aurora__ping", "shell").await;
        let proxy = RefusingProxy::start().await;
        let mut s = adapter
            .start_session(goose_opts(
                up.base(),
                &proxy.url,
                dialect,
                "say",
                None,
                Uuid::now_v7(),
                None,
            ))
            .await
            .unwrap();
        let ev = drain(s.run_turn(turn(GOOSE.say)).await.unwrap()).await;
        let u = done_usage(&ev);
        // The mock reports 11 input (3 cached, 2 written on Messages) and 4 output.
        let (read, write) = (
            3,
            if dialect == "anthropic_messages" {
                2
            } else {
                0
            },
        );
        let input = if dialect == "anthropic_messages" {
            11
        } else {
            8
        };
        let want = [
            ("input_tokens", input),
            ("output_tokens", 4),
            ("cache_read_tokens", read),
            ("cache_write_tokens", write),
        ];
        assert_eq!(u["by_model"].as_array().unwrap().len(), 1, "{dialect}: {u}");
        assert_eq!(u["by_model"][0]["model"], "say", "{dialect}");
        for (k, v) in want {
            assert_eq!(u[k], v, "{dialect}: {k}: {u}");
            assert_eq!(u["by_model"][0][k], v, "{dialect}: by_model {k}");
        }
        s.release().await.unwrap();
    }
}

#[tokio::test]
async fn goose_platform_tool_calls_come_from_a_descendant_of_the_goose_process() {
    let Some(adapter) = goose_adapter() else {
        return;
    };
    let up = HarnessUpstream::start_with_tools("aurora__ping", "shell").await;
    let proxy = RefusingProxy::start().await;
    let log = std::env::temp_dir().join(format!("p5-harn-goose-probe-{}.log", Uuid::now_v7()));
    let probe = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mcp-probe.js");
    let mut o = goose_opts(
        up.base(),
        &proxy.url,
        "anthropic_messages",
        "mcp",
        None,
        Uuid::now_v7(),
        None,
    );
    o.mcp_bridge = McpBridgeSpec {
        command: vec!["node".into(), probe.into()],
    };
    o.env
        .push(("MCP_PROBE_LOG".into(), log.display().to_string()));
    let mut s = adapter.start_session(o).await.unwrap();
    let ev = drain(s.run_turn(turn("call the ping tool")).await.unwrap()).await;
    assert!(
        matches!(assert_one_terminal(&ev), HarnessEvent::Done { .. }),
        "{ev:?}"
    );
    let call = ev.iter().find_map(|e| match e {
        HarnessEvent::ToolStarted { call_id, name, .. } if name == "mcp__aurora__ping" => {
            Some(call_id.clone())
        }
        _ => None,
    });
    let call = call.unwrap_or_else(|| panic!("no mcp__aurora__ping call: {ev:?}"));
    assert!(
        ev.iter().any(|e| matches!(e, HarnessEvent::ToolFinished { call_id, ok: true, .. } if *call_id == call)),
        "{ev:?}"
    );
    // D-301: some ancestor of the MCP server is the goose process this adapter started.
    let probed = std::fs::read_to_string(&log).expect("the MCP server was never called");
    let first: serde_json::Value = serde_json::from_str(probed.lines().next().unwrap()).unwrap();
    let chain = first["chain"].as_array().unwrap();
    assert!(
        chain.iter().skip(1).any(|p| p["cmd"]
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .any(|w| w.rsplit('/').next() == Some("goose"))),
        "no ancestor of the MCP server is the goose process: {first}"
    );
    assert_eq!(probed.lines().count(), 1, "one call, one probe line");
    s.release().await.unwrap();
}

#[tokio::test]
async fn goose_subagent_requests_reach_the_model_of_the_role_the_choice_names() {
    let Some(adapter) = goose_adapter() else {
        return;
    };
    let up = HarnessUpstream::start_with_tools("aurora__ping", "shell").await;
    let proxy = RefusingProxy::start().await;
    let mut o = goose_opts(
        up.base(),
        &proxy.url,
        "anthropic_messages",
        "delegate",
        None,
        Uuid::now_v7(),
        None,
    );
    o.model.roles = Some(aurora_protocol::runner::ModelRoleIds {
        large: None,
        medium: None,
        small: Some("sub-model".into()),
    });
    o.model.subagent_role = Some("small".into());
    let mut s = adapter.start_session(o).await.unwrap();
    let rx = s.run_turn(turn("use a subagent")).await.unwrap();
    // The subagent must end by calling goose's own `final_output` tool, which the mock does not
    // model: it keeps asking. The proof needed is its first request, so wait for it (within a
    // deadline) and end the turn.
    let models = |up: &HarnessUpstream| -> Vec<String> {
        up.requests()
            .iter()
            .filter_map(|r| r.body["model"].as_str().map(str::to_owned))
            .collect()
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !models(&up).contains(&"sub-model".to_owned()) && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    s.abort().await.unwrap();
    assert_one_terminal(&drain(rx).await);
    let models = models(&up);
    assert!(models.contains(&"delegate".to_owned()), "{models:?}");
    assert!(
        models.contains(&"sub-model".to_owned()),
        "the subagent's request used the role's model: {models:?}"
    );
    s.release().await.unwrap();
}

#[tokio::test]
async fn goose_live_conforms_when_enabled() {
    if std::env::var("AURORA_TEST_HARNESS_GOOSE_LIVE").as_deref() != Ok("1") {
        eprintln!(
            "skipped: set AURORA_TEST_HARNESS_GOOSE_LIVE=1 (+ GOOSE_LIVE_BASE_URL, GOOSE_LIVE_DIALECT) for the live suite"
        );
        return;
    }
    let base = std::env::var("GOOSE_LIVE_BASE_URL").expect("GOOSE_LIVE_BASE_URL");
    let dialect =
        std::env::var("GOOSE_LIVE_DIALECT").unwrap_or_else(|_| "anthropic_messages".into());
    let model = std::env::var("AURORA_LIVE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".into());
    let mk = move |case: &str, resume: Option<String>| {
        let mut o = pi_opts("", "http://127.0.0.1:9", &dialect, case, resume);
        o.model.base_url = base.clone();
        o.model.model = model.clone();
        o.env = vec![];
        o
    };
    let profile = Profile {
        fail: None,
        ..GOOSE
    };
    suite_with(&GooseAdapter::default(), &profile, &mk).await;
}
