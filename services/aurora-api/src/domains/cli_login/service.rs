use super::dto::*;
use crate::app::AppState;
use crate::http::ratelimit::Quota;
use aurora_core::token::{self, TokenPrefix};
use aurora_core::{ApiError, FieldError, sanitize};
use aurora_db::authz;
use aurora_db::repo::api_keys::NewKey;
use aurora_db::repo::cli_logins;
use aws_lc_rs::rand;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http::StatusCode;
use std::time::Duration;
use uuid::Uuid;

pub const INTERVAL_SECS: u32 = 5;
pub const EXPIRES_IN_SECS: u32 = 600;

const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn fill(buf: &mut [u8]) -> Result<(), ApiError> {
    rand::fill(buf).map_err(|_| ApiError::internal("Random generator unavailable."))
}

fn gen_user_code() -> Result<String, ApiError> {
    let mut b = [0u8; 8];
    fill(&mut b)?;
    let c: Vec<char> = b
        .iter()
        .map(|x| char::from(ALPHABET[(x & 31) as usize]))
        .collect();
    Ok(format!(
        "{}-{}",
        c[..4].iter().collect::<String>(),
        c[4..].iter().collect::<String>()
    ))
}

/// Canonical `ABCD-EFGH` or `None` if malformed.
pub fn normalize_user_code(s: &str) -> Option<String> {
    let raw: String = s
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if raw.len() != 8 || !raw.bytes().all(|b| ALPHABET.contains(&b)) {
        return None;
    }
    Some(format!("{}-{}", &raw[..4], &raw[4..]))
}

fn oauth_err(code: &'static str, msg: &'static str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, code, msg)
}

pub async fn start(s: &AppState, req: StartRequest) -> Result<StartResponse, ApiError> {
    let hostname = req
        .hostname
        .as_deref()
        .map(sanitize::label)
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown host".to_owned());
    let mut dc = [0u8; 32];
    fill(&mut dc)?;
    let device_code = URL_SAFE_NO_PAD.encode(dc);
    let hash = token::hash(&device_code);
    for _ in 0..5 {
        let user_code = gen_user_code()?;
        let id = Uuid::now_v7();
        match cli_logins::create(
            &s.pool,
            id,
            &hash,
            &user_code,
            &hostname,
            i64::from(EXPIRES_IN_SECS),
        )
        .await
        {
            Ok(()) => {
                return Ok(StartResponse {
                    verification_url: format!(
                        "{}/auth/cli?code={user_code}",
                        s.config.external_url.trim_end_matches('/')
                    ),
                    device_code,
                    user_code,
                    interval: INTERVAL_SECS,
                    expires_in: EXPIRES_IN_SECS,
                });
            }
            Err(e) if e.constraint() == Some(cli_logins::USER_CODE_CONSTRAINT) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::internal("Could not allocate a login code."))
}

/// Approve a pending login for a workspace the approver can use.
pub async fn approve(
    s: &AppState,
    c: &crate::http::auth::Caller,
    req: ApproveRequest,
) -> Result<(Uuid, Uuid), ApiError> {
    s.rate_limiter.check_quota(
        "cli_approve",
        &c.user_id.to_string(),
        Quota {
            capacity: 10,
            per: Duration::from_secs(60),
        },
    )?;
    let nf = || ApiError::not_found("Login code not found or expired.");
    let code = normalize_user_code(&req.user_code).ok_or_else(|| {
        ApiError::invalid_request(
            "Invalid code.",
            vec![FieldError::new("/user_code", "expected format ABCD-EFGH")],
        )
    })?;
    let a = c.workspace(s, req.workspace_id).await?;
    let login = cli_logins::find_pending_by_user_code(&s.pool, &code)
        .await?
        .ok_or_else(nf)?;
    if !cli_logins::approve(&s.pool, login.id, c.user_id, req.workspace_id).await? {
        return Err(nf());
    }
    Ok((login.id, a.org_id))
}

pub struct Consumed {
    pub response: TokenResponse,
    pub key_id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub workspace_id: Uuid,
    pub org_id: Uuid,
}

/// Poll for the token (RFC 8628 style errors, HTTP 400).
pub async fn token(s: &AppState, device_code: &str) -> Result<Consumed, ApiError> {
    let hash = token::hash(device_code);
    let Some(login) = cli_logins::find_by_device_hash(&s.pool, &hash).await? else {
        return Err(oauth_err("invalid_token", "Unknown device code."));
    };
    match login.status.as_str() {
        "consumed" => return Err(oauth_err("invalid_token", "Device code already used.")),
        "denied" => return Err(oauth_err("access_denied", "The login was denied.")),
        "expired" => return Err(oauth_err("expired_token", "The login has expired.")),
        _ => {}
    }
    if login.expired {
        return Err(oauth_err("expired_token", "The login has expired."));
    }
    if login.status == "pending" {
        let key: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        // Slightly under `interval` to tolerate client jitter.
        if s.rate_limiter
            .check_quota(
                "cli_poll",
                &key,
                Quota {
                    capacity: 1,
                    per: Duration::from_secs(u64::from(INTERVAL_SECS) - 1),
                },
            )
            .is_err()
        {
            return Err(oauth_err("slow_down", "Polling too fast; slow down."));
        }
        return Err(oauth_err(
            "authorization_pending",
            "Waiting for the login to be approved.",
        ));
    }
    // approved
    let (Some(user), Some(ws)) = (login.approved_by, login.workspace_id) else {
        return Err(oauth_err("invalid_token", "Invalid login state."));
    };
    // The approver must still be able to use the workspace.
    let Some(access) = authz::workspace_access(&s.pool, user, ws).await? else {
        cli_logins::set_status(&s.pool, login.id, "approved", "denied").await?;
        return Err(oauth_err("access_denied", "The login was denied."));
    };
    let summary = cli_logins::workspace_summary(&s.pool, ws)
        .await?
        .ok_or_else(|| oauth_err("invalid_token", "Unknown device code."))?;
    let tok = token::generate(TokenPrefix::WorkspaceKey);
    let tok = tok.expose();
    let name = format!(
        "CLI on {}",
        login.hostname.as_deref().unwrap_or("unknown host")
    );
    let Some(row) = cli_logins::consume(
        &s.pool,
        login.id,
        &NewKey {
            id: Uuid::now_v7(),
            org_id: access.org_id,
            workspace_id: Some(ws),
            user_id: user,
            name: &name,
            token_hash: &token::hash(tok),
            display_prefix: &token::display_prefix(tok),
            expires_at: None,
        },
    )
    .await?
    else {
        return Err(oauth_err("invalid_token", "Device code already used."));
    };
    let email = cli_logins::user_email(&s.pool, user)
        .await?
        .unwrap_or_default();
    Ok(Consumed {
        response: TokenResponse {
            api_key: tok.clone(),
            workspace: CliWorkspace {
                id: summary.0,
                slug: summary.1,
                name: summary.2,
                org_id: summary.3,
                org_slug: summary.4,
            },
        },
        key_id: row.id,
        user_id: user,
        email,
        workspace_id: ws,
        org_id: access.org_id,
    })
}
