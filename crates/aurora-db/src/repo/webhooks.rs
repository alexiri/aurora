//! Webhooks and their delivery log (docs/03 §8, docs/10 §Webhooks).

use crate::DbError;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WebhookRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub name: String,
    pub instructions: String,
    pub display_prefix: String,
    /// `{ kind, header, secret_ref }` or `None` for unverified hooks.
    pub verification: Option<Value>,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub consecutive_failures: i32,
    pub creator_user_id: Option<Uuid>,
    pub creator_name: Option<String>,
    /// `creator` | `workspace` (RFE-021): the fire-time authority.
    pub anchor: String,
    pub origin_conversation_id: Option<Uuid>,
    pub run_conversation_id: Option<Uuid>,
    pub last_delivery_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "w.id, w.agent_id, w.name, w.instructions, w.display_prefix, w.verification, \
    w.enabled, w.disabled_reason, w.consecutive_failures, w.creator_user_id, \
    (SELECT u.display_name FROM users u WHERE u.id = w.creator_user_id) AS creator_name, w.anchor, \
    w.origin_conversation_id, w.run_conversation_id, \
    (SELECT max(d.created_at) FROM webhook_deliveries d WHERE d.webhook_id = w.id) \
        AS last_delivery_at, w.created_at, w.updated_at";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeliveryRow {
    pub id: Uuid,
    pub webhook_id: Uuid,
    pub turn_id: Option<Uuid>,
    /// `accepted` | `rejected` | `paused`.
    pub status: String,
    pub size_bytes: i32,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct NewWebhook<'a> {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub name: &'a str,
    pub instructions: &'a str,
    pub token_hash: &'a [u8],
    pub display_prefix: &'a str,
    pub verification: Option<&'a Value>,
    pub creator_user_id: Option<Uuid>,
    pub anchor: &'a str,
    pub origin_conversation_id: Option<Uuid>,
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<WebhookRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM webhooks w WHERE w.id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

pub async fn get_by_token_hash<'e, E: PgExecutor<'e>>(
    exec: E,
    hash: &[u8],
) -> Result<Option<WebhookRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM webhooks w WHERE w.token_hash = $1"
    ))
    .bind(hash)
    .fetch_optional(exec)
    .await?)
}

/// Lock the webhook row (serializes deliveries of one webhook) and return it.
pub async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<Option<WebhookRow>, DbError> {
    let found: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM webhooks WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    match found {
        Some(id) => get(&mut *conn, id).await,
        None => Ok(None),
    }
}

pub async fn list_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<WebhookRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM webhooks w WHERE w.agent_id = $1 ORDER BY w.created_at, w.id LIMIT 500"
    ))
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

pub async fn count_for_agent<'e, E: PgExecutor<'e>>(exec: E, agent: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM webhooks WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(exec)
            .await?,
    )
}

pub async fn insert(conn: &mut PgConnection, n: &NewWebhook<'_>) -> Result<WebhookRow, DbError> {
    sqlx::query(
        "INSERT INTO webhooks (id, agent_id, name, instructions, token_hash, display_prefix,
            verification, creator_user_id, anchor, origin_conversation_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(n.id)
    .bind(n.agent_id)
    .bind(n.name)
    .bind(n.instructions)
    .bind(n.token_hash)
    .bind(n.display_prefix)
    .bind(n.verification)
    .bind(n.creator_user_id)
    .bind(n.anchor)
    .bind(n.origin_conversation_id)
    .execute(&mut *conn)
    .await?;
    get(&mut *conn, n.id).await?.ok_or(DbError::NotFound)
}

/// RFE-021: change the fire-time authority (creator and/or anchor); `re_enable` also clears
/// `disabled_reason` and the failure count.
pub async fn set_authority(
    conn: &mut PgConnection,
    id: Uuid,
    creator: Option<Uuid>,
    anchor: &str,
    re_enable: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE webhooks SET creator_user_id = $2, anchor = $3, updated_at = now(),
            enabled = enabled OR $4, disabled_reason = CASE WHEN $4 THEN NULL ELSE disabled_reason END,
            consecutive_failures = CASE WHEN $4 THEN 0 ELSE consecutive_failures END
         WHERE id = $1",
    )
    .bind(id)
    .bind(creator)
    .bind(anchor)
    .bind(re_enable)
    .execute(conn)
    .await?;
    Ok(())
}

/// RFE-021: webhooks of a workspace whose creator is `user` (any state), locked for update;
/// optionally narrowed to `ids`.
pub async fn lock_by_creator(
    conn: &mut PgConnection,
    workspace: Uuid,
    user: Uuid,
    ids: Option<&[Uuid]>,
) -> Result<Vec<WebhookRow>, DbError> {
    let locked: Vec<Uuid> = sqlx::query_scalar(
        "SELECT w.id FROM webhooks w JOIN agents a ON a.id = w.agent_id
         WHERE a.workspace_id = $1 AND w.creator_user_id = $2
           AND ($3::uuid[] IS NULL OR w.id = ANY($3))
         ORDER BY w.id FOR UPDATE OF w",
    )
    .bind(workspace)
    .bind(user)
    .bind(ids)
    .fetch_all(&mut *conn)
    .await?;
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM webhooks w WHERE w.id = ANY($1) ORDER BY w.id"
    ))
    .bind(&locked)
    .fetch_all(conn)
    .await?)
}

/// Full-row edit of the user-controlled fields.
pub async fn update(conn: &mut PgConnection, r: &WebhookRow) -> Result<WebhookRow, DbError> {
    sqlx::query(
        "UPDATE webhooks SET name=$2, instructions=$3, verification=$4, enabled=$5,
            disabled_reason=$6, consecutive_failures=$7, updated_at=now()
         WHERE id=$1",
    )
    .bind(r.id)
    .bind(&r.name)
    .bind(&r.instructions)
    .bind(&r.verification)
    .bind(r.enabled)
    .bind(&r.disabled_reason)
    .bind(r.consecutive_failures)
    .execute(&mut *conn)
    .await?;
    get(&mut *conn, r.id).await?.ok_or(DbError::NotFound)
}

/// Replace the capability token (the old one stops resolving at once).
pub async fn set_token(
    conn: &mut PgConnection,
    id: Uuid,
    hash: &[u8],
    prefix: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE webhooks SET token_hash=$2, display_prefix=$3, updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .bind(hash)
    .bind(prefix)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM webhooks WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?
        .rows_affected()
        == 1)
}

pub async fn set_run_conversation(
    conn: &mut PgConnection,
    id: Uuid,
    conversation: Uuid,
) -> Result<(), DbError> {
    sqlx::query("UPDATE webhooks SET run_conversation_id = $2 WHERE id = $1")
        .bind(id)
        .bind(conversation)
        .execute(conn)
        .await?;
    Ok(())
}

/// Disable with a reason (`authorization` | `failures` | `user`).
pub async fn disable(conn: &mut PgConnection, id: Uuid, reason: &str) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE webhooks SET enabled = false, disabled_reason = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(reason)
    .execute(conn)
    .await?;
    Ok(())
}

/// Count one failed turn and return the new consecutive-failure count.
pub async fn add_failure(conn: &mut PgConnection, id: Uuid) -> Result<i32, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE webhooks SET consecutive_failures = consecutive_failures + 1 WHERE id = $1
         RETURNING consecutive_failures",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?
    .unwrap_or(0))
}

pub async fn reset_failures(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE webhooks SET consecutive_failures = 0 WHERE id = $1 AND consecutive_failures <> 0",
    )
    .bind(id)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn insert_delivery(
    conn: &mut PgConnection,
    id: Uuid,
    webhook: Uuid,
    turn: Option<Uuid>,
    status: &str,
    size_bytes: i32,
    reason: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO webhook_deliveries (id, webhook_id, turn_id, status, size_bytes, reason)
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(id)
    .bind(webhook)
    .bind(turn)
    .bind(status)
    .bind(size_bytes)
    .bind(reason)
    .execute(conn)
    .await?;
    Ok(())
}

/// An accepted delivery with this idempotency key newer than `window_secs`.
pub async fn find_duplicate(
    conn: &mut PgConnection,
    webhook: Uuid,
    key: &str,
    window_secs: f64,
) -> Result<Option<DeliveryRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, webhook_id, turn_id, status, size_bytes, reason, created_at
         FROM webhook_deliveries
         WHERE webhook_id = $1 AND status = 'accepted' AND reason = $2
           AND created_at > now() - make_interval(secs => $3)
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(webhook)
    .bind(key)
    .bind(window_secs)
    .fetch_optional(conn)
    .await?)
}

/// Deliveries received in the last `window_secs` (rate limit).
pub async fn count_recent(
    conn: &mut PgConnection,
    webhook: Uuid,
    window_secs: f64,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM webhook_deliveries
         WHERE webhook_id = $1 AND created_at > now() - make_interval(secs => $2)",
    )
    .bind(webhook)
    .bind(window_secs)
    .fetch_one(conn)
    .await?)
}

pub async fn list_deliveries<'e, E: PgExecutor<'e>>(
    exec: E,
    webhook: Uuid,
    limit: i64,
) -> Result<Vec<DeliveryRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, webhook_id, turn_id, status, size_bytes, reason, created_at
         FROM webhook_deliveries WHERE webhook_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2",
    )
    .bind(webhook)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}
