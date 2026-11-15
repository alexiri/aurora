//! Files-stream grants (phase 5, P5-FILES-1, D-316, SEC-125, C-901; docs/16 §W21): single-use
//! `auf_` grants bound to `(agent, sandbox, boot, person)`, hashed (the value is never stored),
//! consumed by one compare-and-set at redemption, pruned after 24 h. A table of its own so the
//! `auf_` prefix can only ever open an `sftp` exec.
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// Mint a files stream grant: insert the binding; consumed by one compare-and-set.
#[allow(clippy::too_many_arguments)]
pub async fn insert_file_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    agent: Uuid,
    sandbox: Uuid,
    boot: Uuid,
    granted_for: Uuid,
    grant_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), crate::DbError> {
    sqlx::query(
        "INSERT INTO file_stream_grants \
             (id, agent_id, sandbox_id, boot_id, granted_for, grant_hash, expires_at) \
           VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(agent)
    .bind(sandbox)
    .bind(boot)
    .bind(granted_for)
    .bind(grant_hash)
    .bind(expires_at)
    .execute(exec)
    .await?;
    Ok(())
}

/// The redeemed grant's binding (C-901).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FileGrantRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub granted_for: Uuid,
}

/// Consume one grant exactly once: `None` when unknown, expired or already consumed.
pub async fn consume_file_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    grant_hash: &str,
) -> Result<Option<FileGrantRow>, crate::DbError> {
    Ok(sqlx::query_as(
        "UPDATE file_stream_grants SET consumed_at = now() \
          WHERE grant_hash = $1 AND consumed_at IS NULL AND expires_at > now() \
          RETURNING id, agent_id, sandbox_id, boot_id, granted_for",
    )
    .bind(grant_hash)
    .fetch_optional(exec)
    .await?)
}

/// Housekeeping: rows older than 24 h (the existing grant sweep calls this).
pub async fn prune_file_grants<'e, E: PgExecutor<'e>>(exec: E) -> Result<u64, crate::DbError> {
    Ok(
        sqlx::query(
            "DELETE FROM file_stream_grants WHERE expires_at < now() - interval '24 hours'",
        )
        .execute(exec)
        .await?
        .rows_affected(),
    )
}
