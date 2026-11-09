//! Connector-host plane `/v1/connector-host/*` (P4-MCON, C-455..C-457; docs/13 §3.1).
//! Machine plane authenticated with the host's anchor token `auh_…`; like `/v1/connector/*` it is
//! not part of the dashboard OpenAPI document.
//!
//! `assignments` is the only place secret settings are decrypted (SEC-70). The response is never
//! logged, and nothing here writes a secret value anywhere.

use super::managed::{self, store_unavailable};
use super::manifest;
use super::plane::verify_manifest;
use super::setup;
use crate::app::AppState;
use crate::http::json::{Json, Query};
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, FieldError, limits, sanitize};
use aurora_db::repo::connector_hosts;
use aurora_db::repo::connectors::{self, ConnectorRow};
use aurora_protocol::common::Validate;
use aurora_protocol::host::{
    Assignment, AssignmentsResponse, DEFAULT_ASSIGNMENTS_WAIT_SECS, DEFAULT_HOST_POLL_SECS,
    HostRegisterRequest, HostRegisterResponse, HostStatusRequest, InstanceState,
    MAX_ASSIGNMENTS_WAIT_SECS, MAX_STATUS_MESSAGE_CHARS, RejectReason, RejectedType,
};
use aurora_secrets::SecretRef;
use axum::extract::{FromRequestParts, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use http::{HeaderValue, StatusCode, header, request::Parts};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::Instant;
use utoipa_axum::router::OpenApiRouter;
use uuid::Uuid;

/// Mounted under `/v1`; plain routes, so excluded from the dashboard OpenAPI document.
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .route("/connector-host/register", post(register))
        .route("/connector-host/assignments", get(assignments))
        .route("/connector-host/status", post(status))
}

/// `Authorization: Bearer auh_…` whose hash is in `connector_host_tokens`.
pub struct HostToken;

impl FromRequestParts<AppState> for HostToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let tok = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|t| token::parse_prefix(t) == Some(TokenPrefix::ConnectorHost))
            .ok_or_else(|| ApiError::unauthenticated("Connector host token required."))?;
        if !connector_hosts::token_exists(&st.pool, &token::hash(tok)).await? {
            return Err(ApiError::unauthenticated("Unknown connector host token."));
        }
        Ok(Self)
    }
}

/// (C-459) Upserts the host token hash from `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN`.
pub async fn bootstrap_host_token(s: &AppState) -> Result<(), aurora_db::DbError> {
    if let Some(t) = &s.config.bootstrap_connector_host_token {
        connector_hosts::upsert_token(&s.pool, &token::hash(t.expose())).await?;
        tracing::info!("bootstrap connector host token applied");
    }
    Ok(())
}

// -------------------------------------------------------------------- register

async fn register(
    _t: HostToken,
    State(s): State<AppState>,
    Json(req): Json<HostRegisterRequest>,
) -> Result<Json<HostRegisterResponse>, ApiError> {
    req.validate()
        .map_err(|e| ApiError::invalid_request(e.to_string(), vec![]))?;
    let mut accepted: Vec<Value> = Vec::new();
    let mut rejected = Vec::new();
    for t in &req.types {
        let reject = |reason| RejectedType {
            type_id: sanitize::one_line(&t.type_id, 64),
            reason,
        };
        let m = &t.manifest;
        if m.package.id != t.type_id {
            rejected.push(reject(RejectReason::IdMismatch));
            continue;
        }
        if m.validate().is_err() || !manifest::validate(m).is_empty() {
            rejected.push(reject(RejectReason::InvalidManifest));
            continue;
        }
        match verify_manifest(&s, m, &t.manifest_signature).await {
            Ok(digest) => accepted.push(json!({
                "type_id": t.type_id,
                "manifest": m,
                "manifest_digest": digest,
            })),
            Err(e) if e.status() == StatusCode::UNPROCESSABLE_ENTITY => {
                rejected.push(reject(RejectReason::Signature));
            }
            Err(e) => return Err(e),
        }
    }
    connector_hosts::upsert_host(&s.pool, req.instance_id, &req.version, &json!(accepted)).await?;
    // Heartbeat rows older than a day are of no use.
    connector_hosts::prune(&s.pool, limits::CONNECTOR_HOST_RETAIN.as_secs_f64()).await?;
    Ok(Json(HostRegisterResponse {
        poll_interval_secs: DEFAULT_HOST_POLL_SECS,
        rejected,
    }))
}

// ----------------------------------------------------------------- assignments

#[derive(Deserialize)]
struct AssignmentsQuery {
    revision: Option<i64>,
    wait: Option<u32>,
    /// The calling host's `instance_id`: only types that host announced are served. Without it
    /// the types of every live host are served.
    instance_id: Option<Uuid>,
}

async fn assignments(
    _t: HostToken,
    State(s): State<AppState>,
    Query(q): Query<AssignmentsQuery>,
) -> Result<Response, ApiError> {
    let wait = q.wait.unwrap_or(DEFAULT_ASSIGNMENTS_WAIT_SECS);
    if wait > MAX_ASSIGNMENTS_WAIT_SECS {
        return Err(ApiError::invalid_request(
            "`wait` must be 0-30.",
            vec![FieldError::new("/wait", "must be 0-30")],
        ));
    }
    let wait = Duration::from_secs(u64::from(wait));
    let mut rx = if wait.is_zero() {
        None
    } else {
        Some(s.connector_bus.subscribe(&s.pool).await?)
    };
    let deadline = Instant::now() + wait;
    let mut revision = connectors::revision(&s.pool).await?;
    // Return at once when the host's revision differs; otherwise wait for a change.
    while q.revision == Some(revision) {
        let left = deadline.saturating_duration_since(Instant::now());
        let Some(rx) = rx.as_mut().filter(|_| !left.is_zero()) else {
            break;
        };
        let _ = tokio::time::timeout(left.min(Duration::from_secs(5)), async {
            loop {
                match rx.recv().await {
                    Ok(super::bus::Msg::Assignments | super::bus::Msg::Resync) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        })
        .await;
        let now = connectors::revision(&s.pool).await?;
        if now != revision {
            revision = now;
            break;
        }
    }
    // The revision is read before the list: a change in between only makes the host fetch again.
    let connectors = build(&s, q.instance_id).await?;
    let mut resp = Json(AssignmentsResponse {
        revision,
        connectors,
    })
    .into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// Types announced by the calling host, or by every live host when it is not named.
async fn announced_types(s: &AppState, instance: Option<Uuid>) -> Result<Vec<String>, ApiError> {
    let hosts = match instance {
        Some(id) => connector_hosts::get_host(&s.pool, id)
            .await?
            .into_iter()
            .collect(),
        None => {
            connector_hosts::list_seen(&s.pool, limits::CONNECTOR_HOST_STALE.as_secs_f64()).await?
        }
    };
    let mut out: Vec<String> = hosts
        .iter()
        .filter_map(|h| h.types.as_array())
        .flatten()
        .filter_map(|t| t.get("type_id").and_then(Value::as_str).map(str::to_owned))
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// The assignments with decrypted secrets (C-456). A store failure answers 503
/// `secret_store_unavailable`; the host keeps its running set.
async fn build(s: &AppState, instance: Option<Uuid>) -> Result<Vec<Assignment>, ApiError> {
    let types = announced_types(s, instance).await?;
    let rows = connectors::list_managed_enabled(&s.pool, &types).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(assignment(s, r).await?);
    }
    Ok(out)
}

async fn assignment(s: &AppState, r: ConnectorRow) -> Result<Assignment, ApiError> {
    let fields = managed::fields_of(&r);
    let mut settings = setup::with_defaults(&fields, r.settings.as_ref());
    if !settings.contains_key("public_url") {
        let base = s.config.external_url.trim_end_matches('/');
        settings.insert(
            "public_url".to_owned(),
            json!(format!("{base}/connectors/{}", r.id)),
        );
    }
    let mut secrets = BTreeMap::new();
    for (name, col) in setup::refs_of(&r.secret_refs) {
        let sealed: SecretRef = col.parse().map_err(|_| store_unavailable())?;
        let plain = s
            .vault
            .open(&managed::seal_ctx(r.secret_owner), &sealed)
            .await
            .map_err(|e| {
                tracing::error!(connector = %r.id, error = %e, "connector secret could not be opened");
                store_unavailable()
            })?;
        let value = String::from_utf8(plain.expose().clone()).map_err(|_| store_unavailable())?;
        secrets.insert(name, value);
    }
    Ok(Assignment {
        type_id: r.type_id.clone().unwrap_or_else(|| r.id.clone()),
        display_name: r.display_name,
        settings: Value::Object(settings),
        secrets,
        status: r.status,
        id: r.id,
    })
}

// ---------------------------------------------------------------------- status

async fn status(
    _t: HostToken,
    State(s): State<AppState>,
    Json(req): Json<HostStatusRequest>,
) -> Result<StatusCode, ApiError> {
    req.validate()
        .map_err(|e| ApiError::invalid_request(e.to_string(), vec![]))?;
    let reports: Vec<(String, String, Option<String>)> = req
        .connectors
        .into_iter()
        .map(|c| {
            let state = match c.state {
                InstanceState::Running => "running",
                InstanceState::Error => "error",
            };
            // Control characters stripped, at most 300 chars; a secret never belongs here.
            let msg = c
                .message
                .map(|m| sanitize::one_line(&m, MAX_STATUS_MESSAGE_CHARS))
                .filter(|m| !m.is_empty());
            (c.id, state.to_owned(), msg)
        })
        .collect();
    connector_hosts::replace_status(&s.pool, req.instance_id, &reports).await?;
    Ok(StatusCode::NO_CONTENT)
}
