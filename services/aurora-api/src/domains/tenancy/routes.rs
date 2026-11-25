use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent};
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
        .routes(routes!(list_workspace_overrides))
        .routes(routes!(get_workspace, patch_workspace, delete_workspace))
        .routes(routes!(list_access))
        .routes(routes!(put_access, patch_access, delete_access))
        .routes(routes!(resolve))
}

fn ws_role(a: &WorkspaceAuth) -> WorkspaceRole {
    parse_ws_role(a.effective_ws_role.as_str())
}

/// (FIX-234, C-1161) The rank of a workspace role (`member` < `approver` < `owner`); `None`
/// when the person holds no binding. "Raising" is moving up this scale.
fn rank_of(role: Option<&str>) -> Option<u8> {
    match role {
        Some("member") => Some(1),
        Some("approver") => Some(2),
        Some("owner") => Some(3),
        _ => None,
    }
}

fn event(c: &Caller, action: &str) -> AuditEvent {
    AuditEvent::by(c, action)
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
    page.map(map)
}

fn after(p: &Pagination) -> Option<(chrono::DateTime<chrono::Utc>, Uuid)> {
    p.after_key()
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
    if s.auth().org_creation == OrgCreation::InstanceAdmins && !c.is_instance_admin {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "org_creation_forbidden",
            "Only instance administrators can create organizations.",
        ));
    }
    let mut tx = begin(&s.pool).await?;
    let created =
        service::create_organization(&mut tx, &s.ops(), c.user_id, &req.name, req.slug.as_deref())
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
            workspace: Workspace::new(created.workspace, Some(WorkspaceRole::Owner), true),
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
    let mut o = load_org(&s, org, c.user_id).await?;
    o.contacts = Some(crate::http::who_can::org_contacts(&s, org).await?);
    Ok(Json(o))
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
    let previews_on = req.settings.as_ref().and_then(|p| p.previews_enabled);
    let meta = json!({
        "name": req.name.is_some(), "slug": req.slug.is_some(),
        "settings": req.settings.is_some(), "previews_enabled": previews_on,
        "calendar_invitations_enabled":
            req.settings.as_ref().and_then(|p| p.calendar_invitations_enabled),
        "outgoing_webhooks_enabled":
            req.settings.as_ref().and_then(|p| p.outgoing_webhooks_enabled),
    });
    let ev = event(&c, "org.update")
        .org(org)
        .target("organization", org)
        .metadata(meta);
    // (P4-PRV-1, C-549) The Port previews switch is its own security-critical audit row,
    // committed in the same transaction as the settings write (docs/04 §Audit).
    let previews_ev = previews_on.map(|on| {
        event(&c, "org.previews.update")
            .org(org)
            .target("organization", org)
            .metadata(json!({ "previews_enabled": on }))
    });
    let ow = &s.ops().outgoing_webhooks;
    let ceiling = (ow.per_minute_max, ow.per_day_max);
    let row = service::patch_org(
        &s.pool,
        org,
        c.user_id,
        req,
        &ev,
        previews_ev.as_ref(),
        ceiling,
    )
    .await?;
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
    let ev = event(&c, "org.delete")
        .org(org)
        .target("organization", org)
        .metadata(json!({"slug": current.slug}));
    service::delete_org(&s, org, &ev).await?;
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
        idp_value: m.idp_value,
        directory: None,
    }
}

fn member_with_scim(m: members::MemberWithScimRow) -> Member {
    let mut v = member(m.member);
    if let Some(state) = m.scim_state {
        v.directory = Some(MemberDirectory {
            state,
            external_id: m.scim_external_id,
        });
    }
    v
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
    let rows = members::list_with_scim(
        &s.pool,
        org,
        after(&p),
        p.fetch(),
        q.status.map(MemberStatus::as_str),
    )
    .await?;
    Ok(Json(page_of(
        rows,
        &p,
        |r| (r.member.created_at, r.member.user_id),
        member_with_scim,
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
    let ev = event(&c, "member.update")
        .org(org)
        .target("user", user)
        .metadata(json!({
            "role": req.role.map(OrgRole::as_str), "status": req.status.map(MemberStatus::as_str)
        }));
    let row = service::patch_member(&s, org, actor_role, user, &req, &actor, ev).await?;
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
    let ev = event(&c, "member.remove").org(org).target("user", user);
    service::remove_member(
        &s,
        org,
        parse_org_role(auth.role.as_str()),
        user,
        reassign,
        &actor,
        &ev,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- workspaces ----

/// What the workspaces tightened in Internet access below the organization (C-964): their
/// published defaults and rule count; only the differing ones unless `?all=true`.
#[utoipa::path(get, path = "/orgs/{org}/workspace-overrides", operation_id = "listWorkspaceOverrides", tag = "workspaces",
    params(("org" = Uuid, Path, description = "Organization id"), WorkspaceOverridesQuery),
    responses((status = 200, body = Page<WorkspaceOverride>), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_workspace_overrides(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<WorkspaceOverridesQuery>,
) -> Result<Json<Page<WorkspaceOverride>>, ApiError> {
    c.org(&s, org).await?.require_admin()?;
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = aurora_db::repo::policy::workspace_overrides(
        &s.pool,
        org,
        q.all.unwrap_or(false),
        after(&p),
        p.fetch(),
    )
    .await?;
    Ok(Json(page_of(
        rows,
        &p,
        |r| (r.created_at, r.id),
        |r| WorkspaceOverride {
            workspace: OverrideWorkspace {
                id: r.id,
                name: r.name,
                personal_of: r
                    .personal_owner_id
                    .zip(r.personal_owner_name)
                    .map(|(id, name)| PersonalOf { id, name }),
            },
            uncredentialed: if r.uncredentialed_default == "block" {
                "block"
            } else {
                "inherit"
            }
            .to_owned(),
            open_lane: if r.open_lane_posture == "enabled" {
                "inherit".to_owned()
            } else {
                r.open_lane_posture
            },
            default_rule: r.default_action,
            rule_count: r.rule_count,
        },
    )))
}

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
    let auth = c.org(&s, org).await?;
    let p = q.resolve()?;
    let all = auth.role.is_admin();
    let rows = workspaces::list_visible(&s.pool, org, c.user_id, all, after(&p), p.fetch()).await?;
    let ids: Vec<Uuid> = rows.iter().map(|w| w.id).collect();
    // (FIX-234, C-1160) Every row's `my_role` is the caller's own access; an administrator
    // without own access sees the workspace with `my_role: null`.
    let roles: std::collections::HashMap<Uuid, String> =
        workspaces::effective_roles_of(&s.pool, c.user_id, &ids)
            .await?
            .into_iter()
            .collect();
    let mut page = page_of(
        rows,
        &p,
        |r| (r.created_at, r.id),
        |w| {
            let role = roles.get(&w.id).map(|r| parse_ws_role(r));
            let administers = all || role == Some(WorkspaceRole::Owner);
            Workspace::new(w, role, administers)
        },
    );
    let ids: Vec<Uuid> = page.items.iter().map(|w| w.id).collect();
    let frozen = crate::domains::freeze::service::workspace_frozen(&s, &ids).await?;
    let owners = aurora_db::repo::placement::personal_owners_of(&s.pool, &ids).await?;
    for w in &mut page.items {
        w.frozen = frozen.get(&w.id).cloned();
        w.personal_of = owners
            .iter()
            .find(|(ws, _, _)| *ws == w.id)
            .map(|(_, id, name)| PersonalOf {
                id: *id,
                name: name.clone(),
            });
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
        &s.ops(),
        org,
        c.user_id,
        &req.name,
        req.slug.as_deref(),
        req.timezone.as_deref(),
        req.memory_language.as_deref(),
    )
    .await?;
    // (C-1061) The default agent of a new workspace in the placement's default organization
    // gets the starter grants too.
    crate::domains::placement::service::apply_new_agent_starters(&mut tx, &s, &agent).await?;
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
            workspace: Workspace::new(ws, Some(WorkspaceRole::Owner), true),
            default_agent: agent.into(),
        }),
    ))
}

async fn load_ws(
    s: &AppState,
    ws: Uuid,
    my_role: Option<WorkspaceRole>,
    administers: bool,
) -> Result<Workspace, ApiError> {
    let row = workspaces::get(&s.pool, ws)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
    with_effective_byo(s, row, my_role, administers).await
}

/// The workspace DTO with its effective BYO posture (RFE-025).
async fn with_effective_byo(
    s: &AppState,
    row: workspaces::WorkspaceRow,
    my_role: Option<WorkspaceRole>,
    administers: bool,
) -> Result<Workspace, ApiError> {
    let view =
        crate::domains::agents::byo::workspace_view(s, row.org_id, row.byo_posture.as_deref())
            .await?;
    let id = row.id;
    let mut w = Workspace::new(row, my_role, administers);
    w.effective_byo_posture = Some(view);
    w.frozen = crate::domains::freeze::service::workspace_frozen(s, &[id])
        .await?
        .remove(&id);
    Ok(w)
}

/// (FIX-234, C-1162) The workspace's people in their own right and its own-right owners, for
/// `Workspace.audience`. Reads the `own_workspace_access` view and people tables: API code only.
async fn workspace_audience(
    s: &AppState,
    ws: Uuid,
    caller: Uuid,
) -> Result<crate::domains::tenancy::dto::WorkspaceAudience, ApiError> {
    use crate::domains::tenancy::dto::{WorkspaceAudience, WorkspaceAudienceOwner};
    let people: i64 =
        sqlx::query_scalar("SELECT count(*) FROM own_workspace_access WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&s.pool)
            .await
            .map_err(DbError::from)?;
    let owners: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT o.user_id, u.display_name FROM own_workspace_access o \
         JOIN users u ON u.id = o.user_id \
         WHERE o.workspace_id = $1 AND o.role = 'owner' \
         ORDER BY u.display_name, o.user_id",
    )
    .bind(ws)
    .fetch_all(&s.pool)
    .await
    .map_err(DbError::from)?;
    let owners_total = owners.len() as i64;
    // The caller first if an owner, then by display name; at most five names.
    let mut owners: Vec<WorkspaceAudienceOwner> = owners
        .into_iter()
        .map(|(id, display_name)| WorkspaceAudienceOwner { id, display_name })
        .collect();
    if let Some(pos) = owners.iter().position(|o| o.id == caller) {
        let me = owners.remove(pos);
        owners.insert(0, me);
    }
    owners.truncate(5);
    // (FIX-236, C-1165, review HIGH-1) The banner: the newest ACTIVE emergency access when any
    // is active, else the newest ended within the last 7 days; every caller who gets `audience`
    // sees it, and simultaneous active grants are all announced.
    let emergency = aurora_db::repo::emergency_access::latest_banner(&s.pool, ws)
        .await?
        .map(|(r, more)| {
            (
                WorkspaceAudienceEmergency {
                    id: r.row.id,
                    by: WorkspaceAudienceOwner {
                        id: r.row.user_id,
                        display_name: r.by_display_name,
                    },
                    reason: r.row.reason,
                    opened_at: r.row.opened_at,
                    expires_at: r.row.expires_at,
                    ended_at: r.row.ended_at,
                    active: r.row.ended_at.is_none() && r.row.expires_at > chrono::Utc::now(),
                },
                more,
            )
        });
    let emergency_active_more = emergency.as_ref().map_or(0, |(_, more)| *more);
    Ok(WorkspaceAudience {
        people,
        owners,
        owners_total,
        emergency: emergency.map(|(e, _)| e),
        emergency_active_more,
    })
}

/// Get a workspace: the caller's own access answers in full; an emergency reader (C-1165) and
/// an administrator without own access (C-1160, C-1162) get the metadata, `contacts` and
/// `audience` — the emergency reader with `emergency_reader: true` and their own-right role
/// (usually none).
#[utoipa::path(get, path = "/workspaces/{ws}", operation_id = "getWorkspace", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    responses((status = 200, body = Workspace), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_workspace(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
) -> Result<Json<Workspace>, ApiError> {
    match c.workspace(&s, ws).await {
        Ok(a) => {
            let mut w = load_ws(&s, ws, Some(ws_role(&a)), true).await?;
            w.contacts = Some(crate::http::who_can::workspace_contacts(&s, ws).await?);
            w.audience = Some(workspace_audience(&s, ws, c.user_id).await?);
            Ok(Json(w))
        }
        Err(e) if e.code() == "workspace_access_required" => {
            // (FIX-236, C-1165) The emergency reader reads the workspace they were opened
            // access to, with their own-right role (an emergency grant is never own access).
            match c.workspace_read(&s, ws).await {
                Ok(read) => {
                    let mut w = load_ws(&s, ws, read.own().map(ws_role), true).await?;
                    w.emergency_reader = true;
                    w.contacts = Some(crate::http::who_can::workspace_contacts(&s, ws).await?);
                    w.audience = Some(workspace_audience(&s, ws, c.user_id).await?);
                    Ok(Json(w))
                }
                Err(_) => {
                    let a = c.administer(&s, ws).await?;
                    debug_assert!(a.own.is_none());
                    let row = workspaces::get(&s.pool, ws)
                        .await?
                        .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
                    let mut w = with_effective_byo(&s, row, None, true).await?;
                    w.contacts = Some(crate::http::who_can::workspace_contacts(&s, ws).await?);
                    w.audience = Some(workspace_audience(&s, ws, c.user_id).await?);
                    Ok(Json(w))
                }
            }
        }
        Err(e) => Err(e),
    }
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
    Ok(Json(
        with_effective_byo(&s, row, Some(ws_role(&a)), true).await?,
    ))
}

/// Delete a workspace (own `can_own` or `can_administer`, C-1160; typed confirmation).
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
    let a = c.administer(&s, ws).await?;
    let w = workspaces::get(&s.pool, ws)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
    if req.confirm != w.slug {
        return Err(service::confirmation_mismatch());
    }
    let ev = event(&c, "workspace.delete")
        .org(a.org_id)
        .workspace(ws)
        .target("workspace", ws)
        .metadata(json!({"slug": w.slug}));
    service::delete_workspace(&s, a.org_id, ws, &ev).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- access ----

/// List the workspace's access bindings (`can_administer`, C-1160; the rows are the bindings in
/// one's own right — since FIX-234 there are no implicit rows). Emergency read access (C-1165)
/// also reads the workspace's people.
#[utoipa::path(get, path = "/workspaces/{ws}/access", operation_id = "listAccess", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id"), crate::domains::groups::dto::AccessQuery),
    responses((status = 200, body = crate::domains::groups::dto::AccessList),
              (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_access(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<crate::domains::groups::dto::AccessQuery>,
) -> Result<Json<crate::domains::groups::dto::AccessList>, ApiError> {
    use crate::domains::groups::dto as g;
    // (FIX-236, C-1165) The emergency reader reads the workspace's people (metadata only). A
    // member with own access does not — the people routes stay administer routes (C-1160) — so
    // only an emergency `ReadAuth` is admitted here; everyone else keeps the administer path.
    match c.workspace_read(&s, ws).await {
        Ok(aurora_db::authz::ReadAuth::Emergency(_)) => {}
        _ => {
            c.administer(&s, ws).await?;
        }
    }
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = workspaces::list_access(&s.pool, ws, after(&p), p.fetch()).await?;
    let page = page_of(rows, &p, |r| (r.created_at, r.user_id), AccessBinding::from);
    let (group_bindings, derived_members) = match q.expand.as_deref() {
        None => (None, None),
        Some("groups") => {
            let bindings = aurora_db::repo::groups::workspace_bindings(&s.pool, ws).await?;
            let dp = Pagination::new(q.limit, q.derived_cursor.as_deref())?;
            let drows =
                aurora_db::repo::groups::derived_members(&s.pool, ws, after(&dp), dp.fetch())
                    .await?;
            let ids: Vec<Uuid> = drows
                .iter()
                .flat_map(|r| r.sources.iter().filter_map(|x| x.group))
                .collect();
            let names: std::collections::HashMap<Uuid, String> =
                aurora_db::repo::groups::names(&s.pool, &ids)
                    .await?
                    .into_iter()
                    .map(|(id, name, _, _)| (id, name))
                    .collect();
            let dpage = Page::from_rows(drows, dp.limit, |r| (r.created_at, r.user_id));
            (
                Some(bindings.into_iter().map(Into::into).collect()),
                Some(dpage.map(|r| g::DerivedMemberView::from_row(r, &names))),
            )
        }
        Some(_) => {
            return Err(ApiError::invalid_request(
                "Invalid expand.",
                vec![aurora_core::FieldError::new("/expand", "must be `groups`")],
            ));
        }
    };
    Ok(Json(g::AccessList {
        items: page.items,
        next_cursor: page.next_cursor,
        group_bindings,
        derived_members,
    }))
}

/// Bind an org member to the workspace (`can_administer`, C-1160). In a personal workspace only
/// its person adds people or raises a role, and the person themselves can never be lowered
/// there (C-1161). An administrator giving *themselves* access is a self-grant: session only,
/// audited critical, the other owners are told.
#[utoipa::path(put, path = "/workspaces/{ws}/access/{user}", operation_id = "putAccess", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("user" = Uuid, Path, description = "User id")),
    request_body = PutAccessRequest,
    responses((status = 200, body = AccessBinding), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn put_access(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, user)): Path<(Uuid, Uuid)>,
    Json(req): Json<PutAccessRequest>,
) -> Result<Json<AccessBinding>, ApiError> {
    let a = c.administer(&s, ws).await?;
    let active = authz::org_membership(&s.pool, user, a.org_id)
        .await?
        .is_some_and(|m| m.is_active());
    if !active {
        return Err(ApiError::unprocessable(
            "not_an_org_member",
            "The user is not an active member of this organization.",
        ));
    }
    // (FIX-234, C-1161) In a personal workspace the person is always the owner (D-480): any
    // change that is not re-putting `owner` on them is a lower — refused, the person included.
    if let Some(person) = a.personal_of {
        if user == person && req.role != WorkspaceRole::Owner {
            return Err(service::personal_workspace_person());
        }
        let own_can_own = a.own.as_ref().is_some_and(|o| o.can_own());
        let raising = match (
            rank_of(
                workspaces::get_access(&s.pool, ws, user)
                    .await?
                    .as_ref()
                    .map(|r| r.role.as_str()),
            ),
            rank_of(Some(req.role.as_str())),
        ) {
            (None, _) => true,
            (Some(p), Some(r)) => r > p,
            (Some(_), None) => false,
        };
        if raising && (c.user_id != person || !own_can_own) {
            return Err(service::personal_workspace_owner_only(
                &service::person_first_name(&s, person).await?,
            ));
        }
    }
    let mut tx = audit::begin(&s).await?;
    if aurora_db::repo::idp_roles::ws_is_managed(&mut *tx, ws, user).await? {
        return Err(crate::domains::admin::service::managed_by_idp());
    }
    // (P5-REFINE-20 batch 6) A role that changes nothing writes nothing (no "changed Carol to Member" for a Member who is
    // already a Member); a person with no direct binding yet is being added, which the audit says with a null `previous_role`.
    let prev = workspaces::get_access(&mut *tx, ws, user).await?;
    if prev
        .as_ref()
        .is_some_and(|r| r.role == req.role.as_str() && r.status == "active")
    {
        drop(tx);
        let row = workspaces::get_access(&s.pool, ws, user)
            .await?
            .ok_or_else(|| ApiError::internal("Internal error."))?;
        return Ok(Json(row.into()));
    }
    // (FIX-234, C-1161) An administrator without own `can_own` who raises *themselves* is a
    // self-grant: a session (never an API key), the critical `workspace_access.self_grant` row
    // and a notice to the other owners.
    let self_grant = user == c.user_id
        && !a.own.as_ref().is_some_and(|o| o.can_own())
        && rank_of(Some(req.role.as_str())) > rank_of(prev.as_ref().map(|r| r.role.as_str()));
    if self_grant && !c.is_session() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "session_required",
            "Adding yourself to a workspace needs a signed-in session, not an API key.",
        ));
    }
    workspaces::bind_user(&mut *tx, ws, user, req.role.as_str()).await?;
    let ev = event(&c, "workspace_access.set")
        .org(a.org_id)
        .workspace(ws)
        .target("user", user)
        .metadata(json!({"role": req.role.as_str(), "previous_role": prev.as_ref().map(|r| r.role.clone())}));
    if self_grant {
        let ev2 = event(&c, "workspace_access.self_grant")
            .org(a.org_id)
            .workspace(ws)
            .target("user", user)
            .metadata(json!({"role": req.role.as_str(), "via": "user"}));
        audit::insert_tx(&mut tx, &ev2).await?;
    }
    audit::commit_critical(tx, &ev).await?;
    if self_grant {
        // (FIX-234 review 5) The notice names the actor by their display name, never the
        // e-mail — `notify_for_user` resolves it like the other notices.
        crate::domains::tenancy::self_grant::notify_for_user(
            &s,
            a.org_id,
            ws,
            &c,
            req.role.as_str(),
        )
        .await?;
    }
    let row = workspaces::get_access(&s.pool, ws, user)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    Ok(Json(row.into()))
}

/// Suspend or restore a person's direct binding in this workspace only (`can_administer`,
/// C-1160; P5-APIGAP-4, C-1051, SEC-138). A suspended binding grants nothing here, direct or
/// through a group. A personal workspace's person is never suspended there (C-1161).
#[utoipa::path(patch, path = "/workspaces/{ws}/access/{user}", operation_id = "patchAccess", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("user" = Uuid, Path, description = "User id")),
    request_body = PatchAccessRequest,
    responses((status = 200, body = AccessBinding), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_access(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, user)): Path<(Uuid, Uuid)>,
    Json(req): Json<PatchAccessRequest>,
) -> Result<Json<AccessBinding>, ApiError> {
    let a = c.administer(&s, ws).await?;
    let action = match req.status {
        AccessStatus::Suspended => "workspace_access.suspend",
        AccessStatus::Active => "workspace_access.restore",
    };
    let ev = event(&c, action)
        .org(a.org_id)
        .workspace(ws)
        .target("user", user)
        .metadata(json!({}));
    service::set_access_status(
        &s,
        a.org_id,
        ws,
        a.personal_of,
        c.user_id,
        user,
        req.status,
        ev,
    )
    .await?;
    let row = workspaces::get_access(&s.pool, ws, user)
        .await?
        .ok_or_else(|| ApiError::not_found("Binding not found."))?;
    Ok(Json(row.into()))
}

/// Remove a user's workspace binding (`can_administer`, C-1160; a personal workspace's person
/// is never removed there, C-1161).
#[utoipa::path(delete, path = "/workspaces/{ws}/access/{user}", operation_id = "deleteAccess", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("user" = Uuid, Path, description = "User id"), ReassignQuery),
    responses((status = 204, description = "Removed"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_access(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, user)): Path<(Uuid, Uuid)>,
    Query(q): Query<ReassignQuery>,
) -> Result<StatusCode, ApiError> {
    let a = c.administer(&s, ws).await?;
    if a.personal_of == Some(user) {
        return Err(service::personal_workspace_person());
    }
    let reassign = q
        .reassign_to
        .as_deref()
        .map(|v| To::parse("/reassign_to", v))
        .transpose()?;
    // (FIX-234, C-1160) An org role grants no workspace right, so an admin's real binding is
    // removed like anyone's; only a personal workspace's person is protected (above).
    // RFE-021: reassignment and the unbind commit or roll back together.
    let mut tx = s.pool.begin().await.map_err(aurora_db::DbError::from)?;
    if aurora_db::repo::idp_roles::ws_is_managed(&mut *tx, ws, user).await? {
        return Err(crate::domains::admin::service::managed_by_idp());
    }
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
    let ev = event(&c, "workspace_access.remove")
        .org(a.org_id)
        .workspace(ws)
        .target("user", user);
    audit::commit_critical(tx, &ev).await?;
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
