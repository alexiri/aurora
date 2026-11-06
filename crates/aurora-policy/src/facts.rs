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
    /// Users with `can_use` on the agent's workspace.
    #[serde(default)]
    pub principals: Vec<Uuid>,
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
