use super::dto::*;
use super::service::{self, Freezer, Target, workspace_authority};
use crate::app::AppState;
use crate::audit::{AuditActor, AuditEvent, Outcome};
use crate::domains::tenancy::dto::ConfirmRequest;
use crate::domains::tenancy::service as tenancy;
use crate::http::auth::{Caller, InstanceAdmin, Principal, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::freeze::{self as repo, Effective, Scope};
use aurora_db::repo::{agents, workspaces};
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;
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
        .routes(routes!(admin_get_org, admin_delete_org))
        .routes(routes!(admin_list_org_workspaces))
        .routes(routes!(admin_delete_workspace))
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
        p.after_key(),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(page.map(admin_org)))
}

fn admin_org(r: repo::AdminOrgRow) -> AdminOrg {
    AdminOrg {
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
        ownerless: r.ownerless,
        member_count: r.member_count,
        workspace_count: r.workspace_count,
        agent_count: r.agent_count,
    }
}

async fn admin_org_row(s: &AppState, org: Uuid) -> Result<repo::AdminOrgRow, ApiError> {
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    repo::get_admin_org(&mut conn, org)
        .await?
        .ok_or_else(|| ApiError::not_found("Organization not found."))
}

/// One organization's operator row (instance administrators; counts, no content).
#[utoipa::path(get, path = "/admin/orgs/{org}", operation_id = "adminGetOrg", tag = "admin",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = AdminOrg), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_get_org(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<AdminOrg>, ApiError> {
    Ok(Json(admin_org(admin_org_row(&s, org).await?)))
}

/// One organization's workspaces (instance administrators; metadata only).
#[utoipa::path(get, path = "/admin/orgs/{org}/workspaces", operation_id = "adminListOrgWorkspaces", tag = "admin",
    params(("org" = Uuid, Path, description = "Organization id"), ListAdminWorkspacesQuery),
    responses((status = 200, body = Page<AdminWorkspace>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_list_org_workspaces(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<ListAdminWorkspacesQuery>,
) -> Result<Json<Page<AdminWorkspace>>, ApiError> {
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    if !repo::exists(&mut conn, Scope::Org, org).await? {
        return Err(ApiError::not_found("Organization not found."));
    }
    let rows = repo::list_admin_workspaces(&mut conn, org, p.after_key(), p.fetch()).await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(page.map(|r| AdminWorkspace {
        id: r.id,
        slug: r.slug,
        name: r.name,
        personal: r.personal,
        agent_count: r.agent_count,
        created_at: r.created_at,
    })))
}

/// The admin deletes are session only: an API key or agent token is refused (FIX-157, D-317:
/// no OIDC re-authentication is required any more).
fn require_session(c: &InstanceAdmin) -> Result<(), ApiError> {
    let Principal::Session { .. } = c.principal else {
        return Err(ApiError::forbidden("Session required."));
    };
    Ok(())
}

/// Delete any organization with everything in it (instance administrators; session only,
/// typed slug). Runs the owner delete's services; allowed while frozen; shows no content.
#[utoipa::path(delete, path = "/admin/orgs/{org}", operation_id = "adminDeleteOrg", tag = "admin",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = ConfirmRequest,
    responses((status = 204, description = "Deleted"), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_delete_org(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<ConfirmRequest>,
) -> Result<StatusCode, ApiError> {
    let row = admin_org_row(&s, org).await?;
    require_session(&c)?;
    if req.confirm != row.slug {
        return Err(tenancy::confirmation_mismatch());
    }
    let ev = AuditEvent::new(
        AuditActor::instance_admin(&c),
        "admin.org.delete",
        Outcome::Success,
    )
    .target("organization", org)
    .metadata(json!({
        "org_id": org, "slug": row.slug, "name": row.name,
        "member_count": row.member_count, "workspace_count": row.workspace_count,
        "agent_count": row.agent_count,
    }));
    tenancy::delete_org(&s, org, &ev).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Delete any workspace (instance administrators; session only, typed slug). Runs the
/// owner delete's services; shows no content.
#[utoipa::path(delete, path = "/admin/workspaces/{ws}", operation_id = "adminDeleteWorkspace", tag = "admin",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = ConfirmRequest,
    responses((status = 204, description = "Deleted"), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn admin_delete_workspace(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<ConfirmRequest>,
) -> Result<StatusCode, ApiError> {
    let (org_id, slug, name, agent_count) = {
        let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
        repo::get_admin_workspace(&mut conn, ws)
            .await?
            .ok_or_else(|| ApiError::not_found("Workspace not found."))?
    };
    require_session(&c)?;
    if req.confirm != slug {
        return Err(tenancy::confirmation_mismatch());
    }
    let ev = AuditEvent::new(
        AuditActor::instance_admin(&c),
        "admin.workspace.delete",
        Outcome::Success,
    )
    .target("workspace", ws)
    .metadata(json!({
        "org_id": org_id, "slug": slug, "name": name, "agent_count": agent_count,
    }));
    tenancy::delete_workspace(&s, org_id, ws, &ev).await?;
    Ok(StatusCode::NO_CONTENT)
}
