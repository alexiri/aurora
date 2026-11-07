use aurora_db::repo::api_keys::ApiKeyRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct KeyOwner {
    pub id: Uuid,
    pub display_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct ApiKey {
    pub id: Uuid,
    pub name: String,
    /// `user` | `service`.
    pub kind: String,
    pub org_id: Uuid,
    /// `null` for organization keys (`auo_`).
    pub workspace_id: Option<Uuid>,
    pub display_prefix: String,
    pub owner: KeyOwner,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<ApiKeyRow> for ApiKey {
    fn from(r: ApiKeyRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            kind: r.kind,
            org_id: r.org_id,
            workspace_id: r.workspace_id,
            display_prefix: r.display_prefix,
            owner: KeyOwner {
                id: r.user_id,
                display_name: r.owner_display_name,
            },
            last_used_at: r.last_used_at,
            expires_at: r.expires_at,
            revoked_at: r.revoked_at,
            created_at: r.created_at,
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct CreateKeyRequest {
    /// 1-80 characters.
    pub name: String,
    /// Must be in the future.
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, ToSchema)]
pub struct CreatedKey {
    pub api_key: ApiKey,
    /// The token; shown only in this response.
    pub key: String,
}
