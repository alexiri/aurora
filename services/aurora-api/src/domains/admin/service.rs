use crate::app::AppState;
use crate::domains::auth::service::normalize_email;
use aurora_core::{ApiError, FieldError, ids};
use aurora_db::repo::account::{self, AdminUserRow};
use aurora_db::repo::sessions;
use aurora_db::{DbError, PgPool};
use serde_json::json;
use uuid::Uuid;

pub async fn patch_user(
    state: &AppState,
    actor: Uuid,
    id: Uuid,
    is_instance_admin: Option<bool>,
    disabled: Option<bool>,
    ev: &crate::audit::AuditEvent,
) -> Result<AdminUserRow, ApiError> {
    if actor == id {
        return Err(ApiError::unprocessable(
            "cannot_modify_self",
            "Administrators cannot modify their own account here.",
        ));
    }
    // RFE-008: `admin.user_update` (instance-admin grant/revoke, disable/enable) commits with its
    // audit row.
    let mut tx = crate::audit::begin(state).await?;
    let row = account::update_admin_flags(&mut *tx, id, is_instance_admin, disabled)
        .await?
        .ok_or_else(|| ApiError::not_found("User not found."))?;
    if disabled == Some(true) {
        sessions::revoke_all_for_user_except_tx(&mut tx, id, None).await?;
    }
    crate::audit::commit_critical(tx, ev).await?;
    if disabled == Some(true) {
        state
            .session_cache
            .invalidate(&json!({"kind": "user", "id": id}));
    }
    Ok(row)
}

/// Pre-provision an instance admin (phase 1, RFE-004): create (or promote) the user with this
/// e-mail — `email_verified_at` set, `is_instance_admin = true`, no password, no identity — and
/// return the sign-in URL. The person's first OIDC sign-in with that verified e-mail binds the
/// identity (`docs/04 §OIDC` step 3). No secret exists; nothing but the IdP can sign in.
///
/// Refuses (`no_oidc_provider`) when no OIDC provider is configured.
pub async fn bootstrap_admin(
    pool: &PgPool,
    external_url: &str,
    providers_configured: bool,
    email: &str,
) -> Result<String, ApiError> {
    if !providers_configured {
        return Err(ApiError::unprocessable(
            "no_oidc_provider",
            "No OIDC provider is configured (auth.oidc_providers or auth.dev_idp): nobody could sign in.",
        ));
    }
    let email = normalize_email(email);
    if !email.contains('@') || email.len() > 254 || email.chars().any(char::is_whitespace) {
        return Err(ApiError::invalid_request(
            "Invalid email.",
            vec![FieldError::new("/email", "must be a valid email address")],
        ));
    }
    let name = email.split('@').next().unwrap_or("admin");
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    account::upsert_bootstrap_admin(&mut *tx, ids::new_id(), &email, name).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(format!("{}/auth/login", external_url.trim_end_matches('/')))
}

/// Clear the instance-admin flag; returns whether the user existed.
pub async fn revoke_admin(pool: &PgPool, email: &str) -> Result<bool, ApiError> {
    let id = account::revoke_instance_admin_by_email(pool, &normalize_email(email)).await?;
    if let Some(id) = id {
        // Make running API processes drop cached `is_instance_admin`.
        aurora_db::repo::notify_auth_revoked(pool, "user", id).await?;
    }
    Ok(id.is_some())
}
