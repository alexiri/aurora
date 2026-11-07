use crate::app::AppState;
use crate::domains::auth::service::{RESET_TTL_SECS, normalize_email};
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, FieldError, ids};
use aurora_db::repo::account::{self, AdminUserRow};
use aurora_db::repo::email_tokens::{self, RESET_PASSWORD};
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
) -> Result<AdminUserRow, ApiError> {
    if actor == id {
        return Err(ApiError::unprocessable(
            "cannot_modify_self",
            "Administrators cannot modify their own account here.",
        ));
    }
    let row = account::update_admin_flags(&state.pool, id, is_instance_admin, disabled)
        .await?
        .ok_or_else(|| ApiError::not_found("User not found."))?;
    if disabled == Some(true) {
        sessions::revoke_all_for_user_except(&state.pool, id, None).await?;
        state
            .session_cache
            .invalidate(&json!({"kind": "user", "id": id}));
    }
    Ok(row)
}

/// Create (or promote) a verified, password-less instance admin and return the one-time
/// password-set link (reset token, 1 h).
pub async fn bootstrap_admin(
    pool: &PgPool,
    external_url: &str,
    email: &str,
) -> Result<String, ApiError> {
    let email = normalize_email(email);
    if !email.contains('@') || email.len() > 254 {
        return Err(ApiError::invalid_request(
            "Invalid email.",
            vec![FieldError::new("/email", "must be a valid email address")],
        ));
    }
    let name = email.split('@').next().unwrap_or("admin");
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let uid = account::upsert_bootstrap_admin(&mut *tx, ids::new_id(), &email, name).await?;
    email_tokens::invalidate_unused(&mut *tx, uid, RESET_PASSWORD).await?;
    let tok = token::generate(TokenPrefix::Invitation).expose().clone();
    email_tokens::create(
        &mut *tx,
        ids::new_id(),
        uid,
        RESET_PASSWORD,
        &token::hash(&tok),
        RESET_TTL_SECS,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(format!(
        "{}/auth/reset?token={tok}",
        external_url.trim_end_matches('/')
    ))
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
