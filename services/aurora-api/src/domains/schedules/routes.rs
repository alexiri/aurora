use super::dto::*;
use super::{fire, service};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::automations::{
    dto::ReassignCreatorRequest,
    service::{self as auto, Acting, To},
};
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::{agents, schedules::ScheduleRow};
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_schedules, create_schedule))
        .routes(routes!(get_schedule, update_schedule, delete_schedule))
        .routes(routes!(list_runs))
        .routes(routes!(run_now))
        .routes(routes!(reassign_creator))
}

/// Schedules need `can_use` on the agent's workspace.
async fn visible_agent(c: &Caller, s: &AppState, id: Uuid) -> Result<agents::AgentRow, ApiError> {
    let row = agents::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let ws = c.workspace(s, row.workspace_id).await?;
    if !ws.can_use() {
        return Err(ApiError::forbidden("Workspace access required."));
    }
    Ok(row)
}

/// Load a schedule and check access through its agent.
async fn authorized(
    c: &Caller,
    s: &AppState,
    id: Uuid,
) -> Result<(ScheduleRow, agents::AgentRow), ApiError> {
    let row = service::get(&s.pool, id).await?;
    let agent = visible_agent(c, s, row.agent_id).await?;
    Ok((row, agent))
}

fn record(c: &Caller, s: &AppState, a: &agents::AgentRow, r: &ScheduleRow, action: &str) {
    audit::record(
        s,
        AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
            .org(a.org_id)
            .workspace(a.workspace_id)
            .target("schedule", r.id)
            .metadata(json!({"name": r.name, "agent_id": a.id})),
    );
}

/// RFE-009: a reference to a direct conversation the caller cannot see is returned as `null`
/// with `conversation_hidden: true`.
async fn redact_all(c: &Caller, s: &AppState, items: &mut [Schedule]) -> Result<(), ApiError> {
    let ids: Vec<Uuid> = items
        .iter()
        .filter_map(|i| i.origin_conversation_id)
        .collect();
    let hidden = aurora_db::authz::hidden_conversations(&s.pool, c.user_id, &ids).await?;
    for i in items.iter_mut() {
        if i.origin_conversation_id
            .is_some_and(|id| hidden.contains(&id))
        {
            i.origin_conversation_id = None;
            i.conversation_hidden = true;
        }
    }
    Ok(())
}

/// List an agent's schedules.
#[utoipa::path(get, path = "/agents/{agent}/schedules", operation_id = "listSchedules",
    tag = "schedules", params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = Page<Schedule>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_schedules(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<Page<Schedule>>, ApiError> {
    visible_agent(&c, &s, agent).await?;
    let items = service::list(&s.pool, agent).await?;
    let mut items: Vec<Schedule> = items.into_iter().map(Schedule::from).collect();
    redact_all(&c, &s, &mut items).await?;
    Ok(Json(Page {
        items,
        next_cursor: None,
    }))
}

/// Create a schedule; the caller becomes its creator (fire-time authority).
#[utoipa::path(post, path = "/agents/{agent}/schedules", operation_id = "createSchedule",
    tag = "schedules", params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = CreateScheduleRequest,
    responses((status = 201, body = Schedule), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_schedule(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(req): Json<CreateScheduleRequest>,
) -> Result<(StatusCode, Json<Schedule>), ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let anchor = match req.anchor.as_deref() {
        Some(v) => auto::parse_anchor(v)?,
        None => "creator",
    };
    if anchor == "workspace" {
        require_own(&c.workspace(&s, a.workspace_id).await?)?;
    }
    let create = service::Create {
        anchor,
        name: req.name,
        prompt: req.prompt,
        cron: req.cron,
        timezone: req.timezone,
        gate_command: req.gate_command,
        gate_description: req.gate_description,
    };
    let row = service::create(&s.pool, &a, create, Some(c.user_id), None).await?;
    record(&c, &s, &a, &row, "schedule.create");
    let row = service::get(&s.pool, row.id).await?;
    let mut out = vec![Schedule::from(row)];
    redact_all(&c, &s, &mut out).await?;
    Ok((StatusCode::CREATED, Json(out.remove(0))))
}

/// Get one schedule.
#[utoipa::path(get, path = "/schedules/{id}", operation_id = "getSchedule", tag = "schedules",
    params(("id" = Uuid, Path, description = "Schedule id")),
    responses((status = 200, body = Schedule), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_schedule(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Schedule>, ApiError> {
    let (row, _) = authorized(&c, &s, id).await?;
    let mut out = vec![Schedule::from(row)];
    redact_all(&c, &s, &mut out).await?;
    Ok(Json(out.remove(0)))
}

/// Update a schedule; `enabled: true` re-enables and clears the failure count.
#[utoipa::path(patch, path = "/schedules/{id}", operation_id = "updateSchedule",
    tag = "schedules", params(("id" = Uuid, Path, description = "Schedule id")),
    request_body = UpdateScheduleRequest,
    responses((status = 200, body = Schedule), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn update_schedule(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateScheduleRequest>,
) -> Result<Json<Schedule>, ApiError> {
    let (cur, a) = authorized(&c, &s, id).await?;
    // RFE-021: switching the anchor needs `can_own`; validate before anything is written.
    let new_anchor = match req.anchor.as_deref() {
        Some(v) => {
            let v = auto::parse_anchor(v)?;
            require_own(&c.workspace(&s, a.workspace_id).await?)?;
            if v == cur.anchor {
                None
            } else {
                let to = auto::anchor_target(v, cur.creator_user_id)?;
                let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
                auto::ensure_target(&mut conn, to, a.workspace_id).await?;
                Some(to)
            }
        }
        None => None,
    };
    let p = service::Patch {
        name: req.name,
        prompt: req.prompt,
        cron: req.cron,
        timezone: req.timezone,
        gate_command: req.gate_command,
        gate_description: req.gate_description,
        enabled: req.enabled,
    };
    let mut row = service::patch(&s.pool, cur, p).await?;
    record(&c, &s, &a, &row, "schedule.update");
    if let Some(to) = new_anchor {
        let actor = AuditActor::from_caller(&c);
        let acting = Acting {
            actor: &actor,
            org: a.org_id,
            workspace: a.workspace_id,
        };
        let mut tx = s.pool.begin().await.map_err(DbError::from)?;
        auto::reassign_schedule(&mut tx, &acting, &row, to, false).await?;
        tx.commit().await.map_err(DbError::from)?;
        row = service::get(&s.pool, id).await?;
    }
    let mut out = vec![Schedule::from(row)];
    redact_all(&c, &s, &mut out).await?;
    Ok(Json(out.remove(0)))
}

/// Delete a schedule and its run log.
#[utoipa::path(delete, path = "/schedules/{id}", operation_id = "deleteSchedule",
    tag = "schedules", params(("id" = Uuid, Path, description = "Schedule id")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_schedule(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (cur, a) = authorized(&c, &s, id).await?;
    service::delete(&s.pool, id).await?;
    record(&c, &s, &a, &cur, "schedule.delete");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, IntoParams)]
struct RunsQuery {
    /// Page size (default 50, max 200).
    limit: Option<u32>,
    /// Opaque cursor from a previous response's `next_cursor`.
    cursor: Option<String>,
}

/// Recent runs of a schedule, newest first.
#[utoipa::path(get, path = "/schedules/{id}/runs", operation_id = "listScheduleRuns",
    tag = "schedules", params(("id" = Uuid, Path, description = "Schedule id"), RunsQuery),
    responses((status = 200, body = Page<ScheduleRun>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_runs(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<RunsQuery>,
) -> Result<Json<Page<ScheduleRun>>, ApiError> {
    authorized(&c, &s, id).await?;
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows =
        aurora_db::repo::schedules::list_runs(&s.pool, id, p.after.map(|c| (c.t, c.id)), p.fetch())
            .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page.items.into_iter().map(ScheduleRun::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

/// Fire the schedule now, outside its cadence; 409 `schedule_busy` while a run is active.
#[utoipa::path(post, path = "/schedules/{id}/run-now", operation_id = "runScheduleNow",
    tag = "schedules", params(("id" = Uuid, Path, description = "Schedule id")),
    responses((status = 201, body = ScheduleRun), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn run_now(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<ScheduleRun>), ApiError> {
    let (row, a) = authorized(&c, &s, id).await?;
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let run = fire::run_now(&mut tx, &row).await?.ok_or_else(|| {
        ApiError::conflict("schedule_busy", "A run of this schedule is still active.")
    })?;
    tx.commit().await.map_err(DbError::from)?;
    record(&c, &s, &a, &row, "schedule.run_now");
    Ok((StatusCode::CREATED, Json(run.into())))
}

/// Reassign the schedule's creator (fire-time authority) to an active member who can use the
/// workspace (`can_own`); `re_enable` re-enables an item disabled for `authorization`. Audited
/// `automation.reassign`.
#[utoipa::path(put, path = "/schedules/{id}/creator", operation_id = "reassignScheduleCreator",
    tag = "schedules", params(("id" = Uuid, Path, description = "Schedule id")),
    request_body = ReassignCreatorRequest,
    responses((status = 200, body = Schedule), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn reassign_creator(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ReassignCreatorRequest>,
) -> Result<Json<Schedule>, ApiError> {
    let (row, a) = authorized(&c, &s, id).await?;
    require_own(&c.workspace(&s, a.workspace_id).await?)?;
    let actor = AuditActor::from_caller(&c);
    let acting = Acting {
        actor: &actor,
        org: a.org_id,
        workspace: a.workspace_id,
    };
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    auto::reassign_schedule(
        &mut tx,
        &acting,
        &row,
        To::User(req.user_id),
        req.re_enable.unwrap_or(false),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    let mut out = vec![Schedule::from(service::get(&s.pool, id).await?)];
    redact_all(&c, &s, &mut out).await?;
    Ok(Json(out.remove(0)))
}
