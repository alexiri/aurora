use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LinkPolicy {
    Ask,
    Allow,
    Blocked,
}

impl LinkPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Allow => "allow",
            Self::Blocked => "blocked",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "allow" => Self::Allow,
            "blocked" => Self::Blocked,
            _ => Self::Ask,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct LinkSelf {
    pub agent_id: Uuid,
    pub policy: LinkPolicy,
}

#[derive(Serialize, ToSchema)]
pub struct LinkPeer {
    pub agent_id: Uuid,
    pub handle: String,
    pub name: String,
    pub workspace_id: Uuid,
    pub policy: LinkPolicy,
}

/// A link seen from one of its two agents.
#[derive(Serialize, ToSchema)]
pub struct AgentLink {
    pub id: Uuid,
    pub revision: i64,
    #[serde(rename = "self")]
    pub self_side: LinkSelf,
    pub peer: LinkPeer,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct AgentLinkList {
    pub items: Vec<AgentLink>,
}

#[derive(Deserialize, ToSchema)]
pub struct SetPolicyBody {
    /// The agent whose side is changed.
    pub agent_id: Uuid,
    pub policy: LinkPolicy,
    /// The `revision` the caller last saw (409 `stale_revision` on mismatch).
    pub revision: i64,
}
