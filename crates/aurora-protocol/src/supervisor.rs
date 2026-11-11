//! W6 supervisor channel (`supervisor/1`). Normative: `docs/16-wire-protocols.md` §W6.
//!
//! WebSocket `ws://<runner>:4105/v1/supervisor`. Bodies are shared with the
//! W5 runner plane (`crate::runner`); the runner adds `sandbox_id`/`boot_id`
//! from the authenticated channel, never from the frame body.

use std::time::Duration;

use aurora_core::limits;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::common::{
    Empty, SecretString, Validate, ValidationError, check_chars, check_max, json_len,
};
use crate::runner::{
    Code, GateResult, HomeSync, HomeSynced, MemoryWrite, PortsState, PreviewStartFrame,
    ProcessStates, SandboxStatus, SendNowResult, SessionPurge, SessionPurged, SessionRelease,
    SessionReport, SteerResult, SupervisorReady, TurnDeliver, TurnEvent, TurnMessage, TurnNotice,
    TurnRef, TurnResult, WatchFired,
};
use crate::tools::{ToolCallBody, ToolError, ToolResult};

/// Max serialized size of one frame.
pub const MAX_FRAME: usize = limits::WS_FRAME_MAX;
/// Max decoded bytes of one `file.part` (144 KiB ⇒ 192 KiB base64, fits `WS_FRAME_MAX`; spec fix S-06).
pub const MAX_FILE_PART_DECODED: usize = 144 * 1024;
/// Max unacknowledged bytes a sender retains (4 MiB).
pub const MAX_UNACKED_BYTES: usize = 4 * 1024 * 1024;
pub const PING_INTERVAL: Duration = Duration::from_secs(20);
pub const SILENCE_TIMEOUT: Duration = Duration::from_secs(60);
pub const ACK_INTERVAL: Duration = Duration::from_secs(1);
/// Close code: unauthorized.
pub const CLOSE_UNAUTHORIZED: u16 = 4401;
/// Close code: bootstrap token spent.
pub const CLOSE_SPENT: u16 = 4409;
/// Path of the WebSocket endpoint.
pub const PATH: &str = "/v1/supervisor";

// -------------------------------------------------------------- handshake

/// HMAC session proof (`issued_at` is unix seconds; informational clocks).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionProof {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub issued_at: i64,
    pub mac: SecretString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HelloBody {
    pub protocol: u32,
    /// `aup_…`; exactly one of `bootstrap_token` and `session` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_token: Option<SecretString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionProof>,
}

/// First message from the supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hello {
    pub hello: HelloBody,
}

impl Validate for Hello {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.hello.bootstrap_token.is_some() == self.hello.session.is_some() {
            return Err(ValidationError::Invalid {
                field: "hello",
                reason: "exactly one of bootstrap_token and session is required",
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WelcomeBody {
    pub session: SessionProof,
    /// Last acked seq of the supervisor's direction.
    pub resume_from: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Welcome {
    pub welcome: WelcomeBody,
}

// ---------------------------------------------------------------- frames

/// `{ seq, ack, kind, body }`. `M` is the direction's message union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Frame<M> {
    pub seq: u64,
    pub ack: u64,
    #[serde(flatten)]
    pub msg: M,
}

impl<M: Serialize + Validate> Validate for Frame<M> {
    fn validate(&self) -> Result<(), ValidationError> {
        self.msg.validate()?;
        check_max("frame", json_len(self), MAX_FRAME)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShutdownBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_secs: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryWriteResult {
    pub key: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileResult {
    pub upload_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilePart {
    pub upload_id: String,
    pub index: u32,
    pub data_b64: String,
}

/// One chunk of an inbound attachment (decision S-11). Frames of one `(turn_id, attachment_id)` are
/// sent in `index` order, `last` on the final one; `size` and `sha256` describe the whole file and
/// repeat on every frame. `filename` is untrusted (the supervisor sanitizes it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileIn {
    pub turn_id: Uuid,
    pub attachment_id: Uuid,
    pub filename: String,
    pub index: u32,
    pub last: bool,
    pub size: u64,
    pub sha256: String,
    pub data_b64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileCommit {
    pub upload_id: String,
    pub conversation_id: Uuid,
    pub turn_id: Uuid,
    pub filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    pub sha256: String,
    pub size: u64,
}

tagged_open! {
    /// Runner -> supervisor frame kinds.
    pub enum RunnerToSupervisor, mode = Adjacent, tag = "kind" {
        TurnDeliver(Box<TurnDeliver>) = "turn.deliver",
        TurnMessage(TurnMessage) = "turn.message",
        TurnSendNow(TurnRef) = "turn.send_now",
        TurnAbort(TurnRef) = "turn.abort",
        TurnNotice(TurnNotice) = "turn.notice",
        SessionRelease(SessionRelease) = "session.release",
        SessionPurge(SessionPurge) = "session.purge",
        HomeSync(HomeSync) = "home.sync",
        ToolResult(ToolResult) = "tool.result",
        MemoryWriteResult(MemoryWriteResult) = "memory.write.result",
        FileResult(FileResult) = "file.result",
        FileIn(FileIn) = "file.in",
        // (phase 4, P4-PRV-1, D-145) forwarded `preview.start`: only the port; the supervisor
        // resolves the command from its own share list (`12 §Previews`).
        PreviewStart(PreviewStartFrame) = "preview.start",
        Shutdown(ShutdownBody) = "shutdown",
        Ack(Empty) = "ack",
    }
}

tagged_open! {
    /// Supervisor -> runner frame kinds.
    pub enum SupervisorToRunner, mode = Adjacent, tag = "kind" {
        Ready(SupervisorReady) = "ready",
        HomeSynced(HomeSynced) = "home.synced",
        TurnStarted(TurnRef) = "turn.started",
        TurnEvent(TurnEvent) = "turn.event",
        TurnProgress(TurnRef) = "turn.progress",
        TurnResult(TurnResult) = "turn.result",
        SteerResult(SteerResult) = "steer.result",
        SendNowResult(SendNowResult) = "send_now.result",
        Session(SessionReport) = "session",
        SessionPurged(SessionPurged) = "session.purged",
        // (phase 4, P4-HAR, D-153) boot refusal before `ready`: `{ state: "failed", reason_code:
        // "harness_unsupported", message }`; the runner forwards it as the sandbox's status.
        SandboxStatus(SandboxStatus) = "sandbox.status",
        ProcessState(ProcessStates) = "process.state",
        WatchFired(WatchFired) = "watch.fired",
        GateResult(GateResult) = "gate.result",
        PortsState(PortsState) = "ports.state",
        ToolCall(ToolCallBody) = "tool.call",
        MemoryWrite(MemoryWrite) = "memory.write",
        FilePart(FilePart) = "file.part",
        FileCommit(FileCommit) = "file.commit",
        Ack(Empty) = "ack",
    }
}

/// Decoded length of standard base64 without decoding it.
fn b64_decoded_len(s: &str) -> usize {
    let pad = s.bytes().rev().take_while(|b| *b == b'=').count().min(2);
    (s.len() / 4) * 3 - pad
}

impl Validate for FilePart {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("upload_id", &self.upload_id, 128)?;
        check_max(
            "data_b64 (decoded)",
            b64_decoded_len(&self.data_b64),
            MAX_FILE_PART_DECODED,
        )
    }
}

impl Validate for FileIn {
    fn validate(&self) -> Result<(), ValidationError> {
        let bad = |reason| ValidationError::Invalid {
            field: "file.in",
            reason,
        };
        if self.filename.is_empty() {
            return Err(bad("filename must not be empty"));
        }
        check_chars("filename", &self.filename, 255)?;
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ValidationError::Invalid {
                field: "sha256",
                reason: "must be 64 hex characters",
            });
        }
        check_max(
            "size",
            self.size as usize,
            limits::ATTACH_IN_FILE_MAX as usize,
        )?;
        if !self.data_b64.len().is_multiple_of(4) {
            return Err(bad("data_b64 is not padded standard base64"));
        }
        let decoded = b64_decoded_len(&self.data_b64);
        check_max("data_b64 (decoded)", decoded, MAX_FILE_PART_DECODED)?;
        if decoded as u64 > self.size {
            return Err(bad("chunk is larger than the declared size"));
        }
        // An empty file is exactly one empty last chunk; any other chunk carries data, so the
        // index cannot exceed the size.
        if self.size == 0 {
            if self.index != 0 || !self.last || decoded != 0 {
                return Err(bad("an empty file is a single empty last chunk"));
            }
        } else if decoded == 0 {
            return Err(bad("empty chunk of a non-empty file"));
        }
        if u64::from(self.index) >= self.size.max(1) {
            return Err(bad("index exceeds the declared size"));
        }
        // A single chunk that is last at index 0 must be the whole file.
        if self.last && self.index == 0 && decoded as u64 != self.size {
            return Err(bad("single-chunk file does not match the declared size"));
        }
        if !self.last && decoded as u64 >= self.size {
            return Err(bad("non-last chunk already covers the declared size"));
        }
        Ok(())
    }
}

impl Validate for FileCommit {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("upload_id", &self.upload_id, 128)?;
        check_chars("filename", &self.filename, 255)?;
        if let Some(c) = &self.caption {
            check_chars("caption", c, limits::ATTACH_CAPTION_MAX)?;
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ValidationError::Invalid {
                field: "sha256",
                reason: "must be 64 hex characters",
            });
        }
        check_max(
            "size",
            self.size as usize,
            limits::ATTACH_OUT_FILE_MAX as usize,
        )
    }
}

impl Validate for RunnerToSupervisor {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::TurnDeliver(v) => v.validate(),
            Self::FileIn(v) => v.validate(),
            Self::TurnMessage(v) => check_chars("text", &v.text, limits::TURN_TEXT_MAX),
            Self::TurnNotice(v) => check_chars("text", &v.text, limits::TURN_TEXT_MAX),
            Self::HomeSync(v) => v.validate(),
            Self::ToolResult(v) => v.validate(),
            _ => Ok(()),
        }
    }
}

impl Validate for SupervisorToRunner {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::ProcessState(v) => v.validate(),
            Self::SandboxStatus(v) => v.message.as_deref().map_or(Ok(()), |m| {
                check_chars("message", m, crate::runner::MAX_MESSAGE_CHARS)
            }),
            Self::ToolCall(v) => v.validate(),
            Self::MemoryWrite(v) => v.validate(),
            Self::FilePart(v) => v.validate(),
            Self::FileCommit(v) => v.validate(),
            Self::WatchFired(v) => {
                check_max("detail", v.detail.len(), crate::runner::MAX_WATCH_DETAIL)
            }
            _ => Ok(()),
        }
    }
}

/// Re-export so users of this module can name error codes.
pub type ErrorCode = Code;
