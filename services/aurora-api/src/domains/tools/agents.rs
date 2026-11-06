//! Agents family (docs/12 Phase 0 catalogue, BE-065): `message_agent`, `complete_task`,
//! `list_peer_tasks`, `cancel_peer_task`. Identity comes from the authenticated call context;
//! arguments never name the caller.
use super::registry::{PlatformTool, ToolCallContext, ToolContext, ToolError};
use crate::domains::peer_tasks::service::{self as peers, SendOutcome, SendRequest};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::peer_tasks::{self as repo, PeerTaskRow};
use aurora_db::repo::{agents, conversations as convs, turns};
use aurora_protocol::tools::{JsonContent, ToolContent};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

pub fn tools() -> Vec<Box<dyn PlatformTool>> {
    vec![
        Box::new(MessageAgent),
        Box::new(CompleteTask),
        Box::new(ListPeerTasks),
        Box::new(CancelPeerTask),
    ]
}

fn out(value: Value) -> Result<Vec<ToolContent>, ToolError> {
    Ok(vec![ToolContent::Json(JsonContent { value })])
}

fn parse<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))
}

fn fail(e: ApiError) -> ToolError {
    if e.status().is_client_error() {
        ToolError::Failed(e.message().to_owned())
    } else {
        tracing::error!(error = %e.message(), "agent tool error");
        ToolError::Failed("internal error".into())
    }
}

fn db(e: DbError) -> ToolError {
    fail(e.into())
}

macro_rules! common {
    ($name:expr, $desc:expr, $schema:expr) => {
        fn name(&self) -> &'static str {
            $name
        }
        fn description(&self, _: &ToolContext) -> String {
            $desc.into()
        }
        fn input_schema(&self) -> Value {
            $schema
        }
        fn available(&self, _: &ToolContext) -> bool {
            true
        }
    };
}

fn task_view(t: &PeerTaskRow, me: Uuid) -> Value {
    json!({"id": t.id,
        "direction": if t.from_agent_id == me { "outgoing" } else { "incoming" },
        "other_agent_id": if t.from_agent_id == me { t.to_agent_id } else { t.from_agent_id },
        "status": t.status, "messages_used": t.messages_used,
        "message_budget": t.message_budget, "report": t.report, "expires_at": t.expires_at})
}

struct MessageAgent;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageArgs {
    agent: String,
    text: String,
}

#[async_trait::async_trait]
impl PlatformTool for MessageAgent {
    common!(
        "message_agent",
        "Ask another agent (id or @handle, see list_agents) to do something. While a person is \
         talking to you this opens a peer task: the other agent works on it and its report \
         comes back to this conversation. Inside an open task you may reply to the requesting \
         agent. Messages may need a person's approval first; the result then says \
         pending_approval.",
        json!({"type": "object", "additionalProperties": false, "required": ["agent", "text"],
            "properties": {"agent": {"type": "string"}, "text": {"type": "string"}}})
    );
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: MessageArgs = parse(args)?;
        let mut tx = ctx.agent.pool.begin().await.map_err(|e| db(e.into()))?;
        let sender = agents::get(&mut *tx, ctx.agent.agent_id)
            .await
            .map_err(db)?
            .ok_or_else(|| ToolError::Failed("Unknown agent.".into()))?;
        let conversation = convs::get(&mut *tx, ctx.conversation_id)
            .await
            .map_err(db)?
            .ok_or_else(|| ToolError::Failed("Unknown conversation.".into()))?;
        let turn = match ctx.turn_id {
            Some(t) => turns::get(&mut *tx, t).await.map_err(db)?,
            None => None,
        };
        let outcome = peers::send_message(
            &mut tx,
            &SendRequest {
                sender: &sender,
                conversation: &conversation,
                turn: turn.as_ref(),
                target: &a.agent,
                text: &a.text,
            },
        )
        .await
        .map_err(fail)?;
        // Refusals may still have written (e.g. a budget close and its report): commit.
        tx.commit().await.map_err(|e| db(e.into()))?;
        match outcome {
            SendOutcome::Opened(t) => out(json!({"status": "sent", "peer_task_id": t.id,
                "note": "The other agent will report back to this conversation."})),
            SendOutcome::Queued(t) => out(json!({"status": "queued", "peer_task_id": t.id,
                "note": "An earlier task to this agent is still open; yours starts when it ends."})),
            SendOutcome::Delivered(t) => out(json!({"status": "delivered",
                "peer_task_id": t.id, "messages_used": t.messages_used,
                "message_budget": t.message_budget})),
            SendOutcome::PendingApproval(rows) => out(json!({"status": "pending_approval",
                "approval_id": rows.first().map(|r| r.id),
                "approval_ids": rows.iter().map(|r| r.id).collect::<Vec<_>>()})),
            SendOutcome::Refused(msg) => Err(ToolError::Failed(msg)),
        }
    }
}

struct CompleteTask;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteArgs {
    report: String,
}

#[async_trait::async_trait]
impl PlatformTool for CompleteTask {
    common!(
        "complete_task",
        "Finish the peer task you were asked to do and send your report to the requester. Only \
         works inside a peer task conversation.",
        json!({"type": "object", "additionalProperties": false, "required": ["report"],
            "properties": {"report": {"type": "string"}}})
    );
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: CompleteArgs = parse(args)?;
        let mut tx = ctx.agent.pool.begin().await.map_err(|e| db(e.into()))?;
        let res = peers::complete(&mut tx, ctx.agent.agent_id, ctx.conversation_id, &a.report)
            .await
            .map_err(fail)?;
        tx.commit().await.map_err(|e| db(e.into()))?;
        match res {
            Ok(t) => out(json!({"status": "completed", "peer_task_id": t.id})),
            Err(msg) => Err(ToolError::Failed(msg)),
        }
    }
}

struct ListPeerTasks;

#[async_trait::async_trait]
impl PlatformTool for ListPeerTasks {
    common!(
        "list_peer_tasks",
        "List your peer tasks (requested and received), newest first.",
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    );
    async fn call(&self, ctx: &ToolCallContext, _: Value) -> Result<Vec<ToolContent>, ToolError> {
        let rows = repo::list_for_agent(&ctx.agent.pool, ctx.agent.agent_id, None, None, 50)
            .await
            .map_err(db)?;
        out(
            json!({"tasks": rows.iter().map(|t| task_view(t, ctx.agent.agent_id)).collect::<Vec<_>>()}),
        )
    }
}

struct CancelPeerTask;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelArgs {
    id: Uuid,
}

#[async_trait::async_trait]
impl PlatformTool for CancelPeerTask {
    common!(
        "cancel_peer_task",
        "Cancel a peer task you requested (queued or open).",
        json!({"type": "object", "additionalProperties": false, "required": ["id"],
            "properties": {"id": {"type": "string"}}})
    );
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: CancelArgs = parse(args)?;
        let mut tx = ctx.agent.pool.begin().await.map_err(|e| db(e.into()))?;
        let res = peers::cancel(&mut tx, ctx.agent.agent_id, a.id)
            .await
            .map_err(fail)?;
        tx.commit().await.map_err(|e| db(e.into()))?;
        match res {
            Ok(t) => out(json!({"status": t.status, "peer_task_id": t.id})),
            Err(msg) => Err(ToolError::Failed(msg)),
        }
    }
}
