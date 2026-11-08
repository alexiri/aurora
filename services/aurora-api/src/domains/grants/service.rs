use super::compile::{compile_connection_grant, compile_secret_grant};
use super::dto::{ConnectionGrant, GrantMode, PutConnectionGrant, SecretGrant};
use crate::app::AppState;
use crate::domains::apps::service as apps;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::agents::{self, AgentRow};
use aurora_db::repo::{connections, credentials, derived_rules, grants, secrets};
use serde_json::json;
use std::collections::HashSet;
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

fn invalid(reason: &str, message: &str) -> ApiError {
    ApiError::unprocessable("grant_invalid", message).with_details(json!({"reason": reason}))
}

fn dedupe(v: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|t| seen.insert(t.clone())).collect()
}

/// Create or replace the connection grant and compile it (07 §Grants), one transaction,
/// then invalidate the agent. The connection must be live in the agent's workspace or org.
pub async fn grant_connection(
    state: &AppState,
    caller: &Caller,
    agent: &AgentRow,
    connection_id: Uuid,
    req: PutConnectionGrant,
) -> Result<ConnectionGrant, ApiError> {
    let conn = connections::get(&state.pool, connection_id)
        .await?
        .filter(|c| {
            c.org_id == agent.org_id
                && c.status != "orphaned"
                && c.workspace_id.is_none_or(|w| w == agent.workspace_id)
        })
        .ok_or_else(|| ApiError::not_found("Connection not found."))?;
    let (allow, ask) = match req.mode {
        GrantMode::Full => (Vec::new(), Vec::new()),
        GrantMode::Custom => (dedupe(req.allow_tools), dedupe(req.ask_tools)),
    };
    // (RFE-011a) The ask timeout belongs to custom grants that have ask tools, within bounds.
    let ask_timeout = match req.ask_timeout_secs {
        None => None,
        Some(t) => {
            if req.mode != GrantMode::Custom || ask.is_empty() {
                return Err(invalid(
                    "ask_timeout_without_ask_tools",
                    "An ask timeout needs a custom grant with ask tools.",
                ));
            }
            if let Some(m) = crate::domains::policy::validate::timeout_range_error(
                t,
                state.config.approvals.max_timeout_secs,
            ) {
                return Err(crate::domains::policy::validate::timeout_out_of_range(
                    vec![aurora_core::FieldError::new("/ask_timeout_secs", m)],
                ));
            }
            // Range-checked above (60..=14400): the conversion cannot fail.
            i32::try_from(t).ok()
        }
    };
    if req.mode == GrantMode::Custom {
        if allow.is_empty() && ask.is_empty() {
            return Err(invalid(
                "empty_custom",
                "A custom grant needs at least one tool; detach the connection instead.",
            ));
        }
        if allow.iter().any(|t| ask.contains(t)) {
            return Err(invalid(
                "tool_in_both",
                "A tool cannot be both allowed and set to ask.",
            ));
        }
        let snap = apps::snapshot(state).await?;
        let known: HashSet<String> = snap
            .as_ref()
            .and_then(|s| s.app(&conn.app_id))
            .map(|a| apps::tools(a).into_iter().map(|t| t.id).collect())
            .unwrap_or_default();
        if allow.iter().chain(&ask).any(|t| !known.contains(t)) {
            return Err(invalid(
                "unknown_tool",
                "A tool is not in the app's catalogue.",
            ));
        }
    }
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let g = grants::upsert_connection_grant(
        &mut *tx,
        agent.workspace_id,
        agent.id,
        conn.id,
        req.mode.as_str(),
        &allow,
        &ask,
        ask_timeout,
        Some(caller.user_id),
    )
    .await?;
    compile_connection_grant(
        &mut tx,
        agent.org_id,
        agent.workspace_id,
        g.id,
        agent.id,
        conn.id,
        &conn.label,
        req.mode,
        &allow,
        &ask,
        ask_timeout,
    )
    .await?;
    credentials::notify_gateway_invalidate(&mut *tx, "agent", agent.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(g.into())
}

/// Remove the connection grant and its derived rules (404 when absent).
pub async fn revoke_connection(
    state: &AppState,
    agent: &AgentRow,
    connection_id: Uuid,
) -> Result<Uuid, ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let id = grants::delete_connection_grant(&mut *tx, agent.id, connection_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Grant not found."))?;
    derived_rules::delete_for_source(&mut *tx, "grant", id).await?;
    credentials::notify_gateway_invalidate(&mut *tx, "agent", agent.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(id)
}
