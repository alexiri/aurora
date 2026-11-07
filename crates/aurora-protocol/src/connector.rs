//! W8 connector plane messages (`connector/1`). Normative: `docs/13-channels-connectors.md`
//! §3, §4 and `docs/16-wire-protocols.md`.
//!
//! Envelope messages (`/v1/connector/*` bodies) are strict structs. Event and
//! operation payloads travel as JSON (`payload: Value`) beside an open `kind`
//! vocabulary: unknown kinds deserialize to `Other` and are logged and skipped
//! (or answered `unsupported`) by consumers, never fatal. `InboundEvent::parse`
//! and `Operation::parse` turn a payload into its typed body.
//!
//! `Content` (text + card) lives in `aurora-channels`, which depends on this
//! crate; operation payloads therefore carry it as raw JSON (`content: Value`)
//! and consumers deserialize it with `aurora_channels::Content`.

use std::collections::BTreeMap;

use aurora_core::limits;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::common::{Validate, ValidationError, check_chars, check_max};
use crate::open_vocab;
use crate::vocab::{BoundedString, OPEN_VOCAB_MAX};

/// Header carrying the connector replica id on every call after `register`.
pub const HDR_REPLICA: &str = "x-aurora-replica";
/// Header carrying the original filename on `PUT /files/{op_id}`.
pub const HDR_FILENAME: &str = "x-aurora-filename";
/// `X-Aurora-Protocol` value of this contract.
pub const PROTOCOL_HEADER_VALUE: &str = "connector/1";
/// Max events per `POST /events`.
pub const MAX_EVENTS_PER_BATCH: usize = 100;
/// Max results per `POST /ops/results`.
pub const MAX_RESULTS_PER_BATCH: usize = 100;
/// Max ops per claim.
pub const MAX_CLAIM: u32 = 100;
/// Max `wait_secs` of `POST /ops/claim`.
pub const MAX_WAIT_SECS: u32 = limits::CONNECTOR_OPS_WAIT.as_secs() as u32;
/// Default poll intervals advertised by `register`.
pub const DEFAULT_CONFIG_SECS: u32 = 15;
/// Max chars of neutral message text carried in an event.
pub const MAX_EVENT_TEXT_CHARS: usize = 64_000;
/// Max history messages per `history.read`.
pub const MAX_HISTORY_LIMIT: u32 = 50;
/// Max chars of free-text error messages.
pub const MAX_MESSAGE_CHARS: usize = 2_000;
const MAX_ID_CHARS: usize = 512;

/// Short open-vocabulary string.
pub type Code = BoundedString<OPEN_VOCAB_MAX>;

open_vocab! {
    /// Connector lifecycle status (`register` response).
    pub enum ConnectorStatus {
        AwaitingRegistration = "awaiting_registration",
        PendingApproval = "pending_approval",
        Active = "active",
        Disabled = "disabled",
    }
}
open_vocab! {
    pub enum EventKind {
        MessagePosted = "message.posted",
        MessageEdited = "message.edited",
        MessageDeleted = "message.deleted",
        BotAdded = "bot.added",
        BotRemoved = "bot.removed",
        ActionInvoked = "action.invoked",
        CommandInvoked = "command.invoked",
        PresenceRemoved = "presence.removed",
        ReactionAdded = "reaction.added",
    }
}
open_vocab! {
    pub enum OpKind {
        MessagePost = "message.post",
        MessageUpdate = "message.update",
        MessageDelete = "message.delete",
        ReactionAdd = "reaction.add",
        ReactionRemove = "reaction.remove",
        StatusSet = "status.set",
        StatusClear = "status.clear",
        FileFetch = "file.fetch",
        UserLookup = "user.lookup",
        SpacesList = "spaces.list",
        SpaceMembers = "space.members",
        HistoryRead = "history.read",
        PresenceProvision = "presence.provision",
        PresenceDeprovision = "presence.deprovision",
        PresenceRename = "presence.rename",
        PresenceSetIcon = "presence.set_icon",
        CredentialsValidate = "credentials.validate",
    }
}
open_vocab! {
    /// Neutral operation error codes (13 §4.2).
    pub enum ErrorCode {
        CredentialDead = "credential_dead",
        RateLimited = "rate_limited",
        Unavailable = "unavailable",
        Forbidden = "forbidden",
        NotFound = "not_found",
        Invalid = "invalid",
        TooLarge = "too_large",
        Unsupported = "unsupported",
        StaleVersion = "stale_version",
    }
}
open_vocab! {
    pub enum SpaceKind {
        Dm = "dm", GroupDm = "group_dm", Channel = "channel", Private = "private",
    }
}
open_vocab! {
    pub enum AuthorKind { Person = "person", Bot = "bot" }
}
open_vocab! {
    pub enum MentionKind {
        User = "user", Bot = "bot", Space = "space", Broadcast = "broadcast",
    }
}
open_vocab! {
    pub enum PresenceRemovedReason {
        Uninstalled = "uninstalled", TokenRevoked = "token_revoked", Deleted = "deleted",
    }
}
open_vocab! {
    pub enum PresenceMode { Dedicated = "dedicated", Shared = "shared" }
}
open_vocab! {
    pub enum ListenMode { Mentions = "mentions", All = "all" }
}

// ------------------------------------------------------------ manifest

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ManifestPackage {
    pub id: String,
    pub version: String,
    pub contract: u32,
    pub publisher: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ManifestAbout {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_png_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_guide_md: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ManifestLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_max_chars: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writes_per_space_per_minute: Option<u32>,
}

/// Connector manifest (13 §3). `capabilities` is an open vocabulary: a name
/// maps to `true`/`false` or an object (`files`); unknown names are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectorManifest {
    pub package: ManifestPackage,
    pub about: ManifestAbout,
    #[serde(default)]
    pub capabilities: BTreeMap<String, Value>,
    #[serde(default)]
    pub integration_setup: Value,
    #[serde(default)]
    pub presence_setup: Value,
    #[serde(default)]
    pub limits: ManifestLimits,
    #[serde(default)]
    pub public_callbacks: Vec<String>,
}

impl ConnectorManifest {
    /// Names of capabilities that are enabled (`true` or an object).
    pub fn enabled_capabilities(&self) -> Vec<&str> {
        self.capabilities
            .iter()
            .filter(|(_, v)| v.as_bool().unwrap_or_else(|| v.is_object()))
            .map(|(k, _)| k.as_str())
            .collect()
    }
}

impl Validate for ConnectorManifest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("package.id", &self.package.id, 64)?;
        if self.package.id.is_empty() {
            return Err(ValidationError::Invalid {
                field: "package.id",
                reason: "empty",
            });
        }
        if self.package.contract != 1 {
            return Err(ValidationError::Invalid {
                field: "package.contract",
                reason: "unsupported contract",
            });
        }
        check_chars("package.version", &self.package.version, 64)?;
        check_chars("about.name", &self.about.name, 120)?;
        check_max("capabilities", self.capabilities.len(), 64)?;
        check_max("public_callbacks", self.public_callbacks.len(), 16)
    }
}

// ------------------------------------------------------------ register

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisterRequest {
    pub manifest: ConnectorManifest,
    /// Detached signature over sha256 of the canonical manifest JSON.
    pub signature: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replica_hint: Option<String>,
}

impl Validate for RegisterRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.manifest.validate()?;
        check_chars("signature", &self.signature, 4096)?;
        check_chars("version", &self.version, 64)?;
        if let Some(h) = &self.replica_hint {
            check_chars("replica_hint", h, 128)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PollConfig {
    pub config_secs: u32,
    pub ops_wait_secs: u32,
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            config_secs: DEFAULT_CONFIG_SECS,
            ops_wait_secs: MAX_WAIT_SECS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisterResponse {
    pub connector_id: String,
    pub replica_id: Uuid,
    pub status: ConnectorStatus,
    pub poll: PollConfig,
}

// -------------------------------------------------------------- config

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SharedBot {
    pub platform_user_ref: String,
    pub handle: String,
    pub display_name: String,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Integration {
    pub id: Uuid,
    pub org_id: Uuid,
    pub tenant_ref: String,
    pub tenant_label: String,
    #[serde(default)]
    pub config: Value,
    /// Decrypted platform credentials (never log).
    #[serde(default)]
    pub credentials: Value,
    #[serde(default)]
    pub credentials_version: u64,
    #[serde(default)]
    pub shared_bot: Option<SharedBot>,
    pub status: String,
}

impl std::fmt::Debug for Integration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Integration")
            .field("id", &self.id)
            .field("tenant_ref", &self.tenant_ref)
            .field("credentials", &"[redacted]")
            .field("credentials_version", &self.credentials_version)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Presence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Presence")
            .field("id", &self.id)
            .field("integration_id", &self.integration_id)
            .field("mode", &self.mode)
            .field("credentials", &"[redacted]")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentRef {
    pub id: Uuid,
    pub handle: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Presence {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub mode: PresenceMode,
    pub agent: AgentRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_user_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_version: Option<u64>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Binding {
    pub integration_id: Uuid,
    pub space_ref: String,
    pub agent_id: Uuid,
    pub listen: ListenMode,
    #[serde(default)]
    pub followups_need_mention: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigResponse {
    pub etag: String,
    pub lease_secs: u32,
    #[serde(default)]
    pub integrations: Vec<Integration>,
    #[serde(default)]
    pub presences: Vec<Presence>,
    #[serde(default)]
    pub space_bindings: Vec<Binding>,
}

// -------------------------------------------------------------- events

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InboundEvent {
    /// The platform's own id (or deterministic hash); the core dedupes on `(connector, event_id)`.
    pub event_id: String,
    pub integration_id: Uuid,
    pub kind: EventKind,
    /// RFC 3339; informational only.
    pub occurred_at: String,
    pub payload: Value,
}

impl Validate for InboundEvent {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("event_id", &self.event_id, MAX_ID_CHARS)?;
        if self.event_id.is_empty() {
            return Err(ValidationError::Invalid {
                field: "event_id",
                reason: "empty",
            });
        }
        check_chars("occurred_at", &self.occurred_at, 64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventsRequest {
    pub events: Vec<InboundEvent>,
}

impl Validate for EventsRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_max("events", self.events.len(), MAX_EVENTS_PER_BATCH)?;
        self.events.iter().try_for_each(Validate::validate)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RejectedEvent {
    pub event_id: String,
    pub code: ErrorCode,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventsResponse {
    #[serde(default)]
    pub accepted: Vec<String>,
    #[serde(default)]
    pub duplicates: Vec<String>,
    #[serde(default)]
    pub rejected: Vec<RejectedEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Space {
    #[serde(rename = "ref")]
    pub reference: String,
    pub kind: SpaceKind,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Author {
    #[serde(rename = "ref")]
    pub reference: String,
    pub kind: AuthorKind,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_verified: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Mention {
    pub kind: MentionKind,
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileRef {
    #[serde(rename = "ref")]
    pub reference: String,
    pub name: String,
    pub size: u64,
    pub content_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessagePosted {
    pub space: Space,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ref: Option<String>,
    pub message_ref: String,
    pub author: Author,
    /// Neutral markup; untrusted.
    pub text: String,
    #[serde(default)]
    pub mentions: Vec<Mention>,
    #[serde(default)]
    pub bot_mentioned: bool,
    #[serde(default)]
    pub addressed_bot_ref: String,
    #[serde(default)]
    pub files: Vec<FileRef>,
}

/// Payload of `message.edited` / `message.deleted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageChanged {
    pub space_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ref: Option<String>,
    pub message_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Payload of `bot.added` / `bot.removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BotMembership {
    pub space: Space,
    pub bot_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActionInvoked {
    /// Opaque action token; never interpreted by the connector.
    pub token: String,
    pub clicker_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandInvoked {
    pub name: String,
    #[serde(default)]
    pub args: String,
    pub space: Space,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ref: Option<String>,
    pub user_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceRemoved {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_id: Option<Uuid>,
    pub bot_ref: String,
    pub reason: PresenceRemovedReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReactionAdded {
    pub space_ref: String,
    pub message_ref: String,
    pub user_ref: String,
    pub emoji: String,
}

/// A typed event payload.
#[derive(Debug, Clone, PartialEq)]
pub enum EventPayload {
    MessagePosted(MessagePosted),
    MessageEdited(MessageChanged),
    MessageDeleted(MessageChanged),
    BotAdded(BotMembership),
    BotRemoved(BotMembership),
    ActionInvoked(ActionInvoked),
    CommandInvoked(CommandInvoked),
    PresenceRemoved(PresenceRemoved),
    ReactionAdded(ReactionAdded),
    /// Kind not known to this build; log and skip.
    Other,
}

impl InboundEvent {
    /// Builds an event from a typed payload.
    pub fn new<T: Serialize>(
        event_id: impl Into<String>,
        integration_id: Uuid,
        kind: EventKind,
        occurred_at: impl Into<String>,
        payload: &T,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            event_id: event_id.into(),
            integration_id,
            kind,
            occurred_at: occurred_at.into(),
            payload: serde_json::to_value(payload)?,
        })
    }

    /// Deserializes the payload according to `kind`.
    pub fn parse(&self) -> Result<EventPayload, serde_json::Error> {
        let p = self.payload.clone();
        Ok(match self.kind {
            EventKind::MessagePosted => EventPayload::MessagePosted(serde_json::from_value(p)?),
            EventKind::MessageEdited => EventPayload::MessageEdited(serde_json::from_value(p)?),
            EventKind::MessageDeleted => EventPayload::MessageDeleted(serde_json::from_value(p)?),
            EventKind::BotAdded => EventPayload::BotAdded(serde_json::from_value(p)?),
            EventKind::BotRemoved => EventPayload::BotRemoved(serde_json::from_value(p)?),
            EventKind::ActionInvoked => EventPayload::ActionInvoked(serde_json::from_value(p)?),
            EventKind::CommandInvoked => EventPayload::CommandInvoked(serde_json::from_value(p)?),
            EventKind::PresenceRemoved => EventPayload::PresenceRemoved(serde_json::from_value(p)?),
            EventKind::ReactionAdded => EventPayload::ReactionAdded(serde_json::from_value(p)?),
            EventKind::Other(_) => EventPayload::Other,
        })
    }
}

// ----------------------------------------------------------------- ops

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Operation {
    pub op_id: Uuid,
    pub kind: OpKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_id: Option<Uuid>,
    #[serde(default = "one")]
    pub attempt: u32,
    #[serde(default)]
    pub maybe_sent: bool,
    #[serde(default)]
    pub payload: Value,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimRequest {
    pub max: u32,
    pub wait_secs: u32,
}

impl Validate for ClaimRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.max == 0 {
            return Err(ValidationError::Invalid {
                field: "max",
                reason: "must be at least 1",
            });
        }
        check_max("max", self.max as usize, MAX_CLAIM as usize)?;
        check_max("wait_secs", self.wait_secs as usize, MAX_WAIT_SECS as usize)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimResponse {
    #[serde(default)]
    pub ops: Vec<Operation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u32>,
}

impl OpError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_secs: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpResult {
    pub op_id: Uuid,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<OpError>,
}

impl OpResult {
    pub fn ok(op_id: Uuid, data: Value) -> Self {
        Self {
            op_id,
            outcome: Outcome::Ok,
            data: Some(data),
            error: None,
        }
    }

    pub fn err(op_id: Uuid, error: OpError) -> Self {
        Self {
            op_id,
            outcome: Outcome::Error,
            data: None,
            error: Some(error),
        }
    }
}

impl Validate for OpResult {
    fn validate(&self) -> Result<(), ValidationError> {
        match (self.outcome, &self.error) {
            (Outcome::Error, None) => Err(ValidationError::Invalid {
                field: "error",
                reason: "required when outcome is error",
            }),
            (Outcome::Ok, Some(_)) => Err(ValidationError::Invalid {
                field: "error",
                reason: "forbidden when outcome is ok",
            }),
            (_, Some(e)) => check_chars("error.message", &e.message, MAX_MESSAGE_CHARS),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResultsRequest {
    pub results: Vec<OpResult>,
}

impl Validate for ResultsRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_max("results", self.results.len(), MAX_RESULTS_PER_BATCH)?;
        self.results.iter().try_for_each(Validate::validate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OkResponse {
    pub ok: bool,
}

// ----------------------------------------------- credentials / link / files

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialsRequest {
    pub target: CredentialTarget,
    pub expected_version: u64,
    pub credentials: Value,
}

impl Validate for CredentialsRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.target.integration_id.is_some() == self.target.presence_id.is_some() {
            return Err(ValidationError::Invalid {
                field: "target",
                reason: "exactly one of integration_id, presence_id",
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialsResponse {
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkStartRequest {
    pub integration_id: Uuid,
    pub platform_user_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkStartResponse {
    pub url: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilePutResponse {
    pub attachment_id: Uuid,
}

/// Error body shape returned by the core on 4xx/5xx (`code` open vocabulary).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorBody {
    pub code: Code,
    #[serde(default)]
    pub message: String,
}

// -------------------------------------------------- operation payloads

/// `"shared"` marker of [`Sender::Shared`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SharedTag {
    #[serde(rename = "shared")]
    Shared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Sender {
    Shared(SharedTag),
    Presence { presence_id: Uuid },
}

impl Sender {
    pub fn shared() -> Self {
        Sender::Shared(SharedTag::Shared)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Target {
    Space {
        space_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_ref: Option<String>,
    },
    User {
        user_ref: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutFile {
    pub attachment_id: Uuid,
    pub name: String,
    pub download_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MessagePostOp {
    pub target: Target,
    pub sender: Sender,
    /// `aurora_channels::Content` as JSON.
    pub content: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<OutFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_to: Option<String>,
    pub op_marker: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessagePostResult {
    pub message_ref: String,
    #[serde(default)]
    pub thread_ref: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MessageUpdateOp {
    pub space_ref: String,
    pub message_ref: String,
    pub sender: Sender,
    pub content: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageDeleteOp {
    pub space_ref: String,
    pub message_ref: String,
    pub sender: Sender,
}

/// `reaction.add` / `reaction.remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReactionOp {
    pub space_ref: String,
    pub message_ref: String,
    pub sender: Sender,
    pub emoji: String,
}

/// `status.set` / `status.clear`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusOp {
    pub space_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ref: Option<String>,
    pub sender: Sender,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileFetchOp {
    pub file_ref: String,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileFetchResult {
    pub upload_url_used: bool,
    pub attachment_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UserLookupOp {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlatformUser {
    #[serde(rename = "ref")]
    pub reference: String,
    pub display_name: String,
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: bool,
    #[serde(default)]
    pub is_bot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsersResult {
    pub users: Vec<PlatformUser>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpacesListOp {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpacesResult {
    pub spaces: Vec<Space>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpaceMembersOp {
    pub space_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryReadOp {
    pub space_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ref: Option<String>,
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryMessage {
    pub message_ref: String,
    pub author: Author,
    pub text: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryResult {
    pub messages: Vec<HistoryMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProvisionAgent {
    pub handle: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceProvisionOp {
    pub agent: ProvisionAgent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceProvisionResult {
    pub platform_user_ref: String,
    pub handle: String,
    pub credentials: Value,
}

/// `presence.deprovision` / `presence.rename` / `presence.set_icon`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceChangeOp {
    pub presence_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialsValidateOp {
    #[serde(default)]
    pub config: Value,
    pub credentials: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialsValidateResult {
    pub tenant_ref: String,
    pub tenant_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_bot: Option<SharedBot>,
    /// May only narrow the manifest's capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<BTreeMap<String, Value>>,
}

/// A typed operation payload.
#[derive(Debug, Clone, PartialEq)]
pub enum OpPayload {
    MessagePost(MessagePostOp),
    MessageUpdate(MessageUpdateOp),
    MessageDelete(MessageDeleteOp),
    ReactionAdd(ReactionOp),
    ReactionRemove(ReactionOp),
    StatusSet(StatusOp),
    StatusClear(StatusOp),
    FileFetch(FileFetchOp),
    UserLookup(UserLookupOp),
    SpacesList(SpacesListOp),
    SpaceMembers(SpaceMembersOp),
    HistoryRead(HistoryReadOp),
    PresenceProvision(PresenceProvisionOp),
    PresenceDeprovision(PresenceChangeOp),
    PresenceRename(PresenceChangeOp),
    PresenceSetIcon(PresenceChangeOp),
    CredentialsValidate(CredentialsValidateOp),
    /// Kind not known to this build; answer `unsupported`.
    Other,
}

impl Operation {
    /// Deserializes the payload according to `kind`.
    pub fn parse(&self) -> Result<OpPayload, serde_json::Error> {
        let p = self.payload.clone();
        Ok(match self.kind {
            OpKind::MessagePost => OpPayload::MessagePost(serde_json::from_value(p)?),
            OpKind::MessageUpdate => OpPayload::MessageUpdate(serde_json::from_value(p)?),
            OpKind::MessageDelete => OpPayload::MessageDelete(serde_json::from_value(p)?),
            OpKind::ReactionAdd => OpPayload::ReactionAdd(serde_json::from_value(p)?),
            OpKind::ReactionRemove => OpPayload::ReactionRemove(serde_json::from_value(p)?),
            OpKind::StatusSet => OpPayload::StatusSet(serde_json::from_value(p)?),
            OpKind::StatusClear => OpPayload::StatusClear(serde_json::from_value(p)?),
            OpKind::FileFetch => OpPayload::FileFetch(serde_json::from_value(p)?),
            OpKind::UserLookup => OpPayload::UserLookup(serde_json::from_value(p)?),
            OpKind::SpacesList => OpPayload::SpacesList(serde_json::from_value(p)?),
            OpKind::SpaceMembers => OpPayload::SpaceMembers(serde_json::from_value(p)?),
            OpKind::HistoryRead => {
                let h: HistoryReadOp = serde_json::from_value(p)?;
                OpPayload::HistoryRead(h)
            }
            OpKind::PresenceProvision => OpPayload::PresenceProvision(serde_json::from_value(p)?),
            OpKind::PresenceDeprovision => {
                OpPayload::PresenceDeprovision(serde_json::from_value(p)?)
            }
            OpKind::PresenceRename => OpPayload::PresenceRename(serde_json::from_value(p)?),
            OpKind::PresenceSetIcon => OpPayload::PresenceSetIcon(serde_json::from_value(p)?),
            OpKind::CredentialsValidate => {
                OpPayload::CredentialsValidate(serde_json::from_value(p)?)
            }
            OpKind::Other(_) => OpPayload::Other,
        })
    }
}

impl Validate for HistoryReadOp {
    fn validate(&self) -> Result<(), ValidationError> {
        check_max("limit", self.limit as usize, MAX_HISTORY_LIMIT as usize)
    }
}

impl Validate for EventsResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        check_max(
            "accepted+duplicates+rejected",
            self.accepted.len() + self.duplicates.len() + self.rejected.len(),
            MAX_EVENTS_PER_BATCH,
        )
    }
}
