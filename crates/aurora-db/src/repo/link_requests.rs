//! Platform-user link requests (docs/13 §4 `link-start`; schema `link_requests`).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// Insert a request whose token hash is `token_hash` (the hint only pre-fills sign-in); returns `(id, expires_at)`.
pub async fn insert<'e, E: PgExecutor<'e>>(
    exec: E,
    integration_id: Uuid,
    platform_user_ref: &str,
    token_hash: &[u8],
    ttl_secs: f64,
    platform_email_hint: Option<&str>,
) -> Result<(Uuid, DateTime<Utc>), DbError> {
    Ok(sqlx::query_as(
        "INSERT INTO link_requests (id, integration_id, platform_user_ref, token_hash, expires_at,
                                    platform_email_hint)
         VALUES ($1,$2,$3,$4, now() + make_interval(secs => $5), $6)
         RETURNING id, expires_at",
    )
    .bind(Uuid::now_v7())
    .bind(integration_id)
    .bind(platform_user_ref)
    .bind(token_hash)
    .bind(ttl_secs)
    .bind(platform_email_hint)
    .fetch_one(exec)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkRequestRow {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub platform_user_ref: String,
    pub expired: bool,
    pub used: bool,
}

/// Lock the request with this token hash (confirm flow).
pub async fn get_for_update_by_hash(
    conn: &mut sqlx::PgConnection,
    token_hash: &[u8],
) -> Result<Option<LinkRequestRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, integration_id, platform_user_ref, expires_at <= now() AS expired,
                used_at IS NOT NULL AS used
           FROM link_requests WHERE token_hash=$1 FOR UPDATE",
    )
    .bind(token_hash)
    .fetch_optional(conn)
    .await?)
}

/// Consume a request for `user` (single use).
pub async fn mark_used<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    user: Uuid,
) -> Result<(), DbError> {
    sqlx::query("UPDATE link_requests SET used_at=now(), user_id=$2, updated_at=now() WHERE id=$1")
        .bind(id)
        .bind(user)
        .execute(exec)
        .await?;
    Ok(())
}
