use super::dto::*;
use super::service::{self, WorkspaceRoleStr};
use crate::app::AppState;
use crate::domains::tenancy::dto::WorkspaceRole;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, PageParams, Pagination};
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::groups as repo;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

/// Items per batch (`add`/`remove`).
const BATCH_MAX: usize = 100;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_groups, create_group))
        .routes(routes!(get_group, patch_group, delete_group))
        .routes(routes!(list_members))
        .routes(routes!(put_member, delete_member))
        .routes(routes!(members_batch))
        .routes(routes!(list_group_workspaces))
        .routes(routes!(group_impact))
        .routes(routes!(put_group_access, delete_group_access))
}

fn after(p: &Pagination) -> Option<(chrono::DateTime<chrono::Utc>, Uuid)> {
    p.after.map(|c| (c.t, c.id))
}

fn if_match(h: &HeaderMap) -> Result<Option<i64>, ApiError> {
    let Some(v) = h.get("if-match") else {
        return Ok(None);
    };
    v.to_str()
        .ok()
        .and_then(|s| s.trim().trim_matches('"').parse::<i64>().ok())
        .map(Some)
        .ok_or_else(|| {
            ApiError::invalid_request(
                "Invalid If-Match.",
                vec![FieldError::new("/if-match", "must be the group's revision")],
            )
        })
}

/// List an organization's groups (org `owner`/`admin`; names and counts for `can_own` of any
/// workspace of the org).
#[utoipa::path(get, path = "/orgs/{org}/groups", operation_id = "listGroups", tag = "groups",
    params(("org" = Uuid, Path, description = "Organization id"), GroupListQuery),
    responses((status = 200, body = Page<Group>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_groups(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<GroupListQuery>,
) -> Result<Json<Page<Group>>, ApiError> {
    let auth = c.org_credential_reader(&s, org).await?;
    // (FIX-083, C-460) `member=` reveals a person's memberships — the groups of a colleague are
    // as private as `GET /groups/{id}/members` (org admin): only an admin, or the caller's own id.
    if let Some(member) = q.member
        && member != c.user_id
        && !auth.role.is_admin()
    {
        return Err(ApiError::forbidden(
            "Organization owner/admin required to list another member's groups.",
        ));
    }
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    if let Some(src) = q.source.as_deref()
        && !matches!(src, "manual" | "scim" | "idp")
    {
        return Err(ApiError::invalid_request(
            "Invalid source.",
            vec![FieldError::new("/source", "manual, scim or idp")],
        ));
    }
    let rows = repo::list(
        &s.pool,
        org,
        q.q.as_deref(),
        q.source.as_deref(),
        q.member,
        after(&p),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page.items.into_iter().map(Group::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

/// Create a group (org `owner`/`admin`); `source: "idp"` links it to a provider's claim value.
#[utoipa::path(post, path = "/orgs/{org}/groups", operation_id = "createGroup", tag = "groups",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CreateGroupRequest,
    responses((status = 201, body = Group), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_group(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CreateGroupRequest>,
) -> Result<(StatusCode, Json<Group>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(service::create(&s, &c, org, req).await?),
    ))
}

/// Get a group (names and counts: org `owner`/`admin`, or `can_own` of a workspace of the org).
#[utoipa::path(get, path = "/groups/{id}", operation_id = "getGroup", tag = "groups",
    params(("id" = Uuid, Path, description = "Group id")),
    responses((status = 200, body = Group), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_group(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Group>, ApiError> {
    let g = repo::get(&s.pool, id)
        .await?
        .ok_or_else(service::group_not_found)?;
    c.org_credential_reader(&s, g.org_id).await.map_err(|e| {
        if e.status() == StatusCode::FORBIDDEN {
            e
        } else {
            service::group_not_found()
        }
    })?;
    Ok(Json(g.into()))
}

/// Update a group's name, description or org-role mapping (org `owner`/`admin`; `If-Match`).
#[utoipa::path(patch, path = "/groups/{id}", operation_id = "patchGroup", tag = "groups",
    params(("id" = Uuid, Path, description = "Group id")),
    request_body = PatchGroupRequest,
    responses((status = 200, body = Group), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_group(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<PatchGroupRequest>,
) -> Result<Json<Group>, ApiError> {
    Ok(Json(
        service::patch(&s, &c, id, if_match(&headers)?, req).await?,
    ))
}

/// Delete a group with its memberships and bindings (org `owner`/`admin`). Policy rules that
/// name it are not rewritten: they match nobody.
#[utoipa::path(delete, path = "/groups/{id}", operation_id = "deleteGroup", tag = "groups",
    params(("id" = Uuid, Path, description = "Group id")),
    responses((status = 204, description = "Deleted"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_group(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service::delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A group's members (org `owner`/`admin`).
#[utoipa::path(get, path = "/groups/{id}/members", operation_id = "listGroupMembers", tag = "groups",
    params(("id" = Uuid, Path, description = "Group id"), PageParams),
    responses((status = 200, body = Page<GroupMember>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_members(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<GroupMember>>, ApiError> {
    service::load_admin(&s, &c, id).await?;
    let p = q.resolve()?;
    let rows = repo::list_members(&s.pool, id, after(&p), p.fetch()).await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.user_id));
    Ok(Json(Page {
        items: page.items.into_iter().map(GroupMember::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

/// Add a member of the organization to a manual group (idempotent).
#[utoipa::path(put, path = "/groups/{id}/members/{user}", operation_id = "putGroupMember", tag = "groups",
    params(("id" = Uuid, Path, description = "Group id"), ("user" = Uuid, Path, description = "User id")),
    responses((status = 200, body = MembersBatchResult), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn put_member(
    c: Caller,
    State(s): State<AppState>,
    Path((id, user)): Path<(Uuid, Uuid)>,
) -> Result<Json<MembersBatchResult>, ApiError> {
    Ok(Json(
        service::change_members(&s, &c, id, &[user], &[]).await?,
    ))
}

/// Remove a member from a manual group (idempotent).
#[utoipa::path(delete, path = "/groups/{id}/members/{user}", operation_id = "deleteGroupMember", tag = "groups",
    params(("id" = Uuid, Path, description = "Group id"), ("user" = Uuid, Path, description = "User id")),
    responses((status = 204, description = "Removed"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_member(
    c: Caller,
    State(s): State<AppState>,
    Path((id, user)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    service::change_members(&s, &c, id, &[], &[user]).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Add and remove members in one transaction (at most 100 each; one audit row per change).
#[utoipa::path(post, path = "/groups/{id}/members:batch", operation_id = "groupMembersBatch", tag = "groups",
    params(("id" = Uuid, Path, description = "Group id")),
    request_body = MembersBatchRequest,
    responses((status = 200, body = MembersBatchResult), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn members_batch(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<MembersBatchRequest>,
) -> Result<Json<MembersBatchResult>, ApiError> {
    if req.add.len() > BATCH_MAX || req.remove.len() > BATCH_MAX {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![FieldError::new(
                "/add",
                "at most 100 `add` and 100 `remove`",
            )],
        ));
    }
    Ok(Json(
        service::change_members(&s, &c, id, &req.add, &req.remove).await?,
    ))
}

/// The workspaces a group is bound to, with roles (org `owner`/`admin`).
#[utoipa::path(get, path = "/groups/{id}/workspaces", operation_id = "listGroupWorkspaces", tag = "groups",
    params(("id" = Uuid, Path, description = "Group id"), PageParams),
    responses((status = 200, body = Page<GroupBinding>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_group_workspaces(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<GroupBinding>>, ApiError> {
    service::load_admin(&s, &c, id).await?;
    let p = q.resolve()?;
    let rows = repo::list_bindings(&s.pool, id, after(&p), p.fetch()).await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.workspace_id));
    Ok(Json(Page {
        items: page.items.into_iter().map(GroupBinding::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

/// What deleting the group (or, with `remove_member`, removing that member; with
/// `unbind_workspace`, removing its binding to that workspace) would take away.
#[utoipa::path(get, path = "/groups/{id}/impact", operation_id = "groupImpact", tag = "groups",
    params(("id" = Uuid, Path, description = "Group id"), ImpactQuery),
    responses((status = 200, body = GroupImpact), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn group_impact(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<ImpactQuery>,
) -> Result<Json<GroupImpact>, ApiError> {
    let g = service::load_admin(&s, &c, id).await?;
    let mut conn = s.pool.acquire().await.map_err(aurora_db::DbError::from)?;
    let rows = repo::impact(&mut conn, &g, q.remove_member, q.unbind_workspace).await?;
    Ok(Json(GroupImpact {
        workspaces: rows.into_iter().map(Into::into).collect(),
    }))
}

fn role_str(r: WorkspaceRole) -> &'static str {
    r.as_str()
}

/// Bind a group to a workspace or change its role (`can_own`; the group must belong to the
/// workspace's org).
#[utoipa::path(put, path = "/workspaces/{ws}/access/groups/{group}", operation_id = "putGroupAccess",
    tag = "groups",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("group" = Uuid, Path, description = "Group id")),
    request_body = PutGroupAccessRequest,
    responses((status = 200, body = GroupAccessBinding), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn put_group_access(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, group)): Path<(Uuid, Uuid)>,
    Json(req): Json<PutGroupAccessRequest>,
) -> Result<Json<GroupAccessBinding>, ApiError> {
    service::put_binding(&s, &c, ws, group, WorkspaceRoleStr(role_str(req.role))).await?;
    let b = repo::workspace_bindings(&s.pool, ws)
        .await?
        .into_iter()
        .find(|b| b.group_id == group)
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    Ok(Json(b.into()))
}

/// Unbind a group from a workspace (`can_own`; idempotent).
#[utoipa::path(delete, path = "/workspaces/{ws}/access/groups/{group}", operation_id = "deleteGroupAccess",
    tag = "groups",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("group" = Uuid, Path, description = "Group id")),
    responses((status = 204, description = "Removed"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_group_access(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, group)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    service::delete_binding(&s, &c, ws, group).await?;
    Ok(StatusCode::NO_CONTENT)
}
