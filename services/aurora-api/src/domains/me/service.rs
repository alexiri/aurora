use super::dto::*;
use crate::app::AppState;
use crate::domains::tenancy::dto::{OrgSummary, parse_org_role, parse_status, parse_ws_role};
use crate::domains::tenancy::service::WorkspaceDeletion;
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::repo::{account, notify_auth_revoked, sessions, users};
use http::StatusCode;
use uuid::Uuid;

/// Max age of the session's last OIDC re-authentication for account deletion (docs/04 §OIDC).
pub const REAUTH_WINDOW_SECS: f64 = 600.0;

pub async fn load_me(state: &AppState, user_id: Uuid) -> Result<Me, ApiError> {
    let user = users::find_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::unauthenticated("Authentication required."))?;
    let rows = account::memberships(&state.pool, user_id).await?;
    let ws = account::usable_workspaces(&state.pool, user_id).await?;
    let eff = aurora_db::repo::groups::effective_org_roles(&state.pool, user_id).await?;
    let mapped = aurora_db::repo::groups::org_role_groups(&state.pool, user_id).await?;
    let groups = aurora_db::repo::groups::groups_of_user(&state.pool, user_id)
        .await?
        .into_iter()
        .map(|(id, name, org_id)| MeGroup { id, name, org_id })
        .collect();
    let memberships = rows
        .into_iter()
        .map(|m| {
            let effective_role = eff
                .iter()
                .find(|(o, _)| *o == m.org_id)
                .map(|(_, r)| parse_org_role(r));
            let mut sources = vec![MeRoleSource {
                kind: "direct".into(),
                group: None,
            }];
            sources.extend(
                mapped
                    .iter()
                    .filter(|g| g.0 == m.org_id)
                    .map(|g| MeRoleSource {
                        kind: "group".into(),
                        group: Some(MeGroupRef {
                            id: g.1,
                            name: g.2.clone(),
                        }),
                    }),
            );
            let workspaces = ws
                .iter()
                .filter(|w| w.org_id == m.org_id)
                .map(|w| MeWorkspace {
                    id: w.id,
                    slug: w.slug.clone(),
                    name: w.name.clone(),
                    my_role: parse_ws_role(&w.role),
                    personal: w.personal,
                })
                .collect();
            MeMembership {
                org: OrgSummary {
                    id: m.org_id,
                    slug: m.slug,
                    name: m.name,
                },
                role: parse_org_role(&m.role),
                status: parse_status(&m.status),
                effective_role,
                sources,
                workspaces,
            }
        })
        .collect();
    let approval_email = account::approval_email_enabled(&state.pool, user_id).await?;
    let idp_roles = aurora_db::repo::idp_roles::get_state(&state.pool, user_id)
        .await?
        .map(Into::into);
    Ok(Me {
        groups,
        idp_roles,
        default_org_id: user.default_org_id,
        approval_email,
        client_address: None,
        user: user.into(),
        memberships,
    })
}

pub async fn patch_me(
    state: &AppState,
    user_id: Uuid,
    req: PatchMeRequest,
) -> Result<Me, ApiError> {
    let name = match req.display_name.as_deref().map(str::trim) {
        Some(n) if n.is_empty() || n.chars().count() > 80 => {
            return Err(ApiError::invalid_request(
                "Invalid display name.",
                vec![FieldError::new(
                    "/display_name",
                    "must be 1 to 80 characters",
                )],
            ));
        }
        other => other,
    };
    if let Some(org) = req.default_org_id
        && !account::is_active_member(&state.pool, user_id, org).await?
    {
        return Err(ApiError::not_found("Organization not found."));
    }
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    if let Some(n) = name {
        account::update_display_name(&mut *tx, user_id, n).await?;
    }
    if let Some(org) = req.default_org_id {
        users::set_default_org(&mut *tx, user_id, org).await?;
    }
    if let Some(on) = req.approval_email {
        account::set_approval_email(&mut *tx, user_id, on).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    load_me(state, user_id).await
}

fn reauth_required() -> ApiError {
    ApiError::new(
        StatusCode::PRECONDITION_FAILED,
        "reauth_required",
        "Re-authenticate with your sign-in provider to delete the account.",
    )
}

/// Delete the caller's account (phase 1, RFE-004): needs an OIDC re-authentication of this
/// session within [`REAUTH_WINDOW_SECS`] (412 `reauth_required` otherwise; the dashboard starts
/// `GET /v1/auth/oidc/{provider}/start?reauth=1`).
pub async fn delete_me(
    state: &AppState,
    user_id: Uuid,
    session_id: Uuid,
    ev: &crate::audit::AuditEvent,
) -> Result<(), ApiError> {
    users::find_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::unauthenticated("Authentication required."))?;
    let age = sessions::reauth_age_secs(&state.pool, session_id).await?;
    if !age.is_some_and(|a| (0.0..=REAUTH_WINDOW_SECS).contains(&a)) {
        return Err(reauth_required());
    }
    // (FIX-097, docs/04 §Deletion semantics) A personal workspace nobody else is bound to goes
    // with its person; seal each deletion's deprovision copies before the transaction.
    let personal_hint: Vec<Uuid> =
        account::sole_member_personal_workspaces(&state.pool, user_id, false)
            .await?
            .into_iter()
            .map(|(w, _, _)| w)
            .collect();
    let mut prepared: Vec<(Uuid, WorkspaceDeletion)> = Vec::new();
    for ws in personal_hint {
        match WorkspaceDeletion::prepare(state, ws).await {
            Ok(p) => prepared.push((ws, p)),
            Err(e) => {
                for (_, p) in &prepared {
                    p.rollback(state).await;
                }
                return Err(e);
            }
        }
    }
    let mut blobs = Vec::new();
    let mut done_ws: Vec<Uuid> = Vec::new();
    let done = async {
        let mut tx = state.pool.begin().await.map_err(DbError::from)?;
        let sole = account::sole_owner_orgs(&mut tx, user_id).await?;
        if !sole.is_empty() {
            return Err(ApiError::conflict(
                "last_owner",
                "You are the last owner of an organization. Transfer ownership or delete it first.",
            ));
        }
        let mut events = Vec::new();
        // Personal workspaces nobody shares: re-read under lock; ones that gained a member
        // since the hint stay (marked personal, never offered as shared).
        let sole_ws = account::sole_member_personal_workspaces(&mut *tx, user_id, true).await?;
        let skip: Vec<Uuid> = sole_ws.iter().map(|(w, _, _)| *w).collect();
        // Direct conversations first, through the conversation-deletion path: the session
        // purge and the content-free audit row that the SQL cascade would skip.
        for c in account::lock_direct_conversations(&mut tx, user_id, &skip).await? {
            let gone = aurora_db::repo::lifecycle::delete_conversation(&mut tx, c.id).await?;
            events.push(
                crate::audit::AuditEvent::new(
                    ev.actor.clone(),
                    "conversation.delete",
                    crate::audit::Outcome::Success,
                )
                .org(c.org_id)
                .workspace(c.workspace_id)
                .target("conversation", c.id)
                .metadata(crate::domains::conversations::lifecycle::delete_meta(
                    &[(c.id, &c.kind)],
                    c.agent_id,
                    &gone,
                    Some("account_deleted"),
                )),
            );
            blobs.extend(gone.blobs);
        }
        for (ws, org, slug) in &sole_ws {
            let Some((_, del)) = prepared.iter_mut().find(|(w, _)| w == ws) else {
                // Became sole after the hint: its deprovision copies were never sealed.
                return Err(ApiError::conflict(
                    "conflict",
                    "A workspace changed while the account was being deleted; retry.",
                ));
            };
            del.run(&mut tx, *org, *ws).await?;
            done_ws.push(*ws);
            events.push(
                crate::audit::AuditEvent::new(
                    ev.actor.clone(),
                    "workspace.delete",
                    crate::audit::Outcome::Success,
                )
                .org(*org)
                .workspace(*ws)
                .target("workspace", *ws)
                .metadata(serde_json::json!({"slug": slug, "via": "account_deletion"})),
            );
        }
        notify_auth_revoked(&mut *tx, "user", user_id).await?;
        account::delete_user(&mut tx, user_id).await?;
        events.push(ev.clone());
        crate::audit::commit_critical_many(tx, &events).await
    }
    .await;
    if let Err(e) = done {
        for (_, p) in &prepared {
            p.rollback(state).await;
        }
        return Err(e);
    }
    for (ws, p) in prepared {
        if done_ws.contains(&ws) {
            p.committed(state).await;
        } else {
            p.rollback(state).await;
        }
    }
    crate::domains::conversations::lifecycle::remove_blobs(&state.pool, &blobs).await;
    state
        .session_cache
        .invalidate(&serde_json::json!({"kind": "user", "id": user_id}));
    Ok(())
}
