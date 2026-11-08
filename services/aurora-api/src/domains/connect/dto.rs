use crate::domains::connections::dto::Connection;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectRequest {
    /// `organization` | `workspace`.
    pub scope: String,
    pub workspace_id: Option<Uuid>,
    /// Required for organization scope unless `reconnect_connection_id` is given.
    pub org_id: Option<Uuid>,
    pub method_id: String,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    pub oauth_client_id: Option<Uuid>,
    pub access_level: Option<String>,
    pub reconnect_connection_id: Option<Uuid>,
    /// A registered self-hosted instance of an instance-bound app (C-243). Absent = the app's
    /// `default_host`.
    pub instance_id: Option<Uuid>,
}

/// `redirect` (navigate the popup) or `connected`.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectResponse {
    Redirect {
        url: String,
    },
    Connected {
        connection: Box<Connection>,
        missing_scopes: Vec<String>,
    },
}
