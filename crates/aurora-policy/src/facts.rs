//! Request facts (07 §Rule anatomy).
use crate::git::GitOp;
use crate::graphql::{self, GraphqlKind};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Scope level a credential lives at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Workspace,
    Organization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    Generic,
    Llm,
}

/// A credential candidate / the winning credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    Connection {
        id: Uuid,
        app_id: String,
        scope: Level,
        /// (P4-GRANTS, D-123) The app's manifest says `grants.custom_only`: a target without
        /// `tools` neither allows nor selects this connection (07 §App-level restrictions).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        custom_only: bool,
        /// (P4-GRANTS, D-123) The app's manifest says `grants.block_untooled`: a request on
        /// which this connection wins and that matches no catalogue tool is blocked.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        block_untooled: bool,
    },
    Secret {
        id: Uuid,
        kind: SecretKind,
        scope: Level,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialKey {
    Connection(Uuid),
    Secret(Uuid),
}

impl Credential {
    pub fn key(&self) -> CredentialKey {
        match self {
            Self::Connection { id, .. } => CredentialKey::Connection(*id),
            Self::Secret { id, .. } => CredentialKey::Secret(*id),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestIdentity {
    pub agent_id: Uuid,
    /// (phase 2, RFE-005, D-49) The person whose turn made the request: set only for hosted
    /// context credentials of person-authored turns whose speaker still holds `can_use`.
    /// `None` (boot, agent, BYO context, schedule/webhook/…, tunnels) means `user` identities
    /// never match (07 §Rule anatomy).
    #[serde(default)]
    pub speaker_user_id: Option<Uuid>,
    /// (phase 4, P4-GRP-1, D-131) The ids of the speaker's groups of the agent's org (the
    /// principal set is `{agent_id} ∪ speaker_group_ids`). Empty without a speaker.
    #[serde(default)]
    pub speaker_group_ids: Vec<Uuid>,
}

/// Classification tags declared by the matched tool (inputs only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolTags {
    #[serde(default)]
    pub graphql: bool,
    #[serde(default)]
    pub git: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestFacts {
    pub host: String,
    pub port: u16,
    /// Normalized path (no query).
    pub path: String,
    #[serde(default)]
    pub query: Option<String>,
    pub method: String,
    #[serde(default)]
    pub body_prefix: Vec<u8>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub tool_id: Option<String>,
    #[serde(default)]
    pub tool_tags: ToolTags,
    #[serde(default)]
    pub winning_credential: Option<Credential>,
    #[serde(default)]
    pub is_llm_host: bool,
    pub identity: RequestIdentity,
    /// Open-lane tunnel judged at `CONNECT` time (07 §CONNECT-time evaluation,
    /// RFE-003): only host, port and identity are knowable and nothing is injected.
    #[serde(default)]
    pub connect_time: bool,
    /// Apps whose snapshot host index serves `host` (used by `app` targets at
    /// `CONNECT` time, where no tool is known).
    #[serde(default)]
    pub host_apps: Vec<String>,
}

impl RequestFacts {
    /// `injected` = a winning credential is present.
    pub fn injected(&self) -> bool {
        self.winning_credential.is_some()
    }

    /// GraphQL kind if the tool is graphql-tagged (fail-closed classification).
    pub fn graphql_kind(&self) -> Option<GraphqlKind> {
        self.tool_tags
            .graphql
            .then(|| graphql::classify(&self.body_prefix, self.truncated))
    }

    /// Git operation if the tool is git-tagged.
    pub fn git_op(&self) -> Option<GitOp> {
        if self.tool_tags.git {
            crate::git::classify(&self.path, self.query.as_deref())
        } else {
            None
        }
    }
}
