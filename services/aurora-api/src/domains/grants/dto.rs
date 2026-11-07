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
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PutConnectionGrant {
    pub mode: GrantMode,
    #[serde(default)]
    pub allow_tools: Vec<String>,
    #[serde(default)]
    pub ask_tools: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectionGrantList {
    pub items: Vec<ConnectionGrant>,
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
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Effective {
    pub credentials: Vec<EffectiveCredential>,
    /// Per tool of every granted connection.
    pub tools: Vec<EffectiveTool>,
}
