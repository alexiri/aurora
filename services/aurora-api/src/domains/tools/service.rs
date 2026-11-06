//! `POST /v1/runner/tool-calls` (W5/W7): identity validation, then registry dispatch.
use super::registry::{self, ToolCallContext, ToolContext};
use crate::app::AppState;
use aurora_core::ApiError;
use aurora_db::{
    DbError,
    repo::{agents, runners::RunnerRow, sandboxes},
};
use aurora_protocol::common::Validate;
use aurora_protocol::tools::{ToolCall, ToolResult};
use http::StatusCode;
use uuid::Uuid;

pub async fn tool_call(
    s: &AppState,
    runner: &RunnerRow,
    req: ToolCall,
) -> Result<ToolResult, ApiError> {
    tool_call_with(s, runner, req, registry::CALL_TIMEOUT).await
}

/// As [`tool_call`] with an explicit budget (tests).
pub async fn tool_call_with(
    s: &AppState,
    runner: &RunnerRow,
    req: ToolCall,
    timeout: std::time::Duration,
) -> Result<ToolResult, ApiError> {
    req.validate()
        .map_err(|e| ApiError::invalid_request(e.to_string(), vec![]))?;
    let ctx = identity(s, runner, &req).await?;
    Ok(registry::registry()
        .call(&ctx, &req.call.tool, req.call.args, timeout)
        .await)
}

/// Derives the call context from authenticated facts: runner-owned sandbox with the current
/// boot -> agent; the conversation must be that agent's; a turn must be one of its active turns.
async fn identity(
    s: &AppState,
    runner: &RunnerRow,
    req: &ToolCall,
) -> Result<ToolCallContext, ApiError> {
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let sb = match sandboxes::get(&mut conn, req.sandbox_id).await? {
        Some(sb) if sb.runner_id == Some(runner.id) => sb,
        _ => return Err(ApiError::not_found("Unknown sandbox.")),
    };
    if sb.boot_id != Some(req.boot_id) || !matches!(sb.state.as_str(), "starting" | "running") {
        return Err(ApiError::conflict("fenced", "Sandbox boot is not current."));
    }
    let conv_agent: Option<Uuid> =
        sqlx::query_scalar("SELECT agent_id FROM conversations WHERE id = $1")
            .bind(req.call.conversation_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
    if conv_agent != Some(sb.agent_id) {
        return Err(ApiError::not_found("Unknown conversation."));
    }
    if let Some(turn) = req.call.turn_id {
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM turns WHERE id = $1 AND conversation_id = $2 \
             AND agent_id = $3 AND status IN ('dispatched', 'running'))",
        )
        .bind(turn)
        .bind(req.call.conversation_id)
        .bind(sb.agent_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
        if !active {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "turn_not_active",
                "The turn is not an active turn of this conversation.",
            ));
        }
    }
    let agent = agents::get(&mut *conn, sb.agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Unknown agent."))?;
    Ok(ToolCallContext {
        agent: ToolContext {
            pool: s.pool.clone(),
            agent_id: agent.id,
            workspace_id: agent.workspace_id,
            org_id: agent.org_id,
        },
        conversation_id: req.call.conversation_id,
        turn_id: req.call.turn_id,
        call_id: req.call.call_id.clone(),
        state: s.clone(),
    })
}
