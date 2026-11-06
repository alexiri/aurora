use aurora_db::repo::grants::SecretGrantRow;
use chrono::{DateTime, Utc};
use serde::Serialize;
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

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrantMode {
    Full,
    Custom,
}

/// Connection grants arrive with M3; the list is empty until then.
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
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveTool {
    pub app_id: String,
    pub tool_id: String,
    pub decision: String,
    pub org_ceiling: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Effective {
    pub credentials: Vec<EffectiveCredential>,
    /// App tools (M3); empty until apps exist.
    pub tools: Vec<EffectiveTool>,
}
