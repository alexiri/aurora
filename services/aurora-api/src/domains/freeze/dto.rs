use aurora_db::repo::freeze::Effective;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// A freeze record (phase 2, RFE-006).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Frozen {
    pub at: DateTime<Utc>,
    /// Who froze it (their e-mail at read time).
    pub by_label: String,
    pub reason: String,
    /// Highest authority of the freezer: `workspace_owner` | `org_admin` | `instance_admin`.
    /// Lifting needs at least this authority.
    pub authority: String,
    /// Where the flag is set: `agent` | `workspace` | `org`.
    pub level: String,
}

impl From<&Effective> for Frozen {
    fn from(e: &Effective) -> Self {
        Self {
            at: e.frozen.at,
            by_label: e.frozen.by_label.clone(),
            reason: e.frozen.reason.clone(),
            authority: e.frozen.authority.clone(),
            level: e.level.as_str().to_owned(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FreezeRequest {
    /// 1..500 characters, sanitized to one line.
    pub reason: String,
}

/// Result of a freeze or unfreeze call.
#[derive(Debug, Serialize, ToSchema)]
pub struct FreezeResult {
    /// The flag of the scope the call acted on: set after `freeze`, `null` after `unfreeze`.
    pub frozen: Option<Frozen>,
    /// Whether the resource is effectively frozen after the call (an agent: agent ∨ workspace ∨
    /// org flag; a workspace: workspace ∨ org flag).
    pub effective_frozen: bool,
}

/// One organization in the instance-admin list (operator view: no org data).
#[derive(Debug, Serialize, ToSchema)]
pub struct AdminOrg {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub frozen: Option<Frozen>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListAdminOrgsQuery {
    /// Page size, 1..=200 (default 50).
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// Only frozen organizations.
    pub frozen: Option<bool>,
}
