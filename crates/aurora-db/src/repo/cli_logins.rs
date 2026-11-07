//! CLI device-login rows (BE-014). The plaintext key is minted at consume time only.

use super::api_keys::{self, ApiKeyRow, NewKey};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

pub const USER_CODE_CONSTRAINT: &str = "cli_logins_user_code_key";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CliLogin {
    pub id: Uuid,
    pub status: String,
    pub approved_by: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    /// Sanitized client hostname given at start (spec fix S-04).
    pub hostname: Option<String>,
    /// `expires_at <= now()`, evaluated by the database clock.
    pub expired: bool,
}

const SEL: &str = "id, status, approved_by, workspace_id, expires_at, hostname,
    (expires_at <= now()) AS expired";

/// Insert a pending login valid for `ttl_secs`. Also prunes long-dead rows.
pub async fn create(
    pool: &PgPool,
    id: Uuid,
    device_hash: &[u8; 32],
    user_code: &str,
    hostname: &str,
    ttl_secs: i64,
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM cli_logins WHERE expires_at < now() - interval '1 day'")
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO cli_logins (id, device_code_hash, user_code, hostname, status, expires_at)
         VALUES ($1,$2,$3,$4,'pending', now() + make_interval(secs => $5))",
    )
    .bind(id)
    .bind(&device_hash[..])
    .bind(user_code)
    .bind(hostname)
    .bind(ttl_secs as f64)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn find_by_device_hash<'e, E: PgExecutor<'e>>(
    exec: E,
    hash: &[u8; 32],
) -> Result<Option<CliLogin>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM cli_logins WHERE device_code_hash = $1"
    ))
    .bind(&hash[..])
    .fetch_optional(exec)
    .await?)
}

/// A pending, unexpired login for `user_code`.
pub async fn find_pending_by_user_code<'e, E: PgExecutor<'e>>(
    exec: E,
    user_code: &str,
) -> Result<Option<CliLogin>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM cli_logins
         WHERE user_code = $1 AND status = 'pending' AND expires_at > now()"
    ))
    .bind(user_code)
    .fetch_optional(exec)
    .await?)
}

/// pending -> approved. Returns whether this call performed the transition.
pub async fn approve<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    user: Uuid,
    workspace: Uuid,
) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE cli_logins SET status='approved', approved_by=$2, workspace_id=$3, updated_at=now()
         WHERE id=$1 AND status='pending' AND expires_at > now()",
    )
    .bind(id)
    .bind(user)
    .bind(workspace)
    .execute(exec)
    .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn set_status<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    from: &str,
    to: &str,
) -> Result<(), DbError> {
    sqlx::query("UPDATE cli_logins SET status=$3, updated_at=now() WHERE id=$1 AND status=$2")
        .bind(id)
        .bind(from)
        .bind(to)
        .execute(exec)
        .await?;
    Ok(())
}

/// Atomically approved -> consumed and mint the key. `None` when another poll won.
pub async fn consume(
    pool: &PgPool,
    id: Uuid,
    key: &NewKey<'_>,
) -> Result<Option<ApiKeyRow>, DbError> {
    let mut tx = pool.begin().await?;
    let r = sqlx::query(
        "UPDATE cli_logins SET status='consumed', updated_at=now()
         WHERE id=$1 AND status='approved' AND expires_at > now()",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if r.rows_affected() == 0 {
        return Ok(None);
    }
    let row = api_keys::insert(&mut *tx, key).await?;
    sqlx::query("UPDATE cli_logins SET api_key_id=$2 WHERE id=$1")
        .bind(id)
        .bind(row.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(row))
}

/// `(workspace id, slug, name, org id, org slug)`.
pub type WorkspaceSummary = (Uuid, String, String, Uuid, String);

pub async fn workspace_summary<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
) -> Result<Option<WorkspaceSummary>, DbError> {
    Ok(sqlx::query_as(
        "SELECT w.id, w.slug, w.name, w.org_id, o.slug
         FROM workspaces w JOIN organizations o ON o.id = w.org_id WHERE w.id = $1",
    )
    .bind(ws)
    .fetch_optional(exec)
    .await?)
}

pub async fn user_email<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT email::text FROM users WHERE id = $1")
            .bind(user)
            .fetch_optional(exec)
            .await?,
    )
}
