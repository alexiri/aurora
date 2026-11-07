#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Session behaviour against a scripted child (no CLI needed).

use std::sync::Mutex as StdMutex;

use aurora_protocol::runner::{ModelChoice, Recall};
use uuid::Uuid;

use super::*;
use crate::harness::McpBridgeSpec;
use config::LaunchSpec;

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
        }
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
            }
        });
        let killed = self.killed.clone();
        Ok(ChildIo {
            stdin: in_tx,
            lines: out_rx,
            kill: Box::new(move || {
                killed.store(true, Ordering::SeqCst);
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

#[tokio::test]
async fn steer_refused_and_background_unsupported_and_caps() {
    let a = ClaudeCodeAdapter::with_launcher(Arc::new(Mock::new(None, REPLY)));
    let caps = a.capabilities();
    assert!(caps.resume && caps.tool_events && caps.thinking_deltas && caps.platform_tools);
    assert!(!caps.steer && !caps.background_tool);
    assert_eq!(caps.dialects, ["anthropic_messages"]);
    assert_eq!(caps.skills_dir.as_deref(), Some(".claude/skills"));
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

#[tokio::test]
async fn resume_ref_is_passed_and_release_keeps_it() {
    let mock = Mock::new(None, REPLY);
    let (specs, killed) = (mock.specs.clone(), mock.killed.clone());
    let a = ClaudeCodeAdapter::with_launcher(Arc::new(mock));
    let s = a.start_session(opts(Some("prev-1"))).await.unwrap();
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
