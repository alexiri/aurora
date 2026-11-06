use super::compile::compile_secret_grant;
use super::dto::SecretGrant;
use crate::app::AppState;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::agents::{self, AgentRow};
use aurora_db::repo::{credentials, derived_rules, grants, secrets};
use uuid::Uuid;

/// Agent visible to the caller (`can_use` on its workspace), else 404.
pub async fn visible_agent(
    caller: &Caller,
    state: &AppState,
    id: Uuid,
) -> Result<(AgentRow, aurora_db::authz::WorkspaceAuth), ApiError> {
    let row = agents::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let auth = caller.workspace(state, row.workspace_id).await?;
    Ok((row, auth))
}

/// Create (idempotent) the grant and its derived rule in one transaction, then invalidate
/// the agent. The secret must be active and live in the agent's workspace or its org.
pub async fn grant_secret(
    state: &AppState,
    caller: &Caller,
    agent: &AgentRow,
    secret_id: Uuid,
) -> Result<SecretGrant, ApiError> {
    let secret = secrets::get(&state.pool, secret_id)
        .await?
        .filter(|s| {
            s.org_id == agent.org_id
                && s.status == "active"
                && s.workspace_id.is_none_or(|w| w == agent.workspace_id)
        })
        .ok_or_else(|| ApiError::not_found("Secret not found."))?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let g = grants::upsert_secret_grant(
        &mut *tx,
        agent.workspace_id,
        agent.id,
        secret.id,
        Some(caller.user_id),
    )
    .await?;
    compile_secret_grant(
        &mut tx,
        agent.org_id,
        agent.workspace_id,
        g.id,
        agent.id,
        secret.id,
        &secret.name,
    )
    .await?;
    credentials::notify_gateway_invalidate(&mut *tx, "agent", agent.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(g.into())
}

/// Remove the grant and its derived rules; returns the grant id (404 when absent).
pub async fn revoke_secret(
    state: &AppState,
    agent: &AgentRow,
    secret_id: Uuid,
) -> Result<Uuid, ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let id = grants::delete_secret_grant(&mut *tx, agent.id, secret_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Grant not found."))?;
    derived_rules::delete_for_source(&mut *tx, "grant", id).await?;
    credentials::notify_gateway_invalidate(&mut *tx, "agent", agent.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(id)
}
