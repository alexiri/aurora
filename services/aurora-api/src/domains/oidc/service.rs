//! OIDC flow: `start` builds the provider redirect, `callback` completes sign-in.
use crate::app::AppState;
use crate::audit::{self, AuditEvent, Outcome};
use crate::domains::auth::service::{self as auth, Client, normalize_email};
use crate::domains::tenancy;
use aurora_config::{OidcProvider, SignupMode};
use aurora_core::{ApiError, FieldError, ids};
use aurora_crypto::Purpose;
use aurora_db::DbError;
use aurora_db::repo::identities::{self, NewSsoUser};
use aurora_db::repo::users::{self, UserRow};
use aurora_db::repo::{members, orgs, workspaces};
use chrono::Utc;
use http::StatusCode;
use openidconnect::core::{CoreAuthenticationFlow, CoreClient};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, Nonce, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Cookie binding the flow to the browser.
pub const OIDC_COOKIE: &str = "__Host-aurora_oidc";

/// Sealed into `state` (and the cookie).
#[derive(Serialize, Deserialize)]
struct FlowState {
    /// Provider id.
    p: String,
    /// Nonce.
    n: String,
    /// PKCE verifier.
    v: String,
    /// Same-origin return path.
    r: String,
    /// Expiry (unix seconds).
    e: i64,
}

fn aad(provider: &str) -> Vec<u8> {
    format!("oidc:{provider}").into_bytes()
}

pub fn unknown_provider() -> ApiError {
    ApiError::not_found("Unknown sign-in provider.")
}

pub fn provider<'a>(state: &'a AppState, id: &str) -> Result<&'a OidcProvider, ApiError> {
    state
        .config
        .auth
        .oidc_providers
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(unknown_provider)
}

/// `return_to` must be a same-origin absolute path.
pub fn validate_return_to(raw: Option<&str>) -> Result<String, ApiError> {
    let Some(r) = raw.filter(|r| !r.is_empty()) else {
        return Ok("/".to_owned());
    };
    let ok = r.starts_with('/')
        && !r.starts_with("//")
        && r.len() <= 2048
        && !r.chars().any(|c| c == '\\' || c.is_control());
    if ok {
        Ok(r.to_owned())
    } else {
        Err(ApiError::invalid_request(
            "Invalid request.",
            vec![FieldError::new(
                "/return_to",
                "must be a same-origin path starting with /",
            )],
        ))
    }
}

fn internal(msg: impl std::fmt::Display) -> ApiError {
    tracing::error!("oidc: {msg}");
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "oidc_failed",
        "The sign-in provider is unavailable.",
    )
}

fn redirect_url(state: &AppState, id: &str) -> Result<RedirectUrl, String> {
    RedirectUrl::new(format!(
        "{}/v1/auth/oidc/{id}/callback",
        state.config.external_url.trim_end_matches('/')
    ))
    .map_err(|e| e.to_string())
}

type Cc = CoreClient<
    openidconnect::EndpointSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointMaybeSet,
    openidconnect::EndpointMaybeSet,
>;

async fn core_client(state: &AppState, p: &OidcProvider) -> Result<Cc, String> {
    let meta = state.oidc.metadata(p).await?;
    let secret = state
        .config
        .oidc_client_secrets
        .get(&p.id)
        .map(|s| ClientSecret::new(s.expose().clone()));
    Ok(
        CoreClient::from_provider_metadata(meta, ClientId::new(p.client_id.clone()), secret)
            .set_redirect_uri(redirect_url(state, &p.id)?),
    )
}

pub struct Started {
    /// Provider authorization URL.
    pub location: String,
    /// Sealed state for the `__Host-aurora_oidc` cookie.
    pub cookie: String,
}

pub async fn start(
    state: &AppState,
    provider_id: &str,
    return_to: Option<&str>,
) -> Result<Started, ApiError> {
    let p = provider(state, provider_id)?;
    let return_to = validate_return_to(return_to)?;
    let client = core_client(state, p).await.map_err(internal)?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let nonce = Nonce::new_random();
    let ttl = i64::try_from(aurora_core::limits::OAUTH_STATE_TTL.as_secs()).unwrap_or(600);
    let flow = FlowState {
        p: p.id.clone(),
        n: nonce.secret().clone(),
        v: verifier.secret().clone(),
        r: return_to,
        e: Utc::now().timestamp() + ttl,
    };
    let sealed = state
        .derived
        .seal(
            Purpose::OidcState,
            &aad(&p.id),
            &serde_json::to_vec(&flow).map_err(internal)?,
        )
        .map_err(internal)?;
    let n = nonce.clone();
    let st = sealed.clone();
    let mut req = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            move || CsrfToken::new(st),
            move || n,
        )
        .set_pkce_challenge(challenge);
    for s in &p.scopes {
        if s != "openid" {
            req = req.add_scope(Scope::new(s.clone()));
        }
    }
    let (url, _, _) = req.url();
    Ok(Started {
        location: url.to_string(),
        cookie: sealed,
    })
}

/// Machine-readable failure for `/auth/login?error=<code>`.
#[derive(Debug)]
pub struct Fail(pub &'static str);

impl From<DbError> for Fail {
    fn from(e: DbError) -> Self {
        tracing::error!("oidc: database error: {e}");
        Fail("oidc_failed")
    }
}

pub struct Params {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

pub struct SignedIn {
    pub session_token: String,
    pub return_to: String,
}

fn open_state(
    state: &AppState,
    provider_id: &str,
    param: Option<&str>,
    cookie: Option<&str>,
) -> Result<FlowState, Fail> {
    const BAD: Fail = Fail("oidc_state_invalid");
    let (param, cookie) = param.zip(cookie).ok_or(BAD)?;
    let equal = param.len() == cookie.len()
        && param
            .bytes()
            .zip(cookie.bytes())
            .fold(0u8, |a, (x, y)| a | (x ^ y))
            == 0;
    if !equal {
        return Err(BAD);
    }
    let plain = state
        .derived
        .open(Purpose::OidcState, &aad(provider_id), param)
        .map_err(|_| BAD)?;
    let flow: FlowState = serde_json::from_slice(plain.expose()).map_err(|_| BAD)?;
    if flow.p != provider_id || flow.e < Utc::now().timestamp() {
        return Err(BAD);
    }
    Ok(flow)
}

struct Verified {
    sub: String,
    email: String,
    email_verified: bool,
    name: Option<String>,
}

async fn exchange(
    state: &AppState,
    p: &OidcProvider,
    flow: &FlowState,
    code: String,
) -> Result<Verified, Fail> {
    const FAILED: Fail = Fail("oidc_failed");
    let client = core_client(state, p).await.map_err(|e| {
        tracing::warn!("oidc: {e}");
        FAILED
    })?;
    let tokens = client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|e| {
            tracing::warn!("oidc: {e}");
            FAILED
        })?
        .set_pkce_verifier(PkceCodeVerifier::new(flow.v.clone()))
        .request_async(&state.oidc.http)
        .await
        .map_err(|e| {
            tracing::warn!("oidc: token exchange failed: {e}");
            FAILED
        })?;
    let id_token = tokens.id_token().ok_or(FAILED)?;
    let nonce = Nonce::new(flow.n.clone());
    let claims = match id_token.claims(&client.id_token_verifier(), &nonce) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("oidc: id token rejected: {e}");
            // Possibly rotated signing keys: refetch on the next attempt.
            state.oidc.forget(&p.id).await;
            return Err(FAILED);
        }
    };
    let email = claims.email().map(|e| normalize_email(e.as_str()));
    let email = email
        .filter(|e| e.contains('@') && e.len() <= 254 && !e.chars().any(char::is_whitespace))
        .ok_or(FAILED)?;
    Ok(Verified {
        sub: claims.subject().as_str().to_owned(),
        email,
        email_verified: claims.email_verified().unwrap_or(false),
        name: claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.as_str().to_owned()),
    })
}

fn domain_allowed(p: &OidcProvider, email: &str) -> bool {
    if p.allowed_email_domains.is_empty() {
        return true;
    }
    let domain = email.rsplit_once('@').map_or("", |(_, d)| d);
    p.allowed_email_domains.iter().any(|d| {
        d.trim()
            .trim_start_matches('@')
            .eq_ignore_ascii_case(domain)
    })
}

fn display_name(v: &Verified) -> String {
    let raw = v
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty() && !n.chars().any(char::is_control))
        .unwrap_or_else(|| v.email.split('@').next().unwrap_or("User"));
    raw.chars().take(80).collect()
}

pub async fn callback(
    state: &AppState,
    client: &Client,
    provider_id: &str,
    params: Params,
    cookie: Option<&str>,
) -> Result<SignedIn, Fail> {
    let p = provider(state, provider_id).map_err(|_| Fail("oidc_failed"))?;
    let flow = open_state(state, provider_id, params.state.as_deref(), cookie)?;
    if let Some(e) = &params.error {
        tracing::info!("oidc: provider returned error `{e}`");
        return Err(Fail("oidc_failed"));
    }
    let code = params
        .code
        .filter(|c| !c.is_empty())
        .ok_or(Fail("oidc_failed"))?;
    let v = exchange(state, p, &flow, code).await?;
    if p.require_email_verified && !v.email_verified {
        deny(state, client, &v.email, p, "email_unverified");
        return Err(Fail("email_unverified"));
    }
    if !domain_allowed(p, &v.email) {
        deny(state, client, &v.email, p, "email_domain_not_allowed");
        return Err(Fail("email_domain_not_allowed"));
    }
    let signed = sign_in(state, client, p, &v).await;
    match signed {
        Ok((session_token, user)) => {
            audit::record(
                state,
                AuditEvent::new(auth::actor(&user, client), "user.login", Outcome::Success)
                    .target("user", user.id)
                    .metadata(json!({"method": "oidc", "provider": p.id})),
            );
            Ok(SignedIn {
                session_token,
                return_to: flow.r,
            })
        }
        Err(Fail(code)) => {
            deny(state, client, &v.email, p, code);
            Err(Fail(code))
        }
    }
}

fn deny(state: &AppState, client: &Client, email: &str, p: &OidcProvider, reason: &str) {
    let actor = aurora_actor(client, email);
    audit::record(
        state,
        AuditEvent::new(actor, "user.login", Outcome::Denied)
            .metadata(json!({"method": "oidc", "provider": p.id, "reason": reason})),
    );
}

fn aurora_actor(client: &Client, email: &str) -> audit::AuditActor {
    audit::AuditActor {
        kind: "user",
        id: None,
        label: email.chars().take(254).collect(),
        source: "dashboard",
        ip: client.ip,
    }
}

async fn sign_in(
    state: &AppState,
    client: &Client,
    p: &OidcProvider,
    v: &Verified,
) -> Result<(String, UserRow), Fail> {
    let listed = v.email_verified && auth::is_listed_admin(state, &v.email);
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;

    let mut created = false;
    let mut user = match identities::find_user(&mut *tx, &p.id, &v.sub).await? {
        Some(u) => u,
        None => {
            let existing = users::find_by_email(&mut *tx, &v.email).await?;
            let user = match existing {
                // Link only on a provider-verified email.
                Some(u) if v.email_verified => {
                    if u.email_verified_at.is_none() {
                        users::mark_email_verified(&mut *tx, u.id).await?
                    } else {
                        u
                    }
                }
                Some(_) => return Err(Fail("email_unverified")),
                None => {
                    created = true;
                    create_user(state, &mut tx, p, v, listed).await?
                }
            };
            identities::insert(&mut *tx, user.id, &p.id, &v.sub, &v.email)
                .await
                .map_err(|e| {
                    tracing::warn!("oidc: identity link failed: {e}");
                    Fail("oidc_failed")
                })?;
            user
        }
    };
    if user.disabled_at.is_some() {
        return Err(Fail("oidc_failed"));
    }

    apply_jit(&mut tx, p, &mut user).await?;

    user = users::record_login(&mut *tx, user.id).await?;
    if listed && !user.is_instance_admin {
        user = users::grant_instance_admin(&mut *tx, user.id).await?;
    }
    let token = auth::issue_session(&mut tx, state, user.id, client)
        .await
        .map_err(|e| {
            tracing::error!("oidc: session: {e:?}");
            Fail("oidc_failed")
        })?;
    tx.commit().await.map_err(DbError::from)?;
    if created {
        audit::record(
            state,
            AuditEvent::new(auth::actor(&user, client), "user.signup", Outcome::Success)
                .target("user", user.id)
                .metadata(json!({"method": "oidc", "provider": p.id})),
        );
    }
    Ok((token, user))
}

async fn create_user(
    state: &AppState,
    tx: &mut sqlx::PgConnection,
    p: &OidcProvider,
    v: &Verified,
    listed: bool,
) -> Result<UserRow, Fail> {
    let jit_org = match &p.jit {
        Some(j) => orgs::get_by_slug(&mut *tx, &j.org_slug).await?,
        None => None,
    };
    let allowed = match state.config.signup_mode {
        SignupMode::Open => true,
        SignupMode::SsoJit => jit_org.is_some(),
        SignupMode::InviteOnly => listed,
    };
    if !allowed {
        return Err(Fail("signup_closed"));
    }
    let name = display_name(v);
    let user = identities::insert_user(
        &mut *tx,
        &NewSsoUser {
            id: ids::new_id(),
            email: &v.email,
            display_name: &name,
            email_verified: v.email_verified,
            is_instance_admin: listed,
        },
    )
    .await
    .map_err(|e| {
        tracing::warn!("oidc: user insert failed: {e}");
        Fail("oidc_failed")
    })?;
    if state.config.signup_mode == SignupMode::Open {
        let short: String = name.chars().take(74).collect();
        let org = tenancy::service::create_organization(
            tx,
            &state.config,
            user.id,
            &format!("{short}'s org"),
            None,
        )
        .await
        .map_err(|e| {
            tracing::error!("oidc: create org: {e:?}");
            Fail("oidc_failed")
        })?;
        users::set_default_org(&mut *tx, user.id, org.org.id).await?;
    }
    Ok(user)
}

/// Add org membership and workspace bindings from the provider's `jit` block.
async fn apply_jit(
    tx: &mut sqlx::PgConnection,
    p: &OidcProvider,
    user: &mut UserRow,
) -> Result<(), Fail> {
    let Some(j) = &p.jit else { return Ok(()) };
    let Some((org_id, _, _)) = orgs::get_by_slug(&mut *tx, &j.org_slug).await? else {
        tracing::warn!("oidc: jit org `{}` does not exist", j.org_slug);
        return Ok(());
    };
    if members::get(&mut *tx, org_id, user.id).await?.is_some() {
        return Ok(());
    }
    let role = if j.role == "admin" { "admin" } else { "member" };
    members::insert(&mut *tx, org_id, user.id, role).await?;
    for slug in &j.workspace_slugs {
        match workspaces::get_by_slug(&mut *tx, org_id, slug).await? {
            Some(ws) => workspaces::bind_user(&mut *tx, ws.id, user.id, "member").await?,
            None => tracing::warn!("oidc: jit workspace `{slug}` does not exist"),
        }
    }
    if user.default_org_id.is_none() {
        users::set_default_org(&mut *tx, user.id, org_id).await?;
        user.default_org_id = Some(org_id);
    }
    Ok(())
}
