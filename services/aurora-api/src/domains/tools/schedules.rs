//! Schedule tools (docs/12 Phase 0 catalogue): `schedule_create/list/update/delete`. An agent
//! manages only its own schedules; the person speaking in the current turn becomes the creator.
use super::registry::{PlatformTool, ToolCallContext, ToolContext, ToolError};
use crate::domains::schedules::once;
use crate::domains::schedules::service::{self, Create, Patch};
use aurora_core::ApiError;
use aurora_db::repo::peer_tasks;
use aurora_db::repo::schedules::ScheduleRow;
use aurora_db::repo::{agents, turns, users, workspaces};
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
        // (C-1094) A field error says which field and why (the API's messages).
        let field = e.details().and_then(|d| d["fields"].get(0)).and_then(|f| {
            Some(format!(
                "{} {}",
                f["path"].as_str()?.trim_start_matches('/'),
                f["message"].as_str()?
            ))
        });
        ToolError::Failed(match field {
            Some(f) => format!("{} {f}", e.message()),
            None => e.message().to_owned(),
        })
    } else {
        tracing::error!(error = %e.message(), "schedule tool error");
        ToolError::Failed("internal error".into())
    }
}

fn view(r: &ScheduleRow) -> Value {
    json!({"id": r.id, "name": r.name, "prompt": r.prompt, "cron": r.cron,
        "run_at": r.run_at.map(|at| once::local_of(at, &r.timezone)),
        "completed_at": r.completed_at,
        "timezone": r.timezone, "gate_command": r.gate_command,
        "gate_description": r.gate_description, "enabled": r.enabled,
        "disabled_reason": r.disabled_reason, "next_fire_at": r.next_fire_at,
        "last_fired_at": r.last_fire_at, "consecutive_failures": r.consecutive_failures})
}

/// The person speaking in this turn (`None` for a schedule, webhook or other turn without one).
async fn speaker(ctx: &ToolCallContext) -> Result<Option<Uuid>, ToolError> {
    Ok(match ctx.turn_id {
        Some(t) => turns::get(&ctx.agent.pool, t)
            .await?
            .and_then(|t| t.speaker_user_id),
        None => None,
    })
}

/// (FIX-253, D-490) The person a schedule created in this turn belongs to, and whether it is the
/// delegated-task fallback: the speaker, or — on a peer turn (`speaker_kind=agent`) of a
/// delegated task that is still open — the peer task's requester: the schedule runs as them.
/// Every other turn without a speaker (a fired schedule or watch, a webhook, a turn of a closed
/// task) refuses: a wake never schedules itself again (SEC-144), and a closed task's
/// conversation is nobody's chat.
async fn creator(ctx: &ToolCallContext) -> Result<Option<(Uuid, bool)>, ToolError> {
    let turn = match ctx.turn_id {
        Some(t) => turns::get(&ctx.agent.pool, t).await?,
        None => None,
    };
    if let Some(u) = turn.as_ref().and_then(|t| t.speaker_user_id) {
        return Ok(Some((u, false)));
    }
    if !turn.is_some_and(|t| t.speaker_kind == "agent") {
        return Ok(None);
    }
    Ok(
        peer_tasks::by_task_conversation(&ctx.agent.pool, ctx.conversation_id)
            .await?
            .filter(|task| task.status == "open")
            .and_then(|t| t.requester_user_id)
            .map(|u| (u, true)),
    )
}

/// (P5-SCHED-ONCE, C-1094) The tools' default time zone: the speaker's own (`users.timezone`,
/// C-1054), else the workspace's: the zone of "Now" in the turn context, so "tomorrow at 9" is
/// the person's 9:00.
async fn default_tz(
    ctx: &ToolCallContext,
    agent: &agents::AgentRow,
    user: Uuid,
) -> Result<String, ToolError> {
    if let Some(tz) = users::timezone(&ctx.agent.pool, user)
        .await?
        .filter(|t| t.parse::<chrono_tz::Tz>().is_ok())
    {
        return Ok(tz);
    }
    Ok(workspaces::get(&ctx.agent.pool, agent.workspace_id)
        .await?
        .map_or_else(|| "UTC".to_owned(), |w| w.timezone))
}

async fn own(ctx: &ToolCallContext, id: Uuid) -> Result<ScheduleRow, ToolError> {
    let r = service::get(&ctx.agent.pool, id).await.map_err(fail)?;
    if r.agent_id != ctx.agent.agent_id {
        return Err(ToolError::Failed("Schedule not found.".into()));
    }
    Ok(r)
}

/// (P5-GUIDE-7, C-654) The family of every tool of this file.
const FAMILY: &str = "schedules";

macro_rules! common {
    ($name:expr, $desc:expr, $schema:expr) => {
        fn name(&self) -> &'static str {
            $name
        }
        fn family(&self) -> &'static str {
            FAMILY
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
    cron: Option<String>,
    run_at: Option<String>,
    timezone: Option<String>,
    gate_command: Option<String>,
    gate_description: Option<String>,
}

#[async_trait::async_trait]
impl PlatformTool for CreateT {
    common!(
        "schedule_create",
        "Wake yourself later with `prompt`. For a reminder or anything that happens once, set \
         `run_at` to a local date-time such as 2026-11-21T09:00 (in `timezone`, default: the \
         person's, the zone of Now above); it runs once. For repeating work set `cron` (5 fields, \
         e.g. `0 10 27 11 *` = every year on 27 November at 10:00; at most every 5 minutes). A \
         one-time schedule made in a person's own chat runs in that chat, so your answer reaches \
         them there. In a delegated task's conversation the schedule belongs to the task's \
         requester and its wake runs in the task conversation — and only a one-time `run_at` \
         wake is possible from there (at most three open ones): a repeating schedule belongs \
         to the person themselves. Optional gate_command runs first; \
         empty output or {\"wake_agent\": false} skips the run.",
        json!({"type": "object", "additionalProperties": false,
            "required": ["name", "prompt"],
            "properties": {"name": {"type": "string"}, "prompt": {"type": "string"},
                "cron": {"type": "string"}, "run_at": {"type": "string"},
                "timezone": {"type": "string"},
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
        // A schedule turn has no speaker, so a run can never schedule itself again (SEC-144).
        // (FIX-253, D-490) In a delegated task's conversation the schedule belongs to the
        // task's requester: it runs as them, so they must still be able to use this agent
        // (re-checked here and again at fire time, never widened).
        let Some((creator, from_task)) = creator(ctx).await? else {
            return Err(ToolError::Failed(
                "Schedules can only be created for a person: the one talking with you, or the requester of this task.".into(),
            ));
        };
        if !aurora_db::authz::can_use(&ctx.agent.pool, creator, agent.workspace_id).await? {
            return Err(ToolError::Failed(
                "The person this schedule would belong to can no longer use this agent.".into(),
            ));
        }
        // (FIX-253 review) A delegated task ends; a repeating schedule from its peer turn would
        // keep running as the requester without the task. From a peer turn: one-time only.
        if from_task && a.run_at.is_none() {
            return Err(ToolError::Failed(
                "a delegated task can only set a one-time wake-up: give `run_at` (a repeating schedule belongs to the person themselves).".into(),
            ));
        }
        // (FIX-253 review) A task conversation is capped: at most 3 open one-time wake-ups.
        if from_task
            && aurora_db::repo::schedules::open_once_count_from(&ctx.agent.pool, ctx.conversation_id)
                .await?
                >= 3
        {
            return Err(ToolError::Failed(
                "This task already has three open wake-ups: finish one (or ask the person to) before setting another.".into(),
            ));
        }
        let timezone = match a.timezone {
            Some(t) => t,
            None => default_tz(ctx, &agent, creator).await?,
        };
        let row = service::create(
            &ctx.agent.pool,
            &agent,
            Create {
                name: a.name,
                prompt: a.prompt,
                cron: a.cron,
                run_at: a.run_at,
                timezone: Some(timezone),
                gate_command: a.gate_command,
                gate_description: a.gate_description,
                // RFE-021: an agent can never mint a workspace-anchored automation.
                anchor: "creator",
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
    run_at: Option<String>,
    timezone: Option<String>,
    gate_command: Option<String>,
    gate_description: Option<String>,
    enabled: Option<bool>,
}

#[async_trait::async_trait]
impl PlatformTool for UpdateT {
    common!(
        "schedule_update",
        "Change one of your schedules (fields omitted are kept); enabled=false pauses it. A new \
         `run_at` makes it run once (and runs a finished one again); a `cron` makes it repeat. \
         Without `timezone` they are read in the person's zone.",
        json!({"type": "object", "additionalProperties": false, "required": ["id"],
            "properties": {"id": {"type": "string"}, "name": {"type": "string"},
                "prompt": {"type": "string"}, "cron": {"type": "string"},
                "run_at": {"type": "string"},
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
        // (C-1094) A new time without a zone is read in the speaker's zone, as on create; with
        // nobody speaking the schedule keeps its zone.
        let timezone = match (&a.timezone, a.cron.is_some() || a.run_at.is_some()) {
            (None, true) => match speaker(ctx).await? {
                Some(user) => {
                    let agent = agents::get(&ctx.agent.pool, ctx.agent.agent_id)
                        .await?
                        .ok_or_else(|| ToolError::Failed("Unknown agent.".into()))?;
                    Some(default_tz(ctx, &agent, user).await?)
                }
                None => None,
            },
            _ => a.timezone,
        };
        let row = service::patch(
            &ctx.agent.pool,
            cur,
            Patch {
                name: a.name,
                prompt: a.prompt,
                cron: a.cron,
                run_at: a.run_at,
                timezone,
                gate_command: a.gate_command.map(Some),
                gate_description: a.gate_description.map(Some),
                enabled: a.enabled,
                paused_by: None,
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
