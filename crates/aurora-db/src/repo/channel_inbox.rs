//! Durable connector inbox (docs/13 §4; schema `channel_inbox`): at-least-once in, deduped on
//! `(connector_id, event_id)`, claimed with a lease by the inbox processor.

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InboxRow {
    pub id: Uuid,
    pub connector_id: String,
    pub integration_id: Option<Uuid>,
    pub event_id: String,
    pub kind: String,
    pub payload: serde_json::Value,
    /// Attempts including the current claim.
    pub attempts: i32,
}

/// `true` when inserted, `false` for a duplicate `(connector_id, event_id)`.
pub async fn insert<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    integration_id: Uuid,
    event_id: &str,
    kind: &str,
    payload: &serde_json::Value,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO channel_inbox (id, connector_id, integration_id, event_id, kind, payload,
            status, next_attempt_at)
         VALUES ($1,$2,$3,$4,$5,$6,'pending',now())
         ON CONFLICT (connector_id, event_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(connector_id)
    .bind(integration_id)
    .bind(event_id)
    .bind(kind)
    .bind(payload)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

/// `(id, connector_id, owner_replica_id)` of the listed integrations.
pub async fn integration_owners<'e, E: PgExecutor<'e>>(
    exec: E,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, String, Option<Uuid>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, connector_id, owner_replica_id FROM channel_integrations WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

/// Claim up to `limit` due rows (`pending`, or `processing` with a lapsed lease) for
/// `lease_secs`. `SKIP LOCKED`, so concurrent workers never share a row.
pub async fn claim<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
    lease_secs: f64,
) -> Result<Vec<InboxRow>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE channel_inbox i
            SET status='processing', lease_until = now() + make_interval(secs => $2),
                attempts = i.attempts + 1, updated_at = now()
           FROM (SELECT id FROM channel_inbox
                  WHERE (status='pending' AND next_attempt_at <= now())
                     OR (status='processing' AND lease_until < now())
                  ORDER BY next_attempt_at, id LIMIT $1 FOR UPDATE SKIP LOCKED) c
          WHERE i.id = c.id
      RETURNING i.id, i.connector_id, i.integration_id, i.event_id, i.kind, i.payload, i.attempts",
    )
    .bind(limit)
    .bind(lease_secs)
    .fetch_all(exec)
    .await?)
}

/// Mark `done`; only the holder of claim number `attempts` can finish the row.
pub async fn complete<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    attempts: i32,
    outcome: &serde_json::Value,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_inbox SET status='done', outcome=$3, error=NULL, lease_until=NULL,
                updated_at=now()
          WHERE id=$1 AND status='processing' AND attempts=$2",
    )
    .bind(id)
    .bind(attempts)
    .bind(outcome)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

/// Back to `pending` after `backoff_secs`.
pub async fn retry<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    attempts: i32,
    error: &str,
    backoff_secs: f64,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_inbox SET status='pending', error=$3, lease_until=NULL,
                next_attempt_at = now() + make_interval(secs => $4), updated_at=now()
          WHERE id=$1 AND status='processing' AND attempts=$2",
    )
    .bind(id)
    .bind(attempts)
    .bind(error)
    .bind(backoff_secs)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

/// Terminal failure.
pub async fn fail<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    attempts: i32,
    error: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_inbox SET status='failed', error=$3, lease_until=NULL, updated_at=now()
          WHERE id=$1 AND status='processing' AND attempts=$2",
    )
    .bind(id)
    .bind(attempts)
    .bind(error)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}
