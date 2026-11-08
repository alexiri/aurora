use super::dto::*;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::auth::mail::{self, Mail};
use crate::domains::tenancy::dto::{Org, OrgSettings, OrgSummary};
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, FieldError, ids};
use aurora_db::DbError;
use aurora_db::repo::invitations::{self as repo, NewInvitation};
use aurora_db::repo::{members, orgs, users, workspaces};
use axum::http::StatusCode;
use sqlx::PgConnection;
use uuid::Uuid;

fn limit_reached() -> ApiError {
    ApiError::unprocessable(
        "member_limit_reached",
        "The organization has reached its member limit.",
    )
}

fn already_member() -> ApiError {
    ApiError::conflict(
        "already_member",
        "This user is already a member of the organization.",
    )
}

fn max_members(settings: &serde_json::Value) -> Option<i64> {
    OrgSettings::from_json(settings).max_members
}

fn valid_email(e: &str) -> bool {
    e.len() <= 254
        && !e.chars().any(|c| c.is_whitespace() || c.is_control())
        && e.split_once('@').is_some_and(|(l, d)| {
            !l.is_empty()
                && !d.contains('@')
                && d.contains('.')
                && !d.starts_with('.')
                && !d.ends_with('.')
        })
}

/// Create (or re-issue) an invitation and email the link. Returns the invitation.
pub async fn create(
    state: &AppState,
    org: Uuid,
    inviter: Uuid,
    req: &CreateInvitationRequest,
    actor: AuditActor,
) -> Result<Invitation, ApiError> {
    let email = req.email.trim().to_lowercase();
    if !valid_email(&email) {
        return Err(ApiError::invalid_request(
            "Invalid email.",
            vec![FieldError::new("/email", "must be a valid email address")],
        ));
    }
    let mut ws: Vec<Uuid> = req.workspace_ids.clone();
    ws.sort_unstable();
    ws.dedup();
    let mut appr: Vec<Uuid> = req.approver_workspace_ids.clone();
    appr.sort_unstable();
    appr.dedup();
    if appr.iter().any(|w| ws.contains(w)) {
        return Err(ApiError::invalid_request(
            "A workspace cannot be bound as both member and approver.",
            vec![FieldError::new(
                "/approver_workspace_ids",
                "must be disjoint from workspace_ids",
            )],
        ));
    }

    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let (org_name, settings) = repo::lock_org(&mut tx, org)
        .await?
        .ok_or_else(|| ApiError::not_found("Organization not found."))?;
    let all_ws: Vec<Uuid> = ws.iter().chain(appr.iter()).copied().collect();
    if !all_ws.is_empty() {
        let found = repo::workspaces_in_org(&mut *tx, org, &all_ws).await?;
        if found.len() != all_ws.len() {
            return Err(ApiError::invalid_request(
                "Unknown workspace.",
                vec![FieldError::new(
                    "/workspace_ids",
                    "all workspaces must belong to the organization",
                )],
            ));
        }
    }
    if repo::is_member(&mut *tx, org, &email).await? {
        return Err(already_member());
    }
    if let Some(cap) = max_members(&settings) {
        let used = repo::count_active_members(&mut *tx, org).await?
            + repo::count_pending_except(&mut *tx, org, &email).await?;
        if used + 1 > cap {
            return Err(limit_reached());
        }
    }
    let tok = token::generate(TokenPrefix::Invitation).expose().clone();
    let id = repo::upsert_pending(
        &mut *tx,
        &NewInvitation {
            id: ids::new_id(),
            org_id: org,
            email: &email,
            role: req.role.as_str(),
            workspace_ids: &ws,
            approver_workspace_ids: &appr,
            token_hash: &token::hash(&tok),
            invited_by: inviter,
        },
    )
    .await?;
    let row = repo::get(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::internal("Invitation vanished."))?;
    // RFE-008: `invitation.create` commits with the invitation.
    let ev = AuditEvent::new(actor, "invitation.create", Outcome::Success)
        .org(org)
        .target("invitation", row.id)
        .metadata(serde_json::json!({"email": row.email, "role": req.role.as_str()}));
    audit::commit_critical(tx, &ev).await?;

    let link = format!(
        "{}/join?token={tok}",
        state.config.external_url.trim_end_matches('/')
    );
    let inviter_name = row.invited_by_name.as_deref().unwrap_or("A teammate");
    mail::send_detached(
        &state.mailer,
        Mail::new(
            &email,
            &format!("You are invited to join {org_name} on Aurora"),
            &format!(
                "{inviter_name} invited you to join {org_name} on Aurora. The link expires in 7 days."
            ),
            &link,
        ),
    );
    Ok(row.into())
}

/// Public preview: only pending, unexpired invitations are visible.
pub async fn preview(state: &AppState, tok: &str) -> Result<InvitationPreview, ApiError> {
    let nf = || ApiError::not_found("Invitation not found.");
    if !tok.starts_with(TokenPrefix::Invitation.as_str()) {
        return Err(nf());
    }
    let r = repo::preview(&state.pool, &token::hash(tok))
        .await?
        .ok_or_else(nf)?;
    Ok(InvitationPreview {
        org: OrgSummary {
            id: r.org_id,
            slug: r.org_slug,
            name: r.org_name,
        },
        email: r.email,
        role: if r.role == "admin" {
            InvitedRole::Admin
        } else {
            InvitedRole::Member
        },
        inviter_name: r.inviter_name,
        status: InvitationStatus::Pending,
        expires_at: r.expires_at,
    })
}

fn forbidden(code: &'static str, msg: &'static str) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, code, msg)
}

/// Accept as `user_id`; membership and workspace bindings are created atomically.
pub async fn accept(
    state: &AppState,
    user_id: Uuid,
    tok: &str,
    actor: AuditActor,
) -> Result<(Org, Vec<Uuid>, Uuid), ApiError> {
    let nf = || ApiError::not_found("Invitation not found.");
    if !tok.starts_with(TokenPrefix::Invitation.as_str()) {
        return Err(nf());
    }
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let inv = repo::lock_pending_by_token(&mut tx, &token::hash(tok))
        .await?
        .ok_or_else(nf)?;
    if inv.expired {
        return Err(ApiError::gone(
            "invitation_expired",
            "This invitation has expired.",
        ));
    }
    let user = users::find_by_id(&mut *tx, user_id)
        .await?
        .ok_or_else(|| ApiError::unauthenticated("Authentication required."))?;
    if user.email_verified_at.is_none() {
        return Err(forbidden(
            "email_unverified",
            "Verify your email address before accepting an invitation.",
        ));
    }
    if !user.email.eq_ignore_ascii_case(&inv.email) {
        return Err(forbidden(
            "email_mismatch",
            "This invitation was sent to a different email address.",
        ));
    }
    let bound = accept_in_tx(&mut tx, &user, &inv).await?;
    let org_row = orgs::get(&mut *tx, inv.org_id, user_id)
        .await?
        .ok_or_else(|| ApiError::internal("Organization missing."))?;
    let org: Org = org_row.into();
    let ev = AuditEvent::new(actor, "invitation.accept", Outcome::Success)
        .org(org.id)
        .target("invitation", inv.id)
        .metadata(serde_json::json!({"role": org.my_role.as_str()}));
    audit::commit_critical(tx, &ev).await?;
    Ok((org, bound, inv.id))
}

/// Accept every pending invitation addressed to the new user's verified e-mail as part of their
/// first OIDC sign-in (phase 1, RFE-004: `invite_only` / `sso_jit` sign-up). A seat-cap or
/// duplicate-membership failure skips that invitation (it stays pending for an explicit accept).
pub async fn accept_pending_for_email(
    tx: &mut PgConnection,
    user: &users::UserRow,
) -> Result<usize, ApiError> {
    let pending = repo::lock_pending_by_email(&mut *tx, &user.email).await?;
    let mut accepted = 0;
    for inv in &pending {
        // A savepoint keeps one failing invitation from poisoning the sign-in transaction.
        sqlx::query("SAVEPOINT accept_inv")
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
        match accept_in_tx(&mut *tx, user, inv).await {
            Ok(_) => {
                // RFE-008: the JIT acceptance is audited in the sign-in transaction; a failed
                // audit write fails the sign-in (nothing is accepted).
                let ev = AuditEvent::new(
                    AuditActor {
                        kind: "user",
                        id: Some(user.id),
                        label: user.email.clone(),
                        source: "dashboard",
                        ip: None,
                    },
                    "invitation.accept",
                    Outcome::Success,
                )
                .org(inv.org_id)
                .target("invitation", inv.id)
                .metadata(serde_json::json!({"role": inv.role, "via": "sign_in"}));
                audit::insert_tx(&mut *tx, &ev)
                    .await
                    .map_err(|_| audit::unavailable())?;
                accepted += 1;
                sqlx::query("RELEASE SAVEPOINT accept_inv")
                    .execute(&mut *tx)
                    .await
                    .map_err(DbError::from)?;
            }
            Err(_) => {
                sqlx::query("ROLLBACK TO SAVEPOINT accept_inv")
                    .execute(&mut *tx)
                    .await
                    .map_err(DbError::from)?;
            }
        }
    }
    Ok(accepted)
}

async fn accept_in_tx(
    tx: &mut PgConnection,
    user: &users::UserRow,
    inv: &repo::AcceptRow,
) -> Result<Vec<Uuid>, ApiError> {
    if members::get(&mut *tx, inv.org_id, user.id).await?.is_some() {
        return Err(already_member());
    }
    let (_, settings) = repo::lock_org(&mut *tx, inv.org_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Organization not found."))?;
    if let Some(cap) = max_members(&settings)
        && repo::count_active_members(&mut *tx, inv.org_id).await? + 1 > cap
    {
        return Err(limit_reached());
    }
    members::insert(&mut *tx, inv.org_id, user.id, &inv.role).await?;
    let mut bound = repo::workspaces_in_org(&mut *tx, inv.org_id, &inv.workspace_ids).await?;
    for ws in &bound {
        workspaces::bind_user(&mut *tx, *ws, user.id, "member").await?;
    }
    // Phase 2, RFE-012: approver bindings (an invitation never grants `owner`).
    let approver =
        repo::workspaces_in_org(&mut *tx, inv.org_id, &inv.approver_workspace_ids).await?;
    for ws in &approver {
        workspaces::bind_user(&mut *tx, *ws, user.id, "approver").await?;
    }
    bound.extend(approver);
    if user.default_org_id.is_none() {
        users::set_default_org(&mut *tx, user.id, inv.org_id).await?;
    }
    repo::mark_accepted(&mut *tx, inv.id, user.id).await?;
    Ok(bound)
}
