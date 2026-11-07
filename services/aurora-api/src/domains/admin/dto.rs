use crate::domains::auth::dto::User;
use aurora_db::repo::account::AdminUserRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Serialize, ToSchema)]
pub struct AdminUser {
    #[serde(flatten)]
    pub user: User,
    pub disabled_at: Option<DateTime<Utc>>,
    pub last_login_at: Option<DateTime<Utc>>,
}

impl From<AdminUserRow> for AdminUser {
    fn from(u: AdminUserRow) -> Self {
        Self {
            user: User {
                id: u.id,
                email: u.email,
                display_name: u.display_name,
                email_verified_at: u.email_verified_at,
                is_instance_admin: u.is_instance_admin,
                created_at: u.created_at,
            },
            disabled_at: u.disabled_at,
            last_login_at: u.last_login_at,
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListUsersQuery {
    /// Case-insensitive substring of email or display name.
    pub q: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct PatchAdminUserRequest {
    pub is_instance_admin: Option<bool>,
    pub disabled: Option<bool>,
}
