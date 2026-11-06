use aurora_db::repo::agents::AgentRow;
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct Agent {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub handle: String,
    pub name: String,
    /// `byo` | `hosted`.
    pub kind: String,
    pub is_default: bool,
    pub harness: Option<String>,
    pub model_provider: Option<String>,
    pub model_endpoint_id: Option<String>,
    pub model_id: Option<String>,
    /// `low` | `medium` | `high` | `max`.
    pub effort: Option<String>,
    /// `ask` | `allow` | `off`.
    pub proactive_messaging: String,
    pub brief: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<AgentRow> for Agent {
    fn from(a: AgentRow) -> Self {
        Self {
            id: a.id,
            org_id: a.org_id,
            workspace_id: a.workspace_id,
            handle: a.handle,
            name: a.name,
            kind: a.kind,
            is_default: a.is_default,
            harness: a.harness,
            model_provider: a.model_provider,
            model_endpoint_id: a.model_endpoint_id,
            model_id: a.model_id,
            effort: a.effort,
            proactive_messaging: a.proactive_messaging,
            brief: a.brief,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}
