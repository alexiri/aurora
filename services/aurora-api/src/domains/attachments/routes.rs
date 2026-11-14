use super::dto::*;
use super::service;
use super::storage::{self, AdminStorage, OrgStorage};
use crate::app::AppState;
use crate::http::auth::{Caller, InstanceAdmin};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use aurora_core::limits::ATTACH_IN_FILE_MAX;
use axum::extract::{DefaultBodyLimit, Multipart, State, multipart::MultipartRejection};
use axum::response::Response;
use http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

/// Multipart framing overhead on top of the file cap.
const FRAMING: usize = 64 * 1024;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(upload_attachment))
        // The default body limit is raised for the upload route only.
        .layer(DefaultBodyLimit::max(ATTACH_IN_FILE_MAX as usize + FRAMING))
        .routes(routes!(download_attachment))
        .routes(routes!(delete_attachment))
        .routes(routes!(org_storage))
        .routes(routes!(refresh_org_storage))
        .routes(routes!(admin_storage))
        .routes(routes!(refresh_admin_storage))
}

/// Upload a file to a conversation (multipart part `file`); it stays `pending` until a message
/// references it.
#[utoipa::path(post, path = "/conversations/{id}/attachments", operation_id = "uploadAttachment", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id")),
    request_body(content = UploadAttachmentRequest, content_type = "multipart/form-data"),
    responses((status = 201, body = Attachment), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
        (status = 422, description = "attachment_quota_exceeded", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn upload_attachment(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<Attachment>), ApiError> {
    let out = service::upload(&s, &c, id, mp).await?;
    Ok((StatusCode::CREATED, Json(out)))
}

/// File bytes. Always `application/octet-stream` with `Content-Disposition: attachment` and
/// `nosniff`, whatever the stored type.
#[utoipa::path(get, path = "/attachments/{id}/content", operation_id = "downloadAttachment", tag = "conversations",
    params(("id" = Uuid, Path, description = "Attachment id")),
    responses((status = 200, description = "File bytes", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 404, body = ErrorEnvelope),
        (status = 410, description = "attachment_purged", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn download_attachment(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    service::download(&s, &c, id).await
}

/// Purge the file's bytes now (phase 2, RFE-015). The row stays, so the transcript chip shows
/// "expired"; downloads then answer 410. The uploader or a workspace owner.
#[utoipa::path(delete, path = "/attachments/{id}", operation_id = "deleteAttachment", tag = "conversations",
    params(("id" = Uuid, Path, description = "Attachment id")),
    responses((status = 204, description = "Deleted (idempotent)"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_attachment(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service::delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Attachment storage of an organization: usage vs quota, retention and the breakdown by
/// workspace and direction (phase 2, RFE-015; org `owner`/`admin`).
#[utoipa::path(get, path = "/orgs/{org}/storage", operation_id = "getOrgStorage", tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = OrgStorage), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn org_storage(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<OrgStorage>, ApiError> {
    Ok(Json(storage::org_storage(&s, &c, org, false).await?))
}

/// Recompute the organization's storage snapshot now (FIX-126; org `owner`/`admin`; the strict
/// `export` rate class).
#[utoipa::path(post, path = "/orgs/{org}/storage/refresh", operation_id = "refreshOrgStorage",
    tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = OrgStorage), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn refresh_org_storage(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<OrgStorage>, ApiError> {
    Ok(Json(storage::org_storage(&s, &c, org, true).await?))
}

/// Instance storage breakdown (phase 2, RFE-015; instance administrators).
#[utoipa::path(get, path = "/admin/storage", operation_id = "adminStorage", tag = "admin",
    responses((status = 200, body = AdminStorage), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_storage(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<AdminStorage>, ApiError> {
    Ok(Json(storage::admin_storage(&s, false).await?))
}

/// Recompute the instance storage snapshot now (FIX-126; instance administrators; the strict
/// `export` rate class).
#[utoipa::path(post, path = "/admin/storage/refresh", operation_id = "refreshAdminStorage",
    tag = "admin",
    responses((status = 200, body = AdminStorage), (status = 403, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn refresh_admin_storage(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<AdminStorage>, ApiError> {
    Ok(Json(storage::admin_storage(&s, true).await?))
}
