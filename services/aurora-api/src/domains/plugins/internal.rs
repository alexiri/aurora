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
use http::{HeaderValue, StatusCode, header, request::Parts};
use serde::Deserialize;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/internal/v1/registry/active", get(active))
        .route("/internal/v1/registry/blobs/{digest}", get(blob))
        .route("/internal/v1/status", get(status))
        .with_state(state)
}

/// `Authorization: Bearer <base64(secret)>` + `X-Aurora-Key-Id` (RFE-007): any key of the
/// internal-secret ring; with the header absent (legacy / rolling) every key is tried.
struct Internal;

impl FromRequestParts<AppState> for Internal {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let auth = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        let key_id = parts
            .headers
            .get(aurora_core::limits::KEY_ID_HEADER)
            .map(|v| v.to_str().unwrap_or("\u{0}"));
        if st.config.internal_ring.verify_bearer_header(key_id, auth) {
            Ok(Self)
        } else {
            Err(ApiError::unauthenticated("Internal secret required."))
        }
    }
}

/// Ids and short fingerprints (never values) of this replica's rings, so an operator can
/// confirm a rotation step converged (`08 §4`).
async fn status(_i: Internal, State(s): State<AppState>) -> axum::Json<serde_json::Value> {
    let (sa, sk) = s.config.signing_ring.status();
    let (ia, ik) = s.config.internal_ring.status();
    let list = |v: Vec<(String, String)>| {
        v.into_iter()
            .map(|(id, fp)| serde_json::json!({"id": id, "fingerprint": fp}))
            .collect::<Vec<_>>()
    };
    axum::Json(serde_json::json!({
        "service": "api",
        "signing_key": {"active": sa, "keys": list(sk)},
        "internal_secret": {"active": ia, "keys": list(ik)},
    }))
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
