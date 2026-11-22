//! (FIX-218, D-459, C-1145) `terminal_shell_preferences`: the person's chosen shell per agent.
//! One writer (`PUT /v1/agents/{agent}/terminal/shell`); readers are the terminal open and the
//! SSH grant redemption. The value is `bash` or `zsh` (a table CHECK); API-only (it names
//! `ssh_sessions`, which the gateway role cannot read).

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The stored choice of `user` for `agent`, `None` when nothing is saved.
pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    user_id: Uuid,
    agent_id: Uuid,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT shell FROM terminal_shell_preferences WHERE user_id = $1 AND agent_id = $2",
    )
    .bind(user_id)
    .bind(agent_id)
    .fetch_optional(exec)
    .await?)
}

/// Saves the choice (`bash` or `zsh`; anything else fails the CHECK).
pub async fn set<'e, E: PgExecutor<'e>>(
    exec: E,
    user_id: Uuid,
    agent_id: Uuid,
    shell: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO terminal_shell_preferences (user_id, agent_id, shell) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, agent_id) DO UPDATE SET shell = EXCLUDED.shell, updated_at = now()",
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(shell)
    .execute(exec)
    .await?;
    Ok(())
}

/// The stored choice of the person who owns SSH session `session_id`, for `agent_id`.
pub async fn for_ssh_session<'e, E: PgExecutor<'e>>(
    exec: E,
    session_id: Uuid,
    agent_id: Uuid,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT p.shell FROM ssh_sessions s \
           JOIN terminal_shell_preferences p ON p.user_id = s.user_id AND p.agent_id = $2 \
          WHERE s.id = $1",
    )
    .bind(session_id)
    .bind(agent_id)
    .fetch_optional(exec)
    .await?)
}
