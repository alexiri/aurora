use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::ApiError;
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_ws_connections))
        .routes(routes!(list_org_connections))
        .routes(routes!(get_connection, patch_connection, delete_connection))
        .routes(routes!(limit_preview))
}

/// Who a lower limit would change (C-990): org owners and admins; nothing is written.
#[utoipa::path(
    get, path = "/connections/{id}/limit-preview", operation_id = "previewConnectionLimit", tag = "connections",
    params(("id" = Uuid, Path, description = "Connection id"), LimitPreviewQuery),
    responses((status = 200, description = "Agents the limit would lower", body = LimitPreview),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn limit_preview(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<LimitPreviewQuery>,
) -> Result<Json<LimitPreview>, ApiError> {
    let (row, access) = service::visible(&caller, &state, id).await?;
    access.require_write()?;
    if row.workspace_id.is_some() {
        return Err(ApiError::invalid_request(
            "Invalid limit.",
            vec![aurora_core::FieldError::new(
                "/limit",
                "only an organization's account has a limit",
            )],
        ));
    }
    let Some(limit) = q.limit else {
        return Err(ApiError::invalid_request(
            "Invalid limit.",
            vec![aurora_core::FieldError::new("/limit", "required")],
        ));
    };
    let lowered = crate::domains::grants::service::limit_preview(&state, &row, limit)
        .await?
        .into_iter()
        .map(|l| LoweredAgentView {
            agent: LoweredAgentRef {
                id: l.agent_id,
                name: l.agent_name,
                handle: l.agent_handle,
            },
            workspace: LoweredWorkspaceRef {
                id: l.workspace_id,
                name: l.workspace_name,
            },
            from: l.from.to_owned(),
            to: l.to.to_owned(),
        })
        .collect();
    Ok(Json(LimitPreview { lowered }))
}

/// List workspace-scoped connections (`can_own`; I-27).
#[utoipa::path(
    get, path = "/workspaces/{ws}/connections", operation_id = "listWorkspaceConnections", tag = "connections",
    params(("ws" = Uuid, Path, description = "Workspace id"), ListQuery),
    responses((status = 200, description = "Connections", body = Page<Connection>),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_ws_connections(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<Connection>>, ApiError> {
    let a = caller.workspace(&state, ws).await?;
    require_own(&a)?;
    Ok(Json(service::list(&state, a.org_id, Some(ws), &q).await?))
}

/// List organization-scoped connections (owner/admin, or `can_own` on a workspace of the org
/// so that org connections can be granted; metadata only; I-27).
#[utoipa::path(
    get, path = "/orgs/{org}/connections", operation_id = "listOrgConnections", tag = "connections",
    params(("org" = Uuid, Path, description = "Organization id"), ListQuery),
    responses((status = 200, description = "Connections", body = Page<Connection>),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_org_connections(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<Connection>>, ApiError> {
    caller.org_credential_reader(&state, org).await?;
    Ok(Json(service::list(&state, org, None, &q).await?))
}

/// Get a connection (`can_own` / owner-admin; I-27; org-scope connections are also readable by
/// a `can_own` workspace owner of the org).
#[utoipa::path(
    get, path = "/connections/{id}", operation_id = "getConnection", tag = "connections",
    params(("id" = Uuid, Path, description = "Connection id")),
    responses((status = 200, description = "Connection", body = Connection),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn get_connection(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Connection>, ApiError> {
    let (row, access) = service::visible(&caller, &state, id).await?;
    access.require_read()?;
    Ok(Json(service::view(&state, row).await?))
}

/// Rename, disable or re-enable a connection (`can_own` / owner-admin).
#[utoipa::path(
    patch, path = "/connections/{id}", operation_id = "updateConnection", tag = "connections",
    params(("id" = Uuid, Path, description = "Connection id")),
    request_body = PatchConnectionRequest,
    responses((status = 200, description = "Connection", body = Connection),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "invalid_state", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn patch_connection(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchConnectionRequest>,
) -> Result<Json<Connection>, ApiError> {
    let (row, access) = service::visible(&caller, &state, id).await?;
    access.require_write()?;
    let updated = service::patch(&state, &caller, &row, &req).await?;
    Ok(Json(service::view(&state, updated).await?))
}

/// Delete a connection, its grants and derived rules (`can_own` / owner-admin).
#[utoipa::path(
    delete, path = "/connections/{id}", operation_id = "deleteConnection", tag = "connections",
    params(("id" = Uuid, Path, description = "Connection id")),
    responses((status = 204, description = "Deleted"),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn delete_connection(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (row, access) = service::visible(&caller, &state, id).await?;
    access.require_write()?;
    service::delete(&state, &caller, &row).await?;
    Ok(StatusCode::NO_CONTENT)
}
