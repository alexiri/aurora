//! Identity lookups for runner tool calls (`POST /v1/runner/tool-calls`).

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The agent that owns a conversation, if the conversation exists.
pub async fn conversation_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation_id: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT agent_id FROM conversations WHERE id = $1")
            .bind(conversation_id)
            .fetch_optional(exec)
            .await?,
    )
}

/// True when `turn_id` is an active (dispatched/running) turn of this agent's conversation.
pub async fn turn_is_active<'e, E: PgExecutor<'e>>(
    exec: E,
    turn_id: Uuid,
    conversation_id: Uuid,
    agent_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM turns WHERE id = $1 AND conversation_id = $2 \
         AND agent_id = $3 AND status IN ('dispatched', 'running'))",
    )
    .bind(turn_id)
    .bind(conversation_id)
    .bind(agent_id)
    .fetch_one(exec)
    .await?)
}
