use crate::domains::connections::dto::ConnectedBy;
use aurora_db::repo::app_instances::InstanceRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// A self-hosted app instance: a host an admin registered for one app (C-240, D-74).
#[derive(Debug, Serialize, ToSchema)]
pub struct AppInstance {
    pub id: Uuid,
    /// `null` = instance-wide.
    pub org_id: Option<Uuid>,
    pub package_id: String,
    pub app_id: String,
    pub host: String,
    pub label: String,
    /// `active` | `disabled`.
    pub status: String,
    pub created_by: Option<ConnectedBy>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Connections bound to this instance.
    pub connections: i64,
}

impl From<InstanceRow> for AppInstance {
    fn from(r: InstanceRow) -> Self {
        Self {
            id: r.id,
            org_id: r.org_id,
            package_id: r.package_id,
            app_id: r.app_id,
            host: r.host,
            label: r.label,
            status: r.status,
            created_by: r.created_by.map(|id| ConnectedBy {
                id,
                display_name: r.created_by_name.unwrap_or_default(),
            }),
            created_at: r.created_at,
            updated_at: r.updated_at,
            connections: r.connections,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    pub app_id: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAppInstanceRequest {
    pub app_id: String,
    /// A lower-case DNS name: no port, path, userinfo or IP literal.
    pub host: String,
    pub label: String,
}

/// The host is immutable: register a new instance instead.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchAppInstanceRequest {
    pub label: Option<String>,
    /// `active` | `disabled`.
    pub status: Option<String>,
}
