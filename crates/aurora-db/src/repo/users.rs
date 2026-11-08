use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub email: String,
    pub email_verified_at: Option<DateTime<Utc>>,
    pub display_name: String,
    pub is_instance_admin: bool,
    pub disabled_at: Option<DateTime<Utc>>,
    pub default_org_id: Option<Uuid>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, email::text AS email, email_verified_at, display_name, \
    is_instance_admin, disabled_at, default_org_id, last_login_at, created_at, updated_at";

pub async fn find_by_id<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<UserRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM users WHERE id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// Case-insensitive (citext) lookup.
pub async fn find_by_email<'e, E: PgExecutor<'e>>(
    exec: E,
    email: &str,
) -> Result<Option<UserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM users WHERE email = $1::citext"
    ))
    .bind(email.trim())
    .fetch_optional(exec)
    .await?)
}

pub async fn set_default_org<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org: Uuid,
) -> Result<(), DbError> {
    sqlx::query("UPDATE users SET default_org_id = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(org)
        .execute(exec)
        .await?;
    Ok(())
}

/// Set `email_verified_at` if unset; returns the updated user.
pub async fn mark_email_verified<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<UserRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE users SET email_verified_at = COALESCE(email_verified_at, now()), updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    ))
    .bind(id)
    .fetch_one(exec)
    .await?)
}

/// Stamp `last_login_at` (churn column: `updated_at` untouched); returns the fresh row.
pub async fn record_login<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<UserRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE users SET last_login_at = now() WHERE id = $1 RETURNING {COLS}"
    ))
    .bind(id)
    .fetch_one(exec)
    .await?)
}

/// Grant (never revoke) the instance-admin flag; returns the fresh row.
pub async fn grant_instance_admin<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<UserRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE users SET is_instance_admin = true, updated_at = now() WHERE id = $1 RETURNING {COLS}"
    ))
    .bind(id)
    .fetch_one(exec)
    .await?)
}
