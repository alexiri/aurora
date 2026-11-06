use aurora_db::repo::users::UserRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub email_verified_at: Option<DateTime<Utc>>,
    pub is_instance_admin: bool,
    pub created_at: DateTime<Utc>,
}

impl From<UserRow> for User {
    fn from(u: UserRow) -> Self {
        Self {
            id: u.id,
            email: u.email,
            display_name: u.display_name,
            email_verified_at: u.email_verified_at,
            is_instance_admin: u.is_instance_admin,
            created_at: u.created_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct UserResponse {
    pub user: User,
}

#[derive(Deserialize, ToSchema)]
pub struct SignupRequest {
    pub email: String,
    pub password: String,
    pub display_name: String,
    #[serde(default)]
    pub invitation_token: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize, ToSchema)]
pub struct VerifyEmailRequest {
    pub token: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ForgotRequest {
    pub email: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ResetRequest {
    pub token: String,
    pub password: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Serialize, ToSchema)]
pub struct RevokedResponse {
    pub revoked: u64,
}
