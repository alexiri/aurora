use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Deserialize, ToSchema, Default)]
pub struct StartRequest {
    /// Client host name (sanitized; used to name the minted key).
    pub hostname: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct StartResponse {
    pub device_code: String,
    /// `ABCD-EFGH`.
    pub user_code: String,
    pub verification_url: String,
    pub interval: u32,
    pub expires_in: u32,
}

#[derive(Deserialize, ToSchema)]
pub struct TokenRequest {
    pub device_code: String,
}

#[derive(Serialize, ToSchema)]
pub struct CliWorkspace {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub org_id: Uuid,
    pub org_slug: String,
}

#[derive(Serialize, ToSchema)]
pub struct TokenResponse {
    /// Workspace API key (`auk_`); returned exactly once.
    pub api_key: String,
    pub workspace: CliWorkspace,
}

#[derive(Deserialize, ToSchema)]
pub struct ApproveRequest {
    pub user_code: String,
    pub workspace_id: Uuid,
}
