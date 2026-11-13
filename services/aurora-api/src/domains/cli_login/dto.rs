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

/// (FIX-108) The pending login as the approving session sees it.
#[derive(Serialize, ToSchema)]
pub struct PendingResponse {
    /// The host name the CLI gave (sanitized, **unverified**: anyone can claim any name).
    pub hostname: String,
    /// When the login was started.
    pub requested_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// Coarse address the request came from (IPv4 /24, IPv6 /48); absent when unknown.
    pub address: Option<String>,
    /// Lifetime of the key an approval creates, in days; absent = it never expires.
    pub key_expires_in_days: Option<u32>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PendingQuery {
    /// The `ABCD-EFGH` code shown by the CLI.
    pub code: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ApproveRequest {
    pub user_code: String,
    pub workspace_id: Uuid,
}
