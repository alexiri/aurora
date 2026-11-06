use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::{Caller, SessionCaller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, PageParams};
use aurora_core::ApiError;
use aurora_db::repo::api_keys;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_org_keys, create_org_key))
        .routes(routes!(list_workspace_keys, create_workspace_key))
        .routes(routes!(list_my_keys))
        .routes(routes!(revoke_key))
}

fn page(rows: Vec<api_keys::ApiKeyRow>, limit: u32) -> Page<ApiKey> {
    let p = Page::from_rows(rows, limit, |r| (r.created_at, r.id));
    Page {
        items: p.items.into_iter().map(Into::into).collect(),
        next_cursor: p.next_cursor,
    }
}

/// List organization API keys.
#[utoipa::path(get, path = "/orgs/{org}/api-keys", operation_id = "listOrgApiKeys", tag = "api-keys",
    params(("org" = Uuid, Path, description = "Organization id"), PageParams),
    responses((status = 200, body = Page<ApiKey>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_org_keys(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<ApiKey>>, ApiError> {
    let p = q.resolve()?;
    c.org(&s, org).await?.require_admin()?;
    let rows = api_keys::list_org(&s.pool, org, p.after.map(|a| (a.t, a.id)), p.fetch()).await?;
    Ok(Json(page(rows, p.limit)))
}

/// Create an organization API key (`auo_`); the token is returned once.
#[utoipa::path(post, path = "/orgs/{org}/api-keys", operation_id = "createOrgApiKey", tag = "api-keys",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CreateKeyRequest,
    responses((status = 201, body = CreatedKey), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_org_key(
    c: SessionCaller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CreateKeyRequest>,
) -> Result<(StatusCode, Json<CreatedKey>), ApiError> {
    c.org(&s, org).await?.require_admin()?;
    let (row, key) = service::create(&s, &c, org, None, req).await?;
    audit::record(
        &s,
        AuditEvent::new(
            AuditActor::from_caller(&c),
            "api_key.create",
            Outcome::Success,
        )
        .org(org)
        .target("api_key", row.id)
        .metadata(json!({"name": row.name, "scope": "org"})),
    );
    Ok((
        StatusCode::CREATED,
        Json(CreatedKey {
            api_key: row.into(),
            key,
        }),
    ))
}

/// List workspace API keys.
#[utoipa::path(get, path = "/workspaces/{ws}/api-keys", operation_id = "listWorkspaceApiKeys", tag = "api-keys",
    params(("ws" = Uuid, Path, description = "Workspace id"), PageParams),
    responses((status = 200, body = Page<ApiKey>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_workspace_keys(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<ApiKey>>, ApiError> {
    let p = q.resolve()?;
    require_own(&c.workspace(&s, ws).await?)?;
    let rows =
        api_keys::list_workspace(&s.pool, ws, p.after.map(|a| (a.t, a.id)), p.fetch()).await?;
    Ok(Json(page(rows, p.limit)))
}

/// Create a workspace API key (`auk_`); the token is returned once.
#[utoipa::path(post, path = "/workspaces/{ws}/api-keys", operation_id = "createWorkspaceApiKey", tag = "api-keys",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = CreateKeyRequest,
    responses((status = 201, body = CreatedKey), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_workspace_key(
    c: SessionCaller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<CreateKeyRequest>,
) -> Result<(StatusCode, Json<CreatedKey>), ApiError> {
    let a = c.workspace(&s, ws).await?;
    require_own(&a)?;
    let (row, key) = service::create(&s, &c, a.org_id, Some(ws), req).await?;
    audit::record(
        &s,
        AuditEvent::new(
            AuditActor::from_caller(&c),
            "api_key.create",
            Outcome::Success,
        )
        .org(a.org_id)
        .workspace(ws)
        .target("api_key", row.id)
        .metadata(json!({"name": row.name, "scope": "workspace"})),
    );
    Ok((
        StatusCode::CREATED,
        Json(CreatedKey {
            api_key: row.into(),
            key,
        }),
    ))
}

/// The caller's own user-kind keys across organizations and workspaces.
#[utoipa::path(get, path = "/me/api-keys", operation_id = "listMyApiKeys", tag = "api-keys",
    params(PageParams),
    responses((status = 200, body = Page<ApiKey>), (status = 401, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_my_keys(
    c: SessionCaller,
    State(s): State<AppState>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<ApiKey>>, ApiError> {
    let p = q.resolve()?;
    let rows = api_keys::list_for_user(&s.pool, c.user_id, p.after.map(|a| (a.t, a.id)), p.fetch())
        .await?;
    Ok(Json(page(rows, p.limit)))
}

/// Revoke an API key (immediate).
#[utoipa::path(delete, path = "/api-keys/{id}", operation_id = "revokeApiKey", tag = "api-keys",
    params(("id" = Uuid, Path, description = "API key id")),
    responses((status = 204, description = "Revoked"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn revoke_key(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let k = api_keys::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("API key not found."))?;
    service::authorize_revoke(&s, &c, &k).await?;
    if api_keys::revoke(&s.pool, id).await? {
        let mut ev = AuditEvent::new(
            AuditActor::from_caller(&c),
            "api_key.revoke",
            Outcome::Success,
        )
        .org(k.org_id)
        .target("api_key", id)
        .metadata(json!({"name": k.name}));
        if let Some(ws) = k.workspace_id {
            ev = ev.workspace(ws);
        }
        audit::record(&s, ev);
    }
    Ok(StatusCode::NO_CONTENT)
}
