#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Harness conformance suite (`docs/12 §Harness interface`): the same cases run against every
//! adapter. Always against `fake`; against `claude-code` only with `AURORA_LIVE_CLAUDE=1`
//! (plus `ANTHROPIC_BASE_URL`, optional `AURORA_LIVE_MODEL`, `AURORA_LIVE_MCP_BRIDGE`), never in CI.

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
    fail: Option<&'static str>,
    tool: Option<&'static str>,
    steerable: Option<&'static str>,
}

const FAKE: Profile = Profile {
    say: "[[say: hello]] [[say: world]]",
    long_running: "[[say: start]] [[sleep: 30s]]",
    fail: Some("[[fail: provider_quota]]"),
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

fn opts(resume: Option<String>) -> SessionOptions {
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
            model: std::env::var("AURORA_LIVE_MODEL")
                .unwrap_or_else(|_| "claude-sonnet-4-5".into()),
            effort: "low".into(),
            client_env: vec![],
        },
        proxy_url: "http://127.0.0.1:9".into(),
        mcp_bridge: McpBridgeSpec {
            command: std::env::var("AURORA_LIVE_MCP_BRIDGE")
                .map(|c| c.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default(),
        },
        env: vec![],
    }
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
    let caps = a.capabilities();

    // answer rule + terminal-event guarantee: text before exactly one Done.
    let mut s = a.start_session(opts(None)).await.unwrap();
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

    // resume: release keeps the ref; a new session with it reports the same ref.
    let r = s.session_ref();
    assert!(
        r.is_some() || !caps.resume,
        "resume-capable sessions expose a ref after a turn"
    );
    s.release().await.unwrap();
    if caps.resume {
        let s2 = a.start_session(opts(r.clone())).await.unwrap();
        assert_eq!(s2.session_ref(), r);
        s2.release().await.unwrap();
    }

    // abort: terminal event still arrives, exactly once.
    let mut s = a.start_session(opts(None)).await.unwrap();
    let rx = s.run_turn(turn(p.long_running)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.abort().await.unwrap();
    let ev = drain(rx).await;
    assert_one_terminal(&ev);
    s.release().await.unwrap();

    // failure classification passthrough: scripted failure ends in Error with that code.
    if let Some(f) = p.fail {
        let mut s = a.start_session(opts(None)).await.unwrap();
        let ev = drain(s.run_turn(turn(f)).await.unwrap()).await;
        assert!(
            matches!(assert_one_terminal(&ev), HarnessEvent::Error { code, .. } if code == "provider_quota"),
            "{ev:?}"
        );
    }

    // tool round-trip: every started call finishes, with matching ids.
    if let (Some(t), true) = (p.tool, caps.tool_events) {
        let mut s = a.start_session(opts(None)).await.unwrap();
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

    // steer while running: accepted -> joined; and a steer to a run that cannot take it is refused.
    if let (Some(sp), true) = (p.steerable, caps.steer) {
        let mut s = a.start_session(opts(None)).await.unwrap();
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
        let mut s = a.start_session(opts(None)).await.unwrap();
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

    // correction handling: any correction must refer to a message already streamed.
    let mut s = a.start_session(opts(None)).await.unwrap();
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
    suite(&ClaudeCodeAdapter::default(), &LIVE).await;
}
