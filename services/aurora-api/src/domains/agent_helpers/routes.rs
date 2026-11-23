//! The routes of C-1143 (P6-TPL-6): list, read and remove an agent's helper files. `S/K`,
//! `can_use` on the agent's workspace (as the agent's other read/write routes). Reading serves
//! the bytes with the C-903 content-safety headers (never inline, never a derived type) plus
//! `X-Helper-Executable`. There is no write route: helpers are written only by the template
//! create/update (P6-TPL-3) and removed here.

use super::dto::{HelperFile, HelperList};
use super::service;
use crate::app::AppState;
use crate::audit::{self, AuditEvent};
use crate::domains::files::content_disposition;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::agent_helpers as repo;
use aurora_db::repo::agents;
use axum::extract::State;
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_helpers))
        .routes(routes!(get_helper))
        .routes(routes!(delete_helper))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct HelperPathQuery {
    /// The helper path below `helpers/`. Absent on the delete route = every helper file.
    pub path: Option<String>,
}

async fn visible_agent(c: &Caller, s: &AppState, id: Uuid) -> Result<agents::AgentRow, ApiError> {
    let row = agents::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    c.workspace(s, row.workspace_id).await?;
    Ok(row)
}

fn record(c: &Caller, s: &AppState, a: &agents::AgentRow, paths: Vec<String>) {
    audit::record(
        s,
        AuditEvent::by(c, "agent.helpers_remove")
            .org(a.org_id)
            .workspace(a.workspace_id)
            .target("agent", a.id)
            .metadata(serde_json::json!({ "paths": paths })),
    );
}

/// List an agent's helper files (C-1143): `{ items: [{ path, size, executable, sha256 }] }`,
/// by path. The bytes travel only through the content route.
#[utoipa::path(get, path = "/agents/{agent}/helpers", operation_id = "listAgentHelpers",
    tag = "helpers", params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = HelperList), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_helpers(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<HelperList>, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let items = repo::list(&s.pool, a.id)
        .await?
        .into_iter()
        .map(HelperFile::from)
        .collect();
    Ok(Json(HelperList { items }))
}

/// Read one helper file's bytes (C-1143): the C-903 content-safety headers — always an
/// attachment of `application/octet-stream`, never rendered inline — plus
/// `X-Helper-Executable: true|false`.
#[utoipa::path(get, path = "/agents/{agent}/helpers/content", operation_id = "getAgentHelper",
    tag = "helpers", params(("agent" = Uuid, Path, description = "Agent id"), HelperPathQuery),
    responses((status = 200, description = "Helper bytes", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 400, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_helper(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<HelperPathQuery>,
) -> Result<Response, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let path = q.path.as_deref().ok_or_else(|| {
        ApiError::unprocessable("template_helper_invalid", "Give the helper path to read.")
    })?;
    let row = repo::get(&s.pool, a.id, path)
        .await?
        .ok_or_else(|| ApiError::not_found("Helper not found."))?;
    let mut res = Response::new(row.content.into());
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(v) = HeaderValue::from_str(&content_disposition(&row.path)) {
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
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(row.size));
    if let Ok(v) = HeaderValue::from_str(if row.executable { "true" } else { "false" }) {
        h.insert(HeaderName::from_static("x-helper-executable"), v);
    }
    Ok(res)
}

/// Remove one helper file (`?path=`) or every helper file of the agent (C-1143) → 204. Audited
/// `agent.helpers_remove` (ordinary) with the removed paths.
#[utoipa::path(delete, path = "/agents/{agent}/helpers", operation_id = "deleteAgentHelper",
    tag = "helpers", params(("agent" = Uuid, Path, description = "Agent id"), HelperPathQuery),
    responses((status = 204), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_helper(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<HelperPathQuery>,
) -> Result<StatusCode, ApiError> {
    let a = visible_agent(&c, &s, agent).await?;
    let paths: Vec<String> = match &q.path {
        Some(p) => vec![p.clone()],
        None => repo::list(&s.pool, a.id)
            .await?
            .into_iter()
            .map(|r| r.path)
            .collect(),
    };
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    service::delete(&mut tx, a.id, q.path.as_deref()).await?;
    tx.commit().await.map_err(DbError::from)?;
    record(&c, &s, &a, paths);
    Ok(StatusCode::NO_CONTENT)
}
