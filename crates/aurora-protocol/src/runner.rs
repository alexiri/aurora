//! W5 runner plane messages (`runner/1`). Normative: `docs/16-wire-protocols.md` §W5.
//!
//! Work-item and report bodies are shared with the W6 supervisor channel
//! (`crate::supervisor`), which carries the same bodies without the
//! `sandbox_id`/`boot_id` envelope.

use std::collections::BTreeMap;

use aurora_core::limits;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::common::{
    SecretString, Validate, ValidationError, check_bytes, check_chars, check_max, json_len,
};
use crate::open_vocab;
use crate::vocab::{BoundedString, OPEN_VOCAB_MAX};

/// Max reports per `POST /reports`.
pub const MAX_REPORTS_PER_BATCH: usize = 200;
/// Max serialized size of a `POST /reports` body (1 MiB).
pub const MAX_REPORTS_BODY: usize = 1024 * 1024;
/// Max `wait_secs` of `POST /work`.
pub const MAX_WAIT_SECS: u32 = limits::RUNNER_POLL_WAIT.as_secs() as u32;
/// Provisional bound for free-text error/status messages (not in the limits registry).
pub const MAX_MESSAGE_CHARS: usize = 2_000;
/// Gate stdout captured and reported (`12 §Process manager`: ≤ 16 KiB).
pub const MAX_GATE_OUTPUT: usize = 16 * 1024;
/// Provisional bound for `watch.fired.detail` (not in the limits registry).
pub const MAX_WATCH_DETAIL: usize = limits::PROCESS_TAIL;
/// Max bytes of a work-item `home.sync` part.
pub const MAX_HOME_SYNC_PART: usize = limits::HOME_SYNC_PART_MAX;
/// Max turn text (chars).
pub const MAX_TURN_TEXT: usize = limits::TURN_TEXT_MAX;
/// Max attachments per delivered turn.
pub const MAX_TURN_ATTACHMENTS: usize = limits::ATTACH_IN_COUNT as usize;

/// Attachment upload headers (`POST /attachments`).
pub const HDR_SANDBOX: &str = "x-aurora-sandbox";
pub const HDR_BOOT: &str = "x-aurora-boot";
pub const HDR_CONVERSATION: &str = "x-aurora-conversation";
pub const HDR_TURN: &str = "x-aurora-turn";
pub const HDR_FILENAME: &str = "x-aurora-filename";
pub const HDR_CAPTION: &str = "x-aurora-caption";
pub const HDR_REPLICA: &str = "x-aurora-replica";

/// UUIDv5 namespace for reports the runner derives from supervisor frames (spec fix S-07).
pub const NAMESPACE_AURORA_REPORT: Uuid =
    Uuid::from_u128(0x6f1c_2a58_9d3e_5b47_8c10_a7e4_52d9_b3f6);

/// Deterministic `report_id = UUIDv5(NAMESPACE_AURORA_REPORT, "<boot_id>:<seq>")`.
pub fn frame_report_id(boot_id: Uuid, seq: u64) -> Uuid {
    Uuid::new_v5(
        &NAMESPACE_AURORA_REPORT,
        format!("{boot_id}:{seq}").as_bytes(),
    )
}

open_vocab! {
    /// Result of applying one report.
    pub enum ReportStatus {
        Applied = "applied",
        Duplicate = "duplicate",
        Fenced = "fenced",
        Invalid = "invalid",
    }
}
open_vocab! {
    pub enum HomeDurability { Resident = "resident", Snapshot = "snapshot" }
}
open_vocab! {
    // (phase 4, P4-HAR, D-122) `codex` is the pinned OpenAI Codex CLI.
    pub enum HarnessId { ClaudeCode = "claude-code", Codex = "codex", Fake = "fake" }
}
open_vocab! {
    pub enum DesiredState { Running = "running", Stopped = "stopped", Deleted = "deleted" }
}
open_vocab! {
    pub enum StopReason {
        Idle = "idle", Cap = "cap", Evict = "evict",
        Restart = "restart", Delete = "delete", Failure = "failure",
        // (phase 2, RFE-006) the agent is effectively frozen: park, keep the home volume.
        Frozen = "frozen",
    }
}
open_vocab! {
    pub enum HomeSyncKind {
        Skills = "skills", Memory = "memory", RenderInputs = "render_inputs",
        Tools = "tools",
        // (phase 4, P4-PRV-1, D-144) The active preview shares; held in supervisor memory only.
        Previews = "previews",
        Manifest = "manifest", Final = "final",
    }
}
open_vocab! {
    pub enum SandboxState {
        Running = "running", Stopped = "stopped", Failed = "failed", Held = "held",
        // (FIX-025, D-158, C-580) start progress: `reason_code` is the phase, `message` the
        // detail; renews the API's start lease while the runner is still working on the boot.
        Starting = "starting",
    }
}
open_vocab! {
    pub enum TurnStatus { Done = "done", Failed = "failed", Aborted = "aborted" }
}
open_vocab! {
    pub enum SteerOutcome {
        Delivered = "delivered", Missed = "missed", Refused = "refused",
    }
}
open_vocab! {
    pub enum SendNowOutcome {
        Moved = "moved", NothingRunning = "nothing_running", Unsupported = "unsupported",
    }
}
open_vocab! {
    pub enum GateOutcome { Quiet = "quiet", Wake = "wake", Failed = "failed" }
}
open_vocab! {
    /// Provisional: the states are not enumerated in `16`; taken from `11`/`12` prose.
    pub enum ProcessStatus {
        Running = "running", Exited = "exited", Parked = "parked", Lost = "lost",
    }
}

/// Short open-vocabulary string (error/reason codes, capability names).
pub type Code = BoundedString<OPEN_VOCAB_MAX>;

// ---------------------------------------------------------------- register

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Range {
    pub min: u32,
    pub max: u32,
}

/// Per-sandbox resource defaults of a runner (FIX-011, D-90).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxDefaults {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub pids: u32,
    pub home_gib: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceRanges {
    pub cpu: Range,
    pub memory_mib: Range,
    pub pids: Range,
    pub home_gib: Range,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunnerCapabilities {
    pub backend: String,
    pub home_durability: HomeDurability,
    pub resources: ResourceRanges,
    pub max_sandboxes: u32,
    pub harnesses: Vec<HarnessId>,
    pub max_attachment_bytes: u64,
    /// The sandbox image this runner starts (`runner.image`); the single source of truth the
    /// API reports in `SandboxView.image`. Absent from older runners.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// `runner.held_awake_ceiling` (docs/11 §Stop): running-but-idle sandboxes above this make
    /// due-work evict for a waiting start. Absent from older runners: the API derives
    /// `max(1, max_sandboxes − 1)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_awake_ceiling: Option<u32>,
    /// (FIX-011, D-90) The runner's configured sandbox defaults (`runner.sandbox_cpus`,
    /// `sandbox_memory_mib`, `sandbox_pids`, `home_gib`): the API uses them when neither the
    /// agent nor the org overrides a resource axis. Absent from older runners.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<SandboxDefaults>,
    /// (phase 1, RFE-003) Capabilities the sandbox image declares in its
    /// `org.aurora.capabilities` label (docs/11 §The boot document); phase 1 defines
    /// `browser`. Open vocabulary; absent = none (fail closed, also for older runners).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisterRequest {
    pub version: String,
    pub backend: String,
    pub capabilities: RunnerCapabilities,
    pub advertised_host: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunnerConfig {
    pub poll_wait_secs: u32,
    pub reconcile_interval_secs: u32,
    pub orphan_grace_secs: u32,
    pub idle_stop_secs: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisterResponse {
    pub runner_id: Uuid,
    pub replica_id: Uuid,
    pub protocol: u32,
    pub config: RunnerConfig,
}

impl Validate for RegisterRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("version", &self.version, 64)?;
        check_chars("backend", &self.backend, 64)?;
        check_chars("advertised_host", &self.advertised_host, 255)?;
        check_max("harnesses", self.capabilities.harnesses.len(), 32)
    }
}

// -------------------------------------------------------------------- work

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Capacity {
    pub running: u32,
    pub max: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkRequest {
    pub wait_secs: u32,
    pub capacity: Capacity,
}

impl Validate for WorkRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_max("wait_secs", self.wait_secs as usize, MAX_WAIT_SECS as usize)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkResponse {
    pub items: Vec<WorkItem>,
    /// True only for the oldest live replica of the runner, computed at poll time (S-08).
    #[serde(default)]
    pub reconciler: bool,
}

/// `[key, value]` pairs, as on the wire.
pub type Pairs = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxResources {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub pids: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxSpec {
    pub image: String,
    pub env: Pairs,
    /// base64url
    pub boot_secret: SecretString,
    pub home_ref: String,
    pub home_gib: u32,
    pub resources: SandboxResources,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxStart {
    pub spec: SandboxSpec,
    /// (FIX-025, D-158, C-580) Bound on the whole start (provisioning, capacity wait, image
    /// pull, container start) the runner must respect; absent from an older API: the runner's
    /// own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_timeout_secs: Option<u32>,
}

/// (FIX-025, C-580) At most this many pod Warning events ride on a `sandbox.status`.
pub const MAX_STATUS_EVENTS: usize = 5;
/// (FIX-025, C-580) Bound of one event line.
pub const MAX_STATUS_EVENT_CHARS: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxStop {
    pub reason: StopReason,
    pub grace_secs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HomeSyncPart {
    pub kind: HomeSyncKind,
    /// Entry shape per part kind is defined in `12 §Sync protocol` (not typed here).
    pub entries: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HomeSync {
    pub generation: u64,
    pub part_index: u32,
    pub part_count: u32,
    pub part: HomeSyncPart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_ref: Option<String>,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelChoice {
    pub provider: String,
    pub endpoint_id: String,
    pub base_url: String,
    pub dialect: String,
    pub model: String,
    pub effort: String,
    pub client_env: Pairs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Recall {
    pub index: String,
    pub snippets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnInput {
    pub text: String,
    pub speaker_label: String,
    pub door: String,
    pub recall: Recall,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentRef {
    pub attachment_id: Uuid,
    pub filename: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Gate {
    pub command: String,
    pub timeout_secs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnDeliver {
    pub conversation_id: Uuid,
    pub turn_id: Uuid,
    pub session: SessionInfo,
    pub model: ModelChoice,
    /// `auc_…`
    pub context_credential: SecretString,
    pub input: TurnInput,
    pub attachments: Vec<AttachmentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<Gate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnMessage {
    pub turn_id: Uuid,
    pub followup_id: Uuid,
    pub text: String,
    pub speaker_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnRef {
    pub turn_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnNotice {
    pub turn_id: Uuid,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionRelease {
    pub conversation_id: Uuid,
}

/// (phase 2, RFE-010) Delete the stored harness session files of one gone conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPurge {
    pub conversation_id: Uuid,
    pub session_ref: String,
    /// (phase 4, P4-HAR, D-153) The harness that created the session: its adapter deletes the
    /// files. Absent = the sandbox's own harness (older API, and rows queued before a switch
    /// recorded it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
}

/// (phase 2, RFE-010) Outcome of a [`SessionPurge`]; `ok = false` keeps the purge pending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionPurged {
    pub conversation_id: Uuid,
    pub session_ref: String,
    pub ok: bool,
}

/// (phase 4, P4-PRV-1, D-145) On-demand start of a share's app: the work item the API enqueues
/// when the share's port is not listening. The command text never travels on the wire: the
/// supervisor looks the command up in its in-memory share list (home-sync part `previews`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PreviewStart {
    pub share_id: Uuid,
}

/// The frame the runner forwards to the supervisor: only the port, resolved from the share
/// (`16 §W5`); the supervisor refuses an unknown port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PreviewStartFrame {
    pub port: u16,
}

tagged_open! {
    /// Work-item body, tagged by `kind`.
    pub enum WorkItemBody, mode = Internal, tag = "kind" {
        SandboxStart(SandboxStart) = "sandbox.start",
        SandboxStop(SandboxStop) = "sandbox.stop",
        HomeSync(HomeSync) = "home.sync",
        TurnDeliver(Box<TurnDeliver>) = "turn.deliver",
        TurnMessage(TurnMessage) = "turn.message",
        TurnSendNow(TurnRef) = "turn.send_now",
        TurnAbort(TurnRef) = "turn.abort",
        TurnNotice(TurnNotice) = "turn.notice",
        SessionRelease(SessionRelease) = "session.release",
        SessionPurge(SessionPurge) = "session.purge",
        PreviewStart(PreviewStart) = "preview.start",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkItem {
    pub item_id: Uuid,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    #[serde(flatten)]
    pub body: WorkItemBody,
}

fn validate_turn_input(i: &TurnInput) -> Result<(), ValidationError> {
    check_chars("input.text", &i.text, MAX_TURN_TEXT)?;
    check_chars(
        "input.recall.index",
        &i.recall.index,
        limits::MEMORY_INDEX_MAX,
    )?;
    check_max(
        "input.recall.snippets",
        i.recall.snippets.len(),
        limits::MEMORY_SNIPPETS as usize,
    )?;
    for s in &i.recall.snippets {
        check_chars("input.recall.snippets[]", s, limits::MEMORY_SNIPPET_CHARS)?;
    }
    Ok(())
}

impl Validate for TurnDeliver {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_turn_input(&self.input)?;
        check_max("attachments", self.attachments.len(), MAX_TURN_ATTACHMENTS)?;
        for a in &self.attachments {
            check_max(
                "attachments[].size",
                a.size as usize,
                limits::ATTACH_IN_FILE_MAX as usize,
            )?;
        }
        if let Some(g) = &self.gate {
            check_max(
                "gate.timeout_secs",
                g.timeout_secs as usize,
                limits::GATE_TIMEOUT.as_secs() as usize,
            )?;
        }
        Ok(())
    }
}

impl Validate for HomeSync {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.part_count == 0 || self.part_index >= self.part_count {
            return Err(ValidationError::Invalid {
                field: "part_index",
                reason: "must be < part_count and part_count > 0",
            });
        }
        check_max("part", json_len(&self.part), MAX_HOME_SYNC_PART)
    }
}

impl Validate for WorkItemBody {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::HomeSync(v) => v.validate(),
            Self::TurnDeliver(v) => v.validate(),
            Self::TurnMessage(v) => check_chars("text", &v.text, MAX_TURN_TEXT),
            Self::TurnNotice(v) => check_chars("text", &v.text, MAX_TURN_TEXT),
            _ => Ok(()),
        }
    }
}

impl Validate for WorkItem {
    fn validate(&self) -> Result<(), ValidationError> {
        self.body.validate()
    }
}

// ----------------------------------------------------------------- reports

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Observed {
    pub cpu_millis: u32,
    pub memory_mib: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxStatus {
    pub state: SandboxState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<Code>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<Observed>,
    /// (FIX-025, C-580) The substrate's latest warning events for this boot (Kubernetes pod
    /// Warning events), newest first, ≤ `MAX_STATUS_EVENTS` × `MAX_STATUS_EVENT_CHARS`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessInfo {
    pub id: HarnessId,
    pub capabilities: Vec<Code>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SupervisorReady {
    pub version: String,
    pub harness: HarnessInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HomeSynced {
    pub generation: u64,
}

/// Harness event as carried on the wire. `12` lists the event kinds (text
/// delta, thinking delta, activity, tool started/finished, correction, message
/// joined, sent-now, session ref, usage, done, error) but not their fields, so
/// the payload is kept open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessEventWire {
    pub kind: Code,
    #[serde(flatten)]
    pub data: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NumberedEvent {
    pub n: u64,
    pub event: HarnessEventWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnEvents {
    pub turn_id: Uuid,
    pub events: Vec<NumberedEvent>,
}

/// Single event frame (W6 `turn.event`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnEvent {
    pub turn_id: Uuid,
    pub n: u64,
    pub event: HarnessEventWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnResult {
    pub turn_id: Uuid,
    pub status: TurnStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<Code>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Shape unspecified in `16`/`12`; kept open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SteerResult {
    pub followup_id: Uuid,
    pub outcome: SteerOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SendNowResult {
    pub turn_id: Uuid,
    pub outcome: SendNowOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionReport {
    pub conversation_id: Uuid,
    pub session_ref: String,
    pub epoch: u64,
}

/// Provisional shape: `16` names the type but never defines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessState {
    pub process_id: String,
    pub state: ProcessStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Last output, at most `PROCESS_TAIL` bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessStates {
    pub processes: Vec<ProcessState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WatchFired {
    pub watch_id: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GateResult {
    pub turn_id: Uuid,
    pub outcome: GateOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PortsState {
    pub ports: Vec<u16>,
}

tagged_open! {
    /// Report body, tagged by `kind`.
    pub enum ReportBody, mode = Internal, tag = "kind" {
        SandboxStatus(SandboxStatus) = "sandbox.status",
        SupervisorReady(SupervisorReady) = "supervisor.ready",
        HomeSynced(HomeSynced) = "home.synced",
        TurnStarted(TurnRef) = "turn.started",
        TurnEvents(TurnEvents) = "turn.events",
        TurnProgress(TurnRef) = "turn.progress",
        TurnResult(TurnResult) = "turn.result",
        SteerResult(SteerResult) = "steer.result",
        SendNowResult(SendNowResult) = "send_now.result",
        Session(SessionReport) = "session",
        SessionPurged(SessionPurged) = "session.purged",
        ProcessState(ProcessStates) = "process.state",
        WatchFired(WatchFired) = "watch.fired",
        GateResult(GateResult) = "gate.result",
        PortsState(PortsState) = "ports.state",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Report {
    /// Sender-generated idempotency id.
    pub report_id: Uuid,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    #[serde(flatten)]
    pub body: ReportBody,
}

impl Validate for ProcessStates {
    fn validate(&self) -> Result<(), ValidationError> {
        for p in &self.processes {
            if let Some(t) = &p.tail {
                check_bytes("processes[].tail", t, limits::PROCESS_TAIL)?;
            }
        }
        Ok(())
    }
}

impl Validate for ReportBody {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::SandboxStatus(v) => {
                check_max("events", v.events.len(), MAX_STATUS_EVENTS)?;
                for e in &v.events {
                    check_chars("events[]", e, MAX_STATUS_EVENT_CHARS)?;
                }
                v.message
                    .as_deref()
                    .map_or(Ok(()), |m| check_chars("message", m, MAX_MESSAGE_CHARS))
            }
            Self::TurnResult(v) => v.error_message.as_deref().map_or(Ok(()), |m| {
                check_chars("error_message", m, MAX_MESSAGE_CHARS)
            }),
            Self::ProcessState(v) => v.validate(),
            Self::WatchFired(v) => check_bytes("detail", &v.detail, MAX_WATCH_DETAIL),
            Self::GateResult(v) => v
                .output
                .as_deref()
                .map_or(Ok(()), |m| check_bytes("output", m, MAX_GATE_OUTPUT)),
            Self::TurnEvents(v) if v.events.is_empty() => Err(ValidationError::Invalid {
                field: "events",
                reason: "must not be empty",
            }),
            _ => Ok(()),
        }
    }
}

impl Validate for Report {
    fn validate(&self) -> Result<(), ValidationError> {
        self.body.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReportsRequest {
    pub reports: Vec<Report>,
}

impl Validate for ReportsRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_max("reports", self.reports.len(), MAX_REPORTS_PER_BATCH)?;
        for r in &self.reports {
            r.validate()?;
        }
        check_max("reports body", json_len(self), MAX_REPORTS_BODY)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportResult {
    pub report_id: Uuid,
    pub status: ReportStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportsResponse {
    pub results: Vec<ReportResult>,
}

// ---------------------------------------------------------------- channel

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelClaimRequest {
    pub bootstrap_token: SecretString,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelClaimResponse {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
}

/// Body of `/channel/hold` and `/channel/release`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelRef {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OkResponse {
    pub ok: bool,
}

// ----------------------------------------------------------- other calls

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryWriteRequest {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    #[serde(flatten)]
    pub write: MemoryWrite,
}

/// Memory write payload (also the W6 `memory.write` body).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryWrite {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub content: String,
}

impl Validate for MemoryWrite {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("key", &self.key, 128)?;
        check_chars("content", &self.content, limits::MEMORY_CONTENT_MAX)?;
        if let Some(t) = &self.title {
            check_chars("title", t, 200)?;
        }
        if let Some(d) = &self.description {
            check_chars("description", d, limits::SKILL_DESCRIPTION_MAX)?;
        }
        Ok(())
    }
}

impl Validate for MemoryWriteRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.write.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryWriteResponse {
    pub revision: u64,
}

/// Attachment upload metadata carried in headers (`HDR_*`); body is the file bytes.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AttachmentUploadMeta {
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub conversation_id: Uuid,
    pub turn_id: Uuid,
    pub filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

impl Validate for AttachmentUploadMeta {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("filename", &self.filename, 255)?;
        if let Some(c) = &self.caption {
            check_chars("caption", c, limits::ATTACH_CAPTION_MAX)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentUploadResponse {
    pub attachment_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxAssignment {
    pub sandbox_id: Uuid,
    pub agent_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<Uuid>,
    pub desired: DesiredState,
    pub home_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxesResponse {
    pub sandboxes: Vec<SandboxAssignment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxesCheckRequest {
    pub sandbox_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxesCheckResponse {
    pub known: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StrayObject {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<Uuid>,
    pub container_ref: String,
    /// Informational (runner clock), RFC 3339.
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StraysRequest {
    pub objects: Vec<StrayObject>,
}

#[cfg(test)]
mod report_id_tests {
    use super::*;

    #[test]
    fn frame_report_id_is_deterministic_v5() {
        let boot = Uuid::from_u128(1);
        assert_eq!(frame_report_id(boot, 5), frame_report_id(boot, 5));
        assert_ne!(frame_report_id(boot, 5), frame_report_id(boot, 6));
        assert_ne!(
            frame_report_id(boot, 5),
            frame_report_id(Uuid::from_u128(2), 5)
        );
        assert_eq!(frame_report_id(boot, 5).get_version_num(), 5);
    }
}
