use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::agents::dto::Agent;
use crate::domains::automations::service::{Acting, To, reassign_all};
use crate::http::auth::{Caller, Principal, SessionCaller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, PageParams, Pagination};
use aurora_config::OrgCreation;
use aurora_core::ApiError;
use aurora_db::authz::{self, WorkspaceAuth};
use aurora_db::repo::{members, orgs, workspaces};
use aurora_db::{DbError, PgPool};
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_orgs, create_org))
        .routes(routes!(get_org, patch_org, delete_org))
        .routes(routes!(list_members))
        .routes(routes!(patch_member, remove_member))
        .routes(routes!(list_workspaces, create_workspace))
        .routes(routes!(get_workspace, patch_workspace, delete_workspace))
        .routes(routes!(list_access))
        .routes(routes!(put_access, delete_access))
        .routes(routes!(resolve))
}

fn ws_role(a: &WorkspaceAuth) -> WorkspaceRole {
    parse_ws_role(a.effective_ws_role.as_str())
}

fn event(c: &Caller, action: &str) -> AuditEvent {
    AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
}

async fn begin(pool: &PgPool) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, ApiError> {
    Ok(pool.begin().await.map_err(DbError::from)?)
}

fn page_of<R, T>(
    rows: Vec<R>,
    p: &Pagination,
    key: impl Fn(&R) -> (chrono::DateTime<chrono::Utc>, Uuid),
    map: impl Fn(R) -> T,
) -> Page<T> {
    let page = Page::from_rows(rows, p.limit, key);
    Page {
        items: page.items.into_iter().map(map).collect(),
        next_cursor: page.next_cursor,
    }
}

fn after(p: &Pagination) -> Option<(chrono::DateTime<chrono::Utc>, Uuid)> {
    p.after.map(|c| (c.t, c.id))
}

// ---- organizations ----

/// List the caller's organizations (active memberships).
#[utoipa::path(get, path = "/orgs", operation_id = "listOrgs", tag = "orgs",
    params(PageParams),
    responses((status = 200, body = Page<Org>), (status = 401, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_orgs(
    c: SessionCaller,
    State(s): State<AppState>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<Org>>, ApiError> {
    let p = q.resolve()?;
    let rows = orgs::list_for_user(&s.pool, c.user_id, after(&p), p.fetch()).await?;
    let mut page = page_of(rows, &p, |r| (r.created_at, r.id), Org::from);
    let ids: Vec<Uuid> = page.items.iter().map(|o| o.id).collect();
    let frozen = crate::domains::freeze::service::org_frozen(&s, &ids).await?;
    for o in &mut page.items {
        o.frozen = frozen.get(&o.id).cloned();
    }
    Ok(Json(page))
}

/// Create an organization with its default workspace and default agent.
#[utoipa::path(post, path = "/orgs", operation_id = "createOrg", tag = "orgs",
    request_body = CreateOrgRequest,
    responses((status = 201, body = CreateOrgResponse), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_org(
    c: SessionCaller,
    State(s): State<AppState>,
    Json(req): Json<CreateOrgRequest>,
) -> Result<(StatusCode, Json<CreateOrgResponse>), ApiError> {
    if s.config.org_creation == OrgCreation::InstanceAdmins && !c.is_instance_admin {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "org_creation_forbidden",
            "Only instance administrators can create organizations.",
        ));
    }
    let mut tx = begin(&s.pool).await?;
    let created = service::create_organization(
        &mut tx,
        &s.config,
        c.user_id,
        &req.name,
        req.slug.as_deref(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        &s,
        event(&c, "org.create")
            .org(created.org.id)
            .target("organization", created.org.id)
            .metadata(json!({"slug": created.org.slug})),
    );
    audit::record(
        &s,
        event(&c, "workspace.create")
            .org(created.org.id)
            .workspace(created.workspace.id)
            .target("workspace", created.workspace.id)
            .metadata(json!({"slug": created.workspace.slug})),
    );
    Ok((
        StatusCode::CREATED,
        Json(CreateOrgResponse {
            org: created.org.into(),
            workspace: Workspace::new(created.workspace, WorkspaceRole::Owner),
            default_agent: Agent::from(created.default_agent),
        }),
    ))
}

async fn load_org(s: &AppState, org: Uuid, user: Uuid) -> Result<Org, ApiError> {
    let mut o = orgs::get(&s.pool, org, user)
        .await?
        .map(Org::from)
        .ok_or_else(|| ApiError::not_found("Organization not found."))?;
    o.frozen = crate::domains::freeze::service::org_frozen(s, &[org])
        .await?
        .remove(&org);
    Ok(o)
}

/// Get an organization.
#[utoipa::path(get, path = "/orgs/{org}", operation_id = "getOrg", tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = Org), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_org(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<Org>, ApiError> {
    c.org(&s, org).await?;
    Ok(Json(load_org(&s, org, c.user_id).await?))
}

/// Update organization name, slug or settings (owner).
#[utoipa::path(patch, path = "/orgs/{org}", operation_id = "patchOrg", tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = PatchOrgRequest,
    responses((status = 200, body = Org), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_org(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<PatchOrgRequest>,
) -> Result<Json<Org>, ApiError> {
    c.org(&s, org).await?.require_owner()?;
    let meta = json!({
        "name": req.name.is_some(), "slug": req.slug.is_some(), "settings": req.settings.is_some()
    });
    let row = service::patch_org(&s.pool, org, c.user_id, req).await?;
    audit::record(
        &s,
        event(&c, "org.update")
            .org(org)
            .target("organization", org)
            .metadata(meta),
    );
    Ok(Json(row.into()))
}

/// Delete an organization and everything in it (owner, typed confirmation).
#[utoipa::path(delete, path = "/orgs/{org}", operation_id = "deleteOrg", tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = ConfirmRequest,
    responses((status = 204, description = "Deleted"), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_org(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<ConfirmRequest>,
) -> Result<StatusCode, ApiError> {
    c.org(&s, org).await?.require_owner()?;
    let current = load_org(&s, org, c.user_id).await?;
    if req.confirm != current.slug {
        return Err(service::confirmation_mismatch());
    }
    service::delete_org(&s.pool, org).await?;
    audit::record(
        &s,
        event(&c, "org.delete")
            .org(org)
            .target("organization", org)
            .metadata(json!({"slug": current.slug})),
    );
    Ok(StatusCode::NO_CONTENT)
}

// ---- members ----

fn member(m: members::MemberRow) -> Member {
    Member {
        user_id: m.user_id,
        email: m.email,
        display_name: m.display_name,
        role: parse_org_role(&m.role),
        status: parse_status(&m.status),
        created_at: m.created_at,
    }
}

/// List organization members (owner/admin).
#[utoipa::path(get, path = "/orgs/{org}/members", operation_id = "listMembers", tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id"), MembersQuery),
    responses((status = 200, body = Page<Member>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_members(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<MembersQuery>,
) -> Result<Json<Page<Member>>, ApiError> {
    c.org(&s, org).await?.require_admin()?;
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = members::list(
        &s.pool,
        org,
        q.status.map(MemberStatus::as_str),
        after(&p),
        p.fetch(),
    )
    .await?;
    Ok(Json(page_of(
        rows,
        &p,
        |r| (r.created_at, r.user_id),
        member,
    )))
}

/// Change a member's role or status (owner/admin).
#[utoipa::path(patch, path = "/orgs/{org}/members/{user}", operation_id = "patchMember", tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id"), ("user" = Uuid, Path, description = "User id")),
    request_body = PatchMemberRequest,
    responses((status = 200, body = Member), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_member(
    c: Caller,
    State(s): State<AppState>,
    Path((org, user)): Path<(Uuid, Uuid)>,
    Json(req): Json<PatchMemberRequest>,
) -> Result<Json<Member>, ApiError> {
    let auth = c.org(&s, org).await?;
    auth.require_admin()?;
    let actor_role = parse_org_role(auth.role.as_str());
    let actor = AuditActor::from_caller(&c);
    let row = service::patch_member(&s, org, actor_role, user, &req, &actor).await?;
    audit::record(
        &s,
        event(&c, "member.update").org(org).target("user", user).metadata(json!({
            "role": req.role.map(OrgRole::as_str), "status": req.status.map(MemberStatus::as_str)
        })),
    );
    Ok(Json(member(row)))
}

/// Remove a member (owner/admin).
#[utoipa::path(delete, path = "/orgs/{org}/members/{user}", operation_id = "removeMember", tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id"), ("user" = Uuid, Path, description = "User id"), ReassignQuery),
    responses((status = 204, description = "Removed"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn remove_member(
    c: Caller,
    State(s): State<AppState>,
    Path((org, user)): Path<(Uuid, Uuid)>,
    Query(q): Query<ReassignQuery>,
) -> Result<StatusCode, ApiError> {
    let auth = c.org(&s, org).await?;
    auth.require_admin()?;
    let reassign = q
        .reassign_to
        .as_deref()
        .map(|v| To::parse("/reassign_to", v))
        .transpose()?;
    let actor = AuditActor::from_caller(&c);
    service::remove_member(
        &s,
        org,
        parse_org_role(auth.role.as_str()),
        user,
        reassign,
        &actor,
    )
    .await?;
    audit::record(&s, event(&c, "member.remove").org(org).target("user", user));
    Ok(StatusCode::NO_CONTENT)
}

// ---- workspaces ----

/// List workspaces of an organization visible to the caller.
#[utoipa::path(get, path = "/orgs/{org}/workspaces", operation_id = "listWorkspaces", tag = "workspaces",
    params(("org" = Uuid, Path, description = "Organization id"), PageParams),
    responses((status = 200, body = Page<Workspace>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_workspaces(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<Workspace>>, ApiError> {
    let p = q.resolve()?;
    let auth = c.org(&s, org).await?;
    let all = auth.role.is_admin();
    let rows = workspaces::list_visible(&s.pool, org, c.user_id, all, after(&p), p.fetch()).await?;
    let ids: Vec<Uuid> = rows.iter().map(|w| w.id).collect();
    let roles: std::collections::HashMap<Uuid, String> = if all {
        Default::default()
    } else {
        workspaces::roles_of(&s.pool, c.user_id, &ids)
            .await?
            .into_iter()
            .collect()
    };
    let mut page = page_of(
        rows,
        &p,
        |r| (r.created_at, r.id),
        |w| {
            let role = if all {
                WorkspaceRole::Owner
            } else {
                roles
                    .get(&w.id)
                    .map_or(WorkspaceRole::Member, |r| parse_ws_role(r))
            };
            Workspace::new(w, role)
        },
    );
    let ids: Vec<Uuid> = page.items.iter().map(|w| w.id).collect();
    let frozen = crate::domains::freeze::service::workspace_frozen(&s, &ids).await?;
    for w in &mut page.items {
        w.frozen = frozen.get(&w.id).cloned();
    }
    Ok(Json(page))
}

/// Create a workspace (owner/admin, or member when allowed by org settings).
#[utoipa::path(post, path = "/orgs/{org}/workspaces", operation_id = "createWorkspace", tag = "workspaces",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CreateWorkspaceRequest,
    responses((status = 201, body = CreateWorkspaceResponse), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_workspace(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<(StatusCode, Json<CreateWorkspaceResponse>), ApiError> {
    let auth = c.org(&s, org).await?;
    if !auth.role.is_admin() {
        let o = load_org(&s, org, c.user_id).await?;
        if !o.settings.members_can_create_workspaces {
            return Err(ApiError::forbidden(
                "Creating workspaces is restricted to organization owners and admins.",
            ));
        }
    }
    let mut tx = begin(&s.pool).await?;
    let (ws, agent) = service::create_workspace_in_tx(
        &mut tx,
        &s.config,
        org,
        c.user_id,
        &req.name,
        req.slug.as_deref(),
        req.timezone.as_deref(),
        req.memory_language.as_deref(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        &s,
        event(&c, "workspace.create")
            .org(org)
            .workspace(ws.id)
            .target("workspace", ws.id)
            .metadata(json!({"slug": ws.slug})),
    );
    Ok((
        StatusCode::CREATED,
        Json(CreateWorkspaceResponse {
            workspace: Workspace::new(ws, WorkspaceRole::Owner),
            default_agent: agent.into(),
        }),
    ))
}

async fn load_ws(s: &AppState, ws: Uuid, a: &WorkspaceAuth) -> Result<Workspace, ApiError> {
    let row = workspaces::get(&s.pool, ws)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
    with_effective_byo(s, row, ws_role(a)).await
}

/// The workspace DTO with its effective BYO posture (RFE-025).
async fn with_effective_byo(
    s: &AppState,
    row: workspaces::WorkspaceRow,
    role: WorkspaceRole,
) -> Result<Workspace, ApiError> {
    let view =
        crate::domains::agents::byo::workspace_view(s, row.org_id, row.byo_posture.as_deref())
            .await?;
    let id = row.id;
    let mut w = Workspace::new(row, role);
    w.effective_byo_posture = Some(view);
    w.frozen = crate::domains::freeze::service::workspace_frozen(s, &[id])
        .await?
        .remove(&id);
    Ok(w)
}

/// Get a workspace.
#[utoipa::path(get, path = "/workspaces/{ws}", operation_id = "getWorkspace", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    responses((status = 200, body = Workspace), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_workspace(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
) -> Result<Json<Workspace>, ApiError> {
    let a = c.workspace(&s, ws).await?;
    Ok(Json(load_ws(&s, ws, &a).await?))
}

/// Update workspace name, timezone or memory language (`can_own`).
#[utoipa::path(patch, path = "/workspaces/{ws}", operation_id = "patchWorkspace", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = PatchWorkspaceRequest,
    responses((status = 200, body = Workspace), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_workspace(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<PatchWorkspaceRequest>,
) -> Result<Json<Workspace>, ApiError> {
    let a = c.workspace(&s, ws).await?;
    require_own(&a)?;
    let name = req
        .name
        .as_deref()
        .map(|n| service::validate_name("/name", n))
        .transpose()?;
    let tz = req
        .timezone
        .as_deref()
        .map(service::validate_timezone)
        .transpose()?;
    let lang = match req.memory_language.as_deref() {
        Some(l) => {
            let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
            Some(service::validate_language(&mut conn, l).await?)
        }
        None => None,
    };
    let mut row =
        workspaces::update(&s.pool, ws, name.as_deref(), tz.as_deref(), lang.as_deref()).await?;
    audit::record(
        &s,
        event(&c, "workspace.update").org(a.org_id).workspace(ws).target("workspace", ws).metadata(
            json!({"name": name.is_some(), "timezone": tz.is_some(), "memory_language": lang.is_some()}),
        ),
    );
    if let Some(p) = &req.byo_posture {
        row = service::set_byo_posture(&s, &c, &a, ws, row, p.as_deref()).await?;
    }
    Ok(Json(with_effective_byo(&s, row, ws_role(&a)).await?))
}

/// Delete a workspace (`can_own`, typed confirmation).
#[utoipa::path(delete, path = "/workspaces/{ws}", operation_id = "deleteWorkspace", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = ConfirmRequest,
    responses((status = 204, description = "Deleted"), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_workspace(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<ConfirmRequest>,
) -> Result<StatusCode, ApiError> {
    let a = c.workspace(&s, ws).await?;
    require_own(&a)?;
    let w = load_ws(&s, ws, &a).await?;
    if req.confirm != w.slug {
        return Err(service::confirmation_mismatch());
    }
    // Dedicated channel presences are deprovisioned through connector ops before the cascade.
    let mut tx = s.pool.begin().await.map_err(aurora_db::DbError::from)?;
    crate::domains::channels::presences::enqueue_deprovision_for_workspace(&mut tx, ws).await?;
    workspaces::delete(&mut *tx, ws).await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    audit::record(
        &s,
        event(&c, "workspace.delete")
            .org(a.org_id)
            .workspace(ws)
            .target("workspace", ws)
            .metadata(json!({"slug": w.slug})),
    );
    Ok(StatusCode::NO_CONTENT)
}

// ---- access ----

/// List workspace access bindings, including implicit org owners/admins (`can_own`; I-27).
#[utoipa::path(get, path = "/workspaces/{ws}/access", operation_id = "listAccess", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id"), PageParams),
    responses((status = 200, body = Page<AccessBinding>), (status = 403, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_access(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<AccessBinding>>, ApiError> {
    let p = q.resolve()?;
    require_own(&c.workspace(&s, ws).await?)?;
    let rows = workspaces::list_access(&s.pool, ws, after(&p), p.fetch()).await?;
    Ok(Json(page_of(
        rows,
        &p,
        |r| (r.created_at, r.user_id),
        AccessBinding::from,
    )))
}

/// Bind an org member to the workspace (`can_own`).
#[utoipa::path(put, path = "/workspaces/{ws}/access/{user}", operation_id = "putAccess", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("user" = Uuid, Path, description = "User id")),
    request_body = PutAccessRequest,
    responses((status = 200, body = AccessBinding), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn put_access(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, user)): Path<(Uuid, Uuid)>,
    Json(req): Json<PutAccessRequest>,
) -> Result<Json<AccessBinding>, ApiError> {
    let a = c.workspace(&s, ws).await?;
    require_own(&a)?;
    let active = authz::org_membership(&s.pool, user, a.org_id)
        .await?
        .is_some_and(|m| m.is_active());
    if !active {
        return Err(ApiError::unprocessable(
            "not_an_org_member",
            "The user is not an active member of this organization.",
        ));
    }
    workspaces::bind_user(&s.pool, ws, user, req.role.as_str()).await?;
    audit::record(
        &s,
        event(&c, "workspace_access.set")
            .org(a.org_id)
            .workspace(ws)
            .target("user", user)
            .metadata(json!({"role": req.role.as_str()})),
    );
    let row = workspaces::get_access(&s.pool, ws, user)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    Ok(Json(row.into()))
}

/// Remove a user's workspace binding (`can_own`).
#[utoipa::path(delete, path = "/workspaces/{ws}/access/{user}", operation_id = "deleteAccess", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("user" = Uuid, Path, description = "User id"), ReassignQuery),
    responses((status = 204, description = "Removed"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_access(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, user)): Path<(Uuid, Uuid)>,
    Query(q): Query<ReassignQuery>,
) -> Result<StatusCode, ApiError> {
    let a = c.workspace(&s, ws).await?;
    require_own(&a)?;
    let reassign = q
        .reassign_to
        .as_deref()
        .map(|v| To::parse("/reassign_to", v))
        .transpose()?;
    if let Some(m) = authz::org_membership(&s.pool, user, a.org_id).await?
        && m.role.is_admin()
    {
        return Err(ApiError::unprocessable(
            "implicit_access",
            "Organization owners and admins have implicit access.",
        ));
    }
    // RFE-021: reassignment and the unbind commit or roll back together.
    let mut tx = s.pool.begin().await.map_err(aurora_db::DbError::from)?;
    if let Some(to) = reassign {
        let actor = AuditActor::from_caller(&c);
        let acting = Acting {
            actor: &actor,
            org: a.org_id,
            workspace: ws,
        };
        if to == To::User(user) {
            return Err(ApiError::invalid_request(
                "Invalid request.",
                vec![aurora_core::FieldError::new(
                    "/reassign_to",
                    "must differ from the person being removed",
                )],
            ));
        }
        crate::domains::automations::service::ensure_target(&mut tx, to, ws).await?;
        reassign_all(&mut tx, &acting, user, to, true, true, None).await?;
    }
    if !workspaces::unbind_user(&mut *tx, ws, user).await? {
        return Err(ApiError::not_found("Binding not found."));
    }
    // RFE-005: a speaker who loses `can_use` stops binding to `user` rules immediately.
    aurora_db::repo::notify_auth_revoked(&mut *tx, "user", user).await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    audit::record(
        &s,
        event(&c, "workspace_access.remove")
            .org(a.org_id)
            .workspace(ws)
            .target("user", user),
    );
    Ok(StatusCode::NO_CONTENT)
}

// ---- resolve ----

/// Map slugs to ids.
#[utoipa::path(get, path = "/resolve", operation_id = "resolve", tag = "orgs",
    params(ResolveQuery),
    responses((status = 200, body = ResolveResponse), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn resolve(
    c: Caller,
    State(s): State<AppState>,
    Query(q): Query<ResolveQuery>,
) -> Result<Json<ResolveResponse>, ApiError> {
    let nf = || ApiError::not_found("Not found.");
    let (org_id, slug, name) = orgs::get_by_slug(&s.pool, &q.org).await?.ok_or_else(nf)?;
    match &c.principal {
        // A workspace key may resolve its own org (and, below, its own workspace).
        Principal::ApiKey {
            org_id: key_org,
            workspace_id: Some(_),
            ..
        } => {
            if *key_org != org_id
                || !authz::org_membership(&s.pool, c.user_id, org_id)
                    .await?
                    .is_some_and(|m| m.is_active())
            {
                return Err(nf());
            }
        }
        _ => {
            c.org(&s, org_id).await.map_err(|_| nf())?;
        }
    }
    let workspace = match q.workspace {
        None => None,
        Some(slug) => {
            let w = workspaces::get_by_slug(&s.pool, org_id, &slug)
                .await?
                .ok_or_else(nf)?;
            c.workspace(&s, w.id).await.map_err(|_| nf())?;
            Some(ResolvedWorkspace {
                id: w.id,
                slug: w.slug,
                name: w.name,
            })
        }
    };
    Ok(Json(ResolveResponse {
        org: OrgSummary {
            id: org_id,
            slug,
            name,
        },
        workspace,
    }))
}
