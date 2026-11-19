//! Routes of eval runs and eval schedules (P4-EVAL-2; C-864, C-870, docs/05 §Evals).

use super::dto::{
    CreateEvalSchedule, EvalRun, EvalRunDetail, EvalSchedule, EvalScheduleList, PatchEvalSchedule,
};
use super::{runs, schedules};
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::ApiError;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_runs, start_run))
        .routes(routes!(get_run, delete_run))
        .routes(routes!(cancel_run))
        .routes(routes!(list_schedules, create_schedule))
        .routes(routes!(get_schedule, patch_schedule, delete_schedule))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct RunListQuery {
    /// Page size, 1..=200 (default 50).
    limit: Option<u32>,
    /// Opaque cursor from a previous response's `next_cursor`.
    cursor: Option<String>,
    /// `queued` | `running` | `done` | `failed` | `cancelled`.
    status: Option<String>,
    /// `manual` | `schedule`.
    trigger: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct ResultsQuery {
    /// Position after which the next page of results starts (default 0).
    results_after: Option<i32>,
}

/// An agent's eval runs, newest first.
#[utoipa::path(get, path = "/agents/{agent}/eval-runs", operation_id = "listEvalRuns", tag = "evals",
    params(("agent" = Uuid, Path, description = "Agent id"), RunListQuery),
    responses((status = 200, body = Page<EvalRun>), (status = 404, body = ErrorEnvelope),
        (status = 422, description = "eval_not_supported, invalid", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_runs(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<RunListQuery>,
) -> Result<Json<Page<EvalRun>>, ApiError> {
    Ok(Json(
        runs::list(&s, &c, agent, q.status, q.trigger, (q.limit, q.cursor)).await?,
    ))
}

/// Start a run: it is queued and the due-work arm runs its questions one at a time.
#[utoipa::path(post, path = "/agents/{agent}/eval-runs", operation_id = "startEvalRun", tag = "evals",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 202, body = EvalRun), (status = 404, body = ErrorEnvelope),
        (status = 409, description = "eval_run_active", body = ErrorEnvelope),
        (status = 422, description = "eval_no_questions, eval_not_supported", body = ErrorEnvelope),
        (status = 423, description = "frozen", body = ErrorEnvelope),
        (status = 429, description = "eval_runs_limited", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn start_run(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<(StatusCode, Json<EvalRun>), ApiError> {
    Ok((
        StatusCode::ACCEPTED,
        Json(runs::start(&s, &c, agent).await?),
    ))
}

/// A run with its fingerprint and one page of its results.
#[utoipa::path(get, path = "/eval-runs/{id}", operation_id = "getEvalRun", tag = "evals",
    params(("id" = Uuid, Path, description = "Run id"), ResultsQuery),
    responses((status = 200, body = EvalRunDetail), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_run(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<ResultsQuery>,
) -> Result<Json<EvalRunDetail>, ApiError> {
    Ok(Json(
        runs::get(&s, &c, id, q.results_after.unwrap_or(0).max(0)).await?,
    ))
}

/// Delete a finished run, its results and its eval conversations (`can_own`).
#[utoipa::path(delete, path = "/eval-runs/{id}", operation_id = "deleteEvalRun", tag = "evals",
    params(("id" = Uuid, Path, description = "Run id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, description = "eval_run_active", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_run(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    runs::delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Cancel a queued or running run; the remaining questions are not scored.
#[utoipa::path(post, path = "/eval-runs/{id}/cancel", operation_id = "cancelEvalRun", tag = "evals",
    params(("id" = Uuid, Path, description = "Run id")),
    responses((status = 202, body = EvalRun), (status = 404, body = ErrorEnvelope),
        (status = 409, description = "eval_run_finished", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn cancel_run(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<EvalRun>), ApiError> {
    Ok((StatusCode::ACCEPTED, Json(runs::cancel(&s, &c, id).await?)))
}

/// An agent's eval schedules.
#[utoipa::path(get, path = "/agents/{agent}/eval-schedules", operation_id = "listEvalSchedules", tag = "evals",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = EvalScheduleList), (status = 404, body = ErrorEnvelope),
        (status = 422, description = "eval_not_supported", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_schedules(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<EvalScheduleList>, ApiError> {
    Ok(Json(schedules::list(&s, &c, agent).await?))
}

/// Create an eval schedule (at least one hour between occurrences).
#[utoipa::path(post, path = "/agents/{agent}/eval-schedules", operation_id = "createEvalSchedule", tag = "evals",
    params(("agent" = Uuid, Path, description = "Agent id")), request_body = CreateEvalSchedule,
    responses((status = 201, body = EvalSchedule), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 422, description = "invalid, eval_schedule_limit, eval_not_supported", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_schedule(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(b): Json<CreateEvalSchedule>,
) -> Result<(StatusCode, Json<EvalSchedule>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(schedules::create(&s, &c, agent, b).await?),
    ))
}

/// One eval schedule.
#[utoipa::path(get, path = "/eval-schedules/{id}", operation_id = "getEvalSchedule", tag = "evals",
    params(("id" = Uuid, Path, description = "Schedule id")),
    responses((status = 200, body = EvalSchedule), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_schedule(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<EvalSchedule>, ApiError> {
    Ok(Json(schedules::get(&s, &c, id).await?))
}

/// Edit an eval schedule.
#[utoipa::path(patch, path = "/eval-schedules/{id}", operation_id = "patchEvalSchedule", tag = "evals",
    params(("id" = Uuid, Path, description = "Schedule id")), request_body = PatchEvalSchedule,
    responses((status = 200, body = EvalSchedule), (status = 404, body = ErrorEnvelope),
        (status = 422, description = "invalid", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_schedule(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<PatchEvalSchedule>,
) -> Result<Json<EvalSchedule>, ApiError> {
    Ok(Json(schedules::patch(&s, &c, id, b).await?))
}

/// Delete an eval schedule (its past runs stay).
#[utoipa::path(delete, path = "/eval-schedules/{id}", operation_id = "deleteEvalSchedule", tag = "evals",
    params(("id" = Uuid, Path, description = "Schedule id")),
    responses((status = 204), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_schedule(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    schedules::delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
