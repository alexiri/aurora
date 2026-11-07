use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// One allow-list row; a null workspace allows the app for the whole organization.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AllowEntry {
    pub app_id: String,
    pub workspace_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AppAvailability {
    /// `open` | `restricted`.
    pub posture: String,
    pub allow: Vec<AllowEntry>,
}
