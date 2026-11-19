//! Adding people to a workspace by e-mail (phase 5, P5-UX-5, D-325, D-328, SEC-128, C-919, C-920).
//!
//! A workspace owner who is not an organization admin gives **one exact address** access to the workspace:
//! an active organization member with that verified e-mail is bound at once; anyone else gets a pending row that a
//! sign-in applies once the person is an active member of the organization and holds that verified e-mail. Nothing
//! here lists or searches people, creates an invitation or makes anyone a member of the organization (D-328).

use super::dto::{WorkspaceRole, parse_ws_role};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::apilimit::{Charge, Spec};
use crate::http::auth::{Caller, require_own};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use crate::http::pagination::Page;
use aurora_config::SignupMode;
use aurora_core::limits::{PENDING_ACCESS_MAX, PENDING_ACCESS_TTL};
use aurora_core::{ApiError, ids};
use aurora_db::DbError;
use aurora_db::repo::pending_access::{self as repo, Added};
use aurora_db::repo::{placement, workspaces};
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

/// `30 per hour per workspace` on top of the `invite` class (per principal): one owner cannot spray addresses, and
/// several owners of one workspace share the cap.
const BY_EMAIL_PER_WORKSPACE_HOUR: u32 = 30;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(add_by_email))
        .routes(routes!(list_pending))
        .routes(routes!(delete_pending))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddByEmailRequest {
    pub email: String,
    pub role: WorkspaceRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AddResult {
    /// The person is an active member of the organization and now has the access.
    Added,
    /// The access waits for the person's first sign-in.
    Pending,
}

/// How the person can get into the organization (the dashboard says it when the access is pending).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrgEntry {
    /// Already an active member (the access was added at once).
    Member,
    /// Anyone who signs in is placed in this organization.
    OpenSignup,
    /// Someone has to invite them to the organization first (organization owners and admins).
    InvitationNeeded,
}

#[derive(Serialize, ToSchema)]
pub struct AddByEmailResponse {
    pub result: AddResult,
    pub org_entry: OrgEntry,
}

/// One person waiting for their first sign-in.
#[derive(Serialize, ToSchema)]
pub struct PendingAccess {
    pub id: Uuid,
    /// The normalized address.
    pub email: String,
    pub role: WorkspaceRole,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl From<repo::PendingRow> for PendingAccess {
    fn from(r: repo::PendingRow) -> Self {
        Self {
            id: r.id,
            email: r.email_normalized,
            role: parse_ws_role(&r.role),
            expires_at: r.expires_at,
            created_at: r.created_at,
        }
    }
}

fn event(c: &Caller, action: &str) -> AuditEvent {
    AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
}

/// Whether anyone who signs in lands in this organization: open sign-up with a shared-organization placement whose
/// default organization is this one. (Invitations, role claims and JIT also admit people; those are for the
/// organization's admins to arrange, so the dashboard says "invitation needed" for them.)
async fn org_entry(state: &AppState, org: Uuid) -> Result<OrgEntry, ApiError> {
    if state.auth().signup_mode != SignupMode::Open {
        return Ok(OrgEntry::InvitationNeeded);
    }
    let shared = placement::get(&state.pool)
        .await?
        .is_some_and(|p| p.mode == "shared_org" && p.default_org_id == Some(org));
    Ok(if shared {
        OrgEntry::OpenSignup
    } else {
        OrgEntry::InvitationNeeded
    })
}

/// Add one person to the workspace by e-mail (`can_own`; C-919).
#[utoipa::path(post, path = "/workspaces/{ws}/access/by-email", operation_id = "addAccessByEmail", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    request_body = AddByEmailRequest,
    responses((status = 200, body = AddByEmailResponse), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn add_by_email(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Json(req): Json<AddByEmailRequest>,
) -> Result<Json<AddByEmailResponse>, ApiError> {
    let a = c.workspace(&s, ws).await?;
    require_own(&a)?;
    s.api_limiter
        .take(&[Charge {
            key: format!("invite:workspace:{ws}"),
            spec: Spec::per_hour(BY_EMAIL_PER_WORKSPACE_HOUR, BY_EMAIL_PER_WORKSPACE_HOUR),
            scope: "workspace",
            class: "invite",
        }])
        .map_err(|r| r.into_error())?;
    // Normalized like invitations; one exact address, compared for equality (no prefix, no wildcard, no search).
    let email = req.email.trim().to_lowercase();
    if !crate::domains::invitations::service::valid_email(&email) {
        return Err(ApiError::invalid_field(
            "Invalid email.",
            "/email",
            "must be a valid email address",
        ));
    }
    let role = req.role.as_str();

    // An active member of the organization with that verified e-mail: bind at once.
    if let Some((user, _)) =
        repo::active_member_by_verified_email(&s.pool, a.org_id, &email).await?
    {
        let mut tx = audit::begin(&s).await?;
        if aurora_db::repo::idp_roles::ws_is_managed(&mut *tx, ws, user).await? {
            return Err(crate::domains::admin::service::managed_by_idp());
        }
        // Never lower a higher role: adding someone who already owns the workspace as a member changes nothing.
        let current = workspaces::get_access(&mut *tx, ws, user).await?;
        let rank = |r: &str| ["member", "approver", "owner"].iter().position(|x| *x == r);
        let kept = current
            .as_ref()
            .is_some_and(|row| rank(row.role.as_str()) >= rank(role));
        if !kept {
            workspaces::bind_user(&mut *tx, ws, user, role).await?;
        }
        let ev = event(&c, "workspace.access.grant")
            .org(a.org_id)
            .workspace(ws)
            .target("user", user)
            .metadata(json!({"role": role, "email": email, "via": "by_email"}));
        audit::commit_critical(tx, &ev).await?;
        return Ok(Json(AddByEmailResponse {
            result: AddResult::Added,
            org_entry: OrgEntry::Member,
        }));
    }

    // Anyone else: a pending row, never an owner (SEC-128), never an invitation or a membership (D-328).
    if req.role == WorkspaceRole::Owner {
        return Err(ApiError::unprocessable(
            "owner_requires_member",
            "Only someone who is already a member of the organization can be made an owner.",
        ));
    }
    let mut tx = audit::begin(&s).await?;
    let added = repo::add(
        &mut tx,
        ids::new_id(),
        ws,
        a.org_id,
        &email,
        role,
        c.user_id,
        PENDING_ACCESS_TTL.as_secs_f64(),
        PENDING_ACCESS_MAX,
    )
    .await?;
    let row = match added {
        Added::Row(r) => r,
        Added::Full => {
            return Err(ApiError::unprocessable(
                "pending_access_limit",
                "Too many people are already waiting. Remove one first.",
            ));
        }
    };
    let ev = event(&c, "workspace.access.pending_create")
        .org(a.org_id)
        .workspace(ws)
        .target("workspace_pending_access", row.id)
        .metadata(json!({"role": role, "email": email}));
    audit::commit_critical(tx, &ev).await?;
    Ok(Json(AddByEmailResponse {
        result: AddResult::Pending,
        org_entry: org_entry(&s, a.org_id).await?,
    }))
}

/// The workspace's people waiting for their first sign-in (`can_own`; C-920).
#[utoipa::path(get, path = "/workspaces/{ws}/access/pending", operation_id = "listPendingAccess", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id")),
    responses((status = 200, body = Page<PendingAccess>), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_pending(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
) -> Result<Json<Page<PendingAccess>>, ApiError> {
    require_own(&c.workspace(&s, ws).await?)?;
    // At most `PENDING_ACCESS_MAX` rows: one page.
    let rows = repo::list(&s.pool, ws).await?;
    Ok(Json(Page {
        items: rows.into_iter().map(PendingAccess::from).collect(),
        next_cursor: None,
    }))
}

/// Remove one waiting person (`can_own`; C-920). A row of another workspace is a 404.
#[utoipa::path(delete, path = "/workspaces/{ws}/access/pending/{id}", operation_id = "deletePendingAccess", tag = "workspaces",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("id" = Uuid, Path, description = "Pending access id")),
    responses((status = 204, description = "Removed"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_pending(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let a = c.workspace(&s, ws).await?;
    require_own(&a)?;
    let mut tx = audit::begin(&s).await?;
    let row = repo::delete(&mut tx, ws, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Pending access not found."))?;
    let ev = event(&c, "workspace.access.pending_delete")
        .org(a.org_id)
        .workspace(ws)
        .target("workspace_pending_access", id)
        .metadata(json!({"role": row.role, "email": row.email_normalized}));
    audit::commit_critical(tx, &ev).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Apply the pending rows that are due at a sign-in (SEC-128, C-920), inside the sign-in transaction, **after** the
/// person's organization membership was made or confirmed.
///
/// A row is applied only when (1) its address equals one of the person's **verified** e-mails (the verified e-mail the
/// provider vouched for in this sign-in, or the one the account already carries as verified), compared for equality
/// after normalization, and (2) the person is an **active** member of the row's organization. Otherwise the row stays
/// as it is: nothing is applied, and no membership is ever created here. An applied row never lowers a higher role,
/// is deleted, and is audited in the same transaction (a failed audit write fails the sign-in). Idempotent.
pub async fn apply_on_sign_in(
    tx: &mut sqlx::PgConnection,
    user: &aurora_db::repo::users::UserRow,
    provider_verified_email: Option<&str>,
) -> Result<usize, ApiError> {
    let mut emails: Vec<String> = Vec::new();
    if user.email_verified_at.is_some() {
        emails.push(user.email.trim().to_lowercase());
    }
    if let Some(e) = provider_verified_email {
        let e = e.trim().to_lowercase();
        if !emails.contains(&e) {
            emails.push(e);
        }
    }
    let due = repo::due_for_sign_in(&mut *tx, user.id, &emails).await?;
    let rank = |r: &str| ["member", "approver", "owner"].iter().position(|x| *x == r);
    for row in &due {
        let current = workspaces::get_access(&mut *tx, row.workspace_id, user.id).await?;
        let kept = current
            .as_ref()
            .is_some_and(|c| rank(c.role.as_str()) >= rank(&row.role));
        if !kept {
            workspaces::bind_user(&mut *tx, row.workspace_id, user.id, &row.role).await?;
        }
        repo::delete_applied(&mut *tx, row.id).await?;
        let ev = AuditEvent::new(
            AuditActor {
                kind: "user",
                id: Some(user.id),
                label: user.email.clone(),
                source: "dashboard",
                ip: None,
            },
            "workspace.access.pending_apply",
            Outcome::Success,
        )
        .org(row.org_id)
        .workspace(row.workspace_id)
        .target("user", user.id)
        .metadata(json!({"role": row.role, "email": row.email_normalized, "via": "sign_in"}));
        audit::insert_tx(&mut *tx, &ev)
            .await
            .map_err(|_| audit::unavailable())?;
    }
    Ok(due.len())
}

/// The sweeper's pass: remove rows past their expiry.
pub async fn sweep_expired(pool: &aurora_db::PgPool) -> Result<u64, DbError> {
    repo::sweep_expired(pool).await
}
