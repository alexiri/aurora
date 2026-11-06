use super::dto::*;
use crate::app::AppState;
use crate::domains::auth::password;
use crate::domains::tenancy::dto::{OrgSummary, parse_org_role, parse_status, parse_ws_role};
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::repo::{account, notify_auth_revoked, users};
use http::StatusCode;
use uuid::Uuid;

/// Max age of the session for password-less (SSO-only) accounts to delete themselves.
/// The `sessions` table does not record the sign-in method, so we require a recent sign-in
/// (session created within this window) instead of "created by OIDC".
pub const REAUTH_WINDOW_SECS: f64 = 600.0;

pub async fn load_me(state: &AppState, user_id: Uuid) -> Result<Me, ApiError> {
    let user = users::find_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::unauthenticated("Authentication required."))?;
    let rows = account::memberships(&state.pool, user_id).await?;
    let ws = account::usable_workspaces(&state.pool, user_id).await?;
    let memberships = rows
        .into_iter()
        .map(|m| {
            let workspaces = ws
                .iter()
                .filter(|w| w.org_id == m.org_id)
                .map(|w| MeWorkspace {
                    id: w.id,
                    slug: w.slug.clone(),
                    name: w.name.clone(),
                    my_role: parse_ws_role(&w.role),
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
                workspaces,
            }
        })
        .collect();
    Ok(Me {
        default_org_id: user.default_org_id,
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
    tx.commit().await.map_err(DbError::from)?;
    load_me(state, user_id).await
}

fn reauth() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "reauthentication_required",
        "Re-authentication required to delete the account.",
    )
}

/// Delete the caller's account. Password users must supply the password; password-less
/// users need a session created within [`REAUTH_WINDOW_SECS`].
pub async fn delete_me(
    state: &AppState,
    user_id: Uuid,
    session_id: Uuid,
    password_in: Option<String>,
) -> Result<(), ApiError> {
    let user = users::find_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::unauthenticated("Authentication required."))?;
    if let Some(hash) = user.password_hash {
        let Some(p) = password_in else {
            return Err(reauth());
        };
        if !password::verify(p, Some(hash)).await {
            return Err(reauth());
        }
    } else {
        let age = account::session_age_secs(&state.pool, session_id).await?;
        if !age.is_some_and(|a| a <= REAUTH_WINDOW_SECS) {
            return Err(reauth());
        }
    }
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let sole = account::sole_owner_orgs(&mut tx, user_id).await?;
    if !sole.is_empty() {
        return Err(ApiError::conflict(
            "last_owner",
            "You are the last owner of an organization. Transfer ownership or delete it first.",
        ));
    }
    notify_auth_revoked(&mut *tx, "user", user_id).await?;
    account::delete_user(&mut tx, user_id).await?;
    tx.commit().await.map_err(DbError::from)?;
    state
        .session_cache
        .invalidate(&serde_json::json!({"kind": "user", "id": user_id}));
    Ok(())
}
