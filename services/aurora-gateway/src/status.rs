//! Internal listener routes: `GET /internal/v1/status` (the internal-secret ring's key **ids**
//! and short fingerprints, never values, so an operator can confirm that a rotation step has
//! converged, docs/08 §4, RFE-007; phase 3, RFE-014: plus the outage state, C-381) and
//! `GET /readyz` (the three states, C-380). Status is authenticated with any key of the ring.

use std::collections::HashMap;
use std::sync::Arc;

use crate::health::{Health, State as HealthState};
use crate::invalidation::{CacheStats, Hub};
use crate::reqlog::RequestLog;
use aurora_crypto::SecretRing;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};

/// What `/readyz` and the status fields read (phase 3, RFE-014).
#[derive(Clone)]
pub struct Outage {
    pub health: Arc<Health>,
    pub hub: Arc<Hub>,
    pub reqlog: Option<Arc<RequestLog>>,
}

#[derive(Clone)]
struct StatusState {
    ring: Arc<SecretRing>,
    outage: Option<Outage>,
}

pub fn router(ring: Arc<SecretRing>) -> Router {
    router_with(ring, None)
}

/// `/internal/v1/status`, with the outage fields when `outage` is set.
pub fn router_with(ring: Arc<SecretRing>, outage: Option<Outage>) -> Router {
    Router::new()
        .route("/internal/v1/status", get(status))
        .with_state(StatusState { ring, outage })
}

/// `GET /readyz` (unauthenticated like the other health routes).
pub fn readyz_router(outage: Outage) -> Router {
    Router::new()
        .route("/readyz", get(readyz))
        .with_state(outage)
}

fn cache_json(c: CacheStats) -> Value {
    json!({
        "entries": c.entries,
        "oldest_age_secs": c.oldest_age.map(|d| d.as_secs()),
    })
}

async fn readyz(State(o): State<Outage>, Query(q): Query<HashMap<String, String>>) -> Response {
    let snap = o.health.evaluate();
    let strict = q.get("strict").is_some_and(|v| v == "true");
    let reason = snap.reason.map(|r| r.as_str());
    match snap.state {
        HealthState::Ready => {
            (StatusCode::OK, axum::Json(json!({ "state": "ready" }))).into_response()
        }
        HealthState::Degraded => {
            let c = o.hub.cache_stats();
            let status = if strict {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::OK
            };
            (
                status,
                axum::Json(json!({
                    "state": "degraded",
                    "reason": reason,
                    "since": snap.since_rfc3339(),
                    "cache_entries": c.entries,
                    "cache_oldest_age_secs": c.oldest_age.map(|d| d.as_secs()),
                })),
            )
                .into_response()
        }
        HealthState::Down => (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({ "state": "down", "reason": reason })),
        )
            .into_response(),
    }
}

async fn status(State(st): State<StatusState>, headers: HeaderMap) -> Response {
    let ring = &st.ring;
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
    let mut body =
        json!({ "service": "gateway", "internal_secret": { "active": active, "keys": keys } });
    if let (Some(o), Some(m)) = (&st.outage, body.as_object_mut()) {
        let snap = o.health.evaluate();
        m.insert("state".into(), json!(snap.state.as_str()));
        m.insert(
            "db_unreachable_since".into(),
            json!(
                o.health
                    .db_unreachable_since()
                    .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            ),
        );
        m.insert(
            "listener_connected".into(),
            json!(o.health.listener_connected()),
        );
        m.insert("cache".into(), cache_json(o.hub.cache_stats()));
        m.insert(
            "request_log_buffered".into(),
            json!(o.reqlog.as_ref().map_or(0, |r| r.buffered())),
        );
    }
    axum::Json(body).into_response()
}
