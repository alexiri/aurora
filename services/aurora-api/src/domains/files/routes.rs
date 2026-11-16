//! The Files routes (phase 5, P5-FILES-1, D-316; C-898, C-903): session cookie only, the web
//! terminal's availability, rate class `files`. The two content routes are exempt from the
//! global body limit (the upload is streamed to the sandbox; its size is `Content-Length`).
use super::dto::*;
use super::paths;
use super::service::{self, Precondition};
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::ApiError;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_files))
        .routes(routes!(file_text))
        .routes(routes!(get_content, put_content))
        .layer(DefaultBodyLimit::disable())
        .routes(routes!(file_ops))
}

fn path_of(q: &PathQuery, default_root: bool) -> Result<String, ApiError> {
    match (&q.path, default_root) {
        (Some(p), _) => Ok(p.clone()),
        (None, true) => Ok(paths::ROOT.to_owned()),
        (None, false) => Err(paths::invalid_path("`path` is required")),
    }
}

/// List a directory (C-898): directories first, then by name; at most 5,000 entries.
#[utoipa::path(get, path = "/agents/{agent}/files", operation_id = "listAgentFiles", tag = "files",
    params(("agent" = Uuid, Path, description = "Agent id"), PathQuery),
    responses((status = 200, body = FileList), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 423, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_files(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<FileList>, ApiError> {
    let p = path_of(&q, true)?;
    Ok(Json(service::list(&s, &c, agent, &p).await?))
}

/// A text file for the editor (C-898): ≤ 1 MiB of UTF-8 without NUL, with its `etag`.
#[utoipa::path(get, path = "/agents/{agent}/files/text", operation_id = "getAgentFileText", tag = "files",
    params(("agent" = Uuid, Path, description = "Agent id"), PathQuery),
    responses((status = 200, body = FileText), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 423, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn file_text(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Json<FileText>, ApiError> {
    let p = path_of(&q, false)?;
    Ok(Json(service::text(&s, &c, agent, &p).await?))
}

/// RFC 5987 `attr-char`: ALPHA / DIGIT / `!#$&+-.^_`|~`.
const ATTR_CHAR: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'!')
    .remove(b'#')
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b'-')
    .remove(b'.')
    .remove(b'^')
    .remove(b'_')
    .remove(b'`')
    .remove(b'|')
    .remove(b'~');

/// `Content-Disposition: attachment` with an ASCII fallback (non-ASCII, `"`, `\`, `/` and
/// control characters become `_`) and the RFC 5987 UTF-8 name (SEC-126).
pub fn content_disposition(name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii() && !ch.is_ascii_control() && !matches!(ch, '"' | '\\' | '/') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "attachment; filename=\"{fallback}\"; filename*=UTF-8''{}",
        utf8_percent_encode(name, ATTR_CHAR)
    )
}

/// Download a file's bytes, streamed (C-903): always an attachment of `octet-stream`.
#[utoipa::path(get, path = "/agents/{agent}/files/content", operation_id = "downloadAgentFile", tag = "files",
    params(("agent" = Uuid, Path, description = "Agent id"), PathQuery),
    responses((status = 200, description = "File bytes", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 423, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_content(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> Result<Response, ApiError> {
    let p = path_of(&q, false)?;
    let d = service::download(&s, &c, agent, &p).await?;
    let mut res = Response::new(d.body);
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(v) = HeaderValue::from_str(&content_disposition(&d.name)) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(d.size));
    Ok(res)
}

fn precondition(headers: &HeaderMap) -> Result<Precondition, ApiError> {
    let none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
    match (none_match, if_match) {
        (Some(_), Some(_)) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Send at most one of If-Match and If-None-Match.",
        )),
        (Some(v), None) if v.trim() == "*" => Ok(Precondition::Create),
        (Some(_), None) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "If-None-Match must be `*`.",
        )),
        (None, Some(v)) => Ok(Precondition::Etag(v.trim().trim_matches('"').to_owned())),
        (None, None) => Ok(Precondition::None),
    }
}

/// Upload or replace a file (C-898): the body is the raw bytes, streamed to a temp file and
/// renamed into place atomically. `Content-Length` is required.
#[utoipa::path(put, path = "/agents/{agent}/files/content", operation_id = "putAgentFile", tag = "files",
    params(("agent" = Uuid, Path, description = "Agent id"), PathQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses((status = 200, body = FileWriteResult), (status = 201, body = FileWriteResult),
        (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 411, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 423, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn put_content(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<PathQuery>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let p = path_of(&q, false)?;
    let pre = precondition(&headers)?;
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let w = service::upload(&s, &c, agent, &p, length, pre, body).await?;
    let status = if w.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(FileWriteResult {
            entry: w.entry,
            etag: w.etag,
        }),
    )
        .into_response())
}

/// Make a directory, rename or delete (C-898).
#[utoipa::path(post, path = "/agents/{agent}/files/ops", operation_id = "agentFileOp", tag = "files",
    params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = FileOp,
    responses((status = 200, body = FileOpResult), (status = 201, body = FileOpResult),
        (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 423, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn file_ops(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(op): Json<FileOp>,
) -> Result<Response, ApiError> {
    Ok(match op {
        FileOp::Mkdir { path, parents } => {
            let (entry, created) = service::mkdir(&s, &c, agent, &path, parents).await?;
            (
                if created {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                Json(FileOpResult {
                    entry: Some(entry),
                    deleted: None,
                }),
            )
                .into_response()
        }
        FileOp::Rename { path, to } => {
            let entry = service::rename(&s, &c, agent, &path, &to).await?;
            Json(FileOpResult {
                entry: Some(entry),
                deleted: None,
            })
            .into_response()
        }
        FileOp::Delete { path, recursive } => {
            let n = service::delete(&s, &c, agent, &path, recursive).await?;
            Json(FileOpResult {
                entry: None,
                deleted: Some(n),
            })
            .into_response()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::content_disposition;

    #[test]
    fn disposition_is_safe_for_hostile_names() {
        let d = content_disposition("a\"b\\c/d\r\ne é.html");
        assert!(
            d.starts_with("attachment; filename=\"a_b_c_d__e _.html\";"),
            "{d}"
        );
        assert!(
            d.contains("filename*=UTF-8''a%22b%5Cc%2Fd%0D%0Ae%20%C3%A9.html"),
            "{d}"
        );
        assert!(!d.contains('\r') && !d.contains('\n'));
    }
}
