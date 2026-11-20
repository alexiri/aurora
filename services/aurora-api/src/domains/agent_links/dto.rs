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
    /// (C-1085) The peer's workspace name.
    pub workspace_name: String,
    pub policy: LinkPolicy,
}

/// (C-1085) Who may hand tasks to whom, read from the asked agent's side.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LinkDirection {
    /// Either agent may hand the other a task.
    Both,
    /// This agent hands tasks to the peer; the peer only works on what it sends.
    Outgoing,
    /// The peer hands tasks to this agent.
    Incoming,
}

impl LinkDirection {
    pub fn parse(s: &str) -> Self {
        match s {
            "outgoing" => Self::Outgoing,
            "incoming" => Self::Incoming,
            _ => Self::Both,
        }
    }
}

/// (C-1085) The direction a person asks for, read from the agent named by the route or body.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkDirectionRequest {
    Both,
    ToPeer,
    FromPeer,
}

impl From<LinkDirectionRequest> for aurora_db::repo::agent_links::SideDirection {
    fn from(d: LinkDirectionRequest) -> Self {
        match d {
            LinkDirectionRequest::Both => Self::Both,
            LinkDirectionRequest::ToPeer => Self::ToPeer,
            LinkDirectionRequest::FromPeer => Self::FromPeer,
        }
    }
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LinkCreatorKind {
    Person,
    Agent,
}

/// (C-1085) Who created the link.
#[derive(Serialize, ToSchema)]
pub struct LinkCreator {
    pub kind: LinkCreatorKind,
    /// The person's display name; null for an agent, or once the person is deleted.
    pub display_name: Option<String>,
}

/// A link seen from one of its two agents.
#[derive(Serialize, ToSchema)]
pub struct AgentLink {
    pub id: Uuid,
    pub revision: i64,
    #[serde(rename = "self")]
    pub self_side: LinkSelf,
    pub peer: LinkPeer,
    /// (C-1085) Who may hand tasks to whom, from this agent's side.
    pub direction: LinkDirection,
    pub created_by: LinkCreator,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// `POST /v1/agents/{agent}/links` (C-1085).
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateLinkBody {
    pub peer_agent_id: Uuid,
    /// Read from `{agent}`'s side.
    pub direction: LinkDirectionRequest,
}

/// `PUT /v1/agent-links/{id}/direction` (C-1085).
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetDirectionBody {
    /// The agent whose side `direction` is read from (one of the link's two agents).
    pub agent_id: Uuid,
    pub direction: LinkDirectionRequest,
    /// The `revision` the caller last saw (409 `stale_revision` on mismatch).
    pub revision: i64,
}

/// A candidate of `GET /v1/agents/{agent}/linkable` (C-1085): nothing else about the agent.
#[derive(Serialize, ToSchema)]
pub struct LinkableAgent {
    pub agent_id: Uuid,
    pub name: String,
    pub handle: String,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    /// A link with this agent exists already.
    pub linked: bool,
    /// False for a BYO agent (not linkable).
    pub hosted: bool,
    /// The agent is paused by an administrator (not linkable).
    pub frozen: bool,
}

#[derive(Serialize, ToSchema)]
pub struct LinkableAgentList {
    pub items: Vec<LinkableAgent>,
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
