use super::dto::*;
use super::service::{self, Freezer, Target, workspace_authority};
use crate::app::AppState;
use crate::audit::AuditActor;
use crate::http::auth::{Caller, InstanceAdmin, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::freeze::{self as repo, Effective, Scope};
use aurora_db::repo::{agents, workspaces};
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(freeze_agent))
        .routes(routes!(unfreeze_agent))
        .routes(routes!(freeze_workspace))
        .routes(routes!(unfreeze_workspace))
        .routes(routes!(freeze_org))
        .routes(routes!(unfreeze_org))
        .routes(routes!(admin_freeze_org))
        .routes(routes!(admin_unfreeze_org))
        .routes(routes!(admin_list_orgs))
}

/// The agent and the caller's authority on it: `can_own` on its workspace.
async fn agent_target(c: &Caller, s: &AppState, id: Uuid) -> Result<(Target, Freezer), ApiError> {
    let a = agents::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let auth = c.workspace(s, a.workspace_id).await?;
    require_own(&auth)?;
    Ok((
        Target {
            scope: Scope::Agent,
            id,
            org_id: a.org_id,
            workspace_id: Some(a.workspace_id),
        },
        freezer(c, workspace_authority(auth.org_role.is_admin())),
    ))
}

async fn workspace_target(
    c: &Caller,
    s: &AppState,
    id: Uuid,
) -> Result<(Target, Freezer), ApiError> {
    let auth = c.workspace(s, id).await?;
    require_own(&auth)?;
    let w = workspaces::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
    Ok((
        Target {
            scope: Scope::Workspace,
            id,
            org_id: w.org_id,
            workspace_id: Some(id),
        },
        freezer(c, workspace_authority(auth.org_role.is_admin())),
    ))
}

async fn org_target(c: &Caller, s: &AppState, id: Uuid) -> Result<(Target, Freezer), ApiError> {
    c.org(s, id).await?.require_owner()?;
    Ok((
        Target {
            scope: Scope::Org,
            id,
            org_id: id,
            workspace_id: None,
        },
        freezer(c, "org_admin"),
    ))
}

fn freezer(c: &Caller, authority: &'static str) -> Freezer {
    Freezer {
        user_id: c.user_id,
        actor: AuditActor::from_caller(c),
        authority,
    }
}

/// Freeze an agent (`can_own`): no turns, no gateway access, sandbox parked, schedules/webhooks/
/// presences paused, state kept. Idempotent; reversible with unfreeze.
#[utoipa::path(post, path = "/agents/{agent}/freeze", operation_id = "freezeAgent", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = FreezeRequest,
    responses((status = 200, body = FreezeResult), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn freeze_agent(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<FreezeRequest>,
) -> Result<Json<FreezeResult>, ApiError> {
    let (t, who) = agent_target(&c, &s, id).await?;
    Ok(Json(service::freeze(&s, &t, &who, &req.reason).await?))
}

/// Unfreeze an agent (`can_own`; 403 `freeze_authority_insufficient` when a higher authority
/// froze it).
#[utoipa::path(post, path = "/agents/{agent}/unfreeze", operation_id = "unfreezeAgent", tag = "agents",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = FreezeResult), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn unfreeze_agent(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<FreezeResult>, ApiError> {
    let (t, who) = agent_target(&c, &s, id).await?;
    Ok(Json(service::unfreeze(&s, &t, &who).await?))
}

/// Freeze a workspace: every agent in it (`can_own`; org owners/admins included).
#[utoipa::path(post, path = "/workspaces/{ws}/freeze", operation_id = "freezeWorkspace", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = FreezeRequest,
    responses((status = 200, body = FreezeResult), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn freeze_workspace(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<FreezeRequest>,
) -> Result<Json<FreezeResult>, ApiError> {
    let (t, who) = workspace_target(&c, &s, ws).await?;
    Ok(Json(service::freeze(&s, &t, &who, &req.reason).await?))
}

/// Unfreeze a workspace (`can_own`; 403 `freeze_authority_insufficient` when a higher authority
/// froze it). Agents frozen on their own stay frozen.
#[utoipa::path(post, path = "/workspaces/{ws}/unfreeze", operation_id = "unfreezeWorkspace", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    responses((status = 200, body = FreezeResult), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn unfreeze_workspace(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
) -> Result<Json<FreezeResult>, ApiError> {
    let (t, who) = workspace_target(&c, &s, ws).await?;
    Ok(Json(service::unfreeze(&s, &t, &who).await?))
}

/// Freeze an organization: every agent of every workspace (org `owner`).
#[utoipa::path(post, path = "/orgs/{org}/freeze", operation_id = "freezeOrg", tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = FreezeRequest,
    responses((status = 200, body = FreezeResult), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn freeze_org(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<FreezeRequest>,
) -> Result<Json<FreezeResult>, ApiError> {
    let (t, who) = org_target(&c, &s, org).await?;
    Ok(Json(service::freeze(&s, &t, &who, &req.reason).await?))
}

/// Unfreeze an organization (org `owner`; 403 `freeze_authority_insufficient` for an
/// instance-admin freeze).
#[utoipa::path(post, path = "/orgs/{org}/unfreeze", operation_id = "unfreezeOrg", tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = FreezeResult), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn unfreeze_org(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<FreezeResult>, ApiError> {
    let (t, who) = org_target(&c, &s, org).await?;
    Ok(Json(service::unfreeze(&s, &t, &who).await?))
}

async fn admin_target(
    c: &InstanceAdmin,
    s: &AppState,
    org: Uuid,
) -> Result<(Target, Freezer), ApiError> {
    // The operator needs no org membership; only the organization has to exist.
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    if !repo::exists(&mut conn, Scope::Org, org).await? {
        return Err(ApiError::not_found("Organization not found."));
    }
    Ok((
        Target {
            scope: Scope::Org,
            id: org,
            org_id: org,
            workspace_id: None,
        },
        Freezer {
            user_id: c.user_id,
            actor: AuditActor::instance_admin(c),
            authority: "instance_admin",
        },
    ))
}

/// Operator kill-switch: freeze an organization (instance administrators). The org's data stays
/// inaccessible to the operator; the freeze lifts only with instance-admin authority.
#[utoipa::path(post, path = "/admin/orgs/{org}/freeze", operation_id = "adminFreezeOrg", tag = "admin",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = FreezeRequest,
    responses((status = 200, body = FreezeResult), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_freeze_org(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<FreezeRequest>,
) -> Result<Json<FreezeResult>, ApiError> {
    let (t, who) = admin_target(&c, &s, org).await?;
    Ok(Json(service::freeze(&s, &t, &who, &req.reason).await?))
}

/// Lift an organization freeze (instance administrators).
#[utoipa::path(post, path = "/admin/orgs/{org}/unfreeze", operation_id = "adminUnfreezeOrg", tag = "admin",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = FreezeResult), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_unfreeze_org(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<FreezeResult>, ApiError> {
    let (t, who) = admin_target(&c, &s, org).await?;
    Ok(Json(service::unfreeze(&s, &t, &who).await?))
}

/// Organizations with their freeze state (instance administrators): the operator view of the
/// kill-switch. Names and slugs only, no org data.
#[utoipa::path(get, path = "/admin/orgs", operation_id = "adminListOrgs", tag = "admin",
    params(ListAdminOrgsQuery),
    responses((status = 200, body = Page<AdminOrg>), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_list_orgs(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Query(q): Query<ListAdminOrgsQuery>,
) -> Result<Json<Page<AdminOrg>>, ApiError> {
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let rows = repo::list_orgs(
        &mut conn,
        q.frozen.unwrap_or(false),
        p.after.map(|c| (c.t, c.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page
            .items
            .into_iter()
            .map(|r| AdminOrg {
                frozen: r.frozen().map(|f| {
                    Frozen::from(&Effective {
                        level: Scope::Org,
                        frozen: f,
                    })
                }),
                id: r.id,
                slug: r.slug,
                name: r.name,
                created_at: r.created_at,
            })
            .collect(),
        next_cursor: page.next_cursor,
    }))
}
