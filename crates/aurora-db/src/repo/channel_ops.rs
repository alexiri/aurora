//! Outbound connector operation queue (docs/13 §4.2; schema `channel_ops`): leased claims,
//! retries and the neutral error-code transitions. Notifications are the caller's business.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OpRow {
    pub id: Uuid,
    pub connector_id: String,
    pub integration_id: Option<Uuid>,
    pub presence_id: Option<Uuid>,
    pub kind: String,
    pub payload: serde_json::Value,
    pub correlation_id: Option<Uuid>,
    pub status: String,
    pub attempts: i32,
    pub not_before: DateTime<Utc>,
    pub lease_until: Option<DateTime<Utc>>,
    pub claimed_by_replica: Option<Uuid>,
    pub result: Option<serde_json::Value>,
    pub error_code: Option<String>,
}

const COLS: &str = "id, connector_id, integration_id, presence_id, kind, payload, correlation_id, \
    status, attempts, not_before, lease_until, claimed_by_replica, result, error_code";

pub struct InsertOp<'a> {
    pub id: Uuid,
    /// Required when `integration_id` is `None`; otherwise the integration's connector is used.
    pub connector_id: Option<&'a str>,
    pub integration_id: Option<Uuid>,
    pub presence_id: Option<Uuid>,
    pub kind: &'a str,
    pub payload: &'a serde_json::Value,
    pub dedupe_key: Option<&'a str>,
    pub correlation_id: Option<Uuid>,
}

/// Insert a `pending` op. Returns `(id, connector_id, inserted)`; with a `dedupe_key` that already
/// exists the earlier op is returned with `inserted = false`. `None` when no connector can be
/// determined.
pub async fn insert(
    conn: &mut PgConnection,
    n: &InsertOp<'_>,
) -> Result<Option<(Uuid, String, bool)>, DbError> {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "INSERT INTO channel_ops (id, connector_id, integration_id, presence_id, kind, payload,
             dedupe_key, correlation_id, status)
         SELECT $1, COALESCE(i.connector_id, $2::text), $3::uuid, $4, $5, $6, $7, $8, 'pending'
           FROM (SELECT 1) x
           LEFT JOIN channel_integrations i ON i.id = $3::uuid
          WHERE COALESCE(i.connector_id, $2::text) IS NOT NULL
         ON CONFLICT (dedupe_key) DO NOTHING
         RETURNING id, connector_id",
    )
    .bind(n.id)
    .bind(n.connector_id)
    .bind(n.integration_id)
    .bind(n.presence_id)
    .bind(n.kind)
    .bind(n.payload)
    .bind(n.dedupe_key)
    .bind(n.correlation_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((id, c)) = row {
        return Ok(Some((id, c, true)));
    }
    let Some(key) = n.dedupe_key else {
        return Ok(None);
    };
    let existing: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, connector_id FROM channel_ops WHERE dedupe_key = $1")
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(existing.map(|(id, c)| (id, c, false)))
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<OpRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM channel_ops WHERE id=$1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// An op as handed to a connector.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Claimed {
    pub id: Uuid,
    pub kind: String,
    pub integration_id: Option<Uuid>,
    pub presence_id: Option<Uuid>,
    pub attempts: i32,
    pub payload: serde_json::Value,
    /// A previous claim's lease lapsed: the platform call may already have happened.
    pub was_leased: bool,
}

/// `SKIP LOCKED` claim of up to `max` due ops of `connector_id` that `replica` may run: ops of
/// integrations it owns, or without an integration. Also takes over `claimed` ops whose lease
/// lapsed. A non-null `lease_until` on the old row (lapsed claim, possibly already reset to
/// `pending` by the sweeper) marks `was_leased`.
pub async fn claim<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    replica: Uuid,
    max: i64,
    lease_secs: f64,
    max_attempts: i32,
) -> Result<Vec<Claimed>, DbError> {
    Ok(sqlx::query_as(
        "WITH c AS (
             SELECT o.id, (o.lease_until IS NOT NULL) AS was_leased
               FROM channel_ops o
              WHERE o.connector_id = $1 AND o.attempts < $5
                AND ((o.status='pending' AND o.not_before <= now())
                     OR (o.status='claimed' AND o.lease_until < now()))
                AND (o.integration_id IS NULL OR EXISTS (
                        SELECT 1 FROM channel_integrations i
                         WHERE i.id = o.integration_id AND i.owner_replica_id = $2))
              ORDER BY o.not_before, o.id LIMIT $3
                FOR UPDATE OF o SKIP LOCKED)
         UPDATE channel_ops o
            SET status='claimed', lease_until = now() + make_interval(secs => $4),
                claimed_by_replica = $2, attempts = o.attempts + 1, updated_at = now()
           FROM c WHERE o.id = c.id
       RETURNING o.id, o.kind, o.integration_id, o.presence_id, o.attempts, o.payload, c.was_leased",
    )
    .bind(connector_id)
    .bind(replica)
    .bind(max)
    .bind(lease_secs)
    .bind(max_attempts)
    .fetch_all(exec)
    .await?)
}

/// Lock an op of `connector_id` for a result transition.
pub async fn lock(
    conn: &mut PgConnection,
    id: Uuid,
    connector_id: &str,
) -> Result<Option<OpRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM channel_ops WHERE id=$1 AND connector_id=$2 FOR UPDATE"
    ))
    .bind(id)
    .bind(connector_id)
    .fetch_optional(conn)
    .await?)
}

/// Terminal transition (`done` | `failed` | `dropped`).
pub async fn finish<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    status: &str,
    result: Option<&serde_json::Value>,
    error_code: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE channel_ops SET status=$2, result=$3, error_code=$4, completed_at=now(),
                lease_until=NULL, updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .bind(status)
    .bind(result)
    .bind(error_code)
    .execute(exec)
    .await?;
    Ok(())
}

/// Back to `pending` after a definitive (not-sent) answer: lease and claimant cleared,
/// `not_before` moved by `delay_secs`, one attempt refunded when `refund`.
pub async fn reschedule<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    delay_secs: f64,
    refund: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE channel_ops SET status='pending', lease_until=NULL, claimed_by_replica=NULL,
                not_before = now() + make_interval(secs => $2),
                attempts = CASE WHEN $3 THEN GREATEST(attempts - 1, 0) ELSE attempts END,
                updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .bind(delay_secs)
    .bind(refund)
    .execute(exec)
    .await?;
    Ok(())
}

/// Lapsed `claimed` ops go back to `pending` (the lease stays set so the next claim reports
/// `maybe_sent`), or `failed` once `max_attempts` claims are used. Returns
/// `(id, connector_id, new_status)`.
pub async fn sweep_lapsed(
    conn: &mut PgConnection,
    max_attempts: i32,
) -> Result<Vec<(Uuid, String, String)>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE channel_ops o
            SET status = CASE WHEN o.attempts >= $1 THEN 'failed' ELSE 'pending' END,
                error_code = CASE WHEN o.attempts >= $1 THEN 'unavailable' END,
                result = CASE WHEN o.attempts >= $1 THEN
                    '{\"error\":{\"code\":\"unavailable\",\"message\":\"lease lapsed; attempts exhausted\"}}'::jsonb END,
                completed_at = CASE WHEN o.attempts >= $1 THEN now() END,
                updated_at = now()
           FROM (SELECT id FROM channel_ops WHERE status='claimed' AND lease_until < now()
                  ORDER BY lease_until LIMIT 500 FOR UPDATE SKIP LOCKED) s
          WHERE o.id = s.id
      RETURNING o.id, o.connector_id, o.status",
    )
    .bind(max_attempts)
    .fetch_all(conn)
    .await?)
}

/// Cancel a still-`pending` op (a correlated caller gave up).
pub async fn drop_pending<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_ops SET status='dropped', completed_at=now(), updated_at=now()
          WHERE id=$1 AND status='pending'",
    )
    .bind(id)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

/// A `message.post` op of `connector_id` claimed by `replica` that references the attachment.
pub async fn claimed_post_references<'e, E: PgExecutor<'e>>(
    exec: E,
    connector_id: &str,
    replica: Uuid,
    attachment_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM channel_ops
          WHERE connector_id=$1 AND kind='message.post' AND status='claimed'
            AND claimed_by_replica=$2
            AND payload->'files' @> jsonb_build_array(jsonb_build_object('attachment_id', $3::text)))",
    )
    .bind(connector_id)
    .bind(replica)
    .bind(attachment_id.to_string())
    .fetch_one(exec)
    .await?)
}

/// Presence -> `disabled` with a reason.
pub async fn disable_presence<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    reason: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_presences SET status='disabled', status_reason=$2, updated_at=now()
          WHERE id=$1",
    )
    .bind(id)
    .bind(reason)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}
