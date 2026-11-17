use super::dto::{
    CreateEvalQuestion, EvalFingerprint, EvalQuestion, EvalQuestionList, EvalSettings,
    PatchEvalQuestion, PutEvalSettings,
};
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_questions, create_question))
        .routes(routes!(get_question, patch_question, delete_question))
        .routes(routes!(get_settings, put_settings))
        .routes(routes!(get_fingerprint))
}

/// An agent's eval questions in position order (`can_use`; hosted agents only).
#[utoipa::path(get, path = "/agents/{agent}/eval-questions", operation_id = "listEvalQuestions",
    tag = "evals", params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = EvalQuestionList), (status = 404, body = ErrorEnvelope),
        (status = 422, description = "eval_not_supported", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_questions(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<EvalQuestionList>, ApiError> {
    Ok(Json(EvalQuestionList {
        questions: service::list_questions(&s, &c, agent).await?,
    }))
}

/// Add a question at the end of the list.
#[utoipa::path(post, path = "/agents/{agent}/eval-questions", operation_id = "createEvalQuestion",
    tag = "evals", params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = CreateEvalQuestion,
    responses((status = 201, body = EvalQuestion), (status = 404, body = ErrorEnvelope),
        (status = 422, description = "invalid, eval_question_limit, eval_not_supported", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_question(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(b): Json<CreateEvalQuestion>,
) -> Result<(StatusCode, Json<EvalQuestion>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(service::create_question(&s, &c, agent, b).await?),
    ))
}

/// One question.
#[utoipa::path(get, path = "/eval-questions/{id}", operation_id = "getEvalQuestion", tag = "evals",
    params(("id" = Uuid, Path, description = "Question id")),
    responses((status = 200, body = EvalQuestion), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_question(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<EvalQuestion>, ApiError> {
    Ok(Json(service::get_question(&s, &c, id).await?))
}

/// Change a question (any subset of its fields, or its position); bumps `version` when a field
/// changes. The kind is fixed.
#[utoipa::path(patch, path = "/eval-questions/{id}", operation_id = "updateEvalQuestion", tag = "evals",
    params(("id" = Uuid, Path, description = "Question id")),
    request_body = PatchEvalQuestion,
    responses((status = 200, body = EvalQuestion), (status = 404, body = ErrorEnvelope),
        (status = 422, description = "invalid", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_question(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<PatchEvalQuestion>,
) -> Result<Json<EvalQuestion>, ApiError> {
    Ok(Json(service::patch_question(&s, &c, id, b).await?))
}

/// Delete a question; the positions after it close up.
#[utoipa::path(delete, path = "/eval-questions/{id}", operation_id = "deleteEvalQuestion", tag = "evals",
    params(("id" = Uuid, Path, description = "Question id")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_question(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service::delete_question(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The agent's eval settings: the judge key and the regression notice.
#[utoipa::path(get, path = "/agents/{agent}/eval-settings", operation_id = "getEvalSettings",
    tag = "evals", params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = EvalSettings), (status = 404, body = ErrorEnvelope),
        (status = 422, description = "eval_not_supported", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_settings(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<EvalSettings>, ApiError> {
    Ok(Json(service::get_settings(&s, &c, agent).await?))
}

/// Replace the eval settings. A change of the judge key needs `can_own`.
#[utoipa::path(put, path = "/agents/{agent}/eval-settings", operation_id = "putEvalSettings",
    tag = "evals", params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = PutEvalSettings,
    responses((status = 200, body = EvalSettings), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 422, description = "eval_judge_not_granted, eval_judge_not_usable, eval_judge_not_permitted, invalid", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn put_settings(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(b): Json<PutEvalSettings>,
) -> Result<Json<EvalSettings>, ApiError> {
    Ok(Json(service::put_settings(&s, &c, agent, b).await?))
}

/// The configuration fingerprint a run started now would record.
#[utoipa::path(get, path = "/agents/{agent}/eval-fingerprint", operation_id = "getEvalFingerprint",
    tag = "evals", params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = EvalFingerprint), (status = 404, body = ErrorEnvelope),
        (status = 422, description = "eval_not_supported", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_fingerprint(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<EvalFingerprint>, ApiError> {
    Ok(Json(service::get_fingerprint(&s, &c, agent).await?))
}
