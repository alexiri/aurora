//! Vendor-neutral harness adapter interface (`docs/12-supervisor-harness.md` §Harness interface).

use std::path::PathBuf;

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
    pub skills_dir: Option<String>,
    pub instruction_files: Vec<String>,
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
}
