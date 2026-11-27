#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Session-level tests of the Pi adapter against a scripted child: the test plays Pi's RPC side.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use aurora_protocol::runner::Recall;
use serde_json::{Value, json};
use tokio::time::timeout;
use uuid::Uuid;

use super::config::tests::opts;
use super::*;

/// What a launched fake child gives the test: its stdout (events in), its stdin (commands out) and
/// whether the adapter killed it.
struct Remote {
    out: mpsc::Sender<String>,
    input: mpsc::Receiver<String>,
    killed: Arc<AtomicBool>,
}

struct TestLauncher {
    tx: mpsc::UnboundedSender<(config::LaunchSpec, Remote)>,
}

#[async_trait]
impl Launcher for TestLauncher {
    async fn launch(&self, spec: config::LaunchSpec) -> Result<ChildIo, HarnessError> {
        let (out_tx, out_rx) = mpsc::channel(256);
        let (in_tx, in_rx) = mpsc::channel(16);
        let killed = Arc::new(AtomicBool::new(false));
        let k = killed.clone();
        self.tx
            .send((
                spec,
                Remote {
                    out: out_tx,
                    input: in_rx,
                    killed,
                },
            ))
            .unwrap();
        Ok(ChildIo {
            stdin: in_tx,
            lines: out_rx,
            kill: Box::new(move || k.store(true, Ordering::SeqCst)),
        })
    }
}

fn adapter() -> (
    PiAdapter,
    mpsc::UnboundedReceiver<(config::LaunchSpec, Remote)>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    (PiAdapter::with_launcher(Arc::new(TestLauncher { tx })), rx)
}

fn tmp_opts(dialect: &str) -> SessionOptions {
    let dir = std::env::temp_dir().join(format!("pi-adapter-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut o = opts(dialect, "");
    o.working_dir = dir;
    o.conversation_id = Uuid::now_v7();
    o
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

fn steer(text: &str) -> SteerInput {
    SteerInput {
        followup_id: Uuid::now_v7(),
        text: text.into(),
        speaker_label: String::new(),
    }
}

const T: Duration = Duration::from_secs(10);

async fn cmd(r: &mut Remote) -> Value {
    let l = timeout(T, r.input.recv())
        .await
        .expect("no command")
        .expect("stdin closed");
    serde_json::from_str(&l).unwrap()
}

async fn say(r: &Remote, lines: &[&str]) {
    for l in lines {
        r.out.send((*l).to_owned()).await.unwrap();
    }
}

async fn drain(mut rx: HarnessEventStream) -> Vec<HarnessEvent> {
    let mut v = Vec::new();
    while let Some(e) = timeout(T, rx.recv()).await.expect("stream stalled") {
        v.push(e);
    }
    v
}

const STARTED: &str =
    r#"{"type":"response","command":"prompt","success":true,"data":{"disposition":"started"}}"#;
const QUEUED: &str =
    r#"{"type":"response","command":"prompt","success":true,"data":{"disposition":"queued"}}"#;
const A_START: &str =
    r#"{"type":"message_start","message":{"role":"assistant","content":[],"model":"m1"}}"#;
const DELTA: &str =
    r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"pong"}}"#;
const A_END: &str = r#"{"type":"message_end","message":{"role":"assistant","model":"m1","usage":{"input":11,"output":4,"cacheRead":3,"cacheWrite":2},"stopReason":"stop"}}"#;
const SETTLED: &str = r#"{"type":"agent_settled","aborted":false}"#;
const SETTLED_ABORTED: &str = r#"{"type":"agent_settled","aborted":true}"#;

#[tokio::test]
async fn a_turn_sends_one_prompt_streams_text_and_ends_with_done_and_the_session_ref() {
    let (a, mut launches) = adapter();
    let o = tmp_opts("anthropic_messages");
    let conv = o.conversation_id.to_string();
    let mut s = a.start_session(o).await.unwrap();
    let (spec, mut pi) = launches.recv().await.unwrap();
    assert_eq!(s.session_ref(), None, "no ref before the first turn");
    assert!(
        spec.args
            .windows(2)
            .any(|w| w == ["--session-id", conv.as_str()])
    );
    assert!(spec.args.contains(&"--no-approve".to_owned()));
    let rx = s.run_turn(turn("hello")).await.unwrap();
    let c = cmd(&mut pi).await;
    assert_eq!(c, json!({"type": "prompt", "message": "hello"}));
    say(&pi, &[STARTED, A_START, DELTA, A_END, SETTLED]).await;
    let ev = drain(rx).await;
    assert_eq!(
        ev[0],
        HarnessEvent::SessionRef {
            session_ref: conv.clone()
        }
    );
    assert!(
        ev.iter()
            .any(|e| matches!(e, HarnessEvent::TextDelta { delta, .. } if delta == "pong"))
    );
    assert!(
        matches!(ev.last(), Some(HarnessEvent::Done { usage: Some(u) }) if u["by_model"][0]["model"] == "m1")
    );
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1);
    assert_eq!(s.session_ref().as_deref(), Some(conv.as_str()));
    // The same child takes the next turn (one long-lived process per session).
    let rx = s.run_turn(turn("again")).await.unwrap();
    assert_eq!(cmd(&mut pi).await["message"], "again");
    say(&pi, &[STARTED, SETTLED]).await;
    assert!(matches!(
        drain(rx).await.last(),
        Some(HarnessEvent::Done { usage: None })
    ));
    assert!(launches.try_recv().is_err(), "no second launch");
}

#[tokio::test]
async fn the_configuration_is_rendered_before_every_start_and_every_turn_and_credentials_are_removed()
 {
    let (a, mut launches) = adapter();
    let o = tmp_opts("openai_chat");
    let home = o.working_dir.clone();
    let cfg = render::config_dir(&home, ID);
    // The agent planted a credential store and a different provider before the session started.
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("auth.json"),
        r#"{"anthropic":{"key":"sk-planted"}}"#,
    )
    .unwrap();
    std::fs::write(cfg.join("models.json"), r#"{"providers":{"evil":{}}}"#).unwrap();
    let mut s = a.start_session(o).await.unwrap();
    let (spec, mut pi) = launches.recv().await.unwrap();
    let rendered = |name: &str| std::fs::read_to_string(cfg.join(name)).unwrap();
    assert!(
        !cfg.join("auth.json").exists(),
        "planted credential removed at start"
    );
    let models: Value = serde_json::from_str(&rendered("models.json")).unwrap();
    assert_eq!(models["providers"]["aurora"]["api"], "openai-completions");
    assert!(models["providers"].get("evil").is_none());
    assert_eq!(
        spec.env
            .iter()
            .find(|(k, _)| k == "PI_CODING_AGENT_DIR")
            .map(|(_, v)| v.as_str()),
        Some(cfg.display().to_string().as_str())
    );
    assert!(rendered("mcp.json").contains("\"exposure\": \"direct\""));
    // Between turns Pi (or the agent) writes the credential store again and edits a file.
    std::fs::write(cfg.join("auth.json"), "{\"k\":\"sk-again\"}").unwrap();
    std::fs::write(cfg.join("mcp.json"), "{\"mcpServers\":{}}").unwrap();
    let rx = s.run_turn(turn("x")).await.unwrap();
    cmd(&mut pi).await;
    assert!(
        !cfg.join("auth.json").exists(),
        "removed again before the turn"
    );
    assert!(
        rendered("mcp.json").contains("aurora-supervisor"),
        "re-rendered before the turn"
    );
    say(&pi, &[STARTED, SETTLED]).await;
    drain(rx).await;
    for f in ["models.json", "mcp.json", "settings.json"] {
        let text = rendered(f);
        assert!(
            !text.contains("sk-") && !text.contains("aurora-placeholder"),
            "{f}: {text}"
        );
    }
}

#[tokio::test]
async fn steer_is_refused_between_turns_and_never_sent_to_pi() {
    let (a, mut launches) = adapter();
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let (_, mut pi) = launches.recv().await.unwrap();
    assert_eq!(s.steer(steer("early")).await.unwrap(), SteerAck::Refused);
    // During a turn but before Pi has taken the turn's prompt: also refused.
    let rx = s.run_turn(turn("go")).await.unwrap();
    cmd(&mut pi).await;
    assert_eq!(
        s.steer(steer("too early")).await.unwrap(),
        SteerAck::Refused
    );
    say(&pi, &[STARTED, SETTLED]).await;
    drain(rx).await;
    assert_eq!(s.steer(steer("late")).await.unwrap(), SteerAck::Refused);
    assert!(pi.input.try_recv().is_err(), "no steer reached Pi");
}

#[tokio::test]
async fn a_steer_during_a_run_is_queued_and_joins_when_pi_takes_it_into_the_transcript() {
    let (a, mut launches) = adapter();
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let (_, mut pi) = launches.recv().await.unwrap();
    let rx = s.run_turn(turn("go")).await.unwrap();
    cmd(&mut pi).await;
    say(&pi, &[STARTED]).await;
    // Wait until the pump has seen the acknowledgement (the session ref event follows it).
    let mut rx = rx;
    assert!(matches!(
        rx.recv().await,
        Some(HarnessEvent::SessionRef { .. })
    ));
    let input = SteerInput {
        followup_id: Uuid::now_v7(),
        text: "more".into(),
        speaker_label: "Ann".into(),
    };
    let id = input.followup_id;
    let steering = tokio::spawn({
        async move {
            let r = s.steer(input).await.unwrap();
            (r, s)
        }
    });
    let c = cmd(&mut pi).await;
    assert_eq!(
        c,
        json!({"type": "prompt", "message": "Ann: more", "streamingBehavior": "steer"})
    );
    say(&pi, &[QUEUED]).await;
    let (ack, _s) = steering.await.unwrap();
    assert_eq!(ack, SteerAck::Accepted);
    say(
        &pi,
        &[
            r#"{"type":"queue_update","steering":[],"followUp":[]}"#,
            r#"{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"Ann: more"}]}}"#,
            A_START,
            A_END,
            SETTLED,
        ],
    )
    .await;
    let ev = drain(rx).await;
    assert!(
        ev.contains(&HarnessEvent::MessageJoined { followup_id: id }),
        "{ev:?}"
    );
    assert!(matches!(ev.last(), Some(HarnessEvent::Done { .. })));
}

#[tokio::test]
async fn a_steer_that_raced_the_end_of_the_turn_is_refused_aborted_and_waited_for() {
    let (a, mut launches) = adapter();
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let (_, mut pi) = launches.recv().await.unwrap();
    let mut rx = s.run_turn(turn("go")).await.unwrap();
    cmd(&mut pi).await;
    say(&pi, &[STARTED]).await;
    assert!(matches!(
        rx.recv().await,
        Some(HarnessEvent::SessionRef { .. })
    ));
    let st = tokio::spawn(async move {
        let r = s.steer(steer("racing")).await.unwrap();
        (r, s)
    });
    assert_eq!(cmd(&mut pi).await["streamingBehavior"], "steer");
    // Pi answers `started`: the run had just ended, so it started one of its own.
    say(&pi, &[STARTED]).await;
    let (ack, mut s) = st.await.unwrap();
    assert_eq!(ack, SteerAck::Refused);
    assert_eq!(
        cmd(&mut pi).await,
        json!({"type": "abort"}),
        "the stray run is aborted"
    );
    say(&pi, &[SETTLED]).await;
    drain(rx).await;
    // The next turn first waits for the stray run's aborted end, then sends its prompt.
    let rx = s.run_turn(turn("next")).await.unwrap();
    say(&pi, &[SETTLED_ABORTED]).await;
    assert_eq!(cmd(&mut pi).await["message"], "next");
    say(&pi, &[STARTED, SETTLED]).await;
    assert!(matches!(
        drain(rx).await.last(),
        Some(HarnessEvent::Done { .. })
    ));
}

#[tokio::test]
async fn abort_asks_pi_to_stop_and_ends_the_turn_exactly_once() {
    let (a, mut launches) = adapter();
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let (_, mut pi) = launches.recv().await.unwrap();
    let rx = s.run_turn(turn("long")).await.unwrap();
    cmd(&mut pi).await;
    say(&pi, &[STARTED, A_START, DELTA]).await;
    s.abort().await.unwrap();
    assert_eq!(cmd(&mut pi).await, json!({"type": "abort"}));
    say(&pi, &[SETTLED_ABORTED]).await;
    let ev = drain(rx).await;
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1);
    assert!(matches!(ev.last(), Some(HarnessEvent::Error { code, .. }) if code == "aborted"));
    assert!(
        !pi.killed.load(Ordering::SeqCst),
        "a child that settles is kept"
    );
}

#[tokio::test(start_paused = true)]
async fn a_child_that_does_not_settle_after_an_abort_is_killed_and_the_next_turn_starts_a_new_one()
{
    let (a, mut launches) = adapter();
    let o = tmp_opts("anthropic_messages");
    let mut s = a.start_session(o).await.unwrap();
    let (spec1, mut pi) = launches.recv().await.unwrap();
    let rx = s.run_turn(turn("long")).await.unwrap();
    cmd(&mut pi).await;
    say(&pi, &[STARTED]).await;
    s.abort().await.unwrap();
    cmd(&mut pi).await; // the abort command; Pi never answers
    let ev = drain(rx).await;
    assert!(matches!(ev.last(), Some(HarnessEvent::Error { code, .. }) if code == "aborted"));
    assert!(pi.killed.load(Ordering::SeqCst));
    // The next turn resumes the session in a new child: same session id and directory.
    let rx = s.run_turn(turn("again")).await.unwrap();
    let (spec2, mut pi2) = launches.recv().await.unwrap();
    assert_eq!(spec1.args, spec2.args);
    cmd(&mut pi2).await;
    say(&pi2, &[STARTED, SETTLED]).await;
    assert!(matches!(
        drain(rx).await.last(),
        Some(HarnessEvent::Done { .. })
    ));
}

#[tokio::test]
async fn a_child_that_exits_fails_the_turn_with_harness_failed_and_is_restarted_for_the_next() {
    let (a, mut launches) = adapter();
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let (_, mut pi) = launches.recv().await.unwrap();
    let rx = s.run_turn(turn("x")).await.unwrap();
    cmd(&mut pi).await;
    drop(pi.out); // stdout closes: the process is gone
    let ev = drain(rx).await;
    assert!(
        matches!(ev.last(), Some(HarnessEvent::Error { code, .. }) if code == "harness_failed")
    );
    let rx = s.run_turn(turn("y")).await.unwrap();
    let (_, mut pi2) = launches.recv().await.unwrap();
    cmd(&mut pi2).await;
    say(&pi2, &[STARTED, SETTLED]).await;
    assert!(matches!(
        drain(rx).await.last(),
        Some(HarnessEvent::Done { .. })
    ));
}

#[tokio::test]
async fn a_refused_prompt_is_a_failed_turn_not_a_hang() {
    let (a, mut launches) = adapter();
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let (_, mut pi) = launches.recv().await.unwrap();
    let rx = s.run_turn(turn("x")).await.unwrap();
    cmd(&mut pi).await;
    say(
        &pi,
        &[r#"{"type":"response","command":"prompt","success":false,"error":"no model"}"#],
    )
    .await;
    let ev = drain(rx).await;
    assert!(
        matches!(ev.last(), Some(HarnessEvent::Error { code, message }) if code == "harness_failed" && message.contains("no model"))
    );
}

#[tokio::test]
async fn unspeakable_dialects_and_unsafe_refs_are_refused_before_anything_starts() {
    let (a, mut launches) = adapter();
    for d in ["anthropic_bedrock", "anthropic_vertex", "gemini"] {
        let e = a.start_session(tmp_opts(d)).await.err().unwrap();
        assert_eq!(e.code, "harness_failed", "{d}");
    }
    for r in ["../x", "-x", "a/b", ""] {
        let mut o = tmp_opts("anthropic_messages");
        o.resume_ref = Some(r.into());
        let e = a.start_session(o).await.err().unwrap();
        assert_eq!(e.code, "invalid_session_ref", "{r}");
    }
    assert!(launches.try_recv().is_err(), "nothing was launched");
}

#[tokio::test]
async fn purge_removes_exactly_one_sessions_directory_and_is_idempotent() {
    let (a, mut launches) = adapter();
    let home = std::env::temp_dir().join(format!("pi-purge-{}", Uuid::now_v7()));
    let mut keep_opts = tmp_opts("anthropic_messages");
    keep_opts.working_dir = home.clone();
    let mut gone_opts = tmp_opts("anthropic_messages");
    gone_opts.working_dir = home.clone();
    let (keep, gone) = (
        keep_opts.conversation_id.to_string(),
        gone_opts.conversation_id.to_string(),
    );
    let mut s1 = a.start_session(keep_opts).await.unwrap();
    let mut s2 = a.start_session(gone_opts).await.unwrap();
    launches.recv().await.unwrap();
    launches.recv().await.unwrap();
    s1.release().await.unwrap();
    s2.release().await.unwrap();
    let dir = |r: &str| render::session_dir(&home, ID, r);
    std::fs::write(dir(&gone).join("2026_session.jsonl"), "{}").unwrap();
    std::fs::write(dir(&keep).join("2026_session.jsonl"), "{}").unwrap();
    a.purge_session(&home, &gone).await.unwrap();
    assert!(!dir(&gone).exists());
    assert!(
        dir(&keep).join("2026_session.jsonl").exists(),
        "another session is untouched"
    );
    assert!(
        render::config_dir(&home, ID).join("models.json").exists(),
        "the config is not state"
    );
    a.purge_session(&home, &gone).await.unwrap(); // idempotent
    for bad in ["../x", "", "a/b", "-x"] {
        assert_eq!(
            a.purge_session(&home, bad).await.unwrap_err().code,
            "invalid_session_ref",
            "{bad}"
        );
    }
}

#[test]
fn capabilities_are_declared_and_equal_the_protocol_table() {
    let (a, _l) = adapter();
    let c = a.capabilities();
    assert!(c.resume && c.steer && c.tool_events && c.platform_tools && c.thinking_deltas);
    assert!(!c.background_tool);
    assert_eq!(
        c.dialects,
        ["anthropic_messages", "openai_responses", "openai_chat"]
    );
    assert_eq!(c.skills_dir, ".agents/skills");
    assert_eq!(c.instruction_files, ["AGENTS.md"]);
    assert_eq!(
        c.model_roles,
        aurora_protocol::runner::harness_model_roles(ID)
            .iter()
            .map(|r| (*r).to_owned())
            .collect::<Vec<_>>()
    );
}
