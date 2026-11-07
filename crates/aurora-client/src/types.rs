//! Wire types mirroring `docs/API_CONTRACTS.md` (unknown fields are ignored).
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default = "default_expires")]
    pub expires_in: u64,
}
fn default_interval() -> u64 {
    5
}
fn default_expires() -> u64 {
    600
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRef {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub org_id: String,
    pub org_slug: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CliToken {
    pub api_key: String,
    pub workspace: WorkspaceRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgSummary {
    pub id: String,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeWorkspace {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub my_role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Membership {
    pub org: OrgSummary,
    pub role: String,
    pub status: String,
    pub workspaces: Vec<MeWorkspace>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// `GET /v1/me`. API keys may see a reduced shape, so most fields default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Me {
    pub user: User,
    #[serde(default)]
    pub default_org_id: Option<String>,
    #[serde(default)]
    pub memberships: Vec<Membership>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: String,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Resolved {
    pub org: OrgSummary,
    pub workspace: Option<WorkspaceSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub handle: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub workspace_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Stub {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerConfig {
    pub proxy_url: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub ca_pem: String,
    #[serde(default)]
    pub ca_path: String,
    /// (phase 1, RFE-002) Trust anchor file for the HTTPS proxy leg.
    #[serde(default)]
    pub proxy_ca_path: String,
    /// (phase 1, RFE-002) `"operator_certificate"`: verify the proxy leg with the system roots.
    #[serde(default)]
    pub proxy_tls: Option<String>,
    #[serde(default)]
    pub llm_env: BTreeMap<String, String>,
    #[serde(default)]
    pub stubs: Vec<Stub>,
    /// (phase 2, RFE-025) Effective BYO controls of the agent.
    #[serde(default)]
    pub byo: Option<ContainerByo>,
    /// (phase 2, RFE-025) Notes for the operator, e.g. that a context credential is required.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// (phase 2, RFE-025) Effective BYO controls in a container config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerByo {
    pub posture: String,
    /// `null` = unrestricted.
    #[serde(default)]
    pub source_cidrs: Option<Vec<String>>,
    #[serde(default)]
    pub require_context: bool,
}

/// (phase 2, RFE-025) `POST /v1/agent/context-credentials` response.
#[derive(Debug, Clone, Deserialize)]
pub struct ContextCredential {
    pub credential: String,
    pub expires_at: String,
    pub proxy_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentToken {
    pub token: String,
    #[serde(default)]
    pub container_config: Option<ContainerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalAgent {
    pub id: String,
    pub name: String,
    pub handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryField {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalSummary {
    pub title: String,
    #[serde(default)]
    pub fields: Vec<SummaryField>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    #[serde(default)]
    pub org_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub agent: Option<ApprovalAgent>,
    pub method: String,
    pub host: String,
    pub path: String,
    #[serde(default)]
    pub rule_name: Option<String>,
    pub summary: ApprovalSummary,
    pub status: String,
    #[serde(default)]
    pub deadline: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Deny,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Approve => "approve",
            Decision::Deny => "deny",
        }
    }
}
