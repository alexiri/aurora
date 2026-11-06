use aurora_db::repo::secrets::SecretRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// Injection recipe (`08 §5`).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InjectionRecipe {
    Bearer,
    Header {
        name: String,
        #[serde(default = "value_fmt")]
        format: String,
    },
    Query {
        name: String,
        #[serde(default = "value_fmt")]
        format: String,
    },
    Basic {
        username: String,
    },
    PathTemplate {
        prefix: String,
    },
    PathRegex {
        pattern: String,
        replacement: String,
    },
}

fn value_fmt() -> String {
    "{value}".to_owned()
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StoredOrigin {
    /// `verified` | `mismatch` | `unverifiable` | `pending`.
    pub status: String,
    /// `endpoint` | `litellm_team` | `provider_account`.
    pub method: Option<String>,
    pub organization_id: Option<String>,
    pub workspace_id: Option<String>,
    pub team_id: Option<String>,
    pub user_id: Option<String>,
    pub value_version: i64,
    pub checked_at: Option<DateTime<Utc>>,
}

impl From<aurora_llm::governance::StoredOrigin> for StoredOrigin {
    fn from(o: aurora_llm::governance::StoredOrigin) -> Self {
        Self {
            status: serde_json::to_value(o.status)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            method: o.method,
            organization_id: o.organization_id,
            workspace_id: o.workspace_id,
            team_id: o.team_id,
            user_id: o.user_id,
            value_version: o.value_version,
            checked_at: o
                .checked_at
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|t| t.with_timezone(&Utc)),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SecretLlm {
    pub provider: String,
    pub endpoint_id: String,
    pub auth_mode: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SecretReference {
    pub store_id: String,
    pub path: String,
    pub field: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedBy {
    pub id: Uuid,
    pub display_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Secret {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    /// `organization` | `workspace`.
    pub scope: String,
    pub name: String,
    /// `generic` | `llm`.
    pub kind: String,
    pub llm: Option<SecretLlm>,
    pub host_pattern: String,
    pub path_pattern: Option<String>,
    pub injection: InjectionRecipe,
    /// `managed` | `reference`.
    pub value_source: String,
    pub reference: Option<SecretReference>,
    /// `active` | `proposed` | `rejected`.
    pub status: String,
    pub origin: Option<StoredOrigin>,
    pub value_version: i32,
    pub proposed_by_agent_id: Option<Uuid>,
    pub created_by: Option<CreatedBy>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<SecretRow> for Secret {
    fn from(r: SecretRow) -> Self {
        let reference = (r.value_source == "reference")
            .then(|| match r.value_ref.parse::<aurora_secrets::SecretRef>() {
                Ok(aurora_secrets::SecretRef::Ext {
                    store_id,
                    path,
                    field,
                    ..
                }) => Some(SecretReference {
                    store_id,
                    path,
                    field,
                }),
                _ => None,
            })
            .flatten();
        Self {
            id: r.id,
            org_id: r.org_id,
            workspace_id: r.workspace_id,
            scope: if r.workspace_id.is_some() {
                "workspace"
            } else {
                "organization"
            }
            .into(),
            name: r.name,
            llm: match (r.llm_provider, r.llm_endpoint_id) {
                (Some(provider), Some(endpoint_id)) => Some(SecretLlm {
                    provider,
                    endpoint_id,
                    auth_mode: r.llm_auth_mode.unwrap_or_else(|| "api_key".into()),
                }),
                _ => None,
            },
            kind: r.kind,
            host_pattern: r.host_pattern,
            path_pattern: r.path_pattern,
            injection: serde_json::from_value(r.injection).unwrap_or(InjectionRecipe::Bearer),
            value_source: r.value_source,
            reference,
            status: r.status,
            origin: r
                .origin
                .and_then(|o| {
                    serde_json::from_value::<aurora_llm::governance::StoredOrigin>(o).ok()
                })
                .map(Into::into),
            value_version: r.value_version,
            proposed_by_agent_id: r.proposed_by_agent_id,
            created_by: r.created_by.map(|id| CreatedBy {
                id,
                display_name: r.created_by_name.unwrap_or_default(),
            }),
            last_used_at: r.last_used_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LlmSpec {
    pub endpoint_id: String,
    /// Defaults to `api_key`.
    pub auth_mode: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSecretRequest {
    pub name: String,
    /// `generic` | `llm`.
    pub kind: String,
    pub llm: Option<LlmSpec>,
    pub host_pattern: Option<String>,
    pub path_pattern: Option<String>,
    pub injection: Option<InjectionRecipe>,
    /// Managed value (exactly one of `value` / `reference`).
    pub value: Option<String>,
    pub reference: Option<SecretReference>,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchSecretRequest {
    pub name: Option<String>,
    pub host_pattern: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub path_pattern: Option<Option<String>>,
    pub injection: Option<InjectionRecipe>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetValueRequest {
    pub value: Option<String>,
    pub reference: Option<SecretReference>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptRequest {
    pub value: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// `generic` | `llm`.
    pub kind: Option<String>,
    /// `active` | `proposed` | `rejected` (default: all but rejected).
    pub status: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct StoreCaps {
    pub managed: bool,
    pub references: bool,
}

#[derive(Serialize, ToSchema)]
pub struct SecretStoreInfo {
    pub id: String,
    pub kind: String,
    pub capabilities: StoreCaps,
}

#[derive(Serialize, ToSchema)]
pub struct SecretStores {
    pub items: Vec<SecretStoreInfo>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ProvidersQuery {
    /// Include the caller's effective governance for this organization.
    pub org_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
pub struct LlmProviderInfo {
    pub id: String,
    pub display_name: String,
    pub auth_modes: Vec<String>,
    pub dialects: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct LlmEndpointInfo {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub base_url: String,
    pub dialects: Vec<String>,
    pub origin_method: Option<String>,
    pub permitted: Option<bool>,
    pub key_scopes: Option<Vec<String>>,
}

#[derive(Serialize, ToSchema)]
pub struct LlmProviders {
    pub providers: Vec<LlmProviderInfo>,
    pub endpoints: Vec<LlmEndpointInfo>,
}

#[derive(Serialize, ToSchema)]
pub struct ModelItem {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct Models {
    pub items: Vec<ModelItem>,
}

#[derive(Serialize, ToSchema)]
pub struct VerifyResult {
    pub origin: StoredOrigin,
}
