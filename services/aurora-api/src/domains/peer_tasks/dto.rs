use aurora_db::repo::peer_tasks::PeerTaskRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct PeerTaskRequester {
    pub id: Uuid,
    pub display_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct PeerTask {
    pub id: Uuid,
    pub link_id: Uuid,
    pub from_agent_id: Uuid,
    pub to_agent_id: Uuid,
    /// `outgoing` when the listed agent requested the task, `incoming` when it received it.
    pub direction: String,
    pub requester: Option<PeerTaskRequester>,
    /// Null when the caller cannot see that (direct) conversation; see `conversation_hidden`.
    pub origin_conversation_id: Option<Uuid>,
    /// True when `origin_conversation_id` was redacted (phase 1, RFE-009).
    pub conversation_hidden: bool,
    pub task_conversation_id: Uuid,
    /// `queued` | `open` | `completed` | `cancelled` | `expired`
    pub status: String,
    pub messages_used: i32,
    pub message_budget: i32,
    pub report: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl PeerTask {
    /// View relative to `agent` (one of the task's two agents).
    pub fn from_row(r: PeerTaskRow, agent: Uuid) -> Self {
        Self {
            direction: if r.from_agent_id == agent {
                "outgoing"
            } else {
                "incoming"
            }
            .to_owned(),
            requester: r.requester_user_id.map(|id| PeerTaskRequester {
                id,
                display_name: r.requester_name.unwrap_or_default(),
            }),
            id: r.id,
            link_id: r.link_id,
            from_agent_id: r.from_agent_id,
            to_agent_id: r.to_agent_id,
            origin_conversation_id: Some(r.origin_conversation_id),
            conversation_hidden: false,
            task_conversation_id: r.task_conversation_id,
            status: r.status,
            messages_used: r.messages_used,
            message_budget: r.message_budget,
            report: r.report,
            expires_at: r.expires_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

pub const STATUSES: [&str; 5] = ["queued", "open", "completed", "cancelled", "expired"];

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// `queued` | `open` | `completed` | `cancelled` | `expired`
    pub status: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}
