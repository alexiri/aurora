use aurora_db::repo::automations::OffboardRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// `PUT /schedules/{id}/creator` and `PUT /webhooks/{id}/creator`.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReassignCreatorRequest {
    /// An active org member who can use the workspace.
    pub user_id: Uuid,
    /// Re-enable an item that was disabled for `authorization` (default false).
    pub re_enable: Option<bool>,
}

/// `POST /workspaces/{ws}/automations/reassign`.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkReassignRequest {
    pub from_user_id: Uuid,
    /// A user id, or `"workspace"` to anchor the items to the workspace.
    pub to: String,
    /// `schedule` and/or `webhook`; default both.
    pub kinds: Option<Vec<String>>,
    /// Only these items; default every creator-anchored item of `from_user_id` in the workspace.
    pub ids: Option<Vec<Uuid>>,
}

#[derive(Serialize, ToSchema)]
pub struct BulkReassignResponse {
    pub reassigned: i64,
}

#[derive(Serialize, ToSchema)]
pub struct NamedRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct OffboardingItem {
    pub id: Uuid,
    pub name: String,
    pub agent: NamedRef,
    pub workspace: NamedRef,
    pub enabled: bool,
    /// `creator` | `workspace`.
    pub anchor: String,
    pub next_fire_at: Option<DateTime<Utc>>,
    /// Always true here: the list holds exactly what the removal would disable.
    pub would_disable: bool,
    /// (phase 4, P4-GRP-1) The groups that currently give the person access to this item's
    /// workspace (a binding on it, or an `admin` org-role mapping).
    pub via_groups: Vec<NamedRef>,
}

impl From<OffboardRow> for OffboardingItem {
    fn from(r: OffboardRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            agent: NamedRef {
                id: r.agent_id,
                name: r.agent_name,
            },
            workspace: NamedRef {
                id: r.workspace_id,
                name: r.workspace_name,
            },
            enabled: r.enabled,
            anchor: r.anchor,
            next_fire_at: r.next_fire_at,
            would_disable: true,
            via_groups: Vec::new(),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct Offboarding {
    pub schedules: Vec<OffboardingItem>,
    pub webhooks: Vec<OffboardingItem>,
    /// Queued or open peer tasks the person requested (`name` is "from → to").
    pub peer_tasks: Vec<OffboardingItem>,
    /// Personal API keys that would stop working.
    pub api_keys: i64,
}
