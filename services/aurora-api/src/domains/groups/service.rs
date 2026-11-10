//! Directory groups service (P4-GRP-1, D-130, D-131, D-156, SEC-71). Every write is one
//! transaction with its security-critical audit row and its invalidation notifications.
//! Nothing here decides access: roles come from `aurora_db::authz`.
use super::dto::*;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::Caller;
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::authz;
use aurora_db::repo::groups::{
    self as repo, GROUP_BINDINGS_PER_GROUP, GROUP_DESCRIPTION_MAX, GROUP_MEMBERS_MAX,
    GROUP_NAME_MAX, GROUPS_PER_ORG, GROUPS_PER_USER, GroupRow,
};
use serde_json::json;
use std::collections::BTreeSet;
use uuid::Uuid;

pub fn event(c: &Caller, action: &str) -> AuditEvent {
    AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid request.", vec![FieldError::new(path, msg)])
}

pub fn managed_by_idp() -> ApiError {
    ApiError::conflict(
        "managed_by_idp",
        "Members of this group come from your sign-in provider and cannot be edited here.",
    )
}

pub fn managed_by_scim() -> ApiError {
    ApiError::conflict(
        "group_managed_by_scim",
        "This group is managed by your directory (SCIM).",
    )
}

pub fn stale() -> ApiError {
    ApiError::conflict("stale_revision", "The group changed since you loaded it.")
}

pub fn group_not_found() -> ApiError {
    ApiError::not_found("Group not found.")
}

/// Trimmed name, 1-80 chars, no control characters.
pub fn validate_name(path: &str, s: &str) -> Result<String, ApiError> {
    let t = s.trim();
    let n = t.chars().count();
    if n == 0 || n > GROUP_NAME_MAX {
        return Err(bad(path, "must be 1 to 80 characters"));
    }
    if t.chars().any(char::is_control) {
        return Err(bad(path, "must not contain control characters"));
    }
    Ok(t.to_owned())
}

fn validate_description(s: &str) -> Result<Option<String>, ApiError> {
    let t = s.trim();
    if t.chars().count() > GROUP_DESCRIPTION_MAX {
        return Err(bad("/description", "must be at most 280 characters"));
    }
    if t.chars().any(|c| c.is_control() && c != '\n') {
        return Err(bad("/description", "must not contain control characters"));
    }
    Ok((!t.is_empty()).then(|| t.to_owned()))
}

/// `admin` | `member`; `owner` and anything else is 422 `group_org_role_invalid` (SEC-71).
pub fn validate_org_role(s: &str) -> Result<&'static str, ApiError> {
    match s {
        "admin" => Ok("admin"),
        "member" => Ok("member"),
        _ => Err(ApiError::unprocessable(
            "group_org_role_invalid",
            "A group can map to the organization role `admin` or `member`, never `owner`.",
        )),
    }
}

fn limit(code: &str, msg: &str) -> ApiError {
    ApiError::unprocessable(code, msg)
}

/// The group, readable by the caller's org: 404 for a group of another org or an org the caller
/// is not an active member of (the existence of a group is not disclosed).
pub async fn load(s: &AppState, c: &Caller, id: Uuid) -> Result<(GroupRow, crate::http::auth::OrgAuth), ApiError> {
    let g = repo::get(&s.pool, id).await?.ok_or_else(group_not_found)?;
    let auth = c.org(s, g.org_id).await.map_err(|_| group_not_found())?;
    Ok((g, auth))
}

/// Org `owner`/`admin`, 404 otherwise for outsiders and 403 for plain members.
pub async fn load_admin(s: &AppState, c: &Caller, id: Uuid) -> Result<GroupRow, ApiError> {
    let (g, auth) = load(s, c, id).await?;
    auth.require_admin()?;
    Ok(g)
}

fn scim_delete_blocked(settings: &serde_json::Value) -> bool {
    settings
        .pointer("/scim/enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

// ---- groups ----

pub async fn create(
    s: &AppState,
    c: &Caller,
    org: Uuid,
    req: CreateGroupRequest,
) -> Result<Group, ApiError> {
    c.org(s, org).await?.require_admin()?;
    let name = validate_name("/name", &req.name)?;
    let description = match req.description.as_deref() {
        Some(d) => validate_description(d)?,
        None => None,
    };
    let org_role = req.org_role.as_deref().map(validate_org_role).transpose()?;
    let source = req.source.as_deref().unwrap_or("manual");
    let link: Option<(String, String)> = match source {
        "manual" => {
            if req.idp_provider.is_some() || req.idp_value.is_some() {
                return Err(bad("/source", "idp_provider and idp_value need source `idp`"));
            }
            None
        }
        "idp" => {
            let provider = req
                .idp_provider
                .as_deref()
                .ok_or_else(|| bad("/idp_provider", "required for an IdP-linked group"))?;
            let value = req
                .idp_value
                .as_deref()
                .ok_or_else(|| bad("/idp_value", "required for an IdP-linked group"))?;
            if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
                return Err(bad("/idp_value", "must be 1 to 256 characters"));
            }
            let configured = s
                .config
                .auth
                .oidc_providers
                .iter()
                .any(|p| p.id == provider && p.group_claim.is_some());
            if !configured {
                return Err(ApiError::unprocessable(
                    "idp_provider_not_configured",
                    "That sign-in provider has no group_claim configured.",
                ));
            }
            Some((provider.to_owned(), value.to_owned()))
        }
        _ => return Err(bad("/source", "must be `manual` or `idp`")),
    };

    let id = Uuid::now_v7();
    let mut tx = audit::begin(s).await?;
    repo::lock_org(&mut tx, org).await?;
    if repo::count_in_org(&mut *tx, org).await? >= GROUPS_PER_ORG {
        return Err(limit(
            "group_limit_reached",
            "This organization has reached its group limit.",
        ));
    }
    repo::insert(
        &mut tx,
        id,
        org,
        &name,
        description.as_deref(),
        source,
        link.as_ref().map(|(p, v)| (p.as_str(), v.as_str())),
        org_role,
        Some(c.user_id),
    )
    .await
    .map_err(name_or_link_taken)?;
    let ev = event(c, "group.create")
        .org(org)
        .target("group", id)
        .metadata(json!({
            "name": name, "source": source, "org_role": org_role,
            "idp_provider": link.as_ref().map(|l| &l.0),
            "idp_value": link.as_ref().map(|l| &l.1),
        }));
    audit::commit_critical(tx, &ev).await?;
    let row = repo::get(&s.pool, id).await?.ok_or_else(group_not_found)?;
    Ok(row.into())
}

fn name_or_link_taken(e: DbError) -> ApiError {
    match e.constraint() {
        Some("groups_idp_link_uq") => ApiError::conflict(
            "group_idp_link_taken",
            "Another group already mirrors that claim value.",
        ),
        _ if matches!(e, DbError::UniqueViolation { .. }) => ApiError::conflict(
            "group_name_taken",
            "A group with that name already exists.",
        ),
        _ => e.into(),
    }
}

pub async fn patch(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    if_match: Option<i64>,
    req: PatchGroupRequest,
) -> Result<Group, ApiError> {
    let g = load_admin(s, c, id).await?;
    let name = req
        .name
        .as_deref()
        .map(|n| validate_name("/name", n))
        .transpose()?;
    if name.is_some() && g.source == "scim" {
        return Err(managed_by_scim());
    }
    let description = match &req.description {
        Some(Some(d)) => Some(validate_description(d)?),
        Some(None) => Some(None),
        None => None,
    };
    let org_role: Option<Option<&'static str>> = match &req.org_role {
        Some(Some(r)) => Some(Some(validate_org_role(r)?)),
        Some(None) => Some(None),
        None => None,
    };
    let seen = req.revision.or(if_match);
    let mut tx = audit::begin(s).await?;
    let cur = repo::lock(&mut tx, id).await?.ok_or_else(group_not_found)?;
    if seen.is_some_and(|r| r != cur.revision) {
        return Err(stale());
    }
    let revision = cur.revision;
    repo::update(
        &mut tx,
        id,
        revision,
        name.as_deref(),
        description.as_ref().map(|d| d.as_deref()),
        org_role,
    )
    .await
    .map_err(name_or_link_taken)?
    .ok_or_else(stale)?;
    let mut change = serde_json::Map::new();
    if let Some(n) = &name
        && *n != cur.name
    {
        change.insert("name".into(), json!(format!("{} → {n}", cur.name)));
    }
    let mapping_changed = org_role.is_some_and(|r| r.map(str::to_owned) != cur.org_role);
    if mapping_changed {
        change.insert(
            "org_role".into(),
            json!(format!(
                "{} → {}",
                cur.org_role.as_deref().unwrap_or("none"),
                org_role.flatten().unwrap_or("none")
            )),
        );
    }
    if description.is_some() {
        change.insert("description".into(), json!(true));
    }
    if mapping_changed {
        repo::notify_org_changed(&mut tx, g.org_id).await?;
    }
    let ev = event(c, "group.update")
        .org(g.org_id)
        .target("group", id)
        .metadata(json!({ "change": change }));
    audit::commit_critical(tx, &ev).await?;
    let row = repo::get(&s.pool, id).await?.ok_or_else(group_not_found)?;
    Ok(row.into())
}

pub async fn delete(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let g = load_admin(s, c, id).await?;
    if g.source == "scim" {
        let settings: serde_json::Value =
            sqlx::query_scalar("SELECT settings FROM organizations WHERE id = $1")
                .bind(g.org_id)
                .fetch_one(&s.pool)
                .await
                .map_err(DbError::from)?;
        if scim_delete_blocked(&settings) {
            return Err(managed_by_scim());
        }
    }
    let mut tx = audit::begin(s).await?;
    repo::lock(&mut tx, id).await?.ok_or_else(group_not_found)?;
    let workspaces = repo::bound_workspaces(&mut *tx, id).await?;
    let members = repo::member_ids(&mut *tx, id).await?;
    repo::delete(&mut tx, id).await?;
    // Rules naming the group are never rewritten (SEC-71): their dead id is simply in no
    // principal set once every cached identity that carried it is dropped.
    repo::notify_workspaces_changed(&mut tx, g.org_id, &workspaces).await?;
    if g.org_role.is_some() {
        repo::notify_org_changed(&mut tx, g.org_id).await?;
    }
    if members.len() > 200 {
        aurora_db::repo::credentials::notify_gateway_invalidate(&mut *tx, "org", g.org_id).await?;
    } else {
        for m in &members {
            repo::notify_member_changed(&mut tx, g.org_id, *m).await?;
        }
    }
    let ev = event(c, "group.delete")
        .org(g.org_id)
        .target("group", id)
        .metadata(json!({
            "name": g.name, "source": g.source,
            "members": members.len(), "workspaces": workspaces.len(),
        }));
    audit::commit_critical(tx, &ev).await?;
    Ok(())
}

// ---- membership ----

fn member_edit_allowed(g: &GroupRow) -> Result<(), ApiError> {
    match g.source.as_str() {
        "manual" => Ok(()),
        "idp" => Err(managed_by_idp()),
        _ => Err(managed_by_scim()),
    }
}

/// Add `users` and remove `remove` in one transaction, one audit row per change. Returns what
/// actually changed. A user who is not a member of the group's org is 422 `not_an_org_member`
/// (a group never creates a membership).
pub async fn change_members(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    add: &[Uuid],
    remove: &[Uuid],
) -> Result<MembersBatchResult, ApiError> {
    let g = load_admin(s, c, id).await?;
    member_edit_allowed(&g)?;
    let mut tx = audit::begin(s).await?;
    let g = repo::lock(&mut tx, id).await?.ok_or_else(group_not_found)?;
    let mut evs = Vec::new();
    let mut result = MembersBatchResult {
        added: vec![],
        removed: vec![],
    };
    let mut count = repo::member_count(&mut *tx, id).await?;
    let add: BTreeSet<Uuid> = add.iter().copied().collect();
    let remove: BTreeSet<Uuid> = remove.iter().copied().collect();
    for u in &remove {
        if repo::remove_member(&mut tx, id, *u).await? {
            count -= 1;
            repo::notify_member_changed(&mut tx, g.org_id, *u).await?;
            evs.push(
                event(c, "group_member.remove")
                    .org(g.org_id)
                    .target("group", id)
                    .metadata(json!({"user_id": u, "group": g.name})),
            );
            result.removed.push(*u);
        }
    }
    for u in &add {
        if authz::org_membership(&mut *tx, *u, g.org_id)
            .await?
            .is_none()
        {
            return Err(ApiError::unprocessable(
                "not_an_org_member",
                "The user is not a member of this organization.",
            )
            .with_details(json!({"user_id": u})));
        }
        if repo::is_member(&mut *tx, id, *u).await? {
            continue;
        }
        if count >= GROUP_MEMBERS_MAX {
            return Err(limit(
                "group_member_limit_reached",
                "This group has reached its member limit.",
            ));
        }
        if repo::groups_of_user_count(&mut *tx, g.org_id, *u).await? >= GROUPS_PER_USER {
            return Err(limit(
                "group_user_limit_reached",
                "That user is in the maximum number of groups.",
            ));
        }
        repo::add_member(&mut tx, id, g.org_id, *u, "manual", Some(c.user_id)).await?;
        count += 1;
        repo::notify_member_changed(&mut tx, g.org_id, *u).await?;
        evs.push(
            event(c, "group_member.add")
                .org(g.org_id)
                .target("group", id)
                .metadata(json!({"user_id": u, "group": g.name})),
        );
        result.added.push(*u);
    }
    if evs.is_empty() {
        // Nothing changed: idempotent, nothing to audit.
        return Ok(result);
    }
    audit::commit_critical_many(tx, &evs).await?;
    Ok(result)
}

// ---- workspace bindings ----

pub async fn put_binding(
    s: &AppState,
    c: &Caller,
    ws: Uuid,
    group: Uuid,
    role: WorkspaceRoleStr,
) -> Result<(), ApiError> {
    let a = c.workspace(s, ws).await?;
    crate::http::auth::require_own(&a)?;
    let g = repo::get(&s.pool, group).await?.ok_or_else(group_not_found)?;
    if g.org_id != a.org_id {
        return Err(group_not_found());
    }
    let mut tx = audit::begin(s).await?;
    repo::lock(&mut tx, group).await?.ok_or_else(group_not_found)?;
    let prev = repo::binding_role(&mut *tx, group, ws).await?;
    if prev.is_none() && repo::binding_count(&mut *tx, group).await? >= GROUP_BINDINGS_PER_GROUP {
        return Err(limit(
            "group_binding_limit_reached",
            "This group is bound to the maximum number of workspaces.",
        ));
    }
    repo::bind(&mut tx, group, ws, a.org_id, role.0, Some(c.user_id)).await?;
    repo::notify_workspaces_changed(&mut tx, a.org_id, &[ws]).await?;
    let ev = event(c, "group_access.set")
        .org(a.org_id)
        .workspace(ws)
        .target("group", group)
        .metadata(json!({"role": role.0, "previous": prev, "group": g.name}));
    audit::commit_critical(tx, &ev).await?;
    Ok(())
}

pub struct WorkspaceRoleStr(pub &'static str);

pub async fn delete_binding(
    s: &AppState,
    c: &Caller,
    ws: Uuid,
    group: Uuid,
) -> Result<(), ApiError> {
    let a = c.workspace(s, ws).await?;
    crate::http::auth::require_own(&a)?;
    let g = repo::get(&s.pool, group).await?;
    if g.as_ref().is_some_and(|g| g.org_id != a.org_id) {
        return Err(group_not_found());
    }
    let mut tx = audit::begin(s).await?;
    if !repo::unbind(&mut tx, group, ws).await? {
        // Idempotent.
        return Ok(());
    }
    repo::notify_workspaces_changed(&mut tx, a.org_id, &[ws]).await?;
    let ev = event(c, "group_access.remove")
        .org(a.org_id)
        .workspace(ws)
        .target("group", group)
        .metadata(json!({"group": g.map(|g| g.name)}));
    audit::commit_critical(tx, &ev).await?;
    Ok(())
}
