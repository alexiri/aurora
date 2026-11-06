//! Platform-user link requests (docs/13 §4 `link-start`; schema `link_requests`).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// Insert a request whose token hash is `token_hash`; returns `(id, expires_at)`.
pub async fn insert<'e, E: PgExecutor<'e>>(
    exec: E,
    integration_id: Uuid,
    platform_user_ref: &str,
    token_hash: &[u8],
    ttl_secs: f64,
) -> Result<(Uuid, DateTime<Utc>), DbError> {
    Ok(sqlx::query_as(
        "INSERT INTO link_requests (id, integration_id, platform_user_ref, token_hash, expires_at)
         VALUES ($1,$2,$3,$4, now() + make_interval(secs => $5))
         RETURNING id, expires_at",
    )
    .bind(Uuid::now_v7())
    .bind(integration_id)
    .bind(platform_user_ref)
    .bind(token_hash)
    .bind(ttl_secs)
    .fetch_one(exec)
    .await?)
}
