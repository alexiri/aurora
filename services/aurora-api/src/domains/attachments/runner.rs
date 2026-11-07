//! Runner-plane attachment bytes (docs/16 §W5): outbound upload (`send_file`) and inbound
//! download for delivery (decision S-11).

use super::blob;
use super::service::{self, NewBlob, sanitize_filename};
use crate::app::AppState;
use aurora_core::limits::{
    ATTACH_CAPTION_MAX, ATTACH_OUT_COUNT, ATTACH_OUT_DAILY_PER_CONVERSATION, ATTACH_OUT_FILE_MAX,
};
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::repo::attachments;
use aurora_db::repo::conversations as convs;
use aurora_db::repo::runners::RunnerRow;
use aurora_db::repo::{sandboxes, turns};
use aurora_protocol::common::Validate;
use aurora_protocol::runner::{
    AttachmentUploadMeta, AttachmentUploadResponse, HDR_BOOT, HDR_CAPTION, HDR_CONVERSATION,
    HDR_FILENAME, HDR_SANDBOX, HDR_TURN,
};
use axum::body::Body;
use axum::response::Response;
use futures::StreamExt;
use http::{HeaderMap, StatusCode, header};
use percent_encoding::percent_decode_str;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

fn bad_header(name: &str, msg: &str) -> ApiError {
    ApiError::invalid_request(
        "Invalid attachment headers.",
        vec![FieldError::new(format!("/headers/{name}"), msg)],
    )
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ApiError> {
    h.get(name)
        .map(|v| v.to_str().map_err(|_| bad_header(name, "must be ASCII")))
        .transpose()
}

fn header_uuid(h: &HeaderMap, name: &str) -> Result<Uuid, ApiError> {
    header_str(h, name)?
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(|| bad_header(name, "a UUID is required"))
}

fn header_text(h: &HeaderMap, name: &str) -> Result<Option<String>, ApiError> {
    header_str(h, name)?
        .map(|v| {
            percent_decode_str(v)
                .decode_utf8()
                .map(|s| s.into_owned())
                .map_err(|_| bad_header(name, "must be percent-encoded UTF-8"))
        })
        .transpose()
}

fn parse_meta(h: &HeaderMap) -> Result<AttachmentUploadMeta, ApiError> {
    let meta = AttachmentUploadMeta {
        sandbox_id: header_uuid(h, HDR_SANDBOX)?,
        boot_id: header_uuid(h, HDR_BOOT)?,
        conversation_id: header_uuid(h, HDR_CONVERSATION)?,
        turn_id: header_uuid(h, HDR_TURN)?,
        filename: header_text(h, HDR_FILENAME)?.unwrap_or_default(),
        caption: header_text(h, HDR_CAPTION)?,
    };
    meta.validate()
        .map_err(|e| ApiError::invalid_request(e.to_string(), vec![]))?;
    Ok(meta)
}

fn quota(code: &str, msg: &str) -> ApiError {
    ApiError::new(StatusCode::TOO_MANY_REQUESTS, code, msg)
}

/// `POST /runner/attachments`: store an outbound file of the agent in the turn's conversation,
/// bound to the turn, with a durable `attachment` transcript event. The sandbox must be the
/// calling runner's and its boot current; conversation and turn must belong to the sandbox's
/// agent and the turn must be in flight.
pub async fn upload(
    state: &AppState,
    runner: &RunnerRow,
    headers: &HeaderMap,
    body: Body,
) -> Result<AttachmentUploadResponse, ApiError> {
    let meta = parse_meta(headers)?;
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    let sandbox = sandboxes::get(&mut conn, meta.sandbox_id)
        .await?
        .filter(|s| s.runner_id == Some(runner.id))
        .ok_or_else(|| ApiError::not_found("Unknown sandbox."))?;
    if sandbox.boot_id != Some(meta.boot_id) {
        return Err(ApiError::conflict("fenced", "Sandbox boot is not current."));
    }
    let conv = convs::get(&mut *conn, meta.conversation_id)
        .await?
        .filter(|c| c.agent_id == sandbox.agent_id)
        .ok_or_else(|| ApiError::not_found("Conversation not found."))?;
    let turn = turns::get(&mut *conn, meta.turn_id)
        .await?
        .filter(|t| t.role == "turn" && t.conversation_id == conv.id)
        .ok_or_else(|| ApiError::not_found("Turn not found."))?;
    if !matches!(turn.status.as_str(), "dispatched" | "running") {
        return Err(ApiError::conflict(
            "turn_not_active",
            "The turn is not in flight.",
        ));
    }
    drop(conn);

    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|n| n > ATTACH_OUT_FILE_MAX)
    {
        return Err(too_large());
    }
    // Streamed with the cap enforced per chunk.
    let mut buf: Vec<u8> = Vec::new();
    let mut hasher = Sha256::new();
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            ApiError::invalid_request(
                "Body could not be read.",
                vec![FieldError::new("/", "body read failed")],
            )
        })?;
        if (buf.len() + chunk.len()) as u64 > ATTACH_OUT_FILE_MAX {
            return Err(too_large());
        }
        hasher.update(&chunk);
        buf.extend_from_slice(&chunk);
    }
    let sha256 = format!("{:x}", hasher.finalize());
    let filename = sanitize_filename(&meta.filename);
    let caption = meta
        .caption
        .as_deref()
        .map(|c| aurora_core::sanitize::one_line(c, ATTACH_CAPTION_MAX))
        .filter(|c| !c.is_empty());

    // Org storage quota (RFE-015): cheap check before the bytes are written; `commit` decides
    // under the org's counter lock.
    super::quota::precheck(state, conv.workspace_id, buf.len() as i64).await?;
    let (backend, store) = blob::current(&state.pool);
    let key = blob::new_key();
    store.put(&key, &buf).await?;
    let result = commit(
        state,
        &conv.workspace_id,
        &meta,
        &filename,
        &sha256,
        caption.as_deref(),
        &key,
        backend,
        buf.len(),
    )
    .await;
    if result.is_err()
        && let Err(e) = store.delete(&key).await
    {
        tracing::warn!(error = %e, "orphan blob after failed outbound attachment");
    }
    Ok(AttachmentUploadResponse {
        attachment_id: result?,
    })
}

fn too_large() -> ApiError {
    ApiError::too_large("File is too large.")
}

#[allow(clippy::too_many_arguments)]
async fn commit(
    state: &AppState,
    workspace_id: &Uuid,
    meta: &AttachmentUploadMeta,
    filename: &str,
    sha256: &str,
    caption: Option<&str>,
    key: &str,
    backend: &str,
    size: usize,
) -> Result<Uuid, ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    // The conversation lock serializes concurrent uploads, so the caps cannot be overshot.
    if !convs::lock(&mut tx, meta.conversation_id).await? {
        return Err(ApiError::not_found("Conversation not found."));
    }
    let (turn_count, daily_bytes) =
        attachments::out_usage(&mut tx, meta.conversation_id, meta.turn_id).await?;
    if turn_count >= i64::from(ATTACH_OUT_COUNT) {
        return Err(quota(
            "attachment_count_exceeded",
            "Too many files sent in this turn.",
        ));
    }
    if daily_bytes + size as i64 > ATTACH_OUT_DAILY_PER_CONVERSATION as i64 {
        return Err(quota(
            "attachment_quota_exceeded",
            "The conversation's daily file quota is used up.",
        ));
    }
    let row = service::insert_in(
        state,
        &mut tx,
        NewBlob {
            workspace_id: *workspace_id,
            conversation_id: meta.conversation_id,
            turn_id: Some(meta.turn_id),
            direction: "out",
            status: "bound",
            filename,
            content_type: "application/octet-stream",
            sha256,
            caption,
            created_by_user_id: None,
        },
        key,
        backend,
        size,
    )
    .await?;
    let mut payload = json!({
        "turn_id": meta.turn_id,
        "attachment_id": row.id,
        "filename": row.filename,
        "size": row.size_bytes,
    });
    if let Some(c) = caption {
        payload["caption"] = json!(c);
    }
    crate::domains::conversations::service::append_event(
        &mut tx,
        meta.conversation_id,
        Some(meta.turn_id),
        "attachment",
        &payload,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(row.id)
}

/// `GET /runner/attachments/{id}`: inbound bytes, only for attachments bound to a turn in flight
/// on a sandbox of this runner (anything else is 404).
pub async fn download(
    state: &AppState,
    runner: &RunnerRow,
    id: Uuid,
) -> Result<Response, ApiError> {
    let row = attachments::get_for_runner(&state.pool, id, runner.id)
        .await?
        .ok_or_else(|| ApiError::not_found("Attachment not found."))?;
    let bytes = service::read_blob(state, &row).await?;
    Ok(service::bytes_response(&row, bytes))
}
