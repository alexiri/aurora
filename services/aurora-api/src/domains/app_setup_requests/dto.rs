use crate::domains::apps::setup::SetupNeed;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// One person's ask: their display name and where they asked from. Never an e-mail.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Asker {
    pub user_id: Uuid,
    pub display_name: String,
    pub workspace_id: Uuid,
    pub asked_at: DateTime<Utc>,
}

/// An organization's request for an app to be made ready (C-1030). There is no free text.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AppSetupRequest {
    pub id: Uuid,
    pub org_id: Uuid,
    pub app_id: String,
    pub app_name: String,
    /// `open` | `resolved` | `declined` | `withdrawn`.
    pub status: String,
    /// The askers; a person who is not an owner or admin of the organization sees only their own ask.
    pub askers: Vec<Asker>,
    /// What is missing (C-1029); empty when the app is switched off.
    pub needs: Vec<SetupNeed>,
    /// The app is switched off for the organization.
    pub unavailable: bool,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListQuery {
    /// `open` (default) | `resolved` | `declined` | `withdrawn`.
    pub status: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppSetupRequestList {
    pub items: Vec<AppSetupRequest>,
}

/// `AppSummary.readiness.requested` (C-1029): whether the caller asked, and how many people did.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Requested {
    pub by_me: bool,
    pub count: i64,
}
