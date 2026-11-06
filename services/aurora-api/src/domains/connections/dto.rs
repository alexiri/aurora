use aurora_db::repo::connections::ConnectionRow;
use aurora_registry::snapshot::{ClaimStatus, Snapshot};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectedBy {
    pub id: Uuid,
    pub display_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Injecting {
    pub host: String,
    pub lost_to_app_id: Option<String>,
}

/// A connection. Credentials are never returned (SEC-12).
#[derive(Debug, Serialize, ToSchema)]
pub struct Connection {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    /// `organization` | `workspace`.
    pub scope: String,
    pub app_id: String,
    pub package_id: String,
    pub app_version: String,
    pub label: String,
    /// `active` | `reauth_required` | `disabled` | `orphaned`.
    pub status: String,
    pub status_reason: Option<String>,
    pub scopes: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub tenant_host: Option<String>,
    pub oauth_client_id: Option<Uuid>,
    pub credentials_expires_at: Option<DateTime<Utc>>,
    pub connected_by: Option<ConnectedBy>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub injecting: Vec<Injecting>,
}

impl Connection {
    pub fn new(r: ConnectionRow, snap: Option<&Snapshot>) -> Self {
        let metadata = r
            .metadata
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        let injecting = match (snap, r.status.as_str()) {
            (Some(s), "active" | "reauth_required") => injecting(s, &r.app_id),
            _ => Vec::new(),
        };
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
            app_id: r.app_id,
            package_id: r.package_id,
            app_version: r.app_version,
            label: r.label,
            status: r.status,
            status_reason: r.status_reason,
            scopes: r.scopes,
            metadata,
            tenant_host: r.tenant_host,
            oauth_client_id: r.oauth_client_id,
            credentials_expires_at: r.credentials_expires_at,
            connected_by: r.connected_by.map(|id| ConnectedBy {
                id,
                display_name: r.connected_by_name.unwrap_or_default(),
            }),
            last_used_at: r.last_used_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
            injecting,
        }
    }
}

/// Hosts the app's rules cover; lost claims name the winning app when one claims the same
/// host and path prefix.
fn injecting(snap: &Snapshot, app_id: &str) -> Vec<Injecting> {
    snap.hosts
        .iter()
        .filter(|h| h.app_id == app_id)
        .map(|h| Injecting {
            host: h.host.clone(),
            lost_to_app_id: (h.claim_status == ClaimStatus::Lost)
                .then(|| {
                    snap.hosts
                        .iter()
                        .find(|o| {
                            o.claim_status == ClaimStatus::Active
                                && o.host == h.host
                                && o.path_prefix == h.path_prefix
                                && o.app_id != app_id
                        })
                        .map(|o| o.app_id.clone())
                })
                .flatten(),
        })
        .collect()
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    pub app_id: Option<String>,
    /// `active` | `reauth_required` | `disabled` | `orphaned`.
    pub status: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchConnectionRequest {
    pub label: Option<String>,
    /// `active` | `disabled`.
    pub status: Option<String>,
}
