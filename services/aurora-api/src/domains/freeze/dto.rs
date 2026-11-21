use aurora_db::repo::freeze::Effective;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// A person as `{ id, display_name }` (C-1102; the shape of `Schedule.paused_by`).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NamedPerson {
    pub id: Uuid,
    pub display_name: String,
}

/// A freeze record (phase 2, RFE-006).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Frozen {
    pub at: DateTime<Utc>,
    /// Who froze it (their e-mail at read time). Kept for compatibility: show `by`.
    pub by_label: String,
    /// (C-1102) Who froze it, by name; null when the user is gone.
    pub by: Option<NamedPerson>,
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
            by: e
                .frozen
                .by_user_id
                .zip(e.frozen.by_name.clone())
                .map(|(id, display_name)| NamedPerson { id, display_name }),
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
    /// What a `freeze` call did: `frozen` (it was not frozen), `escalated` (it was frozen by a
    /// lower authority; the record now carries the caller's authority, reason and identity) or
    /// `unchanged` (already frozen by an equal or higher authority: nothing changed). `null`
    /// after `unfreeze`.
    pub outcome: Option<String>,
}

/// One organization in the instance-admin list (operator view: no org data).
#[derive(Debug, Serialize, ToSchema)]
pub struct AdminOrg {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub frozen: Option<Frozen>,
    pub created_at: DateTime<Utc>,
    /// (C-733) No active owner.
    pub ownerless: bool,
    /// Active members.
    pub member_count: i64,
    pub workspace_count: i64,
    pub agent_count: i64,
}

/// One workspace of an organization in the instance-admin view: metadata only (C-733).
#[derive(Debug, Serialize, ToSchema)]
pub struct AdminWorkspace {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub personal: bool,
    pub agent_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListAdminWorkspacesQuery {
    /// Page size, 1..=200 (default 50).
    pub limit: Option<u32>,
    pub cursor: Option<String>,
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
