use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
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
}

/// Upload a file to a conversation (multipart part `file`); it stays `pending` until a message
/// references it.
#[utoipa::path(post, path = "/conversations/{id}/attachments", operation_id = "uploadAttachment", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id")),
    request_body(content = UploadAttachmentRequest, content_type = "multipart/form-data"),
    responses((status = 201, body = Attachment), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope)),
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
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn download_attachment(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    service::download(&s, &c, id).await
}
