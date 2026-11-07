use aurora_db::repo::action_approvals::ActionApprovalRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

pub const STATUSES: [&str; 6] = [
    "pending", "approved", "executed", "failed", "rejected", "expired",
];

#[derive(Serialize, ToSchema)]
pub struct ActionApprovalDecider {
    pub id: Uuid,
    pub display_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct ActionApproval {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    /// Null when the approval belongs to another workspace than its conversation (the decider
    /// never learns that conversation's id), see `conversation_hidden`.
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    /// True when `conversation_id`/`turn_id` were redacted (phase 1, RFE-009, D-47 clarification C).
    pub conversation_hidden: bool,
    pub action_key: String,
    /// Platform-composed one-liner; never raw model text.
    pub summary: String,
    /// `pending` | `approved` | `executed` | `failed` | `rejected` | `expired`
    pub status: String,
    pub decided_by: Option<ActionApprovalDecider>,
    pub decided_at: Option<DateTime<Utc>>,
    pub executed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl From<ActionApprovalRow> for ActionApproval {
    fn from(r: ActionApprovalRow) -> Self {
        // Clarification C of D-47: a decider in another workspace than the conversation sees only
        // the platform-composed summary, never the conversation or turn id.
        let foreign =
            r.conversation_id.is_some() && r.conversation_workspace_id != Some(r.workspace_id);
        let (conversation_id, turn_id) = if foreign {
            (None, None)
        } else {
            (r.conversation_id, r.turn_id)
        };
        Self {
            id: r.id,
            workspace_id: r.workspace_id,
            agent_id: r.agent_id,
            conversation_id,
            turn_id,
            conversation_hidden: foreign,
            action_key: r.action_key,
            summary: r.summary,
            status: r.status,
            decided_by: r.decided_by_user_id.map(|id| ActionApprovalDecider {
                id,
                display_name: r.decided_by_name.unwrap_or_default(),
            }),
            decided_at: r.decided_at,
            executed_at: r.executed_at,
            error: r.error,
            expires_at: r.expires_at,
            created_at: r.created_at,
        }
    }
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ActionDecision {
    Approve,
    Deny,
}

#[derive(Deserialize, ToSchema)]
pub struct ActionDecisionBody {
    pub decision: ActionDecision,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// `pending` | `approved` | `executed` | `failed` | `rejected` | `expired`
    pub status: Option<String>,
    pub agent_id: Option<Uuid>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}
