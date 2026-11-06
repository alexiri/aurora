//! Schedule tools (docs/12 Phase 0 catalogue): `schedule_create/list/update/delete`. An agent
//! manages only its own schedules; the person speaking in the current turn becomes the creator.
use super::registry::{PlatformTool, ToolCallContext, ToolContext, ToolError};
use crate::domains::schedules::service::{self, Create, Patch};
use aurora_core::ApiError;
use aurora_db::repo::schedules::ScheduleRow;
use aurora_db::repo::{agents, turns};
use aurora_protocol::tools::{JsonContent, ToolContent};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

pub fn tools() -> Vec<Box<dyn PlatformTool>> {
    vec![
        Box::new(ListT),
        Box::new(CreateT),
        Box::new(UpdateT),
        Box::new(DeleteT),
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
        tracing::error!(error = %e.message(), "schedule tool error");
        ToolError::Failed("internal error".into())
    }
}

fn view(r: &ScheduleRow) -> Value {
    json!({"id": r.id, "name": r.name, "prompt": r.prompt, "cron": r.cron,
        "timezone": r.timezone, "gate_command": r.gate_command,
        "gate_description": r.gate_description, "enabled": r.enabled,
        "disabled_reason": r.disabled_reason, "next_fire_at": r.next_fire_at,
        "last_fired_at": r.last_fire_at, "consecutive_failures": r.consecutive_failures})
}

async fn own(ctx: &ToolCallContext, id: Uuid) -> Result<ScheduleRow, ToolError> {
    let r = service::get(&ctx.agent.pool, id).await.map_err(fail)?;
    if r.agent_id != ctx.agent.agent_id {
        return Err(ToolError::Failed("Schedule not found.".into()));
    }
    Ok(r)
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

struct ListT;

#[async_trait::async_trait]
impl PlatformTool for ListT {
    common!(
        "schedule_list",
        "List your schedules.",
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    );
    async fn call(&self, ctx: &ToolCallContext, _: Value) -> Result<Vec<ToolContent>, ToolError> {
        let rows = service::list(&ctx.agent.pool, ctx.agent.agent_id)
            .await
            .map_err(fail)?;
        out(json!({"schedules": rows.iter().map(view).collect::<Vec<_>>()}))
    }
}

struct CreateT;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    name: String,
    prompt: String,
    cron: String,
    timezone: Option<String>,
    gate_command: Option<String>,
    gate_description: Option<String>,
}

#[async_trait::async_trait]
impl PlatformTool for CreateT {
    common!(
        "schedule_create",
        "Create a recurring task: a 5-field cron expression (minimum interval 5 minutes) in an \
         IANA timezone (default: the workspace's). Optional gate_command runs before waking you; \
         empty output or {\"wake_agent\": false} skips the run.",
        json!({"type": "object", "additionalProperties": false,
            "required": ["name", "prompt", "cron"],
            "properties": {"name": {"type": "string"}, "prompt": {"type": "string"},
                "cron": {"type": "string"}, "timezone": {"type": "string"},
                "gate_command": {"type": "string"}, "gate_description": {"type": "string"}}})
    );
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: CreateArgs = parse(args)?;
        let agent = agents::get(&ctx.agent.pool, ctx.agent.agent_id)
            .await?
            .ok_or_else(|| ToolError::Failed("Unknown agent.".into()))?;
        let creator = match ctx.turn_id {
            Some(t) => turns::get(&ctx.agent.pool, t)
                .await?
                .and_then(|t| t.speaker_user_id),
            None => None,
        };
        let Some(creator) = creator else {
            return Err(ToolError::Failed(
                "Schedules can only be created while a person is talking to you.".into(),
            ));
        };
        let row = service::create(
            &ctx.agent.pool,
            &agent,
            Create {
                name: a.name,
                prompt: a.prompt,
                cron: a.cron,
                timezone: a.timezone,
                gate_command: a.gate_command,
                gate_description: a.gate_description,
            },
            Some(creator),
            Some(ctx.conversation_id),
        )
        .await
        .map_err(fail)?;
        out(view(&row))
    }
}

struct UpdateT;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    id: Uuid,
    name: Option<String>,
    prompt: Option<String>,
    cron: Option<String>,
    timezone: Option<String>,
    gate_command: Option<String>,
    gate_description: Option<String>,
    enabled: Option<bool>,
}

#[async_trait::async_trait]
impl PlatformTool for UpdateT {
    common!(
        "schedule_update",
        "Change one of your schedules (fields omitted are kept); enabled=false pauses it.",
        json!({"type": "object", "additionalProperties": false, "required": ["id"],
            "properties": {"id": {"type": "string"}, "name": {"type": "string"},
                "prompt": {"type": "string"}, "cron": {"type": "string"},
                "timezone": {"type": "string"}, "gate_command": {"type": "string"},
                "gate_description": {"type": "string"}, "enabled": {"type": "boolean"}}})
    );
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: UpdateArgs = parse(args)?;
        let cur = own(ctx, a.id).await?;
        let row = service::patch(
            &ctx.agent.pool,
            cur,
            Patch {
                name: a.name,
                prompt: a.prompt,
                cron: a.cron,
                timezone: a.timezone,
                gate_command: a.gate_command.map(Some),
                gate_description: a.gate_description.map(Some),
                enabled: a.enabled,
            },
        )
        .await
        .map_err(fail)?;
        out(view(&row))
    }
}

struct DeleteT;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteArgs {
    id: Uuid,
}

#[async_trait::async_trait]
impl PlatformTool for DeleteT {
    common!(
        "schedule_delete",
        "Delete one of your schedules.",
        json!({"type": "object", "additionalProperties": false, "required": ["id"],
            "properties": {"id": {"type": "string"}}})
    );
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: DeleteArgs = parse(args)?;
        own(ctx, a.id).await?;
        service::delete(&ctx.agent.pool, a.id).await.map_err(fail)?;
        out(json!({"deleted": a.id}))
    }
}
