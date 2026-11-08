use super::blob;
use super::dto::Attachment;
use super::{quota, sweep};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::conversations::service as conv;
use crate::http::auth::Caller;
use aurora_core::limits::{ATTACH_IN_FILE_MAX, ATTACH_PENDING_TTL};
use aurora_core::{ApiError, FieldError};
use aurora_db::PgPool;
use aurora_db::repo::attachments::{self, AttachmentRow, NewAttachment};
use axum::body::Body;
use axum::extract::{Multipart, multipart::MultipartError, multipart::MultipartRejection};
use axum::response::Response;
use http::{HeaderValue, StatusCode, header};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const FILENAME_MAX: usize = 128;
const CONTENT_TYPE_MAX: usize = 127;
const OCTET: &str = "application/octet-stream";
/// Rows removed per sweep statement.
const SWEEP_BATCH: i64 = 500;

pub(super) fn purged() -> ApiError {
    ApiError::gone(
        "attachment_purged",
        "This file was deleted or expired by the retention policy.",
    )
}

fn att_404() -> ApiError {
    ApiError::not_found("Attachment not found.")
}

/// Sanitize an untrusted filename to `[A-Za-z0-9._-]`, at most [`FILENAME_MAX`] characters:
/// path components are dropped, every other character becomes `_`, empty or dot-only names
/// become `file`.
pub fn sanitize_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let mut out: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(FILENAME_MAX)
        .collect();
    if out.chars().all(|c| c == '.') {
        out = "file".to_owned();
    }
    out
}

/// Keep a well-formed `type/subtype` (parameters dropped, lower-cased); anything else becomes
/// `application/octet-stream`.
pub fn sanitize_content_type(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return OCTET.to_owned();
    };
    let essence = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    match essence.split_once('/') {
        Some((t, st)) if essence.len() <= CONTENT_TYPE_MAX && token(t) && token(st) => essence,
        _ => OCTET.to_owned(),
    }
}

fn multipart_error(e: &MultipartError) -> ApiError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::too_large("File is too large.")
    } else {
        ApiError::invalid_request(
            "Malformed multipart body.",
            vec![FieldError::new("/", "malformed multipart body")],
        )
    }
}

fn file_too_large() -> ApiError {
    ApiError::too_large("File is too large.")
}

/// Upload one file to a conversation: `pending` inbound attachment owned by the caller.
/// Authorization (workspace `can_use` + conversation visibility) comes before body parsing.
pub async fn upload(
    state: &AppState,
    c: &Caller,
    conversation_id: Uuid,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<Attachment, ApiError> {
    let conv_row = conv::load_conversation_for_write(state, c, conversation_id).await?;
    // (phase 2, RFE-006) An attachment is part of starting work: refused for a frozen agent.
    crate::domains::freeze::service::ensure_agent_active_pool(state, conv_row.agent_id).await?;
    let mut mp = mp.map_err(|_| {
        ApiError::invalid_request(
            "Expected multipart/form-data.",
            vec![FieldError::new(
                "/",
                "content type must be multipart/form-data",
            )],
        )
    })?;
    let mut file: Option<(String, String, Vec<u8>, String)> = None;
    while let Some(mut field) = mp.next_field().await.map_err(|e| multipart_error(&e))? {
        if field.name() != Some("file") || file.is_some() {
            return Err(ApiError::invalid_request(
                "Unexpected multipart part.",
                vec![FieldError::new(
                    "/file",
                    "exactly one part `file` is allowed",
                )],
            ));
        }
        let filename = sanitize_filename(field.file_name().unwrap_or(""));
        let content_type = sanitize_content_type(field.content_type());
        // Streamed: the cap is enforced chunk by chunk, so an oversize upload never buffers
        // more than `ATTACH_IN_FILE_MAX` bytes.
        let mut buf: Vec<u8> = Vec::new();
        let mut hasher = Sha256::new();
        while let Some(chunk) = field.chunk().await.map_err(|e| multipart_error(&e))? {
            if (buf.len() + chunk.len()) as u64 > ATTACH_IN_FILE_MAX {
                return Err(file_too_large());
            }
            hasher.update(&chunk);
            buf.extend_from_slice(&chunk);
        }
        file = Some((
            filename,
            content_type,
            buf,
            format!("{:x}", hasher.finalize()),
        ));
    }
    let (filename, content_type, bytes, sha256) = file.ok_or_else(|| {
        ApiError::invalid_request(
            "Missing file.",
            vec![FieldError::new("/file", "part `file` is required")],
        )
    })?;
    let row = store(
        state,
        NewBlob {
            workspace_id: conv_row.workspace_id,
            conversation_id,
            turn_id: None,
            direction: "in",
            status: "pending",
            filename: &filename,
            content_type: &content_type,
            sha256: &sha256,
            caption: None,
            created_by_user_id: Some(c.user_id),
        },
        &bytes,
    )
    .await?;
    Ok(row.into())
}

pub struct NewBlob<'a> {
    pub workspace_id: Uuid,
    pub conversation_id: Uuid,
    pub turn_id: Option<Uuid>,
    pub direction: &'a str,
    pub status: &'a str,
    pub filename: &'a str,
    pub content_type: &'a str,
    pub sha256: &'a str,
    pub caption: Option<&'a str>,
    pub created_by_user_id: Option<Uuid>,
}

/// Put the bytes and insert the row; the blob is removed again if the quota check or the insert
/// fails. The org quota is checked before the bytes are written (cheap, unlocked) and again,
/// authoritatively, under the org's counter lock in the inserting transaction.
async fn store(state: &AppState, n: NewBlob<'_>, bytes: &[u8]) -> Result<AttachmentRow, ApiError> {
    let size = bytes.len() as i64;
    quota::precheck(state, n.workspace_id, size).await?;
    let (backend, store) = blob::current(&state.pool);
    let key = blob::new_key();
    store.put(&key, bytes).await?;
    match insert_reserved(state, n, &key, backend, bytes.len()).await {
        Ok(r) => Ok(r),
        Err(e) => {
            if let Err(d) = store.delete(&key).await {
                tracing::warn!(error = %d, "orphan blob after failed attachment insert");
            }
            Err(e)
        }
    }
}

async fn insert_reserved(
    state: &AppState,
    n: NewBlob<'_>,
    key: &str,
    backend: &str,
    size: usize,
) -> Result<AttachmentRow, ApiError> {
    let mut tx = state.pool.begin().await.map_err(aurora_db::DbError::from)?;
    let row = insert_in(state, &mut tx, n, key, backend, size).await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(row)
}

/// Insert an attachment inside the caller's transaction (blob already stored under `key`), after
/// the org quota check under the org's counter lock (RFE-015).
pub(super) async fn insert_in(
    state: &AppState,
    conn: &mut sqlx::PgConnection,
    n: NewBlob<'_>,
    key: &str,
    backend: &str,
    size: usize,
) -> Result<AttachmentRow, ApiError> {
    quota::reserve(state, &mut *conn, n.workspace_id, size as i64).await?;
    let new = NewAttachment {
        id: Uuid::now_v7(),
        workspace_id: n.workspace_id,
        conversation_id: n.conversation_id,
        turn_id: n.turn_id,
        direction: n.direction,
        status: n.status,
        filename: n.filename,
        content_type: n.content_type,
        size_bytes: size as i64,
        sha256: n.sha256,
        blob_backend: backend,
        blob_key: key,
        caption: n.caption,
        created_by_user_id: n.created_by_user_id,
    };
    Ok(attachments::insert(&mut *conn, &new).await?)
}

/// Download: `can_use` on the workspace plus conversation visibility; unseen -> 404.
pub async fn download(state: &AppState, c: &Caller, id: Uuid) -> Result<Response, ApiError> {
    let row = attachments::get(&state.pool, id)
        .await?
        .filter(|a| a.status != "failed")
        .ok_or_else(att_404)?;
    conv::read_conversation(state, c, row.conversation_id)
        .await
        .map_err(|_| att_404())?;
    if row.purged_at.is_some() {
        return Err(purged());
    }
    let bytes = read_blob(state, &row).await?;
    Ok(bytes_response(&row, bytes))
}

/// `DELETE /attachments/{id}`: purge the bytes now (`purge_reason = deleted`; the row stays so
/// transcripts keep their chip). Authority: the uploader or `can_own`, always subject to
/// `can_see_conversation` (404 otherwise). Idempotent; audited best-effort (`attachment.delete`).
pub async fn delete(state: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let row = attachments::get(&state.pool, id)
        .await?
        .filter(|a| a.status != "failed")
        .ok_or_else(att_404)?;
    let (conv_row, auth) = conv::load_conversation_auth(state, c, row.conversation_id)
        .await
        .map_err(|_| att_404())?;
    if !(auth.can_own() || row.created_by_user_id == Some(c.user_id)) {
        return Err(ApiError::forbidden(
            "Only the uploader or a workspace owner can delete this file.",
        ));
    }
    let mut tx = state.pool.begin().await.map_err(aurora_db::DbError::from)?;
    let purged = attachments::purge_one(&mut tx, id, "deleted").await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    if let Some(p) = purged {
        sweep::remove_blobs(&state.pool, std::slice::from_ref(&p)).await;
        audit::record(
            state,
            AuditEvent::new(
                AuditActor::from_caller(c),
                "attachment.delete",
                Outcome::Success,
            )
            .org(auth.org_id)
            .workspace(conv_row.workspace_id)
            .target("attachment", id)
            .metadata(serde_json::json!({
                "conversation_id": row.conversation_id, "direction": row.direction,
                "size_bytes": p.size_bytes,
            })),
        );
    }
    Ok(())
}

pub(super) async fn read_blob(state: &AppState, row: &AttachmentRow) -> Result<Vec<u8>, ApiError> {
    let store = blob::for_backend(&row.blob_backend, &state.pool).ok_or_else(|| {
        tracing::error!(attachment = %row.id, backend = %row.blob_backend, "unknown blob backend");
        ApiError::internal("Attachment storage is unavailable.")
    })?;
    store.get(&row.blob_key).await?.ok_or_else(|| {
        tracing::error!(attachment = %row.id, "attachment blob is missing");
        att_404()
    })
}

/// Bytes with the fixed safe headers: never the stored type, never inline.
pub(super) fn bytes_response(row: &AttachmentRow, bytes: Vec<u8>) -> Response {
    let name = sanitize_filename(&row.filename);
    let mut resp = Response::new(Body::from(bytes));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(OCTET));
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
    resp
}

/// Hourly job: delete `pending` rows older than `ATTACH_PENDING_TTL` and their blobs. Rows are
/// claimed (deleted) first, so a message binding the attachment concurrently either wins before
/// the delete or finds the row gone; a crash between row and blob delete only leaks a blob.
/// Idempotent. Returns the number of rows removed.
pub async fn sweep_pending(pool: &PgPool) -> Result<usize, aurora_core::ApiError> {
    let mut total = 0;
    loop {
        let rows = attachments::delete_expired_pending(
            pool,
            ATTACH_PENDING_TTL.as_secs_f64(),
            SWEEP_BATCH,
        )
        .await?;
        for (backend, key) in &rows {
            match blob::for_backend(backend, pool) {
                Some(store) => {
                    if let Err(e) = store.delete(key).await {
                        tracing::warn!(error = %e, key, "pending attachment blob not deleted");
                    }
                }
                None => tracing::warn!(backend, key, "unknown blob backend; blob left in place"),
            }
        }
        total += rows.len();
        if (rows.len() as i64) < SWEEP_BATCH {
            return Ok(total);
        }
    }
}

/// Blobs younger than this are never treated as orphans: the upload paths `put` the blob and
/// insert the attachment row in separate steps (not one transaction), so a fresh blob may simply
/// not have its row committed yet.
pub const ORPHAN_BLOB_MIN_AGE_SECS: f64 = 3600.0;

/// Hourly job part 2: delete `postgres` blobs that no attachment row references any more
/// (cascades from conversation/workspace/agent/org deletes remove rows but not blobs), so deleted
/// content does not persist. Batched, `SKIP LOCKED`, idempotent across replicas. Returns the
/// number of blobs removed.
pub async fn sweep_orphan_blobs(pool: &PgPool) -> Result<usize, aurora_core::ApiError> {
    let mut total = 0usize;
    loop {
        let n = aurora_db::repo::blobs::delete_unreferenced(
            pool,
            ORPHAN_BLOB_MIN_AGE_SECS,
            SWEEP_BATCH,
        )
        .await?;
        total += n as usize;
        if (n as i64) < SWEEP_BATCH {
            return Ok(total);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_is_sanitized() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("C:\\x\\a b.txt"), "a_b.txt");
        assert_eq!(sanitize_filename(""), "file");
        assert_eq!(sanitize_filename(".."), "file");
        assert_eq!(sanitize_filename("<svg>é.html"), "_svg__.html");
        assert_eq!(sanitize_filename(&"a".repeat(300)).len(), FILENAME_MAX);
    }

    #[test]
    fn content_type_is_sanitized() {
        assert_eq!(sanitize_content_type(None), OCTET);
        assert_eq!(
            sanitize_content_type(Some("Text/HTML; charset=utf-8")),
            "text/html"
        );
        assert_eq!(sanitize_content_type(Some("no slash")), OCTET);
        assert_eq!(sanitize_content_type(Some("a/b\r\nx: y")), OCTET);
    }
}
