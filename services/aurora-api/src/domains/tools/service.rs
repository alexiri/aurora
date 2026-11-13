//! `POST /v1/runner/tool-calls` (W5/W7): identity validation, then registry dispatch.
use super::registry::{self, ToolCallContext, ToolContext};
use crate::app::AppState;
use aurora_core::ApiError;
use aurora_db::{
    DbError,
    repo::{agents, runners::RunnerRow, sandboxes, tool_identity},
};
use aurora_protocol::common::Validate;
use aurora_protocol::tools::{ToolCall, ToolResult};
use http::StatusCode;

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
    let conv_agent =
        tool_identity::conversation_agent(&mut *conn, req.call.conversation_id).await?;
    if conv_agent != Some(sb.agent_id) {
        return Err(ApiError::not_found("Unknown conversation."));
    }
    // (FIX-110, SEC-24, docs/16 W7) A tool call is fenced on the agent's active turns: `turn_id`
    // is required. A process left behind by an earlier turn cannot act, unattributed, later.
    let Some(turn) = req.call.turn_id else {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "no_active_turn",
            "Platform tools can only be used while a turn is running.",
        ));
    };
    let active =
        tool_identity::turn_is_active(&mut *conn, turn, req.call.conversation_id, sb.agent_id)
            .await?;
    if !active {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "turn_not_active",
            "The turn is not an active turn of this conversation.",
        ));
    }
    let agent = agents::get(&mut *conn, sb.agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Unknown agent."))?;
    drop(conn);
    // (FIX-110, SEC-45) An effectively frozen agent's own tools are refused too.
    crate::domains::freeze::service::ensure_agent_active_pool(s, agent.id).await?;
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
