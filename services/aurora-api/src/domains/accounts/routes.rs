use super::dto::{Account, AccountsQuery};
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::ApiError;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_workspace_accounts))
        .routes(routes!(list_org_accounts))
}

/// One list of what a workspace's agents can use (`can_use`; C-917): the workspace's own apps,
/// websites and AI keys, and the organization's accounts its agents can be given. With
/// `agent_id` the list is read for that agent (`given`, given first).
#[utoipa::path(
    get, path = "/workspaces/{ws}/accounts", operation_id = "listWorkspaceAccounts", tag = "accounts",
    params(("ws" = Uuid, Path, description = "Workspace id"), AccountsQuery),
    responses((status = 200, description = "Accounts", body = Page<Account>),
              (status = 400, description = "Invalid filter", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_workspace_accounts(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<AccountsQuery>,
) -> Result<Json<Page<Account>>, ApiError> {
    Ok(Json(
        service::list_workspace(&state, &caller, ws, &q).await?,
    ))
}

/// The accounts shared with the whole organization (owners and admins; C-917), each with the
/// agents of every workspace that use it and its limit.
#[utoipa::path(
    get, path = "/orgs/{org}/accounts", operation_id = "listOrgAccounts", tag = "accounts",
    params(("org" = Uuid, Path, description = "Organization id"), AccountsQuery),
    responses((status = 200, description = "Accounts", body = Page<Account>),
              (status = 400, description = "Invalid filter", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_org_accounts(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<AccountsQuery>,
) -> Result<Json<Page<Account>>, ApiError> {
    Ok(Json(service::list_org(&state, &caller, org, &q).await?))
}
