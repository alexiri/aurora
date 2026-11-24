use super::service::{self, InstanceUsage, OrgUsage, WorkspaceUsage};
use crate::app::AppState;
use crate::http::auth::{Caller, InstanceAdmin};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::ApiError;
use aurora_db::DbError;
use axum::extract::State;
use chrono::Utc;
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(workspace_usage))
        .routes(routes!(instance_usage))
        .routes(routes!(org_usage))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorkspaceUsageQuery {
    /// First UTC day, `YYYY-MM-DD`, inclusive. Default: `to` − 29 days.
    pub from: Option<String>,
    /// Last UTC day, `YYYY-MM-DD`, inclusive. Default: today (UTC).
    pub to: Option<String>,
    /// Only this agent (any id, also a deleted agent's).
    pub agent_id: Option<Uuid>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct InstanceUsageQuery {
    /// First UTC day, `YYYY-MM-DD`, inclusive. Default: `to` − 29 days.
    pub from: Option<String>,
    /// Last UTC day, `YYYY-MM-DD`, inclusive. Default: today (UTC).
    pub to: Option<String>,
    /// Only this organization (any id, also a deleted organization's).
    pub org_id: Option<Uuid>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OrgUsageQuery {
    /// First UTC day, `YYYY-MM-DD`, inclusive. Default: `to` − 29 days.
    pub from: Option<String>,
    /// Last UTC day, `YYYY-MM-DD`, inclusive. Default: today (UTC).
    pub to: Option<String>,
    /// Only this workspace (any id, also a deleted workspace's; another organization's workspace has no
    /// rows here).
    pub workspace_id: Option<Uuid>,
}

/// Usage of a workspace: turns, turn time, awake time and tokens per day, agent and model (aggregates
/// only: invisible direct conversations are counted). No cost. Aggregates are metadata, so
/// `can_administer` is enough (FIX-234, C-1160) — never a conversation's content.
#[utoipa::path(get, path = "/workspaces/{ws}/usage", operation_id = "getWorkspaceUsage", tag = "usage",
    params(("ws" = Uuid, Path, description = "Workspace id"), WorkspaceUsageQuery),
    responses((status = 200, body = WorkspaceUsage), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn workspace_usage(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<WorkspaceUsageQuery>,
) -> Result<Json<WorkspaceUsage>, ApiError> {
    c.administer(&s, ws).await?;
    let (from, to) =
        service::parse_range(q.from.as_deref(), q.to.as_deref(), Utc::now().date_naive())?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    Ok(Json(
        service::workspace(&mut conn, ws, q.agent_id, from, to).await?,
    ))
}

/// Usage of the whole instance: per day, organization, top workspaces and model (instance
/// administrators). No cost.
#[utoipa::path(get, path = "/admin/usage", operation_id = "getInstanceUsage", tag = "usage",
    params(InstanceUsageQuery),
    responses((status = 200, body = InstanceUsage), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn instance_usage(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Query(q): Query<InstanceUsageQuery>,
) -> Result<Json<InstanceUsage>, ApiError> {
    let (from, to) =
        service::parse_range(q.from.as_deref(), q.to.as_deref(), Utc::now().date_naive())?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    Ok(Json(
        service::instance(&mut conn, q.org_id, from, to).await?,
    ))
}

/// Usage of an organization: per day, top workspaces and model (organization owners and admins). No
/// conversation, agent or person is visible through it. No cost.
#[utoipa::path(get, path = "/orgs/{org}/usage", operation_id = "getOrgUsage", tag = "usage",
    params(("org" = Uuid, Path, description = "Organization id"), OrgUsageQuery),
    responses((status = 200, body = OrgUsage), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn org_usage(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<OrgUsageQuery>,
) -> Result<Json<OrgUsage>, ApiError> {
    c.org(&s, org).await?.require_admin()?;
    let (from, to) =
        service::parse_range(q.from.as_deref(), q.to.as_deref(), Utc::now().date_naive())?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    Ok(Json(
        service::org(&mut conn, org, q.workspace_id, from, to).await?,
    ))
}
