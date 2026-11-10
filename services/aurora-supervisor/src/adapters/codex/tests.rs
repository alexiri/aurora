#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Session behaviour against a scripted child (no CLI needed), the rendered files and the purge.

use std::time::Duration;

use aurora_protocol::runner::{ModelChoice, Recall};
use tokio::time::timeout;
use uuid::Uuid;

use super::*;
use crate::harness::McpBridgeSpec;
use config::LaunchSpec;
use launcher::ChildIo;

const TEXT: &str = include_str!("fixtures/text_turn.jsonl");

/// Scripted launcher: replays `reply` lines (or stays silent, or ends at once) per launch.
struct Mock {
    reply: Vec<String>,
    /// Never answer and never end (for abort).
    silent: bool,
    specs: Arc<StdMutex<Vec<LaunchSpec>>>,
    killed: Arc<AtomicBool>,
}

impl Mock {
    fn new(reply: &str) -> Self {
        Self {
            reply: reply.lines().map(str::to_owned).collect(),
            silent: false,
            specs: Arc::default(),
            killed: Arc::default(),
        }
    }
}

#[async_trait]
impl Launcher for Mock {
    async fn launch(&self, spec: LaunchSpec) -> Result<ChildIo, HarnessError> {
        self.specs.lock().unwrap().push(spec);
        let (out_tx, out_rx) = mpsc::channel::<String>(64);
        let keep_open = out_tx.clone();
        let (reply, silent) = (self.reply.clone(), self.silent);
        tokio::spawn(async move {
            if silent {
                // Hold the pipe open until the kill switch drops it.
                tokio::time::sleep(Duration::from_secs(60)).await;
                drop(keep_open);
                return;
            }
            for l in reply {
                let _ = out_tx.send(l).await;
            }
            // End of output: the sender is dropped here and `lines` closes.
        });
        let killed = self.killed.clone();
        Ok(ChildIo {
            lines: out_rx,
            kill: Box::new(move || killed.store(true, Ordering::SeqCst)),
        })
    }
}

fn opts(dir: &Path, resume: Option<&str>) -> SessionOptions {
    SessionOptions {
        conversation_id: Uuid::nil(),
        working_dir: dir.to_path_buf(),
        resume_ref: resume.map(str::to_owned),
        model: ModelChoice {
            provider: "litellm".into(),
            endpoint_id: "e".into(),
            base_url: "https://gw/v1".into(),
            dialect: "openai_responses".into(),
            model: "gpt-x".into(),
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

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("p4-har-{tag}-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

async fn drain(mut rx: HarnessEventStream) -> Vec<HarnessEvent> {
    let mut v = Vec::new();
    while let Some(e) = timeout(Duration::from_secs(5), rx.recv()).await.unwrap() {
        v.push(e);
    }
    v
}

fn terminals(ev: &[HarnessEvent]) -> usize {
    ev.iter().filter(|e| e.is_terminal()).count()
}

#[tokio::test]
async fn capabilities_are_the_codex_contract() {
    let c = CodexAdapter::default().capabilities();
    assert!(c.resume && c.tool_events && c.platform_tools);
    assert!(!c.steer && !c.background_tool);
    assert_eq!(c.dialects, ["openai_responses"]);
    assert_eq!(c.instruction_files, ["AGENTS.md"]);
    assert_eq!(c.skills_dir.as_deref(), Some(".codex/skills/aurora"));
    assert_eq!(CodexAdapter::default().id(), "codex");
}

#[tokio::test]
async fn text_turn_ends_with_exactly_one_done_and_records_the_thread_id() {
    let dir = tmp("text");
    let mock = Mock::new(TEXT);
    let specs = mock.specs.clone();
    let a = CodexAdapter::with_launcher(Arc::new(mock));
    let mut s = a.start_session(opts(&dir, None)).await.unwrap();
    let ev = drain(s.run_turn(turn("hi")).await.unwrap()).await;
    assert_eq!(terminals(&ev), 1);
    assert!(matches!(ev.last(), Some(HarnessEvent::Done { .. })));
    assert!(
        ev.iter()
            .any(|e| matches!(e, HarnessEvent::TextDelta { delta, .. } if delta == "pong"))
    );
    assert_eq!(
        s.session_ref().as_deref(),
        Some("01a1c3dc-f235-78e1-920f-5c889ce01480")
    );
    // The next turn resumes the thread; the prompt is on stdin, never in argv.
    drain(s.run_turn(turn("again")).await.unwrap()).await;
    let specs = specs.lock().unwrap();
    assert_eq!(specs[0].args[..2], ["exec", "--json"]);
    assert_eq!(
        specs[1].args[..3],
        ["exec", "resume", "01a1c3dc-f235-78e1-920f-5c889ce01480"]
    );
    assert_eq!(specs[1].stdin, "again");
    assert!(!specs[1].args.iter().any(|a| a.contains("again")));
}

#[tokio::test]
async fn crash_without_a_result_is_one_harness_failed() {
    let dir = tmp("crash");
    let a = CodexAdapter::with_launcher(Arc::new(Mock::new(
        r#"{"type":"thread.started","thread_id":"t1"}"#,
    )));
    let mut s = a.start_session(opts(&dir, None)).await.unwrap();
    let ev = drain(s.run_turn(turn("hi")).await.unwrap()).await;
    assert_eq!(terminals(&ev), 1);
    assert!(
        matches!(ev.last(), Some(HarnessEvent::Error { code, .. }) if code == "harness_failed")
    );
}

#[tokio::test]
async fn abort_yields_exactly_one_terminal_event_and_kills_the_child() {
    let dir = tmp("abort");
    let mut mock = Mock::new("");
    mock.silent = true;
    let killed = mock.killed.clone();
    let a = CodexAdapter::with_launcher(Arc::new(mock));
    let mut s = a.start_session(opts(&dir, None)).await.unwrap();
    let rx = s.run_turn(turn("long")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    s.abort().await.unwrap();
    let ev = drain(rx).await;
    assert_eq!(terminals(&ev), 1);
    assert!(matches!(ev.last(), Some(HarnessEvent::Error { code, .. }) if code == "aborted"));
    assert!(killed.load(Ordering::SeqCst));
    s.release().await.unwrap();
}

#[tokio::test]
async fn steer_is_always_refused() {
    let dir = tmp("steer");
    let a = CodexAdapter::with_launcher(Arc::new(Mock::new(TEXT)));
    let mut s = a.start_session(opts(&dir, None)).await.unwrap();
    for _ in 0..2 {
        let ack = s
            .steer(SteerInput {
                followup_id: Uuid::nil(),
                text: "more".into(),
                speaker_label: String::new(),
            })
            .await
            .unwrap();
        assert_eq!(ack, SteerAck::Refused);
        drain(s.run_turn(turn("go")).await.unwrap()).await;
    }
}

#[tokio::test]
async fn unsafe_resume_references_are_refused() {
    let dir = tmp("ref");
    let a = CodexAdapter::with_launcher(Arc::new(Mock::new(TEXT)));
    for bad in [
        "--dangerously-bypass-approvals-and-sandbox",
        "../x",
        "a b",
        "",
    ] {
        assert!(
            a.start_session(opts(&dir, Some(bad))).await.is_err(),
            "{bad}"
        );
        assert!(a.purge_session(&dir, bad).await.is_err(), "{bad}");
    }
}

/// SEC-66: `config.toml` is rendered at every start, holds no secret, and any `auth.json` is
/// removed; the process environment holds only the placeholder.
#[tokio::test]
async fn start_renders_config_without_secrets_and_removes_auth_json() {
    let dir = tmp("render");
    let home = dir.join(".codex");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("auth.json"), r#"{"OPENAI_API_KEY":"sk-PLANTED"}"#).unwrap();
    std::fs::write(home.join("config.toml"), "model = \"stale\"\n").unwrap();
    let mock = Mock::new(TEXT);
    let specs = mock.specs.clone();
    let a = CodexAdapter::with_launcher(Arc::new(mock));
    let mut o = opts(&dir, None);
    o.env = vec![("OPENAI_API_KEY".into(), "sk-REAL-SECRET".into())];
    let mut s = a.start_session(o).await.unwrap();
    assert!(!home.join("auth.json").exists());
    let cfg = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(!cfg.contains("stale") && !cfg.contains("SECRET") && !cfg.contains("PLANTED"));
    drain(s.run_turn(turn("hi")).await.unwrap()).await;
    let spec = specs.lock().unwrap()[0].clone();
    assert!(
        spec.env
            .iter()
            .any(|(k, v)| k == "OPENAI_API_KEY" && v == config::PLACEHOLDER)
    );
    assert!(spec.env.iter().all(|(_, v)| !v.contains("SECRET")));
    assert!(
        spec.env
            .iter()
            .any(|(k, v)| k == "CODEX_HOME" && *v == home.display().to_string())
    );
}

#[test]
fn rendered_config_is_valid_toml_with_the_documented_settings() {
    let o = opts(Path::new("/workspace"), None);
    let text = config::render_config(&o).unwrap();
    let t: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(t["model"].as_str(), Some("gpt-x"));
    assert_eq!(t["model_provider"].as_str(), Some("aurora"));
    assert_eq!(t["model_reasoning_effort"].as_str(), Some("medium"));
    assert_eq!(t["approval_policy"].as_str(), Some("never"));
    assert_eq!(t["sandbox_mode"].as_str(), Some("danger-full-access"));
    assert_eq!(t["check_for_update_on_startup"].as_bool(), Some(false));
    assert_eq!(t["analytics"]["enabled"].as_bool(), Some(false));
    let p = &t["model_providers"]["aurora"];
    assert_eq!(p["base_url"].as_str(), Some("https://gw/v1"));
    assert_eq!(p["wire_api"].as_str(), Some("responses"));
    assert_eq!(p["env_key"].as_str(), Some("OPENAI_API_KEY"));
    let m = &t["mcp_servers"]["aurora"];
    assert_eq!(m["command"].as_str(), Some("sup"));
    assert_eq!(
        m["args"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a.as_str())
            .collect::<Vec<_>>(),
        ["mcp-bridge"]
    );
    // No credential key of any kind anywhere in the file.
    // (`env_key` only *names* the environment variable that holds the placeholder.)
    let scrubbed = text
        .replace("env_key = \"OPENAI_API_KEY\"", "")
        .to_ascii_lowercase();
    for needle in ["api_key", "token", "secret", "password", "bearer", "sk-"] {
        assert!(!scrubbed.contains(needle), "{needle}");
    }
}

#[test]
fn provider_block_follows_the_endpoint_for_litellm_azure_and_compatible_endpoints() {
    // LiteLLM supplies the OpenAI form of its base URL in the placeholder-only client env.
    let mut o = opts(Path::new("/workspace"), None);
    o.model.base_url = "https://llm.example".into();
    o.model.client_env = vec![("OPENAI_BASE_URL".into(), "https://llm.example/v1/".into())];
    let t: toml::Value = toml::from_str(&config::render_config(&o).unwrap()).unwrap();
    assert_eq!(
        t["model_providers"]["aurora"]["base_url"].as_str(),
        Some("https://llm.example/v1")
    );
    // Azure OpenAI / OpenAI-compatible: the endpoint's own base URL.
    o.model.provider = "azure_openai".into();
    o.model.base_url = "https://res.openai.azure.com/openai/v1".into();
    o.model.client_env = vec![];
    let t: toml::Value = toml::from_str(&config::render_config(&o).unwrap()).unwrap();
    assert_eq!(
        t["model_providers"]["aurora"]["base_url"].as_str(),
        Some("https://res.openai.azure.com/openai/v1")
    );
}

#[tokio::test]
async fn purge_deletes_only_the_named_session_and_is_idempotent() {
    let dir = tmp("purge");
    let home = dir.join(".codex");
    let day = home.join("sessions/2026/11/10");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::create_dir_all(home.join("shell_snapshots")).unwrap();
    let mine = day.join("rollout-2026-11-10T06-10-38-sess-A.jsonl");
    let other = day.join("rollout-2026-11-10T06-10-39-sess-B.jsonl");
    let lookalike = day.join("rollout-2026-11-10T06-10-40-xsess-A.jsonl");
    let snap = home.join("shell_snapshots/sess-A.123.sh");
    for p in [&mine, &other, &lookalike, &snap] {
        std::fs::write(p, "x").unwrap();
    }
    let a = CodexAdapter::with_launcher(Arc::new(Mock::new(TEXT)));
    a.purge_session(&dir, "sess-A").await.unwrap();
    assert!(!mine.exists() && !snap.exists());
    assert!(other.exists() && lookalike.exists());
    a.purge_session(&dir, "sess-A").await.unwrap();
    a.purge_session(&dir, "never-existed").await.unwrap();
}
