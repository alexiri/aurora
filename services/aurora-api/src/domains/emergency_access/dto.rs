//! DTOs of emergency read access (FIX-236, C-1164). Responses carry metadata only — never
//! conversation content (C-1165: "the open/list/end routes return metadata only").

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, ToSchema)]
pub struct OpenEmergencyAccessRequest {
    /// Why the access is needed; 10–500 characters (C-1164).
    pub reason: String,
    /// How long it stays open, in hours: 1–24 (default 4). It can never be extended (D-478(d)):
    /// a new grant is opened instead, and that notifies again.
    pub hours: Option<i64>,
}

/// One emergency access row (C-1164). `active` is `ended_at IS NULL AND expires_at > now()`,
/// evaluated at answer time.
#[derive(Debug, Serialize, ToSchema)]
pub struct EmergencyAccess {
    pub id: Uuid,
    pub workspace: EmergencyAccessWorkspace,
    pub by: EmergencyAccessBy,
    pub reason: String,
    pub opened_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub end_reason: Option<String>,
    pub active: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EmergencyAccessWorkspace {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EmergencyAccessBy {
    pub id: Uuid,
    pub display_name: String,
}
