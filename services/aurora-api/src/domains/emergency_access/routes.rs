//! Emergency read access routes (FIX-236, D-478, C-1164). Break-glass: an organization owner,
//! in a session, opens time-limited read-only access to one workspace of their organization,
//! with a reason. The open, list and end routes carry metadata only (C-1165) — never content.

use super::dto::{
    EmergencyAccess, EmergencyAccessBy, EmergencyAccessWorkspace, OpenEmergencyAccessRequest,
};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::emergency_access::notify;
use crate::http::apilimit::{Charge, Spec};
use crate::http::auth::{Caller, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, PageParams};
use aurora_core::{ApiError, sanitize};
use aurora_db::DbError;
use aurora_db::authz::{self, OrgRole};
use aurora_db::repo::{emergency_access as repo, workspaces};
use axum::extract::State;
use chrono::{Duration, Utc};
use http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(open_emergency_access))
        .routes(routes!(list_workspace_emergency_access))
        .routes(routes!(end_emergency_access))
        .routes(routes!(list_org_emergency_access))
}

/// Hours of one grant: 1–24, default 4 (D-478(a)).
const DEFAULT_HOURS: i64 = 4;
const MIN_HOURS: i64 = 1;
const MAX_HOURS: i64 = 24;
const REASON_MIN: usize = 10;
const REASON_MAX: usize = 500;
/// The `invite` class' per-hour ceiling on one break-glass open (per principal, like the other
/// invite-class routes): one compromised dashboard cannot spray grants.
const OPEN_PER_HOUR: u32 = 30;

fn reason_invalid(msg: &str) -> ApiError {
    ApiError::unprocessable("reason_invalid", msg)
}

fn hours_invalid(msg: &str) -> ApiError {
    ApiError::unprocessable("hours_invalid", msg)
}

/// 403 `org_owner_required`: an org admin, a member, or an instance admin who is not an org
/// owner (C-1164).
fn org_owner_required() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "org_owner_required",
        "Only an organization owner can open or manage emergency read access.",
    )
}

fn row_of(r: repo::EmergencyRow, ws_name: String, by_name: String) -> EmergencyAccess {
    EmergencyAccess {
        id: r.id,
        workspace: EmergencyAccessWorkspace {
            id: r.workspace_id,
            name: ws_name,
        },
        by: EmergencyAccessBy {
            id: r.user_id,
            display_name: by_name,
        },
        reason: r.reason,
        opened_at: r.opened_at,
        expires_at: r.expires_at,
        ended_at: r.ended_at,
        end_reason: r.end_reason,
        active: r.ended_at.is_none() && r.expires_at > Utc::now(),
    }
}

/// The workspace's row or 404; also its org id.
async fn ws_row(s: &AppState, ws: Uuid) -> Result<workspaces::WorkspaceRow, ApiError> {
    workspaces::get(&s.pool, ws)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))
}

/// The caller's effective org role in the workspace's organization, 404 when they are not an
/// active member of it (the workspace stays invisible).
async fn member_role(s: &AppState, user: Uuid, org: Uuid) -> Result<OrgRole, ApiError> {
    authz::effective_org_role(&s.pool, user, org)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))
}

/// Open emergency read access on a workspace (C-1164): a **session** of an active effective org
/// owner of the workspace's organization, with a reason (10–500 characters) and 1–24 hours
/// (default 4). The critical audit row `workspace.emergency_access.open` commits with the grant;
/// the notices (C-1165) go out after the commit and never fail it.
#[utoipa::path(post, path = "/workspaces/{ws}/emergency-access", operation_id = "openEmergencyAccess", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = OpenEmergencyAccessRequest,
    responses((status = 201, body = EmergencyAccess), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn open_emergency_access(
    SessionCaller(c): SessionCaller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<OpenEmergencyAccessRequest>,
) -> Result<(StatusCode, Json<EmergencyAccess>), ApiError> {
    let w = ws_row(&s, ws).await?;
    // Owner means the effective org role `owner` (D-478(a)); an org admin, a member and an
    // instance admin who is not an org owner get 403 `org_owner_required`.
    if member_role(&s, c.user_id, w.org_id).await? != OrgRole::Owner {
        return Err(org_owner_required());
    }
    s.api_limiter
        .take(&[Charge {
            key: format!("invite:emergency:user:{}", c.user_id),
            spec: Spec::per_hour(OPEN_PER_HOUR, OPEN_PER_HOUR),
            scope: "workspace",
            class: "invite",
        }])
        .map_err(|r| r.into_error())?;
    // A caller who already has own `can_own` there gains nothing: 409.
    if authz::can_own(&s.pool, c.user_id, ws).await? {
        return Err(ApiError::conflict(
            "already_has_access",
            "You already hold owner access to this workspace.",
        ));
    }
    // One active grant per (workspace, person); the partial unique index backs this up.
    if repo::active_of(&s.pool, ws, c.user_id).await?.is_some() {
        return Err(ApiError::conflict(
            "emergency_access_active",
            "You already have emergency read access open on this workspace.",
        ));
    }
    let reason = sanitize::one_line(&req.reason, REASON_MAX);
    let chars = reason.chars().count();
    if !(REASON_MIN..=REASON_MAX).contains(&chars) {
        return Err(reason_invalid("must be 10 to 500 characters"));
    }
    let hours = req.hours.unwrap_or(DEFAULT_HOURS);
    if !(MIN_HOURS..=MAX_HOURS).contains(&hours) {
        return Err(hours_invalid("must be between 1 and 24"));
    }
    let expires_at = Utc::now() + Duration::hours(hours);
    let id = aurora_core::ids::new_id();
    let mut tx = audit::begin(&s).await?;
    // (Review LOW-10, verify MEDIUM-1) The reopen-in-the-sweep-window case: end the caller's
    // own expired-but-unswept row here, so the insert cannot trip the partial unique index; a
    // truly concurrent open (two racing requests) still hits it and maps to the same conflict.
    // The ended grants get the sweep's critical expire rows in THIS transaction — the audit
    // log must show open → expire → open even when the reopen beats the sweep.
    let stale = repo::expire_own_stale(&mut tx, ws, c.user_id).await?;
    for r in &stale {
        let ev = AuditEvent::new(
            AuditActor::system("emergency-access-expiry"),
            "workspace.emergency_access.expire",
            Outcome::Success,
        )
        .org(r.org_id)
        .workspace(r.workspace_id)
        .target("workspace_emergency_access", r.id)
        .metadata(serde_json::json!({
            "opener": r.user_id,
            "expires_at": r.expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        }));
        audit::insert_tx(&mut tx, &ev).await?;
    }
    let row = repo::open(&mut tx, id, w.org_id, ws, c.user_id, &reason, expires_at)
        .await
        .map_err(|e| match e {
            DbError::UniqueViolation { .. } => ApiError::conflict(
                "emergency_access_active",
                "You already have emergency read access open on this workspace.",
            ),
            e => e.into(),
        })?;
    let ev = AuditEvent::by(&c, "workspace.emergency_access.open")
        .org(w.org_id)
        .workspace(ws)
        .target("workspace_emergency_access", id)
        .metadata(serde_json::json!({
            "reason": reason,
            "hours": hours,
            "workspace_id": ws,
        }));
    audit::commit_critical(tx, &ev).await?;
    // (D-478(c), review MEDIUM-6) The people and the other owners are told at once — from a
    // spawned task **after** the commit: a large workspace must not hold the request, and a
    // disconnect must not silently stop the remaining notices. A notice failure is logged
    // inside the task; it never undoes or fails the grant.
    let s2 = s.clone();
    let reason2 = reason.clone();
    tokio::spawn(async move {
        if let Err(e) =
            notify::notify_open_for(&s2, w.org_id, ws, c.user_id, &reason2, expires_at).await
        {
            tracing::error!(error = %e, "emergency-access notices failed");
        }
    });
    let (by_name,): (String,) = sqlx::query_as("SELECT display_name FROM users WHERE id = $1")
        .bind(c.user_id)
        .fetch_one(&s.pool)
        .await
        .map_err(DbError::from)?;
    Ok((StatusCode::CREATED, Json(row_of(row, w.name, by_name))))
}

/// The workspace's emergency access history (`GET /v1/workspaces/{ws}/emergency-access`): any
/// caller with own access or `can_administer` (C-1164). Metadata only, newest first, paged.
#[utoipa::path(get, path = "/workspaces/{ws}/emergency-access", operation_id = "listWorkspaceEmergencyAccess", tag = "workspaces",
    params(PageParams, ("ws" = Uuid, Path, description = "Workspace id")),
    responses((status = 200, body = Page<EmergencyAccess>), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_workspace_emergency_access(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(p): Query<PageParams>,
) -> Result<Json<Page<EmergencyAccess>>, ApiError> {
    // Own access, or the administer right (FIX-234's two refusals stay).
    match c.workspace(&s, ws).await {
        Ok(_) => {}
        Err(e) if e.code() == "workspace_access_required" => {
            c.administer(&s, ws).await?;
        }
        Err(e) => return Err(e),
    }
    let p = p.resolve()?;
    let rows = repo::list_for_workspace(&s.pool, ws, p.fetch(), p.after_key()).await?;
    Ok(Json(
        Page::from_rows(rows, p.limit, |r| (r.row.opened_at, r.row.id))
            .map(|r| row_of(r.row, r.workspace_name, r.by_display_name)),
    ))
}

/// End emergency read access early (C-1164): the opener, or any active org owner of the
/// organization, in a session. Idempotent: a grant that already ended answers 200 with the row.
#[utoipa::path(post, path = "/emergency-access/{id}/end", operation_id = "endEmergencyAccess", tag = "workspaces",
    params(("id" = Uuid, Path, description = "Emergency access id")),
    responses((status = 200, body = EmergencyAccess), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn end_emergency_access(
    SessionCaller(c): SessionCaller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<EmergencyAccess>, ApiError> {
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Emergency access not found."))?;
    if row.user_id != c.user_id && member_role(&s, c.user_id, row.org_id).await? != OrgRole::Owner {
        return Err(org_owner_required());
    }
    let mut tx = audit::begin(&s).await?;
    // Lock so two enders cannot race; already-ended rows answer idempotently with no new row.
    // The row was just read, so the lock's None is unreachable — but panic-free all the same.
    let current = repo::lock(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Emergency access not found."))?;
    if repo::end(&mut tx, id, Some(c.user_id), repo::END_ENDED).await? {
        let ev = AuditEvent::by(&c, "workspace.emergency_access.end")
            .org(current.org_id)
            .workspace(current.workspace_id)
            .target("workspace_emergency_access", id)
            .metadata(serde_json::json!({ "opener": current.user_id }));
        audit::commit_critical(tx, &ev).await?;
    } else {
        tx.rollback().await.map_err(DbError::from)?;
    }
    let (ws_name,): (String,) = sqlx::query_as("SELECT name FROM workspaces WHERE id = $1")
        .bind(row.workspace_id)
        .fetch_one(&s.pool)
        .await
        .map_err(DbError::from)?;
    let (by_name,): (String,) = sqlx::query_as("SELECT display_name FROM users WHERE id = $1")
        .bind(row.user_id)
        .fetch_one(&s.pool)
        .await
        .map_err(DbError::from)?;
    let ended = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Emergency access not found."))?;
    Ok(Json(row_of(ended, ws_name, by_name)))
}

/// The organization's emergency access history (`GET /v1/orgs/{org}/emergency-access`): org
/// owners and admins (C-1164). Metadata only, newest first, paged.
#[utoipa::path(get, path = "/orgs/{org}/emergency-access", operation_id = "listOrgEmergencyAccess", tag = "orgs",
    params(PageParams, ("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = Page<EmergencyAccess>), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_org_emergency_access(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(p): Query<PageParams>,
) -> Result<Json<Page<EmergencyAccess>>, ApiError> {
    c.org(&s, org).await?.require_admin()?;
    let p = p.resolve()?;
    let rows = repo::list_for_org(&s.pool, org, p.fetch(), p.after_key()).await?;
    Ok(Json(
        Page::from_rows(rows, p.limit, |r| (r.row.opened_at, r.row.id))
            .map(|r| row_of(r.row, r.workspace_name, r.by_display_name)),
    ))
}
