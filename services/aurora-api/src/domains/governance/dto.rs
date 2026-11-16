use aurora_llm::governance::OrgGovernance;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema)]
pub struct GovernanceDocument {
    /// The `08 §11.2` organization governance document (normalized).
    #[schema(value_type = OrgGovernance)]
    pub document: Value,
    pub revision: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PutGovernance {
    #[schema(value_type = OrgGovernance)]
    pub document: Value,
    pub revision: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Sourced<T> {
    pub value: T,
    /// `instance` | `org`.
    pub source: String,
    /// (SET-2, C-783) The instance setting the value comes from, when `source` is `instance`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_setting: Option<InstanceSetting>,
}

/// Names an instance setting (`section` + registry `key`), so the dashboard can link it.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InstanceSetting {
    pub section: String,
    pub key: String,
}

impl InstanceSetting {
    /// Any section's setting (`retention`, `scim`, … — C-783).
    #[must_use]
    pub fn new(section: &str, key: &str) -> Self {
        Self {
            section: section.to_owned(),
            key: key.to_owned(),
        }
    }

    /// The `{ "instance_setting": { section, key } }` details of a refusal.
    #[must_use]
    pub fn details(section: &str, key: &str) -> serde_json::Value {
        serde_json::json!({ "instance_setting": { "section": section, "key": key } })
    }

    #[must_use]
    pub fn governance(key: &str) -> Self {
        Self {
            section: "governance".to_owned(),
            key: key.to_owned(),
        }
    }
}

impl<T> Sourced<T> {
    /// Name the instance setting when the value is the instance's (C-783).
    #[must_use]
    pub fn setting(mut self, key: &str) -> Self {
        self.instance_setting =
            (self.source == "instance").then(|| InstanceSetting::governance(key));
        self
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OriginRule {
    pub method: String,
    pub allowed_organization_ids: Vec<String>,
    pub allowed_workspace_ids: Vec<String>,
    pub allowed_team_ids: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveEndpoint {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub permitted: Sourced<bool>,
    pub key_scopes: Sourced<Vec<String>>,
    pub allowed_models: Sourced<Vec<String>>,
    pub origin_rule: Option<OriginRule>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveLlm {
    pub posture: Sourced<String>,
    pub endpoints: Vec<EffectiveEndpoint>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AllowlistEntry {
    pub host: String,
    pub path: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveEgress {
    pub posture: Sourced<String>,
    /// (RFE-003) effective open-lane posture: `enabled` | `allowlist_only` | `disabled`.
    pub open_lane: Sourced<String>,
    pub org_allowlist: Vec<AllowlistEntry>,
    pub baseline_allowlist: Vec<AllowlistEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NonCompliantSecret {
    pub id: Uuid,
    pub name: String,
    pub workspace_id: Option<Uuid>,
    /// `mismatch` | `unverifiable` | `not_permitted` | `pending`.
    pub reason: String,
}

/// (phase 2, RFE-025) Effective org-level BYO controls (instance ∩ org); the workspace layer is
/// applied per workspace/agent.
#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveByo {
    /// `enabled` | `owners_only` | `disabled`.
    pub posture: Sourced<String>,
    /// Default source CIDR list of BYO agents without their own; `null` = unrestricted.
    pub source_cidrs: Sourced<Option<Vec<String>>>,
    pub require_context_credentials: Sourced<bool>,
}

/// (phase 3, RFE-013) Effective BYO agent-token maximum age.
#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveAgentTokens {
    /// Days: the smaller of the instance and org values; `null` = no limit.
    pub max_age_days: Sourced<Option<u32>>,
}

/// (phase 3, RFE-013) The instance level of the agent-token maximum age.
#[derive(Debug, Serialize, ToSchema)]
pub struct InstanceAgentTokens {
    pub max_age_days: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveGovernance {
    pub llm: EffectiveLlm,
    pub egress: EffectiveEgress,
    pub byo: EffectiveByo,
    pub agent_tokens: EffectiveAgentTokens,
    pub non_compliant_secrets: Vec<NonCompliantSecret>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InstanceEndpoint {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub allowed_models: Vec<String>,
    pub key_origin: Option<OriginRule>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InstanceLlm {
    pub allowed_key_scopes: Vec<String>,
    pub minimum_posture: String,
    pub require_key_origin: bool,
    pub known_llm_hosts: Vec<String>,
    pub endpoints: Vec<InstanceEndpoint>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InstanceEgress {
    pub minimum_posture: String,
    /// (RFE-003) `enabled` | `allowlist_only` | `disabled`.
    pub open_lane_minimum: String,
    pub baseline_allowlist: Vec<AllowlistEntry>,
}

/// (phase 2, RFE-025) The instance level of BYO governance (`[byo]`).
#[derive(Debug, Serialize, ToSchema)]
pub struct InstanceByo {
    pub enabled: bool,
    /// `enabled` | `owners_only` | `disabled`.
    pub minimum_posture: String,
    pub default_source_cidrs: Vec<String>,
    pub require_context_credentials: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InstanceGovernance {
    pub llm: InstanceLlm,
    pub egress: InstanceEgress,
    pub byo: InstanceByo,
    pub agent_tokens: InstanceAgentTokens,
}
