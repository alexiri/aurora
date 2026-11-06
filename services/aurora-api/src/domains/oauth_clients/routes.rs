use super::dto::*;
use super::service::{self, Scope};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::{Caller, InstanceAdmin, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::ApiError;
use aurora_db::repo::oauth_clients::{self as repo, ClientRow};
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_ws_clients, create_ws_client))
        .routes(routes!(list_org_clients, create_org_client))
        .routes(routes!(list_instance_clients, create_instance_client))
        .routes(routes!(patch_client, delete_client))
}

fn record(state: &AppState, actor: AuditActor, action: &str, row: &ClientRow) {
    let mut ev = AuditEvent::new(actor, action, Outcome::Success)
        .target("oauth_client", row.id)
        .metadata(json!({"package_id": row.package_id, "client_key": row.client_key}));
    if let Some(o) = row.org_id {
        ev = ev.org(o);
    }
    if let Some(w) = row.workspace_id {
        ev = ev.workspace(w);
    }
    audit::record(state, ev);
}

/// List workspace OAuth clients (`can_own`; I-27).
#[utoipa::path(
    get, path = "/workspaces/{ws}/oauth-clients", operation_id = "listWorkspaceOauthClients", tag = "oauth-clients",
    params(("ws" = Uuid, Path, description = "Workspace id"), ListQuery),
    responses((status = 200, description = "Clients", body = Page<OAuthClient>),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_ws_clients(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<OAuthClient>>, ApiError> {
    let a = caller.workspace(&state, ws).await?;
    require_own(&a)?;
    let scope = Scope {
        org: Some(a.org_id),
        ws: Some(ws),
    };
    Ok(Json(service::list(&state, scope, &q).await?))
}

/// Create a workspace OAuth client (`can_own`).
#[utoipa::path(
    post, path = "/workspaces/{ws}/oauth-clients", operation_id = "createWorkspaceOauthClient", tag = "oauth-clients",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = CreateOAuthClientRequest,
    responses((status = 201, description = "Created", body = OAuthClient),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "conflict", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn create_ws_client(
    caller: Caller,
    State(state): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<CreateOAuthClientRequest>,
) -> Result<(StatusCode, Json<OAuthClient>), ApiError> {
    let a = caller.workspace(&state, ws).await?;
    require_own(&a)?;
    let scope = Scope {
        org: Some(a.org_id),
        ws: Some(ws),
    };
    let row = service::create(&state, scope, req).await?;
    record(
        &state,
        AuditActor::from_caller(&caller),
        "oauth_client.create",
        &row,
    );
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// List organization OAuth clients (owner/admin; I-27).
#[utoipa::path(
    get, path = "/orgs/{org}/oauth-clients", operation_id = "listOrgOauthClients", tag = "oauth-clients",
    params(("org" = Uuid, Path, description = "Organization id"), ListQuery),
    responses((status = 200, description = "Clients", body = Page<OAuthClient>),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_org_clients(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<OAuthClient>>, ApiError> {
    caller.org(&state, org).await?.require_admin()?;
    let scope = Scope {
        org: Some(org),
        ws: None,
    };
    Ok(Json(service::list(&state, scope, &q).await?))
}

/// Create an organization OAuth client (owner/admin).
#[utoipa::path(
    post, path = "/orgs/{org}/oauth-clients", operation_id = "createOrgOauthClient", tag = "oauth-clients",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CreateOAuthClientRequest,
    responses((status = 201, description = "Created", body = OAuthClient),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "conflict", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn create_org_client(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CreateOAuthClientRequest>,
) -> Result<(StatusCode, Json<OAuthClient>), ApiError> {
    caller.org(&state, org).await?.require_admin()?;
    let scope = Scope {
        org: Some(org),
        ws: None,
    };
    let row = service::create(&state, scope, req).await?;
    record(
        &state,
        AuditActor::from_caller(&caller),
        "oauth_client.create",
        &row,
    );
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// List instance OAuth clients (instance admin).
#[utoipa::path(
    get, path = "/admin/oauth-clients", operation_id = "listInstanceOauthClients", tag = "oauth-clients",
    params(ListQuery),
    responses((status = 200, description = "Clients", body = Page<OAuthClient>),
              (status = 403, description = "Forbidden", body = ErrorEnvelope)),
    security(("session" = []))
)]
async fn list_instance_clients(
    _admin: InstanceAdmin,
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<OAuthClient>>, ApiError> {
    let scope = Scope {
        org: None,
        ws: None,
    };
    Ok(Json(service::list(&state, scope, &q).await?))
}

/// Create an instance OAuth client (instance admin).
#[utoipa::path(
    post, path = "/admin/oauth-clients", operation_id = "createInstanceOauthClient", tag = "oauth-clients",
    request_body = CreateOAuthClientRequest,
    responses((status = 201, description = "Created", body = OAuthClient),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 409, description = "conflict", body = ErrorEnvelope)),
    security(("session" = []))
)]
async fn create_instance_client(
    InstanceAdmin(caller): InstanceAdmin,
    State(state): State<AppState>,
    Json(req): Json<CreateOAuthClientRequest>,
) -> Result<(StatusCode, Json<OAuthClient>), ApiError> {
    let scope = Scope {
        org: None,
        ws: None,
    };
    let row = service::create(&state, scope, req).await?;
    record(
        &state,
        AuditActor::instance_admin(&caller),
        "oauth_client.create",
        &row,
    );
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Resolve write access to a client: workspace `can_own`, org owner/admin, instance admin.
async fn writable(caller: &Caller, state: &AppState, id: Uuid) -> Result<ClientRow, ApiError> {
    let row = repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("OAuth client not found."))?;
    match (row.org_id, row.workspace_id) {
        (Some(_), Some(ws)) => require_own(&caller.workspace(state, ws).await?)?,
        (Some(org), None) => caller.org(state, org).await?.require_admin()?,
        (None, _) => {
            if !(caller.is_instance_admin && caller.is_session()) {
                return Err(ApiError::forbidden("Instance administrator required."));
            }
        }
    }
    Ok(row)
}

fn actor(caller: &Caller, row: &ClientRow) -> AuditActor {
    if row.org_id.is_none() {
        AuditActor::instance_admin(caller)
    } else {
        AuditActor::from_caller(caller)
    }
}

/// Update fields or the enabled flag of an OAuth client.
#[utoipa::path(
    patch, path = "/oauth-clients/{id}", operation_id = "updateOauthClient", tag = "oauth-clients",
    params(("id" = Uuid, Path, description = "OAuth client id")),
    request_body = PatchOAuthClientRequest,
    responses((status = 200, description = "Updated", body = OAuthClient),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn patch_client(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchOAuthClientRequest>,
) -> Result<Json<OAuthClient>, ApiError> {
    let row = writable(&caller, &state, id).await?;
    let updated = service::patch(&state, &row, req).await?;
    record(
        &state,
        actor(&caller, &row),
        "oauth_client.update",
        &updated,
    );
    Ok(Json(updated.into()))
}

/// Delete an OAuth client; refused while connections use it.
#[utoipa::path(
    delete, path = "/oauth-clients/{id}", operation_id = "deleteOauthClient", tag = "oauth-clients",
    params(("id" = Uuid, Path, description = "OAuth client id")),
    responses((status = 204, description = "Deleted"),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "client_in_use", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn delete_client(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let row = writable(&caller, &state, id).await?;
    service::delete(&state, &row).await?;
    record(&state, actor(&caller, &row), "oauth_client.delete", &row);
    Ok(StatusCode::NO_CONTENT)
}
