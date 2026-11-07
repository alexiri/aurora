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

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectiveGovernance {
    pub llm: EffectiveLlm,
    pub egress: EffectiveEgress,
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

#[derive(Debug, Serialize, ToSchema)]
pub struct InstanceGovernance {
    pub llm: InstanceLlm,
    pub egress: InstanceEgress,
}
