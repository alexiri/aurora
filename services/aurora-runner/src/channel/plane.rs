//! Plane calls made on behalf of a supervisor: tool calls, memory writes, file uploads.
//! Identity comes from the authenticated [`Session`], never from the frame.

use std::sync::Arc;

use aurora_core::limits::ATTACH_OUT_FILE_MAX;
use aurora_protocol::runner::{AttachmentUploadMeta, Code, MemoryWriteRequest};
use aurora_protocol::supervisor::{FileCommit, FilePart, FileResult, MemoryWriteResult};
use aurora_protocol::tools::{ToolCall, ToolCallBody, ToolError, ToolResult};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};

use super::{Session, Shared, Upload};
use crate::api::ApiError;

/// Concurrent partial uploads per session.
const MAX_UPLOADS: usize = 4;

fn tool_error(code: &str, message: impl Into<String>) -> Option<ToolError> {
    Some(ToolError {
        code: Code::new(code).ok()?,
        message: message.into(),
    })
}

fn api_error(e: &ApiError) -> Option<ToolError> {
    match e {
        ApiError::Status { status, body } => {
            tool_error("api_rejected", format!("api answered {status}: {body}"))
        }
        _ => tool_error("api_unavailable", "the platform API is unreachable"),
    }
}

fn reply<T: serde::Serialize>(session: &Session, kind: &str, body: &T) {
    let Ok(v) = serde_json::to_value(body) else {
        return;
    };
    // Results wait in the replay buffer if the supervisor is between connections.
    if let Err(e) = session.push(kind, v, false) {
        tracing::warn!(sandbox = %session.sandbox_id, kind, error = %e, "result frame not queued");
    }
}

pub(super) fn spawn_tool_call(shared: &Arc<Shared>, session: &Arc<Session>, call: ToolCallBody) {
    let (shared, session) = (shared.clone(), session.clone());
    tokio::spawn(async move {
        let Ok(_permit) = shared.plane.acquire().await else {
            return;
        };
        let call_id = call.call_id.clone();
        let result = match shared.api() {
            Some(api) => {
                let req = ToolCall {
                    sandbox_id: session.sandbox_id,
                    boot_id: session.boot_id,
                    call,
                };
                match api.tool_call(&req).await {
                    Ok(mut r) => {
                        r.call_id = call_id;
                        Some(r)
                    }
                    Err(e) => api_error(&e).map(|err| ToolResult::failure(call_id, err)),
                }
            }
            None => None,
        };
        if let Some(r) = result {
            reply(&session, "tool.result", &r);
        }
    });
}

pub(super) fn spawn_memory_write(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    write: aurora_protocol::runner::MemoryWrite,
) {
    let (shared, session) = (shared.clone(), session.clone());
    tokio::spawn(async move {
        let Ok(_permit) = shared.plane.acquire().await else {
            return;
        };
        let Some(api) = shared.api() else { return };
        let key = write.key.clone();
        let req = MemoryWriteRequest {
            sandbox_id: session.sandbox_id,
            boot_id: session.boot_id,
            write,
        };
        let out = match api.memory_write(&req).await {
            Ok(r) => MemoryWriteResult {
                key,
                ok: true,
                revision: Some(r.revision),
                error: None,
            },
            Err(e) => MemoryWriteResult {
                key,
                ok: false,
                revision: None,
                error: api_error(&e),
            },
        };
        reply(&session, "memory.write.result", &out);
    });
}

/// Buffer one part. Failures are remembered and reported by the commit.
pub(super) fn file_part(session: &Session, p: &FilePart) {
    let mut st = session.lock();
    if !st.uploads.contains_key(&p.upload_id) && st.uploads.len() >= MAX_UPLOADS {
        st.uploads.insert(
            p.upload_id.clone(),
            Upload {
                failed: Some("too_many_uploads"),
                ..Upload::default()
            },
        );
        return;
    }
    let up = st.uploads.entry(p.upload_id.clone()).or_default();
    if up.failed.is_some() {
        return;
    }
    if p.index != up.next {
        up.failed = Some("part_out_of_order");
        return;
    }
    match STANDARD.decode(&p.data_b64) {
        Ok(bytes) if up.buf.len() + bytes.len() <= ATTACH_OUT_FILE_MAX as usize => {
            up.buf.extend_from_slice(&bytes);
            up.next += 1;
        }
        Ok(_) => {
            up.failed = Some("file_too_large");
            up.buf = Vec::new();
        }
        Err(_) => up.failed = Some("bad_base64"),
    }
}

pub(super) fn spawn_file_commit(shared: &Arc<Shared>, session: &Arc<Session>, c: FileCommit) {
    let upload = session.lock().uploads.remove(&c.upload_id);
    let (shared, session) = (shared.clone(), session.clone());
    tokio::spawn(async move {
        let Ok(_permit) = shared.plane.acquire().await else {
            return;
        };
        let fail = |code: &str, msg: &str| FileResult {
            upload_id: c.upload_id.clone(),
            ok: false,
            attachment_id: None,
            error: tool_error(code, msg),
        };
        let out = match check(&c, upload) {
            Err(code) => fail(code, "upload rejected"),
            Ok(bytes) => match shared.api() {
                None => fail("api_unavailable", "the platform API is unreachable"),
                Some(api) => {
                    let meta = AttachmentUploadMeta {
                        sandbox_id: session.sandbox_id,
                        boot_id: session.boot_id,
                        conversation_id: c.conversation_id,
                        turn_id: c.turn_id,
                        filename: c.filename.clone(),
                        caption: c.caption.clone(),
                    };
                    match api.upload_attachment(&meta, bytes).await {
                        Ok(r) => FileResult {
                            upload_id: c.upload_id.clone(),
                            ok: true,
                            attachment_id: Some(r.attachment_id),
                            error: None,
                        },
                        Err(e) => FileResult {
                            upload_id: c.upload_id.clone(),
                            ok: false,
                            attachment_id: None,
                            error: api_error(&e),
                        },
                    }
                }
            },
        };
        reply(&session, "file.result", &out);
    });
}

/// Validate the assembled upload against the commit.
fn check(c: &FileCommit, upload: Option<Upload>) -> Result<Vec<u8>, &'static str> {
    let up = match upload {
        Some(u) => u,
        None if c.size == 0 => Upload::default(),
        None => return Err("unknown_upload"),
    };
    if let Some(f) = up.failed {
        return Err(f);
    }
    if up.buf.len() as u64 != c.size {
        return Err("size_mismatch");
    }
    if !hex::encode(Sha256::digest(&up.buf)).eq_ignore_ascii_case(&c.sha256) {
        return Err("checksum_mismatch");
    }
    Ok(up.buf)
}
