use super::notify_auth_revoked;
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, PgPool};
use std::net::IpAddr;
use uuid::Uuid;

pub struct NewSession<'a> {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: [u8; 32],
    pub ip: Option<IpAddr>,
    pub user_agent: Option<&'a str>,
    pub expires_at: DateTime<Utc>,
}

pub async fn create<'e, E: PgExecutor<'e>>(exec: E, s: &NewSession<'_>) -> Result<(), DbError> {
    let ua = s
        .user_agent
        .map(|u| u.chars().take(256).collect::<String>());
    sqlx::query(
        "INSERT INTO sessions (id, user_id, token_hash, ip, user_agent, expires_at)
         VALUES ($1, $2, $3, $4::text::inet, $5, $6)",
    )
    .bind(s.id)
    .bind(s.user_id)
    .bind(&s.token_hash[..])
    .bind(s.ip.map(|i| i.to_string()))
    .bind(ua)
    .bind(s.expires_at)
    .execute(exec)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SessionRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// Not revoked, not idle/absolutely expired (database clock), user not disabled.
    pub live: bool,
    pub user_email: String,
    pub is_instance_admin: bool,
}

pub async fn find_by_hash<'e, E: PgExecutor<'e>>(
    exec: E,
    hash: &[u8; 32],
    idle_secs: u64,
) -> Result<Option<SessionRecord>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.id, s.user_id, s.last_seen_at, s.expires_at, s.revoked_at,
                (s.revoked_at IS NULL AND s.expires_at > now()
                 AND s.last_seen_at > now() - make_interval(secs => $2::float8)
                 AND u.disabled_at IS NULL) AS live,
                u.email::text AS user_email, u.is_instance_admin
         FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = $1",
    )
    .bind(&hash[..])
    .bind(idle_secs as f64)
    .fetch_optional(exec)
    .await?)
}

/// Update `last_seen_at` at most once per minute. Returns whether a write happened.
pub async fn touch<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE sessions SET last_seen_at = now()
         WHERE id = $1 AND last_seen_at < now() - interval '1 minute'",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Revoke one session and notify. Returns whether it was newly revoked.
pub async fn revoke(pool: &PgPool, id: Uuid) -> Result<bool, DbError> {
    let mut tx = pool.begin().await?;
    let r = sqlx::query(
        "UPDATE sessions SET revoked_at = now(), updated_at = now()
         WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    notify_auth_revoked(&mut *tx, "session", id).await?;
    tx.commit().await?;
    Ok(r.rows_affected() > 0)
}

/// Revoke all of a user's sessions except `except`; returns the count.
pub async fn revoke_all_for_user_except(
    pool: &PgPool,
    user_id: Uuid,
    except: Option<Uuid>,
) -> Result<u64, DbError> {
    let mut tx = pool.begin().await?;
    let n = revoke_all_for_user_except_tx(&mut tx, user_id, except).await?;
    tx.commit().await?;
    Ok(n)
}

/// [`revoke_all_for_user_except`] inside the caller's transaction.
pub async fn revoke_all_for_user_except_tx(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
    except: Option<Uuid>,
) -> Result<u64, DbError> {
    let r = sqlx::query(
        "UPDATE sessions SET revoked_at = now(), updated_at = now()
         WHERE user_id = $1 AND revoked_at IS NULL AND ($2::uuid IS NULL OR id <> $2)",
    )
    .bind(user_id)
    .bind(except)
    .execute(&mut *conn)
    .await?;
    notify_auth_revoked(&mut *conn, "user", user_id).await?;
    Ok(r.rows_affected())
}

/// Whether session `id` is still live (same rule as [`SessionRecord::live`]); unknown => false.
pub async fn is_live<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    idle_secs: u64,
) -> Result<bool, DbError> {
    let live: Option<bool> = sqlx::query_scalar(
        "SELECT (s.revoked_at IS NULL AND s.expires_at > now()
                 AND s.last_seen_at > now() - make_interval(secs => $2::float8)
                 AND u.disabled_at IS NULL)
         FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.id = $1",
    )
    .bind(id)
    .bind(idle_secs as f64)
    .fetch_optional(exec)
    .await?;
    Ok(live.unwrap_or(false))
}

/// Stamp `reauth_at = now()` on a live session of `user_id` (OIDC re-authentication, docs/04).
/// Returns whether a session was updated.
pub async fn stamp_reauth<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    user_id: Uuid,
) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE sessions SET reauth_at = now(), updated_at = now()
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(exec)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Seconds since the session's last re-authentication (database clock); `None` if never.
pub async fn reauth_age_secs<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<f64>, DbError> {
    let age: Option<Option<f64>> = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (now() - reauth_at))::float8 FROM sessions WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(exec)
    .await?;
    Ok(age.flatten())
}

/// The session's absolute expiry (the preview credential is capped by it, docs/04
/// §Preview access step 3); `None` when the session is gone.
pub async fn expires_of<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT expires_at FROM sessions WHERE id = $1")
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}
