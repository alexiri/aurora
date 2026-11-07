//! Gateway credentials for agents (`agent` and BYO `context` kinds).

use crate::DbError;
use crate::bus::{self, BusError, Channel};
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

pub async fn insert_agent_credential<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    token_hash: &[u8],
    display_prefix: &str,
) -> Result<Uuid, DbError> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id)
         VALUES ($1,'agent',$2,$3,$4)",
    )
    .bind(id)
    .bind(token_hash)
    .bind(display_prefix)
    .bind(agent_id)
    .execute(exec)
    .await?;
    Ok(id)
}

pub async fn insert_context_credential<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    conversation_id: Option<Uuid>,
    token_hash: &[u8],
    display_prefix: &str,
    label: &str,
    ttl_secs: i64,
) -> Result<(Uuid, DateTime<Utc>), DbError> {
    let id = Uuid::now_v7();
    let exp: DateTime<Utc> = sqlx::query_scalar(
        "INSERT INTO gateway_credentials
            (id, kind, token_hash, display_prefix, agent_id, conversation_id, context_label, expires_at)
         VALUES ($1,'context',$2,$3,$4,$5,$6, now() + make_interval(secs => $7))
         RETURNING expires_at",
    )
    .bind(id)
    .bind(token_hash)
    .bind(display_prefix)
    .bind(agent_id)
    .bind(conversation_id)
    .bind(label)
    .bind(ttl_secs as f64)
    .fetch_one(exec)
    .await?;
    Ok((id, exp))
}

/// Revoke live credentials of the agent: `agent`+`context` kinds, or all kinds.
pub async fn revoke_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    all_kinds: bool,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE gateway_credentials SET revoked_at = now(), updated_at = now()
         WHERE agent_id = $1 AND revoked_at IS NULL AND ($2 OR kind IN ('agent','context'))",
    )
    .bind(agent_id)
    .bind(all_kinds)
    .execute(exec)
    .await?
    .rows_affected())
}

/// Conversation belongs to the agent.
pub async fn conversation_of_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation: Uuid,
    agent_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM conversations WHERE id = $1 AND agent_id = $2)",
    )
    .bind(conversation)
    .bind(agent_id)
    .fetch_one(exec)
    .await?)
}

/// `aurora_gateway_invalidate {scope, id}` (delivered on commit inside a tx).
pub async fn notify_gateway_invalidate<'e, E: PgExecutor<'e>>(
    exec: E,
    scope: &str,
    id: Uuid,
) -> Result<(), DbError> {
    bus::notify(
        exec,
        Channel::GatewayInvalidate,
        &serde_json::json!({"scope": scope, "id": id}),
    )
    .await
    .map_err(|e| match e {
        BusError::Db(e) => e.into(),
        other => DbError::Other(sqlx::Error::Protocol(other.to_string())),
    })
}
