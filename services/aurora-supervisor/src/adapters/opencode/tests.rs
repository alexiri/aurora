#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Session behaviour against a scripted child (no CLI needed), the rendered files and the purge.

use std::time::Duration;

use aurora_protocol::runner::Recall;
use tokio::sync::oneshot;
use tokio::time::timeout;
use uuid::Uuid;

use super::config::tests::opts;
use super::*;
use config::LaunchSpec;
use launcher::ChildIo;

const TEXT1: &str = r#"{"type":"text","sessionID":"ses_a","part":{"type":"text","text":"pong","messageID":"msg_1"}}"#;
const STOP: &str = r#"{"type":"step_finish","sessionID":"ses_a","part":{"reason":"stop","type":"step-finish","tokens":{"input":11,"output":4,"cache":{"write":2,"read":3}}}}"#;
const ROWS: &str = r#"[{"model":"claude-sonnet-4-5","input":11,"output":4,"cache_read":3,"cache_write":2},{"model":"sub-model","input":7,"output":2,"cache_read":0,"cache_write":0}]"#;

/// Scripted launcher: replays `reply` lines per launch and then ends the stream (or keeps it open),
/// exits with `exit`, and answers `opencode db` queries from `queries`.
struct Mock {
    /// One reply per launch (the last repeats).
    replies: Vec<Vec<String>>,
    stderr: Vec<String>,
    exit: Option<i32>,
    /// Keep the output open after the replies (a lingering child, or a silent one).
    linger: bool,
    /// Answers of the queries in order (the last repeats); `Err` = the query fails.
    queries: Vec<Result<String, ()>>,
    specs: Arc<StdMutex<Vec<LaunchSpec>>>,
    asked: Arc<StdMutex<Vec<LaunchSpec>>>,
    killed: Arc<AtomicBool>,
}

impl Mock {
    fn new(replies: &[&[&str]]) -> Self {
        Self {
            replies: replies
                .iter()
                .map(|r| r.iter().map(|l| (*l).to_owned()).collect())
                .collect(),
            stderr: vec![],
            exit: Some(0),
            linger: false,
            queries: vec![Ok(ROWS.to_owned())],
            specs: Arc::default(),
            asked: Arc::default(),
            killed: Arc::default(),
        }
    }
}

#[async_trait]
impl Launcher for Mock {
    async fn launch(&self, spec: LaunchSpec) -> Result<ChildIo, HarnessError> {
        let n = {
            let mut s = self.specs.lock().unwrap();
            s.push(spec);
            s.len()
        };
        let (out_tx, out_rx) = mpsc::channel::<String>(64);
        let reply = self.replies[(n - 1).min(self.replies.len() - 1)].clone();
        let linger = self.linger;
        let (exit_tx, exit_rx) = oneshot::channel();
        let exit = self.exit;
        tokio::spawn(async move {
            for l in reply {
                let _ = out_tx.send(l).await;
            }
            if linger {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            let _ = exit_tx.send(exit);
            drop(out_tx);
        });
        let killed = self.killed.clone();
        Ok(ChildIo {
            lines: out_rx,
            stderr: Arc::new(StdMutex::new(self.stderr.clone())),
            kill: Box::new(move || killed.store(true, Ordering::SeqCst)),
            exit: exit_rx,
        })
    }

    async fn query(&self, spec: LaunchSpec) -> Result<String, HarnessError> {
        let n = {
            let mut s = self.asked.lock().unwrap();
            s.push(spec);
            s.len()
        };
        self.queries[(n - 1).min(self.queries.len() - 1)]
            .clone()
            .map_err(|()| HarnessError::new("harness_failed", "query failed"))
    }
}

fn tmp_opts(dialect: &str) -> SessionOptions {
    let dir = std::env::temp_dir().join(format!("opencode-adapter-{}", Uuid::now_v7()));
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

async fn drain(mut rx: HarnessEventStream) -> Vec<HarnessEvent> {
    let mut v = Vec::new();
    while let Some(e) = timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("stream stalled")
    {
        v.push(e);
    }
    v
}

type Specs = Arc<StdMutex<Vec<LaunchSpec>>>;

fn adapter(m: Mock) -> (OpenCodeAdapter, Specs, Specs) {
    let (specs, asked) = (m.specs.clone(), m.asked.clone());
    (OpenCodeAdapter::with_launcher(Arc::new(m)), specs, asked)
}

#[tokio::test]
async fn a_turn_streams_text_ends_with_one_done_with_per_model_usage_and_the_next_turn_continues_the_session()
 {
    let (a, specs, asked) = adapter(Mock::new(&[&[TEXT1, STOP]]));
    let o = tmp_opts("anthropic_messages");
    let conv = o.conversation_id.to_string();
    let mut s = a.start_session(o).await.unwrap();
    assert_eq!(s.session_ref(), None);
    let ev = drain(s.run_turn(turn("hello")).await.unwrap()).await;
    assert_eq!(
        ev[0],
        HarnessEvent::SessionRef {
            session_ref: conv.clone()
        },
        "the ref is the conversation's id, the directory's name"
    );
    assert!(
        ev.iter()
            .any(|e| matches!(e, HarnessEvent::TextDelta { delta, .. } if delta == "pong"))
    );
    // Two models from the database: the totals are their sum.
    let Some(HarnessEvent::Done { usage: Some(u) }) = ev.last() else {
        panic!("{ev:?}")
    };
    assert_eq!(u["by_model"].as_array().unwrap().len(), 2, "{u}");
    assert_eq!(u["input_tokens"], 18);
    assert_eq!(u["output_tokens"], 6);
    assert_eq!(u["cache_read_tokens"], 3);
    assert_eq!(u["cache_write_tokens"], 2);
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1);
    assert_eq!(s.session_ref().as_deref(), Some(conv.as_str()));
    let q = asked.lock().unwrap()[0].args[1].clone();
    assert!(
        q.contains("'ses_a'") && q.contains("time_created >="),
        "{q}"
    );
    drain(s.run_turn(turn("again")).await.unwrap()).await;
    let specs = specs.lock().unwrap();
    assert_eq!(specs[0].stdin, "hello", "the prompt goes on stdin");
    assert!(
        !specs[0].args.contains(&"--session".to_owned()),
        "a new session is created"
    );
    assert!(
        specs[1]
            .args
            .windows(2)
            .any(|w| w == ["--session", "ses_a"]),
        "the session now exists: {:?}",
        specs[1].args
    );
    assert!(
        !specs[0].args.iter().any(|x| x.contains("hello")),
        "the prompt is never an argument"
    );
}

#[tokio::test]
async fn a_prompt_over_128_kib_reaches_stdin_whole_and_no_argument_carries_it() {
    let (a, specs, _) = adapter(Mock::new(&[&[TEXT1, STOP]]));
    let mut s = a.start_session(tmp_opts("openai_chat")).await.unwrap();
    let big = format!("{}END", "word ".repeat(40_000));
    drain(s.run_turn(turn(&big)).await.unwrap()).await;
    let specs = specs.lock().unwrap();
    assert!(specs[0].stdin.len() > 128 * 1024 && specs[0].stdin.ends_with("END"));
    assert!(
        specs[0].args.iter().all(|a| a.len() < 4096),
        "{:?}",
        specs[0].args
    );
}

#[tokio::test]
async fn a_restarted_process_finds_the_session_through_the_database_and_a_purged_one_starts_fresh()
{
    // The resume reference is given: the id comes from `opencode db`.
    let mut m = Mock::new(&[&[TEXT1, STOP]]);
    m.queries = vec![Ok(r#"[{"id":"ses_old"}]"#.into()), Ok(ROWS.into())];
    let (a, specs, asked) = adapter(m);
    let mut o = tmp_opts("anthropic_messages");
    o.resume_ref = Some("ref-1".into());
    let mut s = a.start_session(o).await.unwrap();
    assert_eq!(s.session_ref().as_deref(), Some("ref-1"));
    drain(s.run_turn(turn("x")).await.unwrap()).await;
    assert!(
        specs.lock().unwrap()[0]
            .args
            .windows(2)
            .any(|w| w == ["--session", "ses_old"])
    );
    assert!(asked.lock().unwrap()[0].args[1].contains("parent_id is null"));

    // The data is gone (a purge): no session to continue, a new one is created.
    let mut m = Mock::new(&[&[TEXT1, STOP]]);
    m.queries = vec![Ok("[]".into())];
    let (a, specs, _) = adapter(m);
    let mut o = tmp_opts("anthropic_messages");
    o.resume_ref = Some("ref-2".into());
    let mut s = a.start_session(o).await.unwrap();
    drain(s.run_turn(turn("x")).await.unwrap()).await;
    assert!(
        !specs.lock().unwrap()[0]
            .args
            .contains(&"--session".to_owned())
    );
}

#[tokio::test]
async fn usage_falls_back_to_one_entry_for_the_default_model_when_the_query_fails() {
    let mut m = Mock::new(&[&[TEXT1, STOP]]);
    m.queries = vec![Err(())];
    let (a, _, _) = adapter(m);
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let ev = drain(s.run_turn(turn("x")).await.unwrap()).await;
    let Some(HarnessEvent::Done { usage: Some(u) }) = ev.last() else {
        panic!("{ev:?}")
    };
    assert_eq!(u["by_model"].as_array().unwrap().len(), 1);
    assert_eq!(u["by_model"][0]["model"], "claude-sonnet-4-5");
    assert_eq!(u["input_tokens"], 11);
    assert_eq!(u["cache_write_tokens"], 2);
}

#[tokio::test]
async fn an_error_event_ends_the_turn_with_the_classified_code_and_nothing_follows() {
    let err = r#"{"type":"error","sessionID":"ses_a","error":{"name":"APIError","data":{"message":"x","statusCode":401,"isRetryable":false,"responseBody":"{\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"bad key\"}}"}}}"#;
    let mut m = Mock::new(&[&[err]]);
    m.exit = Some(1);
    let (a, _, asked) = adapter(m);
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let ev = drain(s.run_turn(turn("x")).await.unwrap()).await;
    assert!(
        matches!(ev.last(), Some(HarnessEvent::Error { code, message }) if code == "provider_auth" && message.contains("bad key")),
        "{ev:?}"
    );
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1);
    assert!(
        asked.lock().unwrap().is_empty(),
        "no usage query after an error"
    );
}

#[tokio::test]
async fn a_start_that_produced_no_event_is_a_failed_turn_with_stderr_and_does_not_continue_next() {
    let mut m = Mock::new(&[&[]]);
    m.stderr = vec!["Error: Unable to connect".into()];
    m.exit = Some(1);
    let (a, specs, _) = adapter(m);
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let ev = drain(s.run_turn(turn("x")).await.unwrap()).await;
    assert!(
        matches!(ev.last(), Some(HarnessEvent::Error { code, message }) if code == "harness_failed" && message.contains("status 1") && message.contains("Unable to connect")),
        "{ev:?}"
    );
    assert_eq!(s.session_ref(), None);
    drain(s.run_turn(turn("y")).await.unwrap()).await;
    assert!(
        !specs.lock().unwrap()[1]
            .args
            .contains(&"--session".to_owned())
    );
}

#[tokio::test]
async fn a_killed_child_without_an_exit_status_is_a_failed_turn() {
    let mut m = Mock::new(&[&[TEXT1]]);
    m.exit = None;
    let (a, _, _) = adapter(m);
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let ev = drain(s.run_turn(turn("x")).await.unwrap()).await;
    assert!(
        matches!(ev.last(), Some(HarnessEvent::Error { code, .. }) if code == "harness_failed"),
        "{ev:?}"
    );
}

#[tokio::test]
async fn a_lingering_process_after_the_complete_answer_does_not_hold_the_turn() {
    let mut m = Mock::new(&[&[TEXT1, STOP]]);
    m.linger = true;
    let killed = m.killed.clone();
    let (a, _, _) = adapter(m);
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let ev = drain(s.run_turn(turn("x")).await.unwrap()).await;
    assert!(
        matches!(ev.last(), Some(HarnessEvent::Done { .. })),
        "{ev:?}"
    );
    assert!(
        killed.load(Ordering::SeqCst),
        "the lingering process was stopped"
    );
}

#[tokio::test]
async fn abort_ends_a_silent_turn_with_one_terminal_event_and_kills_the_child() {
    let mut m = Mock::new(&[&[]]);
    m.linger = true;
    let killed = m.killed.clone();
    let (a, _, _) = adapter(m);
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let rx = s.run_turn(turn("x")).await.unwrap();
    s.abort().await.unwrap();
    let ev = drain(rx).await;
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1);
    assert!(matches!(ev.last(), Some(HarnessEvent::Error { code, .. }) if code == "aborted"));
    assert!(killed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn steer_is_always_refused_and_a_second_turn_while_running_is_an_error() {
    let mut m = Mock::new(&[&[]]);
    m.linger = true;
    let (a, _, _) = adapter(m);
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    assert!(!a.capabilities().steer);
    assert_eq!(
        s.steer(SteerInput {
            followup_id: Uuid::now_v7(),
            text: "x".into(),
            speaker_label: String::new()
        })
        .await
        .unwrap(),
        SteerAck::Refused
    );
    let _rx = s.run_turn(turn("x")).await.unwrap();
    assert!(s.run_turn(turn("y")).await.is_err());
    s.abort().await.unwrap();
}

#[tokio::test]
async fn the_root_is_rendered_before_every_start_and_planted_files_links_and_credentials_are_gone()
{
    let (a, _, _) = adapter(Mock::new(&[&[TEXT1, STOP]]));
    let o = tmp_opts("anthropic_messages");
    let root = render::session_dir(&o.working_dir, ID, &o.conversation_id.to_string());
    let mut s = a.start_session(o).await.unwrap();
    let cfg = root.join("config/opencode.json");
    let rendered = std::fs::read_to_string(&cfg).unwrap();
    assert!(rendered.contains("\"autoupdate\": false"));
    assert!(root.join("config/opencode/package-lock.json").exists());

    // Between turns an agent plants a credential, a second configuration, an agent definition, a
    // changed `opencode.json` and a link where `data` goes.
    let elsewhere = root.parent().unwrap().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("auth.json"), "keep me").unwrap();
    std::fs::write(
        cfg,
        r#"{"provider":{"x":{"options":{"apiKey":"sk-planted"}}}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("config/opencode/agents")).unwrap();
    std::fs::write(root.join("config/opencode/agents/evil.md"), "x").unwrap();
    std::fs::write(root.join("config/opencode/opencode.json"), "{}").unwrap();
    std::fs::remove_dir_all(root.join("data")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("data")).unwrap();
    drain(s.run_turn(turn("x")).await.unwrap()).await;

    assert_eq!(
        std::fs::read_to_string(root.join("config/opencode.json")).unwrap(),
        rendered
    );
    assert!(!root.join("config/opencode/agents").exists());
    assert!(!root.join("config/opencode/opencode.json").exists());
    assert!(root.join("data").is_dir() && !root.join("data").is_symlink());
    assert_eq!(
        std::fs::read_to_string(elsewhere.join("auth.json")).unwrap(),
        "keep me",
        "the link was not followed"
    );
    // A credential store planted where OpenCode keeps it is deleted at the next start too.
    std::fs::write(
        root.join("data/opencode/auth.json"),
        r#"{"x":{"key":"sk-planted"}}"#,
    )
    .unwrap();
    std::fs::write(root.join("data/opencode/mcp-auth.json"), "{}").unwrap();
    drain(s.run_turn(turn("y")).await.unwrap()).await;
    assert!(!root.join("data/opencode/auth.json").exists());
    assert!(!root.join("data/opencode/mcp-auth.json").exists());
}

#[tokio::test]
async fn purge_removes_one_conversations_directory_exactly_and_is_idempotent() {
    let (a, _, _) = adapter(Mock::new(&[&[TEXT1, STOP]]));
    let o1 = tmp_opts("anthropic_messages");
    let home = o1.working_dir.clone();
    let mut o2 = tmp_opts("anthropic_messages");
    o2.working_dir = home.clone();
    let (c1, c2) = (
        o1.conversation_id.to_string(),
        o2.conversation_id.to_string(),
    );
    a.start_session(o1).await.unwrap();
    a.start_session(o2).await.unwrap();
    let (d1, d2) = (
        render::session_dir(&home, ID, &c1),
        render::session_dir(&home, ID, &c2),
    );
    assert!(d1.exists() && d2.exists());
    a.purge_session(&home, &c1).await.unwrap();
    assert!(!d1.exists() && d2.exists());
    a.purge_session(&home, &c1).await.unwrap();
    for bad in ["../x", "-x", "a/b", ""] {
        assert_eq!(
            a.purge_session(&home, bad).await.unwrap_err().code,
            "invalid_session_ref"
        );
    }
}

#[tokio::test]
async fn models_and_dialects_the_cli_cannot_use_are_refused_at_the_start() {
    let (a, _, _) = adapter(Mock::new(&[&[]]));
    let mut o = tmp_opts("anthropic_bedrock");
    assert!(a.start_session(o.clone()).await.is_err());
    o.model.dialect = "anthropic_messages".into();
    o.model.model = "bad model;x".into();
    assert!(a.start_session(o).await.is_err());
    let caps = a.capabilities();
    assert_eq!(
        caps.dialects,
        ["anthropic_messages", "openai_responses", "openai_chat"]
    );
    assert_eq!(caps.model_roles, ["large", "medium", "small"]);
    assert!(caps.resume && caps.tool_events && caps.platform_tools && !caps.thinking_deltas);
}
