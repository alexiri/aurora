//! Serde models: the organization governance document (08 §11.2) and stored key origin (§11.3).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

/// LLM posture, ordered by strictness (`open` < `sanctioned_only`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum LlmPosture {
    #[default]
    Open,
    SanctionedOnly,
}

/// Egress posture, ordered by strictness (`open` < `credentialed_only`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EgressPosture {
    #[default]
    Open,
    CredentialedOnly,
}

impl LlmPosture {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "sanctioned_only" => Some(Self::SanctionedOnly),
            _ => None,
        }
    }
}

impl EgressPosture {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "credentialed_only" => Some(Self::CredentialedOnly),
            _ => None,
        }
    }
}

fn yes() -> bool {
    true
}

/// Normalized organization governance document (`organizations.governance`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct OrgGovernance {
    pub llm: OrgLlm,
    pub egress: OrgEgress,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct OrgLlm {
    pub posture: LlmPosture,
    /// Endpoints absent here are not permitted.
    pub endpoints: BTreeMap<String, OrgEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct OrgEndpoint {
    pub allowed: bool,
    /// Empty = the instance scopes.
    pub key_scopes: Vec<String>,
    /// Empty = the instance list for the endpoint (empty there = any).
    pub allowed_models: Vec<String>,
    pub origin: OrgOrigin,
}

impl Default for OrgEndpoint {
    fn default() -> Self {
        Self {
            allowed: yes(),
            key_scopes: vec![],
            allowed_models: vec![],
            origin: OrgOrigin::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct OrgOrigin {
    pub allowed_organization_ids: Vec<String>,
    pub allowed_workspace_ids: Vec<String>,
    pub allowed_team_ids: Vec<String>,
}

impl OrgOrigin {
    pub fn is_empty(&self) -> bool {
        self.allowed_organization_ids.is_empty()
            && self.allowed_workspace_ids.is_empty()
            && self.allowed_team_ids.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct OrgEgress {
    pub posture: EgressPosture,
    pub allowlist: Vec<AllowEntry>,
}

/// An egress allow-list entry in policy host/path pattern syntax.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = OrgAllowEntry)]
pub struct AllowEntry {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// One validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GovernanceError {
    /// `governance_invalid` or `governance_below_instance_minimum`.
    pub code: &'static str,
    /// Dotted path into the document, e.g. `llm.endpoints.anthropic.key_scopes[0]`.
    pub path: String,
    pub message: String,
}

pub const INVALID: &str = "governance_invalid";
pub const BELOW_MIN: &str = "governance_below_instance_minimum";

impl GovernanceError {
    pub(crate) fn new(code: &'static str, path: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
            message: msg.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginStatus {
    Verified,
    Mismatch,
    Unverifiable,
    Pending,
}

/// `secrets.origin` JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredOrigin {
    pub status: OriginStatus,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub value_version: i64,
    #[serde(default)]
    pub checked_at: Option<String>,
}

impl StoredOrigin {
    pub fn to_key_origin(&self) -> crate::KeyOrigin {
        crate::KeyOrigin {
            organization_id: self.organization_id.clone(),
            workspace_id: self.workspace_id.clone(),
            team_id: self.team_id.clone(),
            user_id: self.user_id.clone(),
        }
    }
}
