//! Vendor-neutral harness adapter interface (`docs/12-supervisor-harness.md` §Harness interface).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use aurora_protocol::runner::{HarnessEventWire, ModelChoice, Recall, SendNowOutcome};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HarnessCapabilities {
    pub resume: bool,
    pub steer: bool,
    pub background_tool: bool,
    pub thinking_deltas: bool,
    pub tool_events: bool,
    pub platform_tools: bool,
    pub dialects: Vec<String>,
    /// (P5-SKILLDIR, D-454) The harness's skills folder, relative to the home root. Required:
    /// every adapter declares one; `SKILLS_DIR` is built from it ([`skills_dir_abs`]).
    pub skills_dir: String,
    pub instruction_files: Vec<String>,
    /// (P5-MODELS-1, D-303, C-877) The roles the adapter maps (`large`/`medium`/`small`); `[]` = none.
    /// Equal to `aurora_protocol::runner::harness_model_roles(id)` (the API's `/v1/harnesses` reads that).
    pub model_roles: Vec<String>,
}

/// (P5-SKILLDIR, D-454, C-1140) The value of `SKILLS_DIR`: `<home>/<the adapter's skills_dir>`,
/// absolute, no trailing slash.
pub fn skills_dir_abs(home: &Path, skills_dir: &str) -> String {
    let p = home.join(skills_dir).to_string_lossy().into_owned();
    match p.trim_end_matches('/') {
        "" => "/".to_owned(),
        t => t.to_owned(),
    }
}

impl HarnessCapabilities {
    /// Capability names as carried by `ready.harness.capabilities`.
    pub fn names(&self) -> Vec<&'static str> {
        [
            (self.resume, "resume"),
            (self.steer, "steer"),
            (self.background_tool, "background_tool"),
            (self.thinking_deltas, "thinking_deltas"),
            (self.tool_events, "tool_events"),
            (self.platform_tools, "platform_tools"),
        ]
        .into_iter()
        .filter_map(|(on, n)| on.then_some(n))
        .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpBridgeSpec {
    /// Command line for `aurora-supervisor mcp-bridge --session <id>`.
    pub command: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub conversation_id: Uuid,
    pub working_dir: PathBuf,
    pub resume_ref: Option<String>,
    pub model: ModelChoice,
    /// Per-session loopback proxy (`http://127.0.0.1:<port>`); carries no credentials.
    pub proxy_url: String,
    pub mcp_bridge: McpBridgeSpec,
    /// App env + proxy/LLM placeholders; never secrets.
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct TurnInput {
    pub turn_id: Uuid,
    pub text: String,
    pub speaker_label: String,
    pub door: String,
    pub recall: Recall,
    pub attachment_paths: Vec<PathBuf>,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SteerInput {
    pub followup_id: Uuid,
    pub text: String,
    pub speaker_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerAck {
    /// Handed to the live run; consumption is confirmed later by [`HarnessEvent::MessageJoined`].
    Accepted,
    /// No live run, or the harness cannot steer.
    Refused,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct HarnessError {
    pub code: String,
    pub message: String,
}

impl HarnessError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Canonical vendor-neutral events (`10 §Transcript events`); `turn_id` is carried by the frame.
#[derive(Debug, Clone, PartialEq)]
pub enum HarnessEvent {
    TextDelta {
        message_id: String,
        delta: String,
        reply_to: Option<String>,
    },
    ThinkingDelta {
        delta: String,
    },
    Activity {
        line: String,
    },
    ToolStarted {
        call_id: String,
        name: String,
        summary: String,
    },
    ToolFinished {
        call_id: String,
        ok: bool,
        summary: Option<String>,
    },
    /// `action` is `retract` or `replace`.
    Correction {
        message_id: String,
        action: String,
        text: Option<String>,
    },
    MessageJoined {
        followup_id: Uuid,
    },
    SentNow,
    SessionRef {
        session_ref: String,
    },
    Usage {
        usage: Value,
    },
    /// Terminal.
    Done {
        usage: Option<Value>,
    },
    /// Terminal.
    Error {
        code: String,
        message: String,
    },
}

impl HarnessEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done { .. } | Self::Error { .. })
    }

    /// Wire form for `turn.event`; terminal events and session refs are not events on the wire
    /// (they become `turn.result` / `session`).
    pub fn to_wire(&self) -> Option<HarnessEventWire> {
        let (kind, data) = match self {
            Self::TextDelta {
                message_id,
                delta,
                reply_to,
            } => (
                "text.delta",
                json!({ "message_id": message_id, "delta": delta, "reply_to": reply_to }),
            ),
            Self::ThinkingDelta { delta } => ("thinking.delta", json!({ "delta": delta })),
            Self::Activity { line } => ("activity", json!({ "line": line })),
            Self::ToolStarted {
                call_id,
                name,
                summary,
            } => (
                "tool.started",
                json!({ "call_id": call_id, "name": name, "summary": summary }),
            ),
            Self::ToolFinished {
                call_id,
                ok,
                summary,
            } => (
                "tool.finished",
                json!({ "call_id": call_id, "ok": ok, "summary": summary }),
            ),
            Self::Correction {
                message_id,
                action,
                text,
            } => (
                "correction",
                json!({ "message_id": message_id, "action": action, "text": text }),
            ),
            Self::MessageJoined { followup_id } => {
                ("message.joined", json!({ "followup_id": followup_id }))
            }
            Self::SentNow => ("message.sent_now", json!({})),
            Self::Usage { usage } => ("usage", json!({ "usage": usage })),
            Self::SessionRef { .. } | Self::Done { .. } | Self::Error { .. } => return None,
        };
        let Value::Object(mut data) = data else {
            return None;
        };
        data.retain(|_, v| !v.is_null());
        Some(HarnessEventWire {
            kind: aurora_protocol::runner::Code::new(kind).ok()?,
            data: Map::from_iter(data),
        })
    }
}

/// Always ends with exactly one terminal event ([`HarnessEvent::Done`] or `Error`), also after abort.
pub type HarnessEventStream = mpsc::Receiver<HarnessEvent>;

#[async_trait]
pub trait HarnessAdapter: Send + Sync {
    /// Display/registry only; never a branch key elsewhere.
    fn id(&self) -> &'static str;
    /// Declared, never probed.
    fn capabilities(&self) -> HarnessCapabilities;
    async fn start_session(
        &self,
        opts: SessionOptions,
    ) -> Result<Box<dyn HarnessSession>, HarnessError>;
    /// (phase 2, RFE-010) Delete the stored files of one session (`12 §Session purge`). No session
    /// may be live for it; idempotent (nothing to delete is success).
    async fn purge_session(
        &self,
        working_dir: &Path,
        session_ref: &str,
    ) -> Result<(), HarnessError>;
}

/// A harness session reference is an opaque token, but it names files on disk: only a short,
/// path-safe token is ever turned into a path (`purge_session` implementations call this).
pub fn is_safe_session_ref(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 128
        && r.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[async_trait]
pub trait HarnessSession: Send {
    fn session_ref(&self) -> Option<String>;
    /// Starts the turn and returns its event stream promptly (steer/abort are called while the
    /// stream is live).
    async fn run_turn(&mut self, input: TurnInput) -> Result<HarnessEventStream, HarnessError>;
    /// MUST refuse between turns (never queue for a future turn).
    async fn steer(&mut self, input: SteerInput) -> Result<SteerAck, HarnessError>;
    /// `Moved` only when confirmed.
    async fn background_running_tool(&mut self) -> Result<SendNowOutcome, HarnessError>;
    async fn abort(&mut self) -> Result<(), HarnessError>;
    /// Let go without ending; a later start with the same ref resumes.
    async fn release(self: Box<Self>) -> Result<(), HarnessError>;
    /// (FIX-253, D-490) The harness child's own background work that outlives the turn: commands
    /// it started in the background and whose end nobody observed. Empty when the adapter cannot
    /// know or the child is gone (its background work dies with it). A plain method: it reads
    /// only what the adapter already knows.
    fn background_work(&self) -> Vec<BackgroundWork> {
        Vec::new()
    }
}

/// (FIX-253, D-490) One piece of the harness child's own background work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundWork {
    pub command: String,
    /// Unix seconds when it was started.
    pub started_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{
        claude_code::ClaudeCodeAdapter, codex::CodexAdapter, goose::GooseAdapter,
        opencode::OpenCodeAdapter, pi::PiAdapter,
    };

    /// (FIX-225, C-1152) Every adapter declares exactly what `harness_turn_controls` says: the API
    /// and the chat cards read the table, never the adapter, so a drift here would show a button
    /// that does nothing (D-470).
    #[test]
    fn every_adapter_declares_what_the_turn_controls_table_says() {
        let adapters: [&dyn HarnessAdapter; 6] = [
            &ClaudeCodeAdapter::default(),
            &CodexAdapter::default(),
            &GooseAdapter::default(),
            &OpenCodeAdapter::default(),
            &PiAdapter::default(),
            &crate::fake::FakeHarness,
        ];
        for a in adapters {
            let want = aurora_protocol::runner::harness_turn_controls(a.id());
            let caps = a.capabilities();
            assert_eq!(caps.steer, want.steer, "{}: steer", a.id());
            assert_eq!(
                caps.background_tool,
                want.background_tool,
                "{}: background_tool",
                a.id()
            );
        }
    }

    /// (P5-SKILLDIR, D-454) `SKILLS_DIR` is `<home>/<skills_dir>`, absolute, no trailing slash,
    /// for each of the five adapters.
    #[test]
    fn skills_dir_is_the_home_joined_with_each_adapters_folder() {
        let home = Path::new("/workspace");
        let cases: [(&dyn HarnessAdapter, &str); 5] = [
            (&ClaudeCodeAdapter::default(), "/workspace/.claude/skills"),
            (&CodexAdapter::default(), "/workspace/.codex/skills/aurora"),
            (&GooseAdapter::default(), "/workspace/.agents/skills"),
            (&OpenCodeAdapter::default(), "/workspace/.agents/skills"),
            (&PiAdapter::default(), "/workspace/.agents/skills"),
        ];
        for (a, want) in cases {
            assert_eq!(
                skills_dir_abs(home, &a.capabilities().skills_dir),
                want,
                "{}",
                a.id()
            );
        }
        assert_eq!(
            skills_dir_abs(Path::new("/workspace/"), ".x/skills/"),
            "/workspace/.x/skills"
        );
    }
}
