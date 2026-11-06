use aurora_db::repo::oauth_clients::ClientRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// An OAuth client. `fields` carries non-secret fields only; secret values are sealed.
#[derive(Debug, Serialize, ToSchema)]
pub struct OAuthClient {
    pub id: Uuid,
    /// `instance` | `organization` | `workspace`.
    pub scope: String,
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub package_id: String,
    pub client_key: String,
    pub fields: BTreeMap<String, String>,
    pub enabled: bool,
    pub connection_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ClientRow> for OAuthClient {
    fn from(r: ClientRow) -> Self {
        let scope = match (r.org_id, r.workspace_id) {
            (None, _) => "instance",
            (Some(_), None) => "organization",
            (Some(_), Some(_)) => "workspace",
        };
        Self {
            id: r.id,
            scope: scope.into(),
            org_id: r.org_id,
            workspace_id: r.workspace_id,
            package_id: r.package_id,
            client_key: r.client_key,
            fields: r
                .fields
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                        .collect()
                })
                .unwrap_or_default(),
            enabled: r.enabled,
            connection_count: r.connection_count,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateOAuthClientRequest {
    pub package_id: String,
    pub client_key: String,
    /// Values for every field the manifest declares, secret ones included (sealed on write).
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchOAuthClientRequest {
    /// Fields to replace; omitted fields keep their value.
    pub fields: Option<BTreeMap<String, String>>,
    pub enabled: Option<bool>,
}
