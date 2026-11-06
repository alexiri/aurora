//! Internal registry routes on the 4111 listener (05 §Authentication surfaces, I-20).
//! Never merged into the public router.

use super::registry;
use crate::app::AppState;
use crate::http::json::{Path, Query};
use aurora_core::ApiError;
use aurora_registry::manifest::CONTRACT;
use axum::Router;
use axum::extract::{FromRequestParts, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http::{HeaderValue, StatusCode, header, request::Parts};
use serde::Deserialize;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/internal/v1/registry/active", get(active))
        .route("/internal/v1/registry/blobs/{digest}", get(blob))
        .with_state(state)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut d = u8::from(a.len() != b.len());
    for i in 0..a.len().max(b.len()) {
        d |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    d == 0
}

/// `Authorization: Bearer <base64(AURORA_INTERNAL_SECRET)>`.
struct Internal;

impl FromRequestParts<AppState> for Internal {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let secret = st.config.internal_secret.expose();
        let ok = !secret.is_empty()
            && parts.headers.get(header::AUTHORIZATION).is_some_and(|v| {
                ct_eq(
                    v.as_bytes(),
                    format!("Bearer {}", STANDARD.encode(secret)).as_bytes(),
                )
            });
        if ok {
            Ok(Self)
        } else {
            Err(ApiError::unauthenticated("Internal secret required."))
        }
    }
}

#[derive(Deserialize)]
struct ActiveQuery {
    contract_max: Option<u32>,
}

async fn active(
    _i: Internal,
    State(s): State<AppState>,
    Query(q): Query<ActiveQuery>,
) -> Result<Response, ApiError> {
    let snap = registry::active_snapshot(&s, q.contract_max.unwrap_or(CONTRACT))
        .await?
        .ok_or_else(|| ApiError::not_found("No active registry snapshot."))?;
    Ok(snap.into_response())
}

async fn blob(
    _i: Internal,
    State(s): State<AppState>,
    Path(digest): Path<String>,
) -> Result<Response, ApiError> {
    let valid = digest.len() == 64
        && digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let bytes = if valid {
        aurora_db::repo::plugins::get_blob(&s.pool, &digest, "hooks_wasm").await?
    } else {
        None
    };
    let bytes = bytes.ok_or_else(|| ApiError::not_found("Blob not found."))?;
    let mut res = (StatusCode::OK, bytes).into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/wasm"),
    );
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    Ok(res)
}
