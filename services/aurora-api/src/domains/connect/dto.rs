use crate::domains::connections::dto::Connection;
use crate::domains::grants::dto::ConnectionGrant;
use crate::domains::grants::levels::Level;
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
    /// (FIX-133, D-240) `read` | `write`: a `read` connection allows only the app's read tools
    /// (the gateway enforces it). Absent = `write` for a new connection; a reconnect keeps the
    /// connection's current value.
    pub access: Option<String>,
    /// (P5-UX-11, C-957, C-990) For a new organization account: the limit no agent's access may
    /// exceed (`read` | `ask` | `act`); `access` follows it. Absent = derived from `access`.
    pub limit: Option<Level>,
    /// (FIX-136, D-241) For a `fields` method that declares scopes: the scopes the pasted token
    /// has ("Which scopes did you give this token?"). Absent = prefilled by the method's read
    /// probes. Each must be one of the method's scopes.
    pub scopes: Option<Vec<String>>,
    pub reconnect_connection_id: Option<Uuid>,
    /// A registered self-hosted instance of an instance-bound app (C-243). Absent = the app's
    /// `default_host`.
    pub instance_id: Option<Uuid>,
    /// (P5-UX-2, C-957, SEC-130) Give these agents access in the same request: the account and
    /// the access are written in one transaction (or, across an OAuth sign-in, at the callback).
    /// Workspace scope, new connections only.
    pub grant: Option<ConnectGrant>,
}

/// The access a connect gives (C-957): the agents and one level for all of them.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectGrant {
    /// 1 to `GRANT_BULK_MAX` distinct agents of the workspace; the caller must own the workspace.
    pub agent_ids: Vec<Uuid>,
    /// Expanded on the server from the app's tools (C-980); never client-supplied tool lists.
    pub level: Level,
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
        /// The access written with the connection (empty without a `grant`).
        grants: Vec<ConnectionGrant>,
    },
}
