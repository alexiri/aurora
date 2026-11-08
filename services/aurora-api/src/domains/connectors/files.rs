//! Connector file transfer (docs/13 §4): `PUT /files/{op_id}` (bytes for a claimed `file.fetch`
//! op) and `GET /files/{attachment_id}` (bytes of a file referenced by a claimed `message.post`).
//!
//! Decisions:
//! * the stored cap is `min(op.max_bytes, ATTACH_IN_FILE_MAX)`, enforced while reading the body;
//! * the op payload (set by the core, BE-072) names the destination: `conversation_id`
//!   (required; the workspace is taken from the conversation), optional `turn_id` and
//!   `caption`. With a `turn_id` the attachment is stored `bound`, otherwise `pending`;
//! * the filename comes from `X-Aurora-Filename`, sanitized like user uploads;
//! * single-shot: the first successful PUT records `_attachment_id`/`_attachment_sha256` on the
//!   op row, in the transaction that inserts the attachment and under the op's row lock, so
//!   concurrent PUTs cannot both win. A later PUT with identical bytes (same sha256) returns the
//!   same `attachment_id` (idempotent retry); different bytes get 409 `file_already_uploaded`;
//! * downloads are served with the same fixed safe headers as dashboard downloads.

use super::auth::ConnectorCtx;
use super::ops::require_active;
use crate::app::AppState;
use crate::domains::attachments::{
    blob, quota, service::sanitize_content_type, service::sanitize_filename,
};
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use aurora_core::limits::ATTACH_IN_FILE_MAX;
use aurora_db::DbError;
use aurora_db::repo::attachments::{self, NewAttachment};
use aurora_db::repo::channel_ops as ops;
use aurora_db::repo::conversations;
use aurora_protocol::connector::{FilePutResponse, HDR_FILENAME};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::response::Response;
use axum::routing::put;
use http::{HeaderMap, HeaderValue, header};
use sha2::{Digest, Sha256};
use utoipa_axum::router::OpenApiRouter;
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    // One path with two methods: `{id}` is an op id for PUT and an attachment id for GET.
    OpenApiRouter::new().route(
        "/connector/files/{id}",
        put(put_file)
            .layer(DefaultBodyLimit::max(ATTACH_IN_FILE_MAX as usize + 1024))
            .get(get_file),
    )
}

fn op_404() -> ApiError {
    ApiError::not_found("Operation not found.")
}

async fn put_file(
    c: ConnectorCtx,
    State(s): State<AppState>,
    Path(op_id): Path<Uuid>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<FilePutResponse>, ApiError> {
    require_active(&c)?;
    let op = ops::get(&s.pool, op_id)
        .await?
        .filter(|o| {
            o.connector_id == c.connector.id
                && o.kind == "file.fetch"
                && o.status == "claimed"
                && o.claimed_by_replica == Some(c.replica)
        })
        .ok_or_else(op_404)?;
    let max_bytes = op
        .payload
        .get("max_bytes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(ATTACH_IN_FILE_MAX)
        .min(ATTACH_IN_FILE_MAX);
    let too_large = || ApiError::too_large("File is too large.");
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|n| n > max_bytes)
    {
        return Err(too_large());
    }
    let ctx = op.payload.as_object();
    let conversation_id = ctx
        .and_then(|p| p.get("conversation_id"))
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<Uuid>().ok())
        .ok_or_else(|| {
            ApiError::unprocessable(
                "op_context_missing",
                "The operation names no destination conversation.",
            )
        })?;
    let turn_id = ctx
        .and_then(|p| p.get("turn_id"))
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<Uuid>().ok());
    let caption = ctx
        .and_then(|p| p.get("caption"))
        .and_then(|v| v.as_str())
        .map(|v| v.chars().take(500).collect::<String>());
    let conv = conversations::get(&s.pool, conversation_id)
        .await?
        .ok_or_else(|| ApiError::unprocessable("op_context_invalid", "Unknown conversation."))?;

    let bytes = axum::body::to_bytes(body, usize::try_from(max_bytes).unwrap_or(usize::MAX))
        .await
        .map_err(|_| too_large())?;
    let filename = sanitize_filename(
        headers
            .get(HDR_FILENAME)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
    );
    let content_type = sanitize_content_type(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
    );
    let sha = format!("{:x}", Sha256::digest(&bytes));

    // Org storage quota (RFE-015): cheap check before the bytes are written; `store_once`
    // decides under the org's counter lock. The inbox marks the file `failed` /
    // `quota_exceeded` and tells the person.
    quota::precheck(
        &s,
        conv.workspace_id,
        i64::try_from(bytes.len()).unwrap_or(i64::MAX),
    )
    .await?;
    let (backend, store) = blob::current(&s.pool);
    let key = blob::new_key();
    store.put(&key, &bytes).await?;
    let id = Uuid::now_v7();
    let new = NewAttachment {
        id,
        workspace_id: conv.workspace_id,
        conversation_id,
        turn_id,
        direction: "in",
        status: if turn_id.is_some() {
            "bound"
        } else {
            "pending"
        },
        filename: &filename,
        content_type: &content_type,
        size_bytes: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
        sha256: &sha,
        blob_backend: backend,
        blob_key: &key,
        caption: caption.as_deref(),
        created_by_user_id: None,
    };
    let outcome = store_once(&s, &c, op_id, &new).await;
    match outcome {
        Ok(Some(existing)) => {
            // Lost to an earlier upload: our blob is surplus.
            if let Err(d) = store.delete(&key).await {
                tracing::warn!(error = %d, "surplus blob after repeated connector upload");
            }
            existing
        }
        Ok(None) => Ok(Json(FilePutResponse { attachment_id: id })),
        Err(e) => {
            if let Err(d) = store.delete(&key).await {
                tracing::warn!(error = %d, "orphan blob after failed connector attachment insert");
            }
            Err(e)
        }
    }
}

/// Under the op's row lock: if it already has an attachment return the idempotent answer
/// (`Some`), else run `insert` and record the upload (`None`).
async fn store_once(
    s: &AppState,
    c: &ConnectorCtx,
    op_id: Uuid,
    new: &NewAttachment<'_>,
) -> Result<Option<Result<Json<FilePutResponse>, ApiError>>, ApiError> {
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let op = ops::lock(&mut tx, op_id, &c.connector.id)
        .await?
        .filter(|o| {
            o.kind == "file.fetch"
                && o.status == "claimed"
                && o.claimed_by_replica == Some(c.replica)
        })
        .ok_or_else(op_404)?;
    if let Some(prev) = op
        .payload
        .get("_attachment_id")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<Uuid>().ok())
    {
        let same = op
            .payload
            .get("_attachment_sha256")
            .and_then(|v| v.as_str())
            == Some(new.sha256);
        return Ok(Some(if same {
            Ok(Json(FilePutResponse {
                attachment_id: prev,
            }))
        } else {
            Err(ApiError::conflict(
                "file_already_uploaded",
                "This operation already received a different file.",
            ))
        }));
    }
    quota::reserve(s, &mut tx, new.workspace_id, new.size_bytes).await?;
    match attachments::insert(&mut *tx, new).await {
        Ok(_) => {}
        Err(DbError::ForeignKeyViolation { .. }) => {
            return Err(ApiError::unprocessable(
                "op_context_invalid",
                "Unknown turn.",
            ));
        }
        Err(e) => return Err(e.into()),
    }
    ops::record_upload(&mut *tx, op_id, new.id, new.sha256).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(None)
}

async fn get_file(
    c: ConnectorCtx,
    State(s): State<AppState>,
    Path(attachment_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    require_active(&c)?;
    let nf = || ApiError::not_found("Attachment not found.");
    if !ops::claimed_post_references(&s.pool, &c.connector.id, c.replica, attachment_id).await? {
        return Err(nf());
    }
    let row = attachments::get(&s.pool, attachment_id)
        .await?
        .filter(|a| a.status != "failed")
        .ok_or_else(nf)?;
    let bytes = blob::for_backend(&row.blob_backend, &s.pool)
        .ok_or_else(|| ApiError::internal("Attachment storage is unavailable."))?
        .get(&row.blob_key)
        .await?
        .ok_or_else(nf)?;
    let mut resp = Response::new(Body::from(bytes));
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    let name = sanitize_filename(&row.filename);
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(resp)
}
