//! Terminal-stream grants (phase 4, P4-TERM, D-161, SEC-87, C-618; docs/16 §W19): single-use
//! `auy_` grants bound to `(agent, sandbox, boot)`, hashed (the value is never stored), consumed
//! by one compare-and-set at redemption, pruned after 24 h. The browser never sees the grant;
//! the API mints it and redeems it at the runner's :4108 listener itself.
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// Mint a terminal stream grant: insert the binding; consumed by one compare-and-set.
#[allow(clippy::too_many_arguments)]
pub async fn insert_terminal_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    agent: Uuid,
    sandbox: Uuid,
    boot: Uuid,
    granted_by: Uuid,
    grant_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), crate::DbError> {
    sqlx::query(
        "INSERT INTO terminal_stream_grants \
             (id, agent_id, sandbox_id, boot_id, granted_by, grant_hash, expires_at) \
           VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(agent)
    .bind(sandbox)
    .bind(boot)
    .bind(granted_by)
    .bind(grant_hash)
    .bind(expires_at)
    .execute(exec)
    .await?;
    Ok(())
}

/// The redeemed grant's binding (C-617): everything the API needs to build the leg-1
/// redemption and the audit row; the hash is not returned (the caller already holds it).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TerminalGrantRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub granted_by: Uuid,
}

/// Consume one grant exactly once (the CAS of `preview_stream_grants::consume_stream_grant`):
/// `None` when unknown, expired or already consumed (403 `grant_invalid`).
pub async fn consume_terminal_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    grant_hash: &str,
) -> Result<Option<TerminalGrantRow>, crate::DbError> {
    Ok(sqlx::query_as(
        "UPDATE terminal_stream_grants SET consumed_at = now() \
          WHERE grant_hash = $1 AND consumed_at IS NULL AND expires_at > now() \
          RETURNING id, agent_id, sandbox_id, boot_id, granted_by",
    )
    .bind(grant_hash)
    .fetch_optional(exec)
    .await?)
}

/// Housekeeping: expired grants and rows older than 24 h (docs/05 §Background jobs).
pub async fn prune_terminal_grants<'e, E: PgExecutor<'e>>(exec: E) -> Result<u64, crate::DbError> {
    Ok(sqlx::query(
        "DELETE FROM terminal_stream_grants WHERE expires_at < now() - interval '24 hours'",
    )
    .execute(exec)
    .await?
    .rows_affected())
}
