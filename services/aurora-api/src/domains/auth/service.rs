//! Authentication domain logic (docs/04 §Authentication, §Sessions).

use super::mail::{self, Mail};
use super::password;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::tenancy;
use crate::http::ratelimit::Bucket;
use aurora_config::SignupMode;
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, FieldError, ids};
use aurora_db::repo::email_tokens::{self, RESET_PASSWORD, VERIFY_EMAIL};
use aurora_db::repo::sessions::{self, NewSession};
use aurora_db::repo::users::{self, NewUser, UserRow};
use aurora_db::{DbError, conflict_code};
use chrono::{Duration, Utc};
use http::StatusCode;
use serde_json::json;
use sqlx::PgConnection;
use std::net::IpAddr;
use uuid::Uuid;

pub const VERIFY_TTL_SECS: u64 = 24 * 3600;
pub const RESET_TTL_SECS: u64 = 3600;

/// Request origin facts.
#[derive(Debug, Clone, Default)]
pub struct Client {
    pub ip: Option<IpAddr>,
    pub user_agent: Option<String>,
}

impl Client {
    fn ip_key(&self) -> String {
        self.ip
            .map_or_else(|| "unknown".to_owned(), |i| i.to_string())
    }
}

/// Result of a successful sign-up/login: the user and the raw session token (for the cookie).
pub struct Authenticated {
    pub user: UserRow,
    pub session_token: String,
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid request.", vec![FieldError::new(path, msg)])
}

fn invalid_credentials() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_credentials",
        "Invalid email or password.",
    )
}

fn invalid_token() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_token",
        "The link is invalid or has expired.",
    )
}

pub fn normalize_email(raw: &str) -> String {
    raw.trim().to_lowercase()
}

fn validate_email(email: &str) -> Result<(), ApiError> {
    let ok = email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email.split_once('@').is_some_and(|(l, d)| {
            !l.is_empty()
                && !d.contains('@')
                && d.contains('.')
                && !d.starts_with('.')
                && !d.ends_with('.')
        });
    if ok {
        Ok(())
    } else {
        Err(bad("/email", "must be a valid email address"))
    }
}

pub(crate) fn is_listed_admin(state: &AppState, email: &str) -> bool {
    state
        .config
        .auth
        .instance_admins
        .iter()
        .any(|a| a.trim().eq_ignore_ascii_case(email))
}

fn links(state: &AppState, path: &str, tok: &str) -> String {
    format!(
        "{}/auth/{path}?token={tok}",
        state.config.external_url.trim_end_matches('/')
    )
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

/// Create a single-use email token (superseding unused ones) and return the raw token.
async fn issue_email_token(
    conn: &mut PgConnection,
    user_id: Uuid,
    purpose: &str,
    ttl: u64,
) -> Result<String, ApiError> {
    email_tokens::invalidate_unused(&mut *conn, user_id, purpose).await?;
    let tok = token::generate(TokenPrefix::Invitation).expose().clone();
    email_tokens::create(
        conn,
        ids::new_id(),
        user_id,
        purpose,
        &token::hash(&tok),
        ttl,
    )
    .await?;
    Ok(tok)
}

fn send_verification(state: &AppState, email: &str, tok: &str) {
    mail::send_detached(
        &state.mailer,
        Mail::new(
            email,
            "Verify your email address",
            "Confirm your email address for Aurora. The link is valid for 24 hours.",
            &links(state, "verify", tok),
        ),
    );
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

pub struct SignupInput {
    pub email: String,
    pub password: String,
    pub display_name: String,
    pub invitation_token: Option<String>,
}

pub async fn signup(
    state: &AppState,
    client: &Client,
    req: SignupInput,
) -> Result<Authenticated, ApiError> {
    state
        .rate_limiter
        .check(Bucket::SignUpIp, &client.ip_key())?;
    let email = normalize_email(&req.email);
    let name = req.display_name.trim().to_owned();
    let mut fields = Vec::new();
    if validate_email(&email).is_err() {
        fields.push(FieldError::new("/email", "must be a valid email address"));
    }
    let n = name.chars().count();
    if n == 0 || n > 80 || name.chars().any(char::is_control) {
        fields.push(FieldError::new(
            "/display_name",
            "must be 1 to 80 characters",
        ));
    }
    if let Err(m) = password::check(&req.password) {
        fields.push(FieldError::new("/password", m));
    }
    // Password first so tests/clients see `/password` as fields[0] when it is the culprit.
    fields.sort_by_key(|f| i32::from(f.path != "/password"));
    if !fields.is_empty() {
        return Err(ApiError::invalid_request("Invalid request.", fields));
    }

    let listed = is_listed_admin(state, &email);
    let invited = match req.invitation_token.as_deref() {
        Some(t) if t.starts_with(TokenPrefix::Invitation.as_str()) => {
            email_tokens::pending_invitation_matches(&state.pool, &token::hash(t), &email).await?
        }
        _ => false,
    };
    let open = match state.config.signup_mode {
        SignupMode::Open => true,
        SignupMode::InviteOnly => false,
        SignupMode::SsoJit => {
            return Err(signup_closed());
        }
    };
    if !open && !invited && !listed {
        return Err(signup_closed());
    }

    let hash = password::hash(req.password).await?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let user = users::insert(
        &mut *tx,
        &NewUser {
            id: ids::new_id(),
            email: &email,
            display_name: &name,
            password_hash: &hash,
            email_verified: invited,
            is_instance_admin: invited && listed,
        },
    )
    .await
    .map_err(|e| conflict_code(e, &[("users_email_key", "email_taken")]))?;
    let mut org_id = None;
    if open {
        let short: String = name.chars().take(74).collect();
        let created = tenancy::service::create_organization(
            &mut tx,
            &state.config,
            user.id,
            &format!("{short}'s org"),
            None,
        )
        .await?;
        users::set_default_org(&mut *tx, user.id, created.org.id).await?;
        org_id = Some(created.org.id);
    }
    let session_token = issue_session(&mut tx, state, user.id, client).await?;
    let verify = if user.email_verified_at.is_none() {
        Some(issue_email_token(&mut tx, user.id, VERIFY_EMAIL, VERIFY_TTL_SECS).await?)
    } else {
        None
    };
    tx.commit().await.map_err(DbError::from)?;
    if let Some(t) = verify {
        send_verification(state, &user.email, &t);
    }
    let mut ev = AuditEvent::new(actor(&user, client), "user.signup", Outcome::Success)
        .target("user", user.id)
        .metadata(json!({"invited": invited}));
    if let Some(o) = org_id {
        ev = ev.org(o);
    }
    audit::record(state, ev);
    Ok(Authenticated {
        user,
        session_token,
    })
}

fn signup_closed() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "signup_closed",
        "Sign-up is closed on this instance.",
    )
}

pub async fn login(
    state: &AppState,
    client: &Client,
    email: &str,
    password_in: String,
) -> Result<Authenticated, ApiError> {
    let email = normalize_email(email);
    state
        .rate_limiter
        .check(Bucket::SignInIp, &client.ip_key())?;
    let key: String = email.chars().take(254).collect();
    state.rate_limiter.check(Bucket::SignInEmail, &key)?;

    let found = users::find_by_email(&state.pool, &email).await?;
    let phc = found.as_ref().and_then(|u| u.password_hash.clone());
    let ok = password::verify(password_in, phc).await;
    let user = match found {
        Some(u) if ok => u,
        other => {
            let a = AuditActor {
                kind: "user",
                id: other.as_ref().map(|u| u.id),
                label: key,
                source: "dashboard",
                ip: client.ip,
            };
            audit::record(
                state,
                AuditEvent::new(a, "user.login", Outcome::Failure)
                    .metadata(json!({"reason": "invalid_credentials"})),
            );
            return Err(invalid_credentials());
        }
    };
    if user.disabled_at.is_some() {
        audit::record(
            state,
            AuditEvent::new(actor(&user, client), "user.login", Outcome::Denied)
                .metadata(json!({"reason": "account_disabled"})),
        );
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "account_disabled",
            "This account is disabled.",
        ));
    }
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let mut user = users::record_login(&mut *tx, user.id).await?;
    if !user.is_instance_admin
        && user.email_verified_at.is_some()
        && is_listed_admin(state, &user.email)
    {
        user = users::grant_instance_admin(&mut *tx, user.id).await?;
    }
    let session_token = issue_session(&mut tx, state, user.id, client).await?;
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        state,
        AuditEvent::new(actor(&user, client), "user.login", Outcome::Success)
            .target("user", user.id),
    );
    Ok(Authenticated {
        user,
        session_token,
    })
}

/// Revoke one session and drop it from the local auth cache.
pub async fn logout(state: &AppState, session_id: Uuid) -> Result<(), ApiError> {
    sessions::revoke(&state.pool, session_id).await?;
    state
        .session_cache
        .invalidate(&json!({"kind": "session", "id": session_id}));
    Ok(())
}

pub async fn verify_email(
    state: &AppState,
    client: &Client,
    tok: &str,
) -> Result<UserRow, ApiError> {
    if !tok.starts_with(TokenPrefix::Invitation.as_str()) {
        return Err(invalid_token());
    }
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let uid = email_tokens::consume(&mut *tx, &token::hash(tok), VERIFY_EMAIL)
        .await?
        .ok_or_else(invalid_token)?;
    let user = users::mark_email_verified(&mut *tx, uid).await?;
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        state,
        AuditEvent::new(actor(&user, client), "user.verify_email", Outcome::Success)
            .target("user", user.id),
    );
    Ok(user)
}

pub async fn resend_verification(state: &AppState, user_id: Uuid) -> Result<(), ApiError> {
    let user = users::find_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::unauthenticated("Authentication required."))?;
    if user.email_verified_at.is_some() {
        return Err(ApiError::conflict(
            "already_verified",
            "Email is already verified.",
        ));
    }
    state
        .rate_limiter
        .check(Bucket::ResetEmail, &format!("verify:{}", user.email))?;
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    let t = issue_email_token(&mut conn, user.id, VERIFY_EMAIL, VERIFY_TTL_SECS).await?;
    send_verification(state, &user.email, &t);
    Ok(())
}

/// Always succeeds from the caller's view (no account enumeration); only rate limits surface.
pub async fn forgot_password(
    state: &AppState,
    client: &Client,
    email: &str,
) -> Result<(), ApiError> {
    let email = normalize_email(email);
    let key: String = email.chars().take(254).collect();
    state.rate_limiter.check(Bucket::ResetEmail, &key)?;
    let Some(user) = users::find_by_email(&state.pool, &email).await? else {
        return Ok(());
    };
    if user.disabled_at.is_some() {
        return Ok(());
    }
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    let t = issue_email_token(&mut conn, user.id, RESET_PASSWORD, RESET_TTL_SECS).await?;
    mail::send_detached(
        &state.mailer,
        Mail::new(
            &user.email,
            "Reset your password",
            "Use this link to choose a new password. It is valid for 1 hour. If you did not ask for this, ignore this email.",
            &links(state, "reset", &t),
        ),
    );
    let _ = client;
    Ok(())
}

pub async fn reset_password(
    state: &AppState,
    client: &Client,
    tok: &str,
    new_password: String,
) -> Result<(), ApiError> {
    password::validate("/password", &new_password)?;
    if !tok.starts_with(TokenPrefix::Invitation.as_str()) {
        return Err(invalid_token());
    }
    let hash = password::hash(new_password).await?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let uid = email_tokens::consume(&mut *tx, &token::hash(tok), RESET_PASSWORD)
        .await?
        .ok_or_else(invalid_token)?;
    users::set_password_hash(&mut *tx, uid, &hash).await?;
    // The link proves control of the inbox.
    let user = users::mark_email_verified(&mut *tx, uid).await?;
    tx.commit().await.map_err(DbError::from)?;
    sessions::revoke_all_for_user_except(&state.pool, uid, None).await?;
    state
        .session_cache
        .invalidate(&json!({"kind": "user", "id": uid}));
    audit::record(
        state,
        AuditEvent::new(
            actor(&user, client),
            "user.password_reset",
            Outcome::Success,
        )
        .target("user", uid),
    );
    Ok(())
}

pub async fn change_password(
    state: &AppState,
    client: &Client,
    user_id: Uuid,
    current: String,
    new: String,
) -> Result<(), ApiError> {
    password::validate("/new_password", &new)?;
    let user = users::find_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(invalid_credentials)?;
    if !password::verify(current, user.password_hash.clone()).await {
        return Err(invalid_credentials());
    }
    let hash = password::hash(new).await?;
    users::set_password_hash(&state.pool, user_id, &hash).await?;
    audit::record(
        state,
        AuditEvent::new(
            actor(&user, client),
            "user.password_change",
            Outcome::Success,
        )
        .target("user", user_id),
    );
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
