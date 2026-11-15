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

/// FIX-112: a chat person is prompted to link at most once per `throttle_secs`. Inserts a request
/// unless an unused, unexpired one for the same (integration, person) was made within the window;
/// `None` then means "already prompted, send nothing". Serialized per person by an advisory lock so
/// concurrent events cannot both pass the check.
#[allow(clippy::too_many_arguments)]
pub async fn insert_throttled(
    conn: &mut sqlx::PgConnection,
    integration_id: Uuid,
    platform_user_ref: &str,
    token_hash: &[u8],
    ttl_secs: f64,
    throttle_secs: f64,
    platform_email_hint: Option<&str>,
    platform_display_name: Option<&str>,
) -> Result<Option<(Uuid, DateTime<Utc>)>, DbError> {
    use sqlx::Connection;
    let mut tx = conn.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("link_request:{integration_id}:{platform_user_ref}"))
        .execute(&mut *tx)
        .await?;
    let recent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM link_requests
                         WHERE integration_id=$1 AND platform_user_ref=$2
                           AND used_at IS NULL AND expires_at > now()
                           AND created_at > now() - make_interval(secs => $3))",
    )
    .bind(integration_id)
    .bind(platform_user_ref)
    .bind(throttle_secs)
    .fetch_one(&mut *tx)
    .await?;
    if recent {
        tx.commit().await?;
        return Ok(None);
    }
    let row: (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO link_requests (id, integration_id, platform_user_ref, token_hash, expires_at,
                                    platform_email_hint, platform_display_name)
         VALUES ($1,$2,$3,$4, now() + make_interval(secs => $5), $6, $7)
         RETURNING id, expires_at",
    )
    .bind(Uuid::now_v7())
    .bind(integration_id)
    .bind(platform_user_ref)
    .bind(token_hash)
    .bind(ttl_secs)
    .bind(platform_email_hint)
    .bind(platform_display_name)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(row))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkRequestRow {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub platform_user_ref: String,
    pub platform_display_name: Option<String>,
    pub platform_email_hint: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub expired: bool,
    pub used: bool,
}

const REQ_COLS: &str = "id, integration_id, platform_user_ref, platform_display_name,
    platform_email_hint::text AS platform_email_hint, expires_at, expires_at <= now() AS expired,
    used_at IS NOT NULL AS used";

/// Lock the request with this token hash (confirm flow).
pub async fn get_for_update_by_hash(
    conn: &mut sqlx::PgConnection,
    token_hash: &[u8],
) -> Result<Option<LinkRequestRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {REQ_COLS} FROM link_requests WHERE token_hash=$1 FOR UPDATE"
    ))
    .bind(token_hash)
    .fetch_optional(conn)
    .await?)
}

/// The request with this token hash, unlocked (the preview the confirm page shows).
pub async fn get_by_hash<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
) -> Result<Option<LinkRequestRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {REQ_COLS} FROM link_requests WHERE token_hash=$1"
    ))
    .bind(token_hash)
    .fetch_optional(exec)
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
