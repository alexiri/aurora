use super::dto::*;
use super::{fire, service};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent};
use crate::domains::automations::{
    dto::ReassignCreatorRequest,
    service::{self as auto, Acting, To},
};
use crate::domains::emergency_access::service::{ReadKind, record_read};
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
        .routes(routes!(preview_schedule))
        .routes(routes!(reassign_creator))
}

/// Schedules need `can_use` on the agent's workspace.
async fn visible_agent(c: &Caller, s: &AppState, id: Uuid) -> Result<agents::AgentRow, ApiError> {
    let row = agents::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    // Membership is the check: `c.workspace` is 404 for a non-member or inactive member.
    c.workspace(s, row.workspace_id).await?;
    Ok(row)
}

/// [`visible_agent`] for a *read* (C-1165): the emergency reader of the workspace lists the
/// schedules. Never for a write. Returns the read authority (the origin-conversation redaction
/// does not apply to an emergency reader: they read every conversation).
async fn visible_agent_read(
    c: &Caller,
    s: &AppState,
    id: Uuid,
) -> Result<(agents::AgentRow, aurora_db::authz::ReadAuth), ApiError> {
    let row = agents::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let read = c.workspace_read(s, row.workspace_id).await?;
    Ok((row, read))
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

/// [`authorized`] for a *read* (C-1165); returns the read authority as well.
async fn authorized_read(
    c: &Caller,
    s: &AppState,
    id: Uuid,
) -> Result<(ScheduleRow, agents::AgentRow, aurora_db::authz::ReadAuth), ApiError> {
    let row = service::get(&s.pool, id).await?;
    let (agent, read) = visible_agent_read(c, s, row.agent_id).await?;
    Ok((row, agent, read))
}

fn record(c: &Caller, s: &AppState, a: &agents::AgentRow, r: &ScheduleRow, action: &str) {
    audit::record(
        s,
        AuditEvent::by(c, action)
            .org(a.org_id)
            .workspace(a.workspace_id)
            .target("schedule", r.id)
            .metadata(json!({"name": r.name, "agent_id": a.id})),
    );
}

/// RFE-009: a reference to a direct conversation the caller cannot see is returned as `null`
/// with `conversation_hidden: true`. (P5-REFINE-7, C-1077) Also fills `next_runs` and
/// `last_result` (one query for the page). An emergency reader reads every conversation
/// (C-1165, D-478(b)): nothing is hidden from them.
async fn redact_all(
    c: &Caller,
    s: &AppState,
    emergency: bool,
    items: &mut [Schedule],
) -> Result<(), ApiError> {
    let now = chrono::Utc::now();
    for i in items.iter_mut().filter(|i| i.enabled) {
        i.next_runs = match &i.cron {
            Some(c) => super::cron::next_n(c, &i.timezone, now, 3).unwrap_or_default(),
            // (P5-SCHED-ONCE, C-1092) A pending one-time schedule: its one instant (a waiting
            // one's recheck); none once Done.
            None if i.completed_at.is_none() => i.next_fire_at.into_iter().collect(),
            None => Vec::new(),
        };
    }
    let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
    let latest = aurora_db::repo::schedules::latest_runs(&s.pool, &ids).await?;
    for r in latest {
        if let Some(i) = items.iter_mut().find(|i| i.id == r.schedule_id) {
            i.last_result = Some(ScheduleRun::from(r).result);
        }
    }
    redact_origins(c, s, emergency, items).await
}

async fn redact_origins(
    c: &Caller,
    s: &AppState,
    emergency: bool,
    items: &mut [Schedule],
) -> Result<(), ApiError> {
    let ids: Vec<Uuid> = items
        .iter()
        .filter_map(|i| i.origin_conversation_id)
        .collect();
    let hidden = if emergency {
        Vec::new()
    } else {
        aurora_db::authz::hidden_conversations(&s.pool, c.user_id, &ids).await?
    };
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
    responses((status = 200, body = Page<Schedule>), (status = 404, body = ErrorEnvelope),
        (status = 503, description = "audit_unavailable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_schedules(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<Page<Schedule>>, ApiError> {
    let (_, read) = visible_agent_read(&c, &s, agent).await?;
    let items = service::list(&s.pool, agent).await?;
    let mut items: Vec<Schedule> = items.into_iter().map(Schedule::from).collect();
    redact_all(
        &c,
        &s,
        matches!(read, aurora_db::authz::ReadAuth::Emergency(_)),
        &mut items,
    )
    .await?;
    record_read(
        &s,
        &AuditActor::from_caller(&c),
        &read,
        ReadKind::Schedule,
        None,
    )
    .await?;
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
        run_at: req.run_at,
        timezone: req.timezone,
        gate_command: req.gate_command,
        gate_description: req.gate_description,
        // A person's own dashboard schedule is not bounded by a task.
        ends_at: None,
    };
    let row = service::create(&s.pool, &a, create, Some(c.user_id), None).await?;
    record(&c, &s, &a, &row, "schedule.create");
    let row = service::get(&s.pool, row.id).await?;
    let mut out = vec![Schedule::from(row)];
    // A write route: the caller holds `WorkspaceAuth`, never an emergency grant.
    redact_all(&c, &s, false, &mut out).await?;
    Ok((StatusCode::CREATED, Json(out.remove(0))))
}

/// Get one schedule.
#[utoipa::path(get, path = "/schedules/{id}", operation_id = "getSchedule", tag = "schedules",
    params(("id" = Uuid, Path, description = "Schedule id")),
    responses((status = 200, body = Schedule), (status = 404, body = ErrorEnvelope),
        (status = 503, description = "audit_unavailable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_schedule(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Schedule>, ApiError> {
    let (row, _, read) = authorized_read(&c, &s, id).await?;
    let mut out = vec![Schedule::from(row)];
    redact_all(
        &c,
        &s,
        matches!(read, aurora_db::authz::ReadAuth::Emergency(_)),
        &mut out,
    )
    .await?;
    record_read(
        &s,
        &AuditActor::from_caller(&c),
        &read,
        ReadKind::Schedule,
        Some(id),
    )
    .await?;
    Ok(Json(out.remove(0)))
}

/// Update a schedule; `enabled: true` re-enables and clears the failure count.
#[utoipa::path(patch, path = "/schedules/{id}", operation_id = "updateSchedule",
    tag = "schedules", params(("id" = Uuid, Path, description = "Schedule id")),
    request_body = UpdateScheduleRequest,
    responses((status = 200, body = Schedule), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 409, description = "schedule_completed", body = ErrorEnvelope)),
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
        run_at: req.run_at,
        timezone: req.timezone,
        gate_command: req.gate_command,
        gate_description: req.gate_description,
        enabled: req.enabled,
        paused_by: Some(c.user_id),
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
    // A write route: the caller holds `WorkspaceAuth`, never an emergency grant.
    redact_all(&c, &s, false, &mut out).await?;
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
    responses((status = 200, body = Page<ScheduleRun>), (status = 404, body = ErrorEnvelope),
        (status = 503, description = "audit_unavailable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_runs(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<RunsQuery>,
) -> Result<Json<Page<ScheduleRun>>, ApiError> {
    let (_, _, read) = authorized_read(&c, &s, id).await?;
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = aurora_db::repo::schedules::runs_with_results(&s.pool, id, p.after_key(), p.fetch())
        .await?;
    record_read(
        &s,
        &AuditActor::from_caller(&c),
        &read,
        ReadKind::Schedule,
        Some(id),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(page.map(ScheduleRun::from)))
}

/// What a cadence would do: the next three instants and whether it fires more often than every
/// five minutes (C-1077). Saves nothing.
#[utoipa::path(post, path = "/agents/{agent}/schedules/preview", operation_id = "previewSchedule",
    tag = "schedules", params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = SchedulePreviewRequest,
    responses((status = 200, body = SchedulePreview), (status = 404, body = ErrorEnvelope),
        (status = 422, description = "invalid_cron | invalid_timezone | invalid_run_at", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn preview_schedule(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(req): Json<SchedulePreviewRequest>,
) -> Result<Json<SchedulePreview>, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let tz = match req.timezone {
        Some(t) => t,
        None => aurora_db::repo::workspaces::get(&s.pool, a.workspace_id)
            .await?
            .map_or_else(|| "UTC".to_owned(), |w| w.timezone),
    };
    let field = |code: &str, path: &str, e: ApiError| {
        ApiError::unprocessable(code, e.message().to_owned())
            .with_details(json!({"fields": [{"path": path, "message": e.message()}]}))
    };
    super::cron::parse_tz(&tz).map_err(|e| field("invalid_timezone", "/timezone", e))?;
    let min_interval_secs = super::cron::MIN_INTERVAL.num_seconds();
    let now = chrono::Utc::now();
    match (req.cron, req.run_at) {
        (Some(c), None) => {
            let p = super::cron::preview(&c, &tz, now)
                .map_err(|e| field("invalid_cron", "/cron", e))?;
            Ok(Json(SchedulePreview {
                next: p.next,
                interval_secs: p.interval_secs,
                too_frequent: p.too_frequent,
                min_interval_secs,
            }))
        }
        // (P5-SCHED-ONCE, C-1092) A one-time schedule: its one instant, or the create messages.
        (None, Some(at)) => {
            let at = super::once::validate(&at, &tz, now)
                .map_err(|e| once_field(e, "invalid_run_at"))?;
            Ok(Json(SchedulePreview {
                next: vec![at],
                interval_secs: None,
                too_frequent: false,
                min_interval_secs,
            }))
        }
        (Some(_), Some(_)) => Err(once_field(
            ApiError::invalid_field(
                "Invalid schedule.",
                "/run_at",
                "set either cron or run_at, not both",
            ),
            "invalid_run_at",
        )),
        (None, None) => Err(once_field(
            ApiError::invalid_field("Invalid schedule.", "/cron", "set either cron or run_at"),
            "invalid_cron",
        )),
    }
}

/// A `once` field error as the preview's 422 with `code`; its field message is the message.
fn once_field(e: ApiError, code: &str) -> ApiError {
    let msg = e
        .details()
        .and_then(|d| d["fields"][0]["message"].as_str())
        .unwrap_or(e.message())
        .to_owned();
    let path = e
        .details()
        .and_then(|d| d["fields"][0]["path"].as_str())
        .unwrap_or("/run_at")
        .to_owned();
    ApiError::unprocessable(code, msg.clone())
        .with_details(json!({"fields": [{"path": path, "message": msg}]}))
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
    let (run, conversation) = fire::run_now(&mut tx, &row).await?.ok_or_else(|| {
        ApiError::conflict("schedule_busy", "A run of this schedule is still active.")
    })?;
    tx.commit().await.map_err(DbError::from)?;
    record(&c, &s, &a, &row, "schedule.run_now");
    Ok((
        StatusCode::CREATED,
        Json(ScheduleRun::started(run, conversation)),
    ))
}

/// Reassign the schedule's creator (fire-time authority) to an active member who can use the
/// workspace (`can_administer`, FIX-234 C-1160; the new creator still needs own `can_use`);
/// `re_enable` re-enables an item disabled for `authorization`. Audited `automation.reassign`.
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
    // (FIX-234 review, C-1160) `can_administer` per docs/04: an org owner/admin reassigns the
    // creator without own access, so the load must not go through `Caller::workspace` (the
    // agent is loaded plainly and its workspace administered instead). The new creator still
    // needs own `can_use` — checked by the reassignment and again at every fire.
    let row = service::get(&s.pool, id).await?;
    let agent = agents::get(&s.pool, row.agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let a = c.administer(&s, agent.workspace_id).await?;
    let actor = AuditActor::from_caller(&c);
    let acting = Acting {
        actor: &actor,
        org: a.org_id,
        workspace: agent.workspace_id,
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
    let own_can_use = aurora_db::authz::own_access(&s.pool, c.user_id, agent.workspace_id)
        .await?
        .is_some();
    let mut out = vec![Schedule::from(service::get(&s.pool, id).await?)];
    // A write route: the caller holds `WorkspaceAuth`, never an emergency grant.
    redact_all(&c, &s, false, &mut out).await?;
    let out = out.remove(0);
    // (FIX-234 review round 3, C-1160) An administerer without own access gets the response
    // shape with the content emptied: the prompt and gate text a person wrote, the provenance
    // conversation and the newest run's output are content, and an administer route never
    // returns content. A caller with own `can_use` keeps everything.
    Ok(Json(if own_can_use {
        out
    } else {
        strip_schedule_content(out)
    }))
}

/// The content fields of a `Schedule` (see `reassign_creator`): emptied for a caller without
/// own `can_use`; the response shape is unchanged (the web discards this body anyway).
fn strip_schedule_content(mut s: Schedule) -> Schedule {
    s.prompt = String::new();
    s.gate_command = None;
    s.gate_description = None;
    s.origin_conversation_id = None;
    s.conversation_hidden = true;
    s.last_result = None;
    s
}
