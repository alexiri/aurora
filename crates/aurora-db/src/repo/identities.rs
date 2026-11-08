//! External (OIDC) identities linked to users.

use super::users::UserRow;
use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

const COLS: &str = "id, email::text AS email, email_verified_at, display_name, \
    is_instance_admin, disabled_at, default_org_id, last_login_at, created_at, updated_at";

/// The user linked to `(provider, subject)`, if any.
pub async fn find_user<'e, E: PgExecutor<'e>>(
    exec: E,
    provider: &str,
    subject: &str,
) -> Result<Option<UserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM users WHERE id =
           (SELECT user_id FROM user_identities WHERE provider_id = $1 AND subject = $2)"
    ))
    .bind(provider)
    .bind(subject)
    .fetch_optional(exec)
    .await?)
}

/// Link `(provider, subject)` to `user`. A duplicate is `DbError::UniqueViolation`
/// on `user_identities_provider_id_subject_key`.
pub async fn insert<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    provider: &str,
    subject: &str,
    email: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO user_identities (id, user_id, provider_id, subject, email_at_link)
         VALUES ($1, $2, $3, $4, $5::citext)",
    )
    .bind(Uuid::now_v7())
    .bind(user)
    .bind(provider)
    .bind(subject)
    .bind(email)
    .execute(exec)
    .await?;
    Ok(())
}

pub struct NewSsoUser<'a> {
    pub id: Uuid,
    /// Already trimmed and lower-cased.
    pub email: &'a str,
    pub display_name: &'a str,
    pub email_verified: bool,
    pub is_instance_admin: bool,
}

/// Insert a user without a password (SSO only).
pub async fn insert_user<'e, E: PgExecutor<'e>>(
    exec: E,
    u: &NewSsoUser<'_>,
) -> Result<UserRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO users (id, email, display_name, email_verified_at, is_instance_admin)
         VALUES ($1, $2::citext, $3, CASE WHEN $4 THEN now() END, $5)
         RETURNING {COLS}"
    ))
    .bind(u.id)
    .bind(u.email)
    .bind(u.display_name)
    .bind(u.email_verified)
    .bind(u.is_instance_admin)
    .fetch_one(exec)
    .await?)
}
