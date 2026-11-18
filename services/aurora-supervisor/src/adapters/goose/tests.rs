#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Session behaviour against a scripted child (no CLI needed), the rendered files and the purge.

use std::time::Duration;

use aurora_protocol::runner::Recall;
use tokio::time::timeout;
use uuid::Uuid;

use super::config::tests::opts;
use super::*;
use config::LaunchSpec;
use launcher::ChildIo;

const TEXT1: &str = r#"{"type":"message","message":{"id":"msg_1","role":"assistant","content":[{"type":"text","text":"pong"}]}}"#;
const COMPLETE: &str = r#"{"type":"complete","total_tokens":20,"input_tokens":16,"output_tokens":4,"cache_read_input_tokens":3,"cache_write_input_tokens":2}"#;

/// Scripted launcher: replays `reply` lines (or stays silent) per launch, ends the stream after them.
struct Mock {
    /// One reply per launch (the last repeats).
    replies: Vec<Vec<String>>,
    stderr: Vec<String>,
    silent: bool,
    specs: Arc<StdMutex<Vec<LaunchSpec>>>,
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
            silent: false,
            specs: Arc::default(),
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
        let keep_open = out_tx.clone();
        let reply = self.replies[(n - 1).min(self.replies.len() - 1)].clone();
        let silent = self.silent;
        tokio::spawn(async move {
            if silent {
                tokio::time::sleep(Duration::from_secs(60)).await;
                drop(keep_open);
                return;
            }
            for l in reply {
                let _ = out_tx.send(l).await;
            }
        });
        let killed = self.killed.clone();
        Ok(ChildIo {
            lines: out_rx,
            stderr: Arc::new(StdMutex::new(self.stderr.clone())),
            kill: Box::new(move || killed.store(true, Ordering::SeqCst)),
        })
    }
}

fn tmp_opts(dialect: &str) -> SessionOptions {
    let dir = std::env::temp_dir().join(format!("goose-adapter-{}", Uuid::now_v7()));
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

fn adapter(m: Mock) -> (GooseAdapter, Arc<StdMutex<Vec<LaunchSpec>>>) {
    let specs = m.specs.clone();
    (GooseAdapter::with_launcher(Arc::new(m)), specs)
}

#[tokio::test]
async fn a_turn_streams_text_ends_with_one_done_and_the_next_turn_resumes_the_session() {
    let (a, specs) = adapter(Mock::new(&[&["   banner", TEXT1, COMPLETE]]));
    let o = tmp_opts("anthropic_messages");
    let conv = o.conversation_id.to_string();
    let mut s = a.start_session(o).await.unwrap();
    assert_eq!(s.session_ref(), None);
    let ev = drain(s.run_turn(turn("hello")).await.unwrap()).await;
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
        matches!(ev.last(), Some(HarnessEvent::Done { usage: Some(u) }) if u["input_tokens"] == 11)
    );
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1);
    assert_eq!(s.session_ref().as_deref(), Some(conv.as_str()));
    drain(s.run_turn(turn("again")).await.unwrap()).await;
    let specs = specs.lock().unwrap();
    assert_eq!(specs[0].stdin, "hello", "the prompt goes on stdin");
    assert!(
        !specs[0].args.contains(&"--resume".to_owned()),
        "a new session is created, not resumed"
    );
    assert!(
        specs[1].args.contains(&"--resume".to_owned()),
        "the session now exists: {:?}",
        specs[1].args
    );
    assert!(
        specs[1]
            .args
            .windows(2)
            .any(|w| w == ["--name", conv.as_str()])
    );
    assert!(
        !specs[0].args.iter().any(|x| x.contains("hello")),
        "the prompt is never an argument"
    );
}

#[tokio::test]
async fn a_resume_reference_resumes_from_the_first_turn_and_names_the_session() {
    let (a, specs) = adapter(Mock::new(&[&[TEXT1, COMPLETE]]));
    let mut o = tmp_opts("anthropic_messages");
    o.resume_ref = Some("ref-1".into());
    let mut s = a.start_session(o).await.unwrap();
    assert_eq!(s.session_ref().as_deref(), Some("ref-1"));
    drain(s.run_turn(turn("x")).await.unwrap()).await;
    let args = specs.lock().unwrap()[0].args.clone();
    assert_eq!(&args[5..8], ["--name", "ref-1", "--resume"]);
}

#[tokio::test]
async fn a_start_that_produced_no_event_is_a_failed_turn_with_stderr_and_does_not_resume_next() {
    let mut m = Mock::new(&[&[]]);
    m.stderr = vec!["Error: No session found with name 'x'".into()];
    let (a, specs) = adapter(m);
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let ev = drain(s.run_turn(turn("x")).await.unwrap()).await;
    assert!(
        matches!(ev.last(), Some(HarnessEvent::Error { code, message }) if code == "harness_failed" && message.contains("No session found")),
        "{ev:?}"
    );
    assert_eq!(s.session_ref(), None);
    drain(s.run_turn(turn("y")).await.unwrap()).await;
    assert!(
        !specs.lock().unwrap()[1]
            .args
            .contains(&"--resume".to_owned()),
        "no session was created"
    );
}

#[tokio::test]
async fn an_error_event_ends_the_turn_with_the_classified_code() {
    let err = r#"{"type":"error","error":"Ran into this error: Authentication error: Authentication failed for http://x/v1/messages. Status: 401 Unauthorized. Response: bad."}"#;
    let (a, _) = adapter(Mock::new(&[&[err]]));
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let ev = drain(s.run_turn(turn("x")).await.unwrap()).await;
    assert!(
        matches!(ev.last(), Some(HarnessEvent::Error { code, .. }) if code == "provider_auth"),
        "{ev:?}"
    );
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1);
}

#[tokio::test]
async fn the_root_is_rendered_before_every_turn_and_planted_credentials_and_links_are_gone() {
    let (a, specs) = adapter(Mock::new(&[&[TEXT1, COMPLETE]]));
    let o = tmp_opts("openai_chat");
    let root = render::session_dir(&o.working_dir, ID, &o.conversation_id.to_string());
    // Before the session starts: a credential store, a different config and a link at `data`.
    let elsewhere = o.working_dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/secrets.yaml"),
        "ANTHROPIC_API_KEY: sk-planted\n",
    )
    .unwrap();
    std::fs::write(root.join("config/config.yaml"), "extensions: {}\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("data")).unwrap();
    let mut s = a.start_session(o).await.unwrap();
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
    assert!(!root.join("config/secrets.yaml").exists());
    assert!(read("config/config.yaml").contains("aurora-supervisor"));
    assert!(
        root.join("data").is_dir() && !root.join("data").is_symlink(),
        "the planted link was replaced"
    );
    assert!(
        std::fs::read_dir(&elsewhere).unwrap().next().is_none(),
        "nothing was written through the link"
    );
    assert!(read("data/model_catalog/models_dev_api.json").contains("litellm"));
    assert!(root.join("data/model_catalog/models_dev_api.etag").exists());
    // Between turns the agent plants them again.
    std::fs::write(root.join("config/secrets.yaml"), "k: v\n").unwrap();
    std::fs::write(root.join("data/model_catalog/models_dev_api.json"), "{}").unwrap();
    drain(s.run_turn(turn("x")).await.unwrap()).await;
    assert!(
        !root.join("config/secrets.yaml").exists(),
        "removed again before the turn"
    );
    assert!(
        read("data/model_catalog/models_dev_api.json").contains("claude-sonnet-4-5"),
        "catalogue re-rendered"
    );
    let env = specs.lock().unwrap()[0].env.clone();
    assert!(env.contains(&("GOOSE_PATH_ROOT".to_owned(), root.display().to_string())));
    for f in [
        "config/config.yaml",
        "data/model_catalog/models_dev_api.json",
    ] {
        let t = read(f);
        assert!(
            !t.contains("sk-") && !t.contains("aurora-placeholder"),
            "{f}: {t}"
        );
    }
}

#[tokio::test]
async fn steer_is_always_refused_and_background_is_unsupported() {
    let (a, _) = adapter(Mock::new(&[&[TEXT1, COMPLETE]]));
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let st = SteerInput {
        followup_id: Uuid::now_v7(),
        text: "x".into(),
        speaker_label: String::new(),
    };
    assert_eq!(s.steer(st.clone()).await.unwrap(), SteerAck::Refused);
    let rx = s.run_turn(turn("go")).await.unwrap();
    assert_eq!(
        s.steer(st).await.unwrap(),
        SteerAck::Refused,
        "also during a turn"
    );
    drain(rx).await;
    assert_eq!(
        s.background_running_tool().await.unwrap(),
        SendNowOutcome::Unsupported
    );
}

#[tokio::test]
async fn abort_kills_the_child_and_ends_the_turn_exactly_once() {
    let mut m = Mock::new(&[&[]]);
    m.silent = true;
    let killed = m.killed.clone();
    let (a, _) = adapter(m);
    let mut s = a
        .start_session(tmp_opts("anthropic_messages"))
        .await
        .unwrap();
    let rx = s.run_turn(turn("long")).await.unwrap();
    s.abort().await.unwrap();
    let ev = drain(rx).await;
    assert_eq!(ev.iter().filter(|e| e.is_terminal()).count(), 1);
    assert!(matches!(ev.last(), Some(HarnessEvent::Error { code, .. }) if code == "aborted"));
    assert!(killed.load(Ordering::SeqCst));
    // The session is still usable for the next turn.
    let e = s.run_turn(turn("again")).await;
    assert!(e.is_ok());
}

#[tokio::test]
async fn unspeakable_dialects_and_unsafe_refs_are_refused_before_anything_starts() {
    let (a, specs) = adapter(Mock::new(&[&[]]));
    for d in [
        "openai_responses",
        "anthropic_bedrock",
        "anthropic_vertex",
        "gemini",
    ] {
        assert_eq!(
            a.start_session(tmp_opts(d)).await.err().unwrap().code,
            "harness_failed",
            "{d}"
        );
    }
    for r in ["../x", "-x", "a/b", ""] {
        let mut o = tmp_opts("anthropic_messages");
        o.resume_ref = Some(r.into());
        assert_eq!(
            a.start_session(o).await.err().unwrap().code,
            "invalid_session_ref",
            "{r}"
        );
    }
    assert!(specs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn purge_removes_exactly_one_sessions_root_and_is_idempotent() {
    let (a, _) = adapter(Mock::new(&[&[TEXT1, COMPLETE]]));
    let home = std::env::temp_dir().join(format!("goose-purge-{}", Uuid::now_v7()));
    let mk = |home: &Path| {
        let mut o = tmp_opts("anthropic_messages");
        o.working_dir = home.to_path_buf();
        o
    };
    let (keep, gone) = (mk(&home), mk(&home));
    let (keep_ref, gone_ref) = (
        keep.conversation_id.to_string(),
        gone.conversation_id.to_string(),
    );
    a.start_session(keep)
        .await
        .unwrap()
        .release()
        .await
        .unwrap();
    a.start_session(gone)
        .await
        .unwrap()
        .release()
        .await
        .unwrap();
    let root = |r: &str| render::session_dir(&home, ID, r);
    std::fs::create_dir_all(root(&gone_ref).join("data/sessions")).unwrap();
    std::fs::write(root(&gone_ref).join("data/sessions/sessions.db"), "x").unwrap();
    a.purge_session(&home, &gone_ref).await.unwrap();
    assert!(!root(&gone_ref).exists());
    assert!(
        root(&keep_ref).join("config/config.yaml").exists(),
        "another session is untouched"
    );
    a.purge_session(&home, &gone_ref).await.unwrap();
    for bad in ["../x", "", "a/b", "-x"] {
        assert_eq!(
            a.purge_session(&home, bad).await.unwrap_err().code,
            "invalid_session_ref",
            "{bad}"
        );
    }
}

#[test]
fn capabilities_are_declared_and_the_roles_equal_the_protocol_table() {
    let (a, _) = adapter(Mock::new(&[&[]]));
    let c = a.capabilities();
    assert!(c.resume && c.tool_events && c.platform_tools);
    assert!(!c.steer && !c.thinking_deltas && !c.background_tool);
    assert_eq!(c.dialects, ["anthropic_messages", "openai_chat"]);
    assert_eq!(c.skills_dir.as_deref(), Some(".agents/skills"));
    assert_eq!(c.instruction_files, ["AGENTS.md"]);
    assert_eq!(c.model_roles, ["large", "medium", "small"]);
}
