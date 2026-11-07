//! Agent avatars (docs/10 §agent fields, 05, 17 SEC-30; contract C-41).
//!
//! `PUT /agents/{agent}/avatar` takes the **raw image bytes** as the body (the client's
//! `Content-Type` is ignored: the type is sniffed). The image is decoded under explicit limits,
//! cover-cropped to 256×256 and re-encoded as a *fresh* PNG, so no input byte, metadata chunk
//! or trailing data is ever stored or served. The public key rotates on every change.

use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::attachments::blob;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::Path;
use aurora_core::ApiError;
use aurora_core::limits::AVATAR_MAX;
use aurora_db::repo::agents;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use image::{ImageDecoder, ImageFormat, ImageReader, Limits, imageops::FilterType};
use serde_json::json;
use std::io::Cursor;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

/// Output edge length in pixels.
pub const AVATAR_PX: u32 = 256;
/// Decoder limits (decompression-bomb guard).
const MAX_DIM: u32 = 4096;
const MAX_ALLOC: u64 = 64 * 1024 * 1024;
const PUBLIC_PREFIX: &str = "/v1/public/avatars/";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(put_avatar, get_avatar, delete_avatar))
        .routes(routes!(get_public_avatar))
}

/// Root-relative public path of an avatar (the Agent DTO's `avatar_url`).
pub fn public_path(key: &str) -> String {
    format!("{PUBLIC_PREFIX}{key}")
}

/// Absolute public URL (for chat platforms, which fetch it themselves).
pub fn public_url(external_url: &str, key: &str) -> String {
    format!("{}{}", external_url.trim_end_matches('/'), public_path(key))
}

fn invalid(msg: &str) -> ApiError {
    ApiError::unprocessable("avatar_invalid", msg)
}

fn not_found() -> ApiError {
    ApiError::not_found("Avatar not found.")
}

/// Decode (bounded), cover-crop and re-encode. Pure CPU; call from `spawn_blocking`.
pub fn reencode(bytes: &[u8]) -> Result<Vec<u8>, ApiError> {
    let fmt = image::guess_format(bytes)
        .ok()
        .filter(|f| matches!(f, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP))
        .ok_or_else(|| invalid("The avatar must be a PNG, JPEG or WebP image."))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIM);
    limits.max_image_height = Some(MAX_DIM);
    limits.max_alloc = Some(MAX_ALLOC);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), fmt);
    reader.limits(limits);
    let bad = |_| invalid("The image could not be decoded (corrupt, or larger than 4096×4096).");
    let mut decoder = reader.into_decoder().map_err(bad)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(bad)?;
    img.apply_orientation(orientation);
    let out = img
        .resize_to_fill(AVATAR_PX, AVATAR_PX, FilterType::Lanczos3)
        .to_rgba8();
    let mut png = Vec::new();
    out.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| {
            tracing::error!(error = %e, "avatar png encode failed");
            ApiError::internal("The avatar could not be encoded.")
        })?;
    Ok(png)
}

fn new_public_key() -> Result<String, ApiError> {
    let mut b = [0u8; 32];
    aws_lc_rs::rand::fill(&mut b)
        .map_err(|_| ApiError::internal("Random generator unavailable."))?;
    Ok(URL_SAFE_NO_PAD.encode(b))
}

async fn visible(c: &Caller, s: &AppState, id: Uuid) -> Result<agents::AgentRow, ApiError> {
    let row = agents::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    c.workspace(s, row.workspace_id).await?;
    Ok(row)
}

fn png_response(bytes: Vec<u8>, cache: &'static str) -> Response {
    let mut r = Response::new(Body::from(bytes));
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    r
}

/// Read the body, refusing anything over `AVATAR_MAX` with 413.
async fn read_body(headers: &axum::http::HeaderMap, body: Body) -> Result<Vec<u8>, ApiError> {
    let too_large = || ApiError::too_large("The avatar is larger than 512 KiB.");
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|n| n > AVATAR_MAX)
    {
        return Err(too_large());
    }
    let bytes = axum::body::to_bytes(body, AVATAR_MAX)
        .await
        .map_err(|_| too_large())?;
    Ok(bytes.to_vec())
}

/// Set the agent's avatar: the raw PNG/JPEG/WebP bytes as the request body (≤ 512 KiB). It is
/// re-encoded to a 256×256 PNG; the public key rotates (`can_use`).
#[utoipa::path(put, path = "/agents/{agent}/avatar", operation_id = "putAgentAvatar", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses((status = 200, description = "Agent", body = super::dto::Agent),
        (status = 404, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
        (status = 422, description = "avatar_invalid", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn put_avatar(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    body: Body,
) -> Result<axum::Json<super::dto::Agent>, ApiError> {
    let before = visible(&c, &s, id).await?;
    let raw = read_body(&headers, body).await?;
    let png = tokio::task::spawn_blocking(move || reencode(&raw))
        .await
        .map_err(|_| ApiError::internal("Avatar processing failed."))??;
    let (_, store) = blob::current(&s.pool);
    let blob_key = blob::new_key();
    store.put(&blob_key, &png).await?;
    let public_key = new_public_key()?;
    let old = match agents::set_avatar(&s.pool, id, Some(&blob_key), Some(&public_key)).await {
        Ok(Some(old)) => old,
        Ok(None) => {
            let _ = store.delete(&blob_key).await;
            return Err(ApiError::not_found("Agent not found."));
        }
        Err(e) => {
            let _ = store.delete(&blob_key).await;
            return Err(e.into());
        }
    };
    if let Some(old) = old
        && let Err(e) = store.delete(&old).await
    {
        tracing::warn!(error = %e.message(), agent_id = %id, "previous avatar blob not deleted");
    }
    sync(&s, id, Some(&public_key)).await;
    record(&s, &c, &before, "agent.avatar_set");
    let row = visible(&c, &s, id).await?;
    Ok(axum::Json(row.into()))
}

/// Remove the agent's avatar (`can_use`).
#[utoipa::path(delete, path = "/agents/{agent}/avatar", operation_id = "deleteAgentAvatar", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 204, description = "Removed"), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_avatar(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let before = visible(&c, &s, id).await?;
    let old = agents::set_avatar(&s.pool, id, None, None)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    if let Some(old) = old {
        let (_, store) = blob::current(&s.pool);
        if let Err(e) = store.delete(&old).await {
            tracing::warn!(error = %e.message(), agent_id = %id, "avatar blob not deleted");
        }
        sync(&s, id, None).await;
    }
    record(&s, &c, &before, "agent.avatar_remove");
    Ok(StatusCode::NO_CONTENT)
}

/// The agent's avatar PNG (`can_use`); 404 when none is set.
#[utoipa::path(get, path = "/agents/{agent}/avatar", operation_id = "getAgentAvatar", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, description = "PNG", content_type = "image/png", body = Vec<u8>),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_avatar(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let row = visible(&c, &s, id).await?;
    // Distinct from `not_found`: the caller may see the agent, it just has no avatar.
    let no_avatar = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "no_avatar",
            "The agent has no avatar.",
        )
    };
    let key = row.avatar_blob_key.ok_or_else(no_avatar)?;
    let (_, store) = blob::current(&s.pool);
    let bytes = store.get(&key).await?.ok_or_else(no_avatar)?;
    Ok(png_response(bytes, "private, no-cache"))
}

/// Public avatar by its current key (a capability: no authentication). Rotated keys give 404.
#[utoipa::path(get, path = "/public/avatars/{key}", operation_id = "getPublicAvatar", tag = "agents",
    params(("key" = String, Path, description = "Current avatar public key")),
    responses((status = 200, description = "PNG", content_type = "image/png", body = Vec<u8>),
        (status = 404, body = ErrorEnvelope)),
    security(()))]
async fn get_public_avatar(
    State(s): State<AppState>,
    Path(key): Path<String>,
) -> Result<Response, ApiError> {
    // Shape check only avoids a query for junk; the answer is the same 404 either way.
    if key.len() != 43
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(not_found());
    }
    let blob_key = agents::avatar_blob_by_public_key(&s.pool, &key)
        .await?
        .ok_or_else(not_found)?;
    let (_, store) = blob::current(&s.pool);
    let bytes = store.get(&blob_key).await?.ok_or_else(not_found)?;
    let mut r = png_response(bytes, "public, max-age=3600");
    r.headers_mut().insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("cross-origin"),
    );
    Ok(r)
}

/// Queue `presence.set_icon` for the agent's active dedicated presences (best effort: the
/// avatar change itself has succeeded).
async fn sync(s: &AppState, agent: Uuid, public_key: Option<&str>) {
    let url = public_key.map(|k| public_url(&s.config.external_url, k));
    if let Err(e) =
        crate::domains::channels::presences::on_avatar_changed(s, agent, url.as_deref()).await
    {
        tracing::warn!(error = %e.message(), agent_id = %agent, "presence.set_icon not queued");
    }
}

fn record(s: &AppState, c: &Caller, row: &agents::AgentRow, action: &'static str) {
    audit::record(
        s,
        AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
            .org(row.org_id)
            .workspace(row.workspace_id)
            .target("agent", row.id)
            .metadata(json!({"handle": row.handle})),
    );
}
