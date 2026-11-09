use aurora_db::repo::grants::{ConnectionGrantRow, SecretGrantRow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema)]
pub struct SecretGrant {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub secret_id: Uuid,
    pub created_at: DateTime<Utc>,
}

impl From<SecretGrantRow> for SecretGrant {
    fn from(r: SecretGrantRow) -> Self {
        Self {
            id: r.id,
            agent_id: r.agent_id,
            secret_id: r.secret_id,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrantMode {
    Full,
    Custom,
}

impl GrantMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Custom => "custom",
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectionGrant {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub connection_id: Uuid,
    pub mode: GrantMode,
    pub allow_tools: Vec<String>,
    pub ask_tools: Vec<String>,
    /// (phase 2, RFE-011a) hold deadline preference of the ask tools in seconds; `null` = none.
    pub ask_timeout_secs: Option<i32>,
    pub created_at: DateTime<Utc>,
}

impl From<ConnectionGrantRow> for ConnectionGrant {
    fn from(r: ConnectionGrantRow) -> Self {
        Self {
            id: r.id,
            agent_id: r.agent_id,
            connection_id: r.connection_id,
            mode: if r.mode == "full" {
                GrantMode::Full
            } else {
                GrantMode::Custom
            },
            allow_tools: r.allow_tools,
            ask_tools: r.ask_tools,
            ask_timeout_secs: r.ask_timeout_secs,
            created_at: r.created_at,
        }
    }
}

/// (FIX-020, `02` `GRANT_BULK_MAX`) most agents in one credential-side bulk grant.
pub const GRANT_BULK_MAX: usize = 50;

/// A secret grant seen from the secret (FIX-020, C-503).
#[derive(Debug, Serialize, ToSchema)]
pub struct SecretGrantView {
    #[serde(flatten)]
    pub grant: SecretGrant,
    pub agent_name: String,
    pub workspace_id: Uuid,
    pub workspace_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SecretGrantViewList {
    pub items: Vec<SecretGrantView>,
}

/// A connection grant seen from the connection (FIX-020, C-504).
#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectionGrantView {
    #[serde(flatten)]
    pub grant: ConnectionGrant,
    pub agent_name: String,
    pub workspace_id: Uuid,
    pub workspace_name: String,
}

/// `POST /v1/secrets/{id}/grants` body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkSecretGrantRequest {
    /// 1–`GRANT_BULK_MAX` distinct agent ids.
    pub agent_ids: Vec<Uuid>,
}

/// `POST /v1/connections/{id}/grants` body: the agent-side grant body plus the agents.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkConnectionGrantRequest {
    /// 1–`GRANT_BULK_MAX` distinct agent ids.
    pub agent_ids: Vec<Uuid>,
    pub mode: GrantMode,
    #[serde(default)]
    pub allow_tools: Vec<String>,
    #[serde(default)]
    pub ask_tools: Vec<String>,
    #[serde(default)]
    pub ask_timeout_secs: Option<i64>,
}

/// `created` | `existing` (an existing grant is never modified).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BulkStatus {
    Created,
    Existing,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BulkSecretGrantItem {
    pub agent_id: Uuid,
    pub grant: SecretGrant,
    pub status: BulkStatus,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BulkSecretGrantResponse {
    pub items: Vec<BulkSecretGrantItem>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BulkConnectionGrantItem {
    pub agent_id: Uuid,
    pub grant: ConnectionGrant,
    pub status: BulkStatus,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BulkConnectionGrantResponse {
    pub items: Vec<BulkConnectionGrantItem>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PutConnectionGrant {
    pub mode: GrantMode,
    #[serde(default)]
    pub allow_tools: Vec<String>,
    #[serde(default)]
    pub ask_tools: Vec<String>,
    /// (phase 2, RFE-011a) custom grants with `ask_tools` only; validated like a rule's timeout
    /// (422 `approval_timeout_out_of_range`).
    #[serde(default)]
    pub ask_timeout_secs: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectionGrantList {
    pub items: Vec<ConnectionGrantView>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GrantsResponse {
    pub secrets: Vec<SecretGrant>,
    pub connections: Vec<ConnectionGrant>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveCredential {
    /// `secret` | `connection`.
    pub kind: String,
    pub id: Uuid,
    pub name: String,
    /// `organization` | `workspace`.
    pub scope: String,
    pub selected: bool,
    /// Logical ids of the allow rules that select it.
    pub selected_by: Vec<Uuid>,
    /// (phase 2, RFE-005) An enabled allow rule with a `user` identity targets this credential,
    /// so whether it is selected changes with the speaker (compare `?speaker=none`).
    pub depends_on_speaker: bool,
    /// (FIX-014) A connection's status (`active` | `disabled` | `reauth_required`); absent for
    /// secrets. A connection that is not `active` is not usable: the gateway refuses it and it
    /// contributes no app env, stubs or snippets to a boot, and no tool decisions are listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Query of `GET /v1/agents/{agent}/effective` (phase 2, RFE-005).
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct EffectiveQuery {
    /// A user id (the person whose turns are evaluated) or `none` (speakerless traffic).
    /// Default: the caller.
    pub speaker: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveTool {
    /// Connection the decision was computed for.
    pub connection_id: Uuid,
    pub app_id: String,
    pub tool_id: String,
    /// `allow` | `needs_approval` | `rate_limited` | `block`, or `inert` for a granted tool
    /// that no longer exists in the app's catalogue.
    pub decision: String,
    pub inert: bool,
    pub org_ceiling: Option<String>,
    /// (phase 2, RFE-011a) Only for `needs_approval`: the hold timeout in seconds a call of this
    /// tool would get (minimum of the set rule timeouts, else `approvals.timeout_secs`).
    pub approval_timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Effective {
    pub credentials: Vec<EffectiveCredential>,
    /// Per tool of every granted connection.
    pub tools: Vec<EffectiveTool>,
}
