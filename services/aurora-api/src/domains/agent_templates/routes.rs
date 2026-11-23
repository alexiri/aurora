//! Admin routes for agent template sources (C-1107, P6-TPL-2). All IA, all audited.

use super::admin;
use super::dto::{
    AddTemplateSourceRequest, AdminTemplateList, AdminTemplateView, AgentTemplateSource,
    AgentTemplateSourceList, AgentTemplateSourcePreview, ApproveRequest,
    PreviewTemplateSourceRequest, TemplateSourceCheckAll, TemplateSourceCheckResult,
    TemplateVersionDetail, UpdateTemplateRequest, UpdateTemplateSourceRequest,
};
use super::{add, check_all, check_now, preview, remove, source_dto, update};
use crate::app::AppState;
use crate::audit::AuditActor;
use crate::http::auth::InstanceAdmin;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_sources, create_source))
        .routes(routes!(preview_source))
        .routes(routes!(get_source, delete_source, update_source))
        .routes(routes!(check_all_route))
        .routes(routes!(check_source_route))
        .routes(routes!(list_templates_route))
        .routes(routes!(get_version_route))
        .routes(routes!(avatar_route))
        .routes(routes!(approve_route))
        .routes(routes!(reject_route))
        .routes(routes!(update_template_route))
}

/// The preview answer is never cached (C-1107, as C-1124)./// The preview answer is never cached (C-1107, as C-1124).
fn no_store<T: serde::Serialize>(body: T) -> Result<Response, ApiError> {
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// List the template sources (instance administrators).
#[utoipa::path(get, path = "/admin/agent-template-sources", operation_id = "adminListTemplateSources", tag = "agent-templates",
    responses((status = 200, body = AgentTemplateSourceList), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_sources(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<AgentTemplateSourceList>, ApiError> {
    let rows = aurora_db::repo::agent_templates::list_sources(&s.pool).await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in &rows {
        items.push(source_dto(&s, r).await?);
    }
    Ok(Json(AgentTemplateSourceList { items }))
}

/// Preview a repository before adding it (C-1107, as C-1124): read once, store nothing.
#[utoipa::path(post, path = "/admin/agent-template-sources/preview", operation_id = "adminPreviewTemplateSource", tag = "agent-templates",
    request_body = PreviewTemplateSourceRequest,
    responses((status = 200, body = AgentTemplateSourcePreview), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn preview_source(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<PreviewTemplateSourceRequest>,
) -> Result<Response, ApiError> {
    let body = preview(&s, c.user_id, &req).await?;
    no_store(body)
}

/// Add a source (C-1107): one audited transaction; the token is sealed, never returned.
#[utoipa::path(post, path = "/admin/agent-template-sources", operation_id = "adminAddTemplateSource", tag = "agent-templates",
    request_body = AddTemplateSourceRequest,
    responses((status = 201, body = AgentTemplateSource), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_source(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<AddTemplateSourceRequest>,
) -> Result<(StatusCode, Json<AgentTemplateSource>), ApiError> {
    let actor = AuditActor::instance_admin(&c);
    let row = add(&s, &actor, c.user_id, &req).await?;
    Ok((StatusCode::CREATED, Json(source_dto(&s, &row).await?)))
}

/// One template source, from its stored rows (never a live read).
#[utoipa::path(get, path = "/admin/agent-template-sources/{id}", operation_id = "adminGetTemplateSource", tag = "agent-templates",
    params(("id" = Uuid, Path, description = "Template source id")),
    responses((status = 200, body = AgentTemplateSource), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_source(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<AgentTemplateSource>, ApiError> {
    let r = aurora_db::repo::agent_templates::get_source(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Template source not found."))?;
    Ok(Json(source_dto(&s, &r).await?))
}

/// Edit a source's schedule, branch, folder or token (C-1107); a change of branch, folder or
/// token re-reads the repository first.
#[utoipa::path(patch, path = "/admin/agent-template-sources/{id}", operation_id = "adminUpdateTemplateSource", tag = "agent-templates",
    params(("id" = Uuid, Path, description = "Template source id")),
    request_body = UpdateTemplateSourceRequest,
    responses((status = 200, body = AgentTemplateSource), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn update_source(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateTemplateSourceRequest>,
) -> Result<Json<AgentTemplateSource>, ApiError> {
    let actor = AuditActor::instance_admin(&c);
    let row = update(&s, &actor, id, &req).await?;
    Ok(Json(source_dto(&s, &row).await?))
}

/// Remove a source (C-1107): always allowed; its templates and versions go with it. Agents made
/// from them do not change (they are copies).
#[utoipa::path(delete, path = "/admin/agent-template-sources/{id}", operation_id = "adminDeleteTemplateSource", tag = "agent-templates",
    params(("id" = Uuid, Path, description = "Template source id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn delete_source(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    remove(&s, &AuditActor::instance_admin(&c), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Check a source now (C-1107, C-596's lease rules): the shared C-1108 path, synchronous.
/// A read failure is a result, not an error.
#[utoipa::path(post, path = "/admin/agent-template-sources/{id}/check", operation_id = "adminCheckTemplateSource", tag = "agent-templates",
    params(("id" = Uuid, Path, description = "Template source id")),
    responses((status = 200, body = TemplateSourceCheckResult), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn check_source_route(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<TemplateSourceCheckResult>, ApiError> {
    Ok(Json(
        check_now(&s, &AuditActor::instance_admin(&c), id).await?,
    ))
}

/// Check every source (C-1107): sequential; a failing or busy source never blocks the others.
#[utoipa::path(post, path = "/admin/agent-template-sources/check-all", operation_id = "adminCheckAllTemplateSources", tag = "agent-templates",
    responses((status = 200, body = TemplateSourceCheckAll), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn check_all_route(
    c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<TemplateSourceCheckAll>, ApiError> {
    Ok(Json(check_all(&s, &AuditActor::instance_admin(&c)).await?))
}

// ---- the templates (C-1108) ---------------------------------------------------------------------

/// The templates of every source, with their versions and what they need (instance
/// administrators).
#[utoipa::path(get, path = "/admin/agent-templates", operation_id = "adminListTemplates", tag = "agent-templates",
    responses((status = 200, body = AdminTemplateList), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_templates_route(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<AdminTemplateList>, ApiError> {
    Ok(Json(admin::list_templates(&s).await?))
}

/// One version of a template: everything the review shows, in full (C-1108).
#[utoipa::path(get, path = "/admin/agent-templates/{template_id}/versions/{version}", operation_id = "adminGetTemplateVersion", tag = "agent-templates",
    params(("template_id" = String, Path, description = "Template id"), ("version" = String, Path, description = "Version")),
    responses((status = 200, body = TemplateVersionDetail), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_version_route(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Path((template_id, version)): Path<(String, String)>,
) -> Result<Json<TemplateVersionDetail>, ApiError> {
    Ok(Json(
        admin::version_detail(&s, &template_id, &version).await?,
    ))
}

/// The version's avatar (C-1108), `image/png` from the stored content.
#[utoipa::path(get, path = "/admin/agent-templates/{template_id}/versions/{version}/avatar", operation_id = "adminGetTemplateVersionAvatar", tag = "agent-templates",
    params(("template_id" = String, Path, description = "Template id"), ("version" = String, Path, description = "Version")),
    responses((status = 200, body = Vec<u8>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn avatar_route(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Path((template_id, version)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let bytes = admin::avatar(&s, &template_id, &version).await?;
    Ok(([(header::CONTENT_TYPE, "image/png")], bytes).into_response())
}

/// Approve a version (C-1108): only from `pending_approval`, bound to the reviewed
/// `content_sha256`.
#[utoipa::path(post, path = "/admin/agent-templates/{template_id}/versions/{version}/approve", operation_id = "adminApproveTemplateVersion", tag = "agent-templates",
    params(("template_id" = String, Path, description = "Template id"), ("version" = String, Path, description = "Version")),
    request_body = ApproveRequest,
    responses((status = 200, body = TemplateVersionDetail), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn approve_route(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path((template_id, version)): Path<(String, String)>,
    Json(req): Json<ApproveRequest>,
) -> Result<Json<TemplateVersionDetail>, ApiError> {
    Ok(Json(
        admin::approve(
            &s,
            &AuditActor::instance_admin(&c),
            &template_id,
            &version,
            &req,
        )
        .await?,
    ))
}

/// Reject a version (C-1108): that version string is never ingested again.
#[utoipa::path(post, path = "/admin/agent-templates/{template_id}/versions/{version}/reject", operation_id = "adminRejectTemplateVersion", tag = "agent-templates",
    params(("template_id" = String, Path, description = "Template id"), ("version" = String, Path, description = "Version")),
    responses((status = 200, body = TemplateVersionDetail), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn reject_route(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path((template_id, version)): Path<(String, String)>,
) -> Result<Json<TemplateVersionDetail>, ApiError> {
    Ok(Json(
        admin::reject(&s, &AuditActor::instance_admin(&c), &template_id, &version).await?,
    ))
}

/// Withdraw a template or offer it again (C-1108): people keep using the current version
/// while it is offered; the stored versions are untouched.
#[utoipa::path(patch, path = "/admin/agent-templates/{template_id}", operation_id = "adminUpdateTemplate", tag = "agent-templates",
    params(("template_id" = String, Path, description = "Template id")),
    request_body = UpdateTemplateRequest,
    responses((status = 200, body = AdminTemplateView), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn update_template_route(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(template_id): Path<String>,
    Json(req): Json<UpdateTemplateRequest>,
) -> Result<Json<AdminTemplateView>, ApiError> {
    Ok(Json(
        admin::set_offered(&s, &AuditActor::instance_admin(&c), &template_id, &req).await?,
    ))
}
