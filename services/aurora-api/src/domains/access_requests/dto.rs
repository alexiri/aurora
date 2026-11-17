use aurora_db::repo::access_requests::RequestRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// An agent's request for an app (or a proposed website's token). Never carries access.
#[derive(Serialize, ToSchema)]
pub struct AccessRequest {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub agent_name: String,
    pub agent_handle: String,
    pub conversation_id: Uuid,
    /// `app` | `website`
    pub kind: String,
    /// For `app`.
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    /// For `website`: the proposed secret and its host.
    pub secret_id: Option<Uuid>,
    pub host: Option<String>,
    /// The agent's own words: one line, at most 300 characters. Show it as such.
    pub reason: String,
    /// `open` | `given` | `dismissed` | `expired`
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize, IntoParams)]
pub struct ListQuery {
    /// `open` (default) | `given` | `dismissed` | `expired`
    pub status: Option<String>,
}

pub const STATUSES: [&str; 4] = ["open", "given", "dismissed", "expired"];

impl AccessRequest {
    pub fn new(
        r: RequestRow,
        agent_name: String,
        agent_handle: String,
        app_name: Option<String>,
        host: Option<String>,
    ) -> Self {
        Self {
            id: r.id,
            agent_id: r.agent_id,
            agent_name,
            agent_handle,
            conversation_id: r.conversation_id,
            kind: r.kind,
            app_id: r.app_id,
            app_name,
            secret_id: r.secret_id,
            host,
            reason: r.reason,
            status: r.status,
            created_at: r.created_at,
        }
    }
}
