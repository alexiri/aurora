#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Session behaviour against a scripted child (no CLI needed).

use std::sync::Mutex as StdMutex;

use aurora_protocol::runner::{ModelChoice, Recall};
use uuid::Uuid;

use super::*;
use crate::harness::McpBridgeSpec;
use config::LaunchSpec;

/// (FIX-297) A link planted anywhere the purge walks: nothing it points at is listed or
/// removed. `etc` is a safe session reference, so a followed link would remove such an entry.
#[tokio::test]
async fn a_session_purge_removes_nothing_through_a_planted_link() {
    use std::fs;
    for link in [
        ".claude",
        ".claude/projects",
        ".claude/projects/p",
        ".claude/session-env",
        ".claude/file-history",
        ".claude/todos",
    ] {
        let dir = std::env::temp_dir().join(format!("claude-purge-{}", Uuid::now_v7()));
        let (home, outside) = (dir.join("home"), dir.join("outside"));
        // Whatever the link stands for, the folder outside holds what a purge would remove.
        let victims = [
            "etc/keep",
            "etc.jsonl",
            "etc-1.json",
            "p/etc/keep",
            "p/etc.jsonl",
            "session-env/etc/keep",
            "file-history/etc/keep",
            "todos/etc-1.json",
            "projects/p/etc/keep",
        ];
        for victim in victims {
            let path = outside.join(victim);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "outside").unwrap();
        }
        let at = home.join(link);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &at).unwrap();

        let adapter = ClaudeCodeAdapter::default().config_home(home.clone());
        let _ = adapter.purge_session(&home, "etc").await;
        for victim in victims {
            assert!(outside.join(victim).exists(), "{link}: {victim} removed");
        }
        fs::remove_dir_all(dir).unwrap();
    }
}

const INIT_OK: &str = r#"{"type":"system","subtype":"init","session_id":"sess-A","tools":["mcp__aurora__memory_save"],"mcp_servers":[{"name":"aurora","status":"connected"}]}"#;
const INIT_BAD: &str = r#"{"type":"system","subtype":"init","session_id":"sess-A","tools":["Bash"],"mcp_servers":[{"name":"aurora","status":"failed"}]}"#;
const REPLY: &str = include_str!("fixtures/text_turn.jsonl");

/// Scripted launcher: optional init (after a delay) at spawn, `reply` lines per user message.
struct Mock {
    init: Option<(&'static str, Duration)>,
    reply: Vec<String>,
    /// Never answer (for abort).
    silent: bool,
    specs: Arc<StdMutex<Vec<LaunchSpec>>>,
    /// (arrival instant, was init already emitted)
    received: Arc<StdMutex<Vec<bool>>>,
    killed: Arc<AtomicBool>,
    /// A kill is only asked for: the child has not ended until something waits for its end.
    lingers: bool,
    /// (FIX-253) Lines written after the turn's `result` (the idle child's own run), `late_after`
    /// behind the reply.
    late: Vec<String>,
    late_after: Duration,
}

impl Mock {
    fn new(init: Option<(&'static str, Duration)>, reply: &str) -> Self {
        Self {
            init,
            reply: reply
                .lines()
                .filter(|l| !l.contains("\"init\""))
                .map(str::to_owned)
                .collect(),
            silent: false,
            specs: Arc::default(),
            received: Arc::default(),
            killed: Arc::default(),
            lingers: false,
            late: Vec::new(),
            late_after: Duration::from_millis(150),
        }
    }
}

/// A scripted child starts no processes: it has ended once it was killed.
struct MockProcs(Arc<AtomicBool>);

#[async_trait]
impl launcher::ChildProcs for MockProcs {
    fn ended(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    async fn end(&self) -> Result<(), HarnessError> {
        self.0.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl Launcher for Mock {
    async fn launch(&self, spec: LaunchSpec) -> Result<ChildIo, HarnessError> {
        self.specs.lock().unwrap().push(spec);
        let (in_tx, mut in_rx) = mpsc::channel::<String>(8);
        let (out_tx, out_rx) = mpsc::channel::<String>(64);
        let emitted = Arc::new(AtomicBool::new(self.init.is_none()));
        if let Some((line, delay)) = self.init {
            let (tx, em) = (out_tx.clone(), emitted.clone());
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                em.store(true, Ordering::SeqCst);
                let _ = tx.send(line.to_owned()).await;
            });
        }
        let (reply, silent, received) = (self.reply.clone(), self.silent, self.received.clone());
        let late = self.late.clone();
        let late_after = self.late_after;
        let tx = out_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = in_rx.recv().await {
                assert!(msg.contains("\"type\":\"user\""));
                received
                    .lock()
                    .unwrap()
                    .push(emitted.load(Ordering::SeqCst));
                if silent {
                    continue;
                }
                for l in &reply {
                    let _ = tx.send(l.clone()).await;
                }
                if !late.is_empty() {
                    let (tx, late) = (tx.clone(), late.clone());
                    tokio::spawn(async move {
                        tokio::time::sleep(late_after).await;
                        for l in late {
                            let _ = tx.send(l).await;
                        }
                    });
                }
            }
        });
        let killed = self.killed.clone();
        let lingers = self.lingers;
        Ok(ChildIo {
            stdin: in_tx,
            lines: out_rx,
            procs: Arc::new(MockProcs(killed.clone())),
            kill: Box::new(move || {
                killed.fetch_or(!lingers, Ordering::SeqCst);
                drop(out_tx); // the scripted child's own clone ends with its task
            }),
        })
    }
}

fn opts(resume: Option<&str>) -> SessionOptions {
    SessionOptions {
        conversation_id: Uuid::nil(),
        working_dir: "/workspace".into(),
        resume_ref: resume.map(str::to_owned),
        model: ModelChoice {
            provider: "litellm".into(),
            endpoint_id: "e".into(),
            base_url: "https://gw/v1".into(),
            dialect: "anthropic_messages".into(),
            model: "m".into(),
            effort: "medium".into(),
            client_env: vec![],
            roles: None,
            subagent_role: None,
            model_meta: None,
        },
        proxy_url: "http://127.0.0.1:1".into(),
        mcp_bridge: McpBridgeSpec {
            command: vec!["sup".into(), "mcp-bridge".into()],
        },
        env: vec![],
    }
}

fn turn(text: &str) -> TurnInput {
    TurnInput {
        turn_id: Uuid::nil(),
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

async fn collect(mut rx: HarnessEventStream) -> Vec<HarnessEvent> {
    let mut v = Vec::new();
    while let Some(e) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
    {
        v.push(e);
    }
    v
}

#[tokio::test]
async fn first_turn_is_held_until_mcp_tools_listed() {
    let mock = Mock::new(Some((INIT_OK, Duration::from_millis(150))), REPLY);
    let received = mock.received.clone();
    let a = ClaudeCodeAdapter::with_launcher(Arc::new(mock));
    let mut s = a.start_session(opts(None)).await.unwrap();
    let ev = collect(s.run_turn(turn("hi")).await.unwrap()).await;
    assert_eq!(
        *received.lock().unwrap(),
        [true],
        "message must arrive after init"
    );
    assert!(matches!(ev.last(), Some(HarnessEvent::Done { .. })));
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1);
    assert_eq!(s.session_ref().as_deref(), Some("sess-A"));
}

#[tokio::test]
async fn init_without_mcp_tools_fails_after_hold_timeout() {
    let mock = Mock::new(Some((INIT_BAD, Duration::ZERO)), REPLY);
    let received = mock.received.clone();
    let a =
        ClaudeCodeAdapter::with_launcher(Arc::new(mock)).hold_timeout(Duration::from_millis(200));
    let mut s = a.start_session(opts(None)).await.unwrap();
    let ev = collect(s.run_turn(turn("hi")).await.unwrap()).await;
    assert!(matches!(&ev[..], [HarnessEvent::Error { code, .. }] if code == "harness_failed"));
    assert!(
        received.lock().unwrap().is_empty(),
        "no message may reach a harness without tools"
    );
}

#[tokio::test]
async fn no_init_before_message_proceeds_after_timeout() {
    let mock = Mock::new(None, REPLY);
    let a =
        ClaudeCodeAdapter::with_launcher(Arc::new(mock)).hold_timeout(Duration::from_millis(50));
    let mut s = a.start_session(opts(None)).await.unwrap();
    let ev = collect(s.run_turn(turn("hi")).await.unwrap()).await;
    assert!(matches!(ev.last(), Some(HarnessEvent::Done { .. })));
}

/// A scripted child that holds the turn's first message (a running tool) and ends its run when a
/// second user line arrives: with the echo `--replay-user-messages` gives it (`echo`), or without
/// (the run ended before it took the line: the CLI would run it as a turn of its own).
struct SteerMock {
    echo: bool,
    killed: Arc<AtomicBool>,
}

#[async_trait]
impl Launcher for SteerMock {
    async fn launch(&self, _spec: LaunchSpec) -> Result<ChildIo, HarnessError> {
        let (in_tx, mut in_rx) = mpsc::channel::<String>(8);
        let (out_tx, out_rx) = mpsc::channel::<String>(64);
        let (echo, tx) = (self.echo, out_tx.clone());
        tokio::spawn(async move {
            let mut first = true;
            while let Some(msg) = in_rx.recv().await {
                if std::mem::take(&mut first) {
                    continue; // the turn's own message: the run is now in a long tool call
                }
                let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
                let text = v["message"]["content"][0]["text"].as_str().unwrap();
                if echo {
                    let line = serde_json::json!({"type": "user", "isReplay": true,
                        "message": {"role": "user", "content": [{"type": "text", "text": text}]}});
                    let _ = tx.send(line.to_string()).await;
                }
                for l in REPLY.lines().filter(|l| !l.contains("\"init\"")) {
                    let _ = tx.send(l.to_owned()).await;
                }
            }
        });
        let killed = self.killed.clone();
        Ok(ChildIo {
            stdin: in_tx,
            lines: out_rx,
            procs: Arc::new(MockProcs(killed.clone())),
            kill: Box::new(move || {
                killed.store(true, Ordering::SeqCst);
                drop(out_tx);
            }),
        })
    }
}

fn steer_input(id: Uuid, label: &str) -> SteerInput {
    SteerInput {
        followup_id: id,
        text: "more".into(),
        speaker_label: label.into(),
    }
}

/// (FIX-225) A steer written during a run is accepted, joins on the CLI's echo of the line, and
/// the run still ends with exactly one terminal event.
#[tokio::test]
async fn claude_code_steer_during_a_run_joins_on_the_echo_and_the_run_ends_once() {
    let killed = Arc::new(AtomicBool::new(false));
    let a = ClaudeCodeAdapter::with_launcher(Arc::new(SteerMock {
        echo: true,
        killed: killed.clone(),
    }))
    .hold_timeout(Duration::from_millis(50));
    let mut s = a.start_session(opts(None)).await.unwrap();
    let rx = s.run_turn(turn("hi")).await.unwrap();
    // The door opens once the turn's own message is queued ahead of any steer.
    let id = Uuid::now_v7();
    let ack = loop {
        let st = s.steer(steer_input(id, "Ann")).await.unwrap();
        if st == SteerAck::Accepted {
            break st;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(ack, SteerAck::Accepted);
    let ev = collect(rx).await;
    assert!(
        ev.contains(&HarnessEvent::MessageJoined { followup_id: id }),
        "{ev:?}"
    );
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1, "{ev:?}");
    assert!(matches!(ev.last(), Some(HarnessEvent::Done { .. })));
    assert!(
        !killed.load(Ordering::SeqCst),
        "an echoed steer is no stray"
    );
    // between turns the same call refuses, whatever the CLI would do with the line
    assert_eq!(
        s.steer(steer_input(Uuid::now_v7(), "")).await.unwrap(),
        SteerAck::Refused
    );
}

/// (FIX-225) A steer the run ended without taking is never joined, never replayed, and the CLI
/// that would run it as a turn of its own is killed (the next turn resumes the session).
#[tokio::test]
async fn claude_code_steer_the_run_did_not_take_is_not_joined_and_the_child_is_killed() {
    let killed = Arc::new(AtomicBool::new(false));
    let a = ClaudeCodeAdapter::with_launcher(Arc::new(SteerMock {
        echo: false,
        killed: killed.clone(),
    }))
    .hold_timeout(Duration::from_millis(50));
    let mut s = a.start_session(opts(None)).await.unwrap();
    let rx = s.run_turn(turn("hi")).await.unwrap();
    let id = Uuid::now_v7();
    loop {
        if s.steer(steer_input(id, "")).await.unwrap() == SteerAck::Accepted {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let ev = collect(rx).await;
    assert!(
        !ev.iter()
            .any(|e| matches!(e, HarnessEvent::MessageJoined { .. })),
        "{ev:?}"
    );
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1, "{ev:?}");
    assert!(killed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn steer_refused_between_turns_background_unsupported_and_caps() {
    let a = ClaudeCodeAdapter::with_launcher(Arc::new(Mock::new(None, REPLY)));
    let caps = a.capabilities();
    assert!(caps.resume && caps.tool_events && caps.thinking_deltas && caps.platform_tools);
    assert!(caps.steer && !caps.background_tool);
    assert_eq!(
        caps.dialects,
        [
            "anthropic_messages",
            "anthropic_bedrock",
            "anthropic_vertex"
        ]
    );
    assert_eq!(caps.skills_dir, ".claude/skills");
    assert_eq!(caps.instruction_files, ["CLAUDE.md", "AGENTS.md"]);
    let mut s = a.start_session(opts(None)).await.unwrap();
    let st = s
        .steer(SteerInput {
            followup_id: Uuid::nil(),
            text: "x".into(),
            speaker_label: String::new(),
        })
        .await
        .unwrap();
    assert_eq!(st, SteerAck::Refused);
    assert_eq!(
        s.background_running_tool().await.unwrap(),
        SendNowOutcome::Unsupported
    );
}

#[tokio::test]
async fn abort_ends_with_single_aborted_and_restart_resumes() {
    let mut mock = Mock::new(Some((INIT_OK, Duration::ZERO)), REPLY);
    mock.silent = true;
    let (specs, killed) = (mock.specs.clone(), mock.killed.clone());
    let a = ClaudeCodeAdapter::with_launcher(Arc::new(mock));
    let mut s = a.start_session(opts(None)).await.unwrap();
    let rx = s.run_turn(turn("hi")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    s.abort().await.unwrap();
    let ev = collect(rx).await;
    assert!(
        matches!(&ev[..], [HarnessEvent::Error { code, .. }] if code == "aborted"),
        "{ev:?}"
    );
    assert!(killed.load(Ordering::SeqCst));
    // next turn restarts the child with --resume <session id>
    let _ = s.run_turn(turn("again")).await.unwrap();
    let sp = specs.lock().unwrap();
    assert_eq!(sp.len(), 2);
    assert!(!sp[0].args.contains(&"--resume".to_owned()));
    assert!(
        sp[1].args.windows(2).any(|w| w == ["--resume", "sess-A"]),
        "{:?}",
        sp[1].args
    );
}

/// (FIX-258) A child that was killed, or killed and replaced, and has not ended yet may still
/// run: the session says so until something has seen it end.
#[tokio::test]
async fn a_killed_child_is_still_ending_until_it_ended() {
    let mut mock = Mock::new(Some((INIT_OK, Duration::ZERO)), REPLY);
    mock.silent = true;
    mock.lingers = true;
    let a = ClaudeCodeAdapter::with_launcher(Arc::new(mock));
    let mut s = a.start_session(opts(None)).await.unwrap();
    assert!(!s.still_ending());
    let rx = s.run_turn(turn("hi")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!s.still_ending(), "a running child is not ending");
    s.abort().await.unwrap();
    collect(rx).await;
    assert!(s.still_ending(), "killed, not ended");
    s.release().await.unwrap();
    assert!(!s.still_ending());
}

#[tokio::test]
async fn resume_ref_is_passed_and_release_keeps_it() {
    let mock = Mock::new(None, REPLY);
    let (specs, killed) = (mock.specs.clone(), mock.killed.clone());
    let a = ClaudeCodeAdapter::with_launcher(Arc::new(mock));
    let mut s = a.start_session(opts(Some("prev-1"))).await.unwrap();
    assert_eq!(s.session_ref().as_deref(), Some("prev-1"));
    s.release().await.unwrap();
    assert!(killed.load(Ordering::SeqCst));
    assert!(
        specs.lock().unwrap()[0]
            .args
            .windows(2)
            .any(|w| w == ["--resume", "prev-1"])
    );
}

#[tokio::test]
async fn child_exit_mid_turn_yields_one_error() {
    // reply without a result line, then the scripted child side ends
    let mut mock = Mock::new(Some((INIT_OK, Duration::ZERO)), "");
    mock.reply = vec![];
    let a = ClaudeCodeAdapter::with_launcher(Arc::new(mock));
    let mut s = a.start_session(opts(None)).await.unwrap();
    let rx = s.run_turn(turn("hi")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    s.abort().await.unwrap();
    let ev = collect(rx).await;
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1);
}

#[tokio::test]
async fn purge_session_removes_every_stored_file_of_that_session_only() {
    let home = std::env::temp_dir().join(format!("aurora-purge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let c = home.join(".claude");
    let proj = c.join("projects/-workspace");
    for d in [
        proj.join("sess-1"),
        proj.join("sess-2"),
        c.join("session-env/sess-1"),
        c.join("file-history/sess-1"),
        c.join("todos"),
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    for f in [
        proj.join("sess-1.jsonl"),
        proj.join("sess-2.jsonl"),
        proj.join("sess-1/subagent.jsonl"),
        c.join("todos/sess-1-agent-sess-1.json"),
        c.join("todos/sess-2-agent-sess-2.json"),
    ] {
        std::fs::write(f, "secret").unwrap();
    }
    let a = ClaudeCodeAdapter::with_launcher(Arc::new(Mock::new(None, REPLY)))
        .config_home(home.clone());
    let wd = home.join("workspace");
    a.purge_session(&wd, "sess-1").await.unwrap();
    assert!(!proj.join("sess-1.jsonl").exists() && !proj.join("sess-1").exists());
    assert!(!c.join("session-env/sess-1").exists() && !c.join("file-history/sess-1").exists());
    assert!(!c.join("todos/sess-1-agent-sess-1.json").exists());
    assert!(
        proj.join("sess-2.jsonl").exists() && c.join("todos/sess-2-agent-sess-2.json").exists()
    );
    // idempotent, and a path-shaped reference is refused without touching anything
    a.purge_session(&wd, "sess-1").await.unwrap();
    assert!(a.purge_session(&wd, "../projects").await.is_err());
    assert!(a.purge_session(&wd, "").await.is_err());
    assert!(proj.join("sess-2").exists());
    let _ = std::fs::remove_dir_all(&home);
}

/// (FIX-253, D-490) The owner's case, reproduced against the scripted child (no CLI): the model
/// starts a background command (`run_in_background`), answers "I'll finish the task report once
/// it's done", the turn ends at its `result` — and the child is not killed, keeps running, and
/// acts on its own.
///
/// What the supervisor sees: the turn's own tool event and the terminal `Done`, nothing else.
/// What is lost: everything the child writes after the `result` — here a whole run that pushed a
/// fix — reaches nobody. The stream is over, no turn is open, so the lines sit unread; the next
/// turn then finds them first and maps the stale run into itself (the old `result` ends it).
#[tokio::test]
async fn background_work_after_the_turn_ends_reaches_nobody() {
    let reply = include_str!("fixtures/background_turn.jsonl");
    let mut mock = Mock::new(Some((INIT_OK, Duration::from_millis(0))), reply);
    mock.late = include_str!("fixtures/background_after.jsonl")
        .lines()
        .map(str::to_owned)
        .collect();
    let killed = mock.killed.clone();
    let adapter = ClaudeCodeAdapter::with_launcher(Arc::new(mock));
    let mut s = adapter.start_session(opts(None)).await.unwrap();

    let events = collect(s.run_turn(turn("run the e2e suite")).await.unwrap()).await;
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(
        matches!(&events[0], HarnessEvent::ToolStarted { name, summary, .. }
            if name == "Bash" && summary == "Bash: npm run e2e:full"),
        "{events:?}"
    );
    assert!(
        matches!(events.last(), Some(HarnessEvent::Done { .. })),
        "{events:?}"
    );
    assert!(
        !killed.load(Ordering::SeqCst),
        "the child is not killed at turn end"
    );

    // The idle child's own run is written to stdout, and nobody is reading: the receiver of the
    // ended turn is exhausted, and no event ever carried "pushed a fix".
    tokio::time::sleep(Duration::from_millis(400)).await;

    // The next turn finds the stale lines first: its own message is queued behind them, and the
    // stale run's `result` ends it. The stale text lands in the new turn as if it were its own.
    let events2 = collect(s.run_turn(turn("and now?")).await.unwrap()).await;
    let flattened = format!("{events2:?}");
    assert!(
        flattened.contains("pushed a fix"),
        "the stale run leaks into the next turn: {flattened}"
    );
}

/// (FIX-253, D-490) Part B: with the idle reader watching, what the child does after the turn is
/// recorded, not lost. The supervisor starts the reader while background work remains; the
/// child's own run becomes events (text, tool calls); a following turn takes the stream back with
/// nothing lost and nothing duplicated — the stale run never leaks into the new turn.
#[tokio::test]
async fn the_idle_reader_records_what_the_child_did_after_the_turn() {
    let reply = include_str!("fixtures/background_turn.jsonl");
    let mut mock = Mock::new(Some((INIT_OK, Duration::from_millis(0))), reply);
    mock.late = include_str!("fixtures/background_after.jsonl")
        .lines()
        .map(str::to_owned)
        .collect();
    let killed = mock.killed.clone();
    let adapter = ClaudeCodeAdapter::with_launcher(Arc::new(mock));
    let mut s = adapter.start_session(opts(None)).await.unwrap();

    let _ = collect(s.run_turn(turn("run the e2e suite")).await.unwrap()).await;
    assert!(!killed.load(Ordering::SeqCst));
    s.start_idle_reader();

    // The idle child's own run is read while nobody else holds the stream: its text arrives as an
    // event instead of sitting unread.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let idle = s.take_idle_events();
    assert!(
        idle.iter().any(
            |e| matches!(e, HarnessEvent::TextDelta { delta, .. } if delta.contains("pushed a fix"))
        ),
        "{idle:?}"
    );
    // (FIX-253 review, medium 5) The idle run's own usage leaves with it, once and in a turn's
    // shape (the run's `result`), so the API can count it.
    let usage: Vec<&serde_json::Value> = idle
        .iter()
        .filter_map(|e| match e {
            HarnessEvent::Usage { usage } => Some(usage),
            _ => None,
        })
        .collect();
    assert_eq!(usage.len(), 1, "{idle:?}");
    assert_eq!(usage[0]["output_tokens"], 30, "{idle:?}");
    // Drained: a second drain is empty (no duplication).
    assert!(s.take_idle_events().is_empty());

    // Hand-over: the next turn gets the stream back promptly and runs clean — the stale run is in
    // neither the turn's events (already recorded) nor a loss.
    let events2 = collect(s.run_turn(turn("and now?")).await.unwrap()).await;
    let flattened = format!("{events2:?}");
    assert!(
        !flattened.contains("pushed a fix"),
        "the recorded idle run leaks into the next turn: {flattened}"
    );
    s.release().await.unwrap();
}

/// (FIX-253, D-490) The same scripted child, but what the supervisor can now do about it: the
/// `run_in_background` command is tracked while mapping and `background_work()` reports it at
/// turn end (once — reading consumes the report), and a dead child's background work is gone.
#[tokio::test]
async fn background_work_is_tracked_and_reported_at_turn_end() {
    let reply = include_str!("fixtures/background_turn.jsonl");
    let adapter = ClaudeCodeAdapter::with_launcher(Arc::new(Mock::new(
        Some((INIT_OK, Duration::from_millis(0))),
        reply,
    )));
    let mut s = adapter.start_session(opts(None)).await.unwrap();
    let _ = collect(s.run_turn(turn("run the e2e suite")).await.unwrap()).await;
    let work = s.background_work();
    assert_eq!(work.len(), 1, "{work:?}");
    assert_eq!(work[0].command, "npm run e2e:full");
    assert!(work[0].started_at > 0);
    // Reading does not consume: still-running work is re-reported at the next turn end (the API
    // side keeps one visible entry until it is stopped).
    assert_eq!(s.background_work().len(), 1);
    // A released child is killed, and its background work died with it.
    s.release().await.unwrap();
}
