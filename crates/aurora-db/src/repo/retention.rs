//! The daily retention pass (FIX-111, docs/05 §Background jobs "Retention pruning"): bounded
//! batch deletes of rows that only ever grow. Every function removes at most `limit` rows per
//! statement (`FOR UPDATE SKIP LOCKED` where the rows are claimable) and returns how many it
//! removed; the caller loops until a batch comes back short. Ages are seconds so the callers
//! pass the documented constants (`CHANNEL_LEDGER_RETENTION`, `SESSION_PRUNE_GRACE`,
//! `WEBHOOK_DELIVERY_RETENTION`) without this crate knowing them.

use crate::DbError;
use sqlx::PgExecutor;

/// Terminal `channel_inbox` rows (`done`/`failed`/`expired`) older than `age_secs`.
pub async fn prune_channel_inbox<'e, E: PgExecutor<'e>>(
    exec: E,
    age_secs: f64,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM channel_inbox WHERE id IN (
           SELECT id FROM channel_inbox
            WHERE status IN ('done', 'failed', 'expired')
              AND updated_at < now() - make_interval(secs => $1)
            ORDER BY updated_at LIMIT $2 FOR UPDATE SKIP LOCKED)",
    )
    .bind(age_secs)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

/// Terminal `channel_ops` rows (`done`/`failed`/`dropped`) older than `age_secs`. A pending or
/// claimed op is never terminal, so a pending `presence.deprovision` survives; a finished
/// deprovision op whose sealed credential copy is still set is kept until the copy is
/// discarded (`deprovision::discard_op_copy`, RFE-024).
pub async fn prune_channel_ops<'e, E: PgExecutor<'e>>(
    exec: E,
    age_secs: f64,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM channel_ops WHERE id IN (
           SELECT id FROM channel_ops
            WHERE status IN ('done', 'failed', 'dropped')
              AND updated_at < now() - make_interval(secs => $1)
              AND NOT (kind = 'presence.deprovision' AND payload->>'credentials_ref' IS NOT NULL)
            ORDER BY updated_at LIMIT $2 FOR UPDATE SKIP LOCKED)",
    )
    .bind(age_secs)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

/// Settled or failed `channel_cards` older than `age_secs`.
pub async fn prune_channel_cards<'e, E: PgExecutor<'e>>(
    exec: E,
    age_secs: f64,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM channel_cards WHERE id IN (
           SELECT id FROM channel_cards
            WHERE state IN ('settled', 'failed')
              AND updated_at < now() - make_interval(secs => $1)
            ORDER BY updated_at LIMIT $2 FOR UPDATE SKIP LOCKED)",
    )
    .bind(age_secs)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

/// `link_requests` that expired (or were used) more than `age_secs` ago.
pub async fn prune_link_requests<'e, E: PgExecutor<'e>>(
    exec: E,
    age_secs: f64,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM link_requests WHERE id IN (
           SELECT id FROM link_requests
            WHERE least(expires_at, coalesce(used_at, expires_at))
                  < now() - make_interval(secs => $1)
            ORDER BY expires_at LIMIT $2 FOR UPDATE SKIP LOCKED)",
    )
    .bind(age_secs)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

/// Sessions that expired or were revoked more than `grace_secs` ago (their preview sessions
/// go by cascade). `docs/03`: "Rows of revoked or expired sessions are removed by the daily
/// session pruning".
pub async fn prune_sessions<'e, E: PgExecutor<'e>>(
    exec: E,
    grace_secs: f64,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM sessions WHERE id IN (
           SELECT id FROM sessions
            WHERE expires_at < now() - make_interval(secs => $1)
               OR revoked_at < now() - make_interval(secs => $1)
            ORDER BY least(expires_at, coalesce(revoked_at, expires_at)) LIMIT $2
            FOR UPDATE SKIP LOCKED)",
    )
    .bind(grace_secs)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

/// Device-login rows (`cli_logins`) that expired more than `grace_secs` ago.
pub async fn prune_cli_logins<'e, E: PgExecutor<'e>>(
    exec: E,
    grace_secs: f64,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM cli_logins WHERE id IN (
           SELECT id FROM cli_logins
            WHERE expires_at < now() - make_interval(secs => $1)
            ORDER BY expires_at LIMIT $2 FOR UPDATE SKIP LOCKED)",
    )
    .bind(grace_secs)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

/// Context `gateway_credentials` that expired or were revoked more than `grace_secs` ago.
pub async fn prune_context_credentials<'e, E: PgExecutor<'e>>(
    exec: E,
    grace_secs: f64,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM gateway_credentials WHERE id IN (
           SELECT id FROM gateway_credentials
            WHERE kind = 'context'
              AND (expires_at < now() - make_interval(secs => $1)
                   OR revoked_at < now() - make_interval(secs => $1))
            ORDER BY created_at LIMIT $2 FOR UPDATE SKIP LOCKED)",
    )
    .bind(grace_secs)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

/// `webhook_deliveries` older than `age_secs`.
pub async fn prune_webhook_deliveries_by_age<'e, E: PgExecutor<'e>>(
    exec: E,
    age_secs: f64,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM webhook_deliveries WHERE id IN (
           SELECT id FROM webhook_deliveries
            WHERE created_at < now() - make_interval(secs => $1)
            ORDER BY created_at LIMIT $2 FOR UPDATE SKIP LOCKED)",
    )
    .bind(age_secs)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}

/// Everything beyond the newest `max_per_webhook` deliveries of a webhook (a flood of rejected
/// requests writes a row each).
pub async fn prune_webhook_deliveries_over_cap<'e, E: PgExecutor<'e>>(
    exec: E,
    max_per_webhook: i64,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM webhook_deliveries WHERE id IN (
           SELECT id FROM (
             SELECT d.id, row_number() OVER (PARTITION BY d.webhook_id
                                             ORDER BY d.created_at DESC, d.id DESC) AS rn
               FROM webhook_deliveries d
              WHERE d.webhook_id IN (SELECT webhook_id FROM webhook_deliveries
                                      GROUP BY webhook_id HAVING count(*) > $1)) t
            WHERE t.rn > $1 LIMIT $2)",
    )
    .bind(max_per_webhook)
    .bind(limit)
    .execute(exec)
    .await?
    .rows_affected())
}
