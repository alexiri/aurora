//! `email_tokens` (verification / reset links) and pending-invitation lookup used by sign-up.

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

pub const VERIFY_EMAIL: &str = "verify_email";
pub const RESET_PASSWORD: &str = "reset_password";

/// Store a token hash expiring `ttl_secs` from now (database clock).
pub async fn create<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    user_id: Uuid,
    purpose: &str,
    token_hash: &[u8; 32],
    ttl_secs: u64,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO email_tokens (id, user_id, purpose, token_hash, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5::float8))",
    )
    .bind(id)
    .bind(user_id)
    .bind(purpose)
    .bind(&token_hash[..])
    .bind(ttl_secs as f64)
    .execute(exec)
    .await?;
    Ok(())
}

/// Mark all of a user's unused tokens of `purpose` used (superseded by a new one).
pub async fn invalidate_unused<'e, E: PgExecutor<'e>>(
    exec: E,
    user_id: Uuid,
    purpose: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE email_tokens SET used_at = now(), updated_at = now()
         WHERE user_id = $1 AND purpose = $2 AND used_at IS NULL",
    )
    .bind(user_id)
    .bind(purpose)
    .execute(exec)
    .await?;
    Ok(())
}

/// Atomically consume a live (unused, unexpired) token; returns its user.
pub async fn consume<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8; 32],
    purpose: &str,
) -> Result<Option<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE email_tokens SET used_at = now(), updated_at = now()
         WHERE token_hash = $1 AND purpose = $2 AND used_at IS NULL AND expires_at > now()
         RETURNING user_id",
    )
    .bind(&token_hash[..])
    .bind(purpose)
    .fetch_optional(exec)
    .await?)
}

/// Is there a pending, unexpired invitation with this token hash addressed to `email`?
pub async fn pending_invitation_matches<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8; 32],
    email: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM invitations
         WHERE token_hash = $1 AND email = $2::citext
           AND status = 'pending' AND expires_at > now())",
    )
    .bind(&token_hash[..])
    .bind(email)
    .fetch_one(exec)
    .await?)
}
