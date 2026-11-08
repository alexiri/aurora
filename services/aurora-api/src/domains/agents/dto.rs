use aurora_db::repo::agents::AgentRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct Agent {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub handle: String,
    pub name: String,
    /// `byo` | `hosted`.
    pub kind: String,
    pub is_default: bool,
    pub harness: Option<String>,
    pub model_provider: Option<String>,
    pub model_endpoint_id: Option<String>,
    pub model_id: Option<String>,
    /// `low` | `medium` | `high` | `max`.
    pub effort: Option<String>,
    /// `ask` | `allow` | `off`.
    pub proactive_messaging: String,
    pub brief: String,
    /// Root-relative public path of the avatar (`/v1/public/avatars/{key}`), or null.
    pub avatar_url: Option<String>,
    /// (phase 2, RFE-025) BYO governance of a BYO agent; `null` for hosted agents and in
    /// responses that do not load it.
    pub byo: Option<super::byo::ByoView>,
    /// (phase 2, RFE-006) The freeze that stops this agent (its own, its workspace's or its
    /// organization's; the broadest wins), `level` = where it is set; `null` when not frozen.
    pub frozen: Option<crate::domains::freeze::dto::Frozen>,
    /// (phase 2, RFE-006) Agent ∨ workspace ∨ organization is frozen.
    pub effective_frozen: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<AgentRow> for Agent {
    fn from(a: AgentRow) -> Self {
        Self {
            id: a.id,
            org_id: a.org_id,
            workspace_id: a.workspace_id,
            handle: a.handle,
            name: a.name,
            kind: a.kind,
            is_default: a.is_default,
            harness: a.harness,
            model_provider: a.model_provider,
            model_endpoint_id: a.model_endpoint_id,
            model_id: a.model_id,
            effort: a.effort,
            proactive_messaging: a.proactive_messaging,
            brief: a.brief,
            avatar_url: a
                .avatar_public_key
                .as_deref()
                .map(super::avatar::public_path),
            byo: None,
            frozen: None,
            effective_frozen: false,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentRequest {
    pub name: String,
    pub handle: Option<String>,
    /// `byo` | `hosted`.
    pub kind: String,
    pub brief: Option<String>,
    pub harness: Option<String>,
    pub model_endpoint_id: Option<String>,
    pub model_id: Option<String>,
    pub effort: Option<String>,
    pub proactive_messaging: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchAgentRequest {
    pub name: Option<String>,
    pub brief: Option<String>,
    pub harness: Option<String>,
    pub model_endpoint_id: Option<String>,
    pub model_id: Option<String>,
    pub effort: Option<String>,
    pub proactive_messaging: Option<String>,
    /// Instance admins only.
    #[schema(value_type = Option<Object>)]
    pub resource_overrides: Option<serde_json::Value>,
}

#[derive(Serialize, ToSchema)]
pub struct StubFile {
    pub path: String,
    pub content: String,
    pub mode: String,
}

#[derive(Serialize, ToSchema)]
pub struct ContainerConfig {
    /// Contains `<AGENT_TOKEN>` unless the token is revealed.
    pub proxy_url: String,
    pub env: BTreeMap<String, String>,
    pub ca_pem: String,
    pub ca_path: String,
    /// (phase 1, RFE-002) File holding the trust anchor for the HTTPS proxy leg (curl
    /// `--proxy-cacert`, git `http.proxySSLCAInfo`); same file as `ca_path`.
    pub proxy_ca_path: String,
    /// (phase 1, RFE-002) `"operator_certificate"` when the proxy listener presents an
    /// operator-supplied certificate (verify the outer leg with the system roots); `null` when
    /// the gateway CA issued it.
    pub proxy_tls: Option<String>,
    pub llm_env: BTreeMap<String, String>,
    pub stubs: Vec<StubFile>,
    /// (phase 2, RFE-025) Effective BYO controls, so the CLI can warn before the first request.
    pub byo: Option<ContainerByo>,
    /// (phase 2, RFE-025) Human-readable notes, e.g. that a context credential is required.
    pub notes: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ContainerByo {
    /// Effective posture: `enabled` | `owners_only` | `disabled`.
    pub posture: String,
    /// Effective source CIDR list; `null` = unrestricted.
    pub source_cidrs: Option<Vec<String>>,
    /// Effective require-context flag; when `true`, `proxy_url` carries `<CONTEXT_TOKEN>`.
    pub require_context: bool,
}

#[derive(Serialize, ToSchema)]
pub struct CreatedAgent {
    pub agent: Agent,
    /// BYO only; shown once.
    pub token: Option<String>,
    pub container_config: Option<ContainerConfig>,
}

#[derive(Serialize, ToSchema)]
pub struct AgentToken {
    pub token: String,
    pub container_config: ContainerConfig,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextCredentialRequest {
    pub label: String,
    pub conversation_id: Option<Uuid>,
    /// 60..=86400, default 3600.
    pub ttl_secs: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct ContextCredential {
    pub credential: String,
    pub expires_at: DateTime<Utc>,
    pub proxy_url: String,
}
