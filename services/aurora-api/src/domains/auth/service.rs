//! Authentication domain logic (docs/04 §Authentication, §Sessions).
//!
//! Phase 1 (RFE-004, D-46): there is no local credential. Sign-in is OIDC (`domains::oidc`); this
//! module keeps the session helpers shared by it and the sign-out / revoke-others routes.

use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, ids};
use aurora_db::repo::sessions::{self, NewSession};
use aurora_db::repo::users::UserRow;
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::PgConnection;
use std::net::IpAddr;
use uuid::Uuid;

/// Request origin facts.
#[derive(Debug, Clone, Default)]
pub struct Client {
    pub ip: Option<IpAddr>,
    pub user_agent: Option<String>,
}

impl Client {
    pub fn ip_key(&self) -> String {
        self.ip
            .map_or_else(|| "unknown".to_owned(), |i| i.to_string())
    }
}

pub fn normalize_email(raw: &str) -> String {
    raw.trim().to_lowercase()
}

pub(crate) fn is_listed_admin(state: &AppState, email: &str) -> bool {
    state
        .config
        .auth
        .instance_admins
        .iter()
        .any(|a| a.trim().eq_ignore_ascii_case(email))
}

pub(crate) async fn issue_session(
    conn: &mut PgConnection,
    state: &AppState,
    user_id: Uuid,
    client: &Client,
) -> Result<String, ApiError> {
    let tok = token::generate(TokenPrefix::Session).expose().clone();
    let secs = i64::try_from(state.config.auth.session_absolute_secs).unwrap_or(i64::MAX / 4_000);
    sessions::create(
        conn,
        &NewSession {
            id: ids::new_id(),
            user_id,
            token_hash: token::hash(&tok),
            ip: client.ip,
            user_agent: client.user_agent.as_deref(),
            expires_at: Utc::now() + Duration::seconds(secs),
        },
    )
    .await?;
    Ok(tok)
}

pub(crate) fn actor(user: &UserRow, client: &Client) -> AuditActor {
    AuditActor {
        kind: "user",
        id: Some(user.id),
        label: user.email.clone(),
        source: "dashboard",
        ip: client.ip,
    }
}

/// Revoke one session and drop it from the local auth cache.
pub async fn logout(state: &AppState, session_id: Uuid) -> Result<(), ApiError> {
    sessions::revoke(&state.pool, session_id).await?;
    state
        .session_cache
        .invalidate(&json!({"kind": "session", "id": session_id}));
    Ok(())
}

pub async fn revoke_others(
    state: &AppState,
    client: &Client,
    user_id: Uuid,
    current: Uuid,
    email: &str,
) -> Result<u64, ApiError> {
    let n = sessions::revoke_all_for_user_except(&state.pool, user_id, Some(current)).await?;
    state
        .session_cache
        .invalidate(&json!({"kind": "user", "id": user_id}));
    audit::record(
        state,
        AuditEvent::new(
            AuditActor {
                kind: "user",
                id: Some(user_id),
                label: email.to_owned(),
                source: "dashboard",
                ip: client.ip,
            },
            "session.revoke_others",
            Outcome::Success,
        )
        .metadata(json!({"revoked": n})),
    );
    Ok(n)
}
