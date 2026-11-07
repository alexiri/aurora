//! `GET /internal/v1/status` on the internal listener: the internal-secret ring's key
//! **ids** and short fingerprints (never values), so an operator can confirm that a rotation
//! step has converged (docs/08 §4, RFE-007). Authenticated with any key of the ring.

use std::sync::Arc;

use aurora_crypto::SecretRing;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;

pub fn router(ring: Arc<SecretRing>) -> Router {
    Router::new()
        .route("/internal/v1/status", get(status))
        .with_state(ring)
}

async fn status(State(ring): State<Arc<SecretRing>>, headers: HeaderMap) -> Response {
    let key_id = headers
        .get(aurora_core::limits::KEY_ID_HEADER)
        .map(|v| v.to_str().unwrap_or("\u{0}"));
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if !ring.verify_bearer_header(key_id, auth) {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({ "error": { "code": "unauthorized", "message": "internal secret required" } })),
        )
            .into_response();
    }
    let (active, keys) = ring.status();
    let keys: Vec<_> = keys
        .into_iter()
        .map(|(id, fp)| json!({ "id": id, "fingerprint": fp }))
        .collect();
    axum::Json(
        json!({ "service": "gateway", "internal_secret": { "active": active, "keys": keys } }),
    )
    .into_response()
}
