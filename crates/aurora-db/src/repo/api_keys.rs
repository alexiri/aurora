use super::notify_auth_revoked;
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct KeyRecord {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub user_id: Uuid,
    /// `user` | `service`.
    pub kind: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// Not revoked/expired, owner not disabled and an *active* member of the key's org.
    pub live: bool,
    pub user_email: String,
    pub is_instance_admin: bool,
}

pub async fn find_by_hash<'e, E: PgExecutor<'e>>(
    exec: E,
    hash: &[u8; 32],
) -> Result<Option<KeyRecord>, DbError> {
    Ok(sqlx::query_as(
        "SELECT k.id, k.org_id, k.workspace_id, k.user_id, k.kind, k.expires_at, k.revoked_at,
                (k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at > now())
                 AND u.disabled_at IS NULL
                 AND EXISTS (SELECT 1 FROM org_members m
                             WHERE m.org_id = k.org_id AND m.user_id = k.user_id
                               AND m.status = 'active')) AS live,
                u.email::text AS user_email, u.is_instance_admin
         FROM api_keys k JOIN users u ON u.id = k.user_id
         WHERE k.token_hash = $1",
    )
    .bind(&hash[..])
    .fetch_optional(exec)
    .await?)
}

/// Update `last_used_at` at most once per minute.
pub async fn touch<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE api_keys SET last_used_at = now()
         WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Revoke a key and notify. Returns whether it was newly revoked.
pub async fn revoke(pool: &PgPool, id: Uuid) -> Result<bool, DbError> {
    let mut tx = pool.begin().await?;
    let r = sqlx::query(
        "UPDATE api_keys SET revoked_at = now(), updated_at = now()
         WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    notify_auth_revoked(&mut *tx, "key", id).await?;
    tx.commit().await?;
    Ok(r.rows_affected() > 0)
}
