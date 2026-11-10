//! OIDC flow: `start` builds the provider redirect, `callback` completes sign-in.
use super::roles;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::auth::service::{self as auth, Client, normalize_email};
use crate::domains::{invitations, tenancy};
use crate::http::ratelimit::Bucket;
use aurora_config::{OidcProvider, SignupMode};
use aurora_core::{ApiError, FieldError, ids};
use aurora_crypto::Purpose;
use aurora_db::DbError;
use aurora_db::repo::identities::{self, NewSsoUser};
use aurora_db::repo::users::{self, UserRow};
use aurora_db::repo::{invitations as inv_repo, members, orgs, sessions, workspaces};
use chrono::Utc;
use http::StatusCode;
use openidconnect::core::{CoreAuthenticationFlow, CoreClient};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, LoginHint, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

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
    /// Re-authentication (phase 1, RFE-004): the signed-in `(session, user)` this flow is for.
    #[serde(default)]
    ra: Option<(Uuid, Uuid)>,
}

/// A re-authentication callback must carry an `auth_time` at most this old (docs/04 §OIDC).
const REAUTH_AUTH_TIME_MAX_AGE_SECS: i64 = 60;

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

/// What `GET /v1/auth/oidc/{provider}/start` was asked for.
#[derive(Default)]
pub struct StartRequest<'a> {
    pub return_to: Option<&'a str>,
    /// `reauth=1`: the caller's live session (must be present, else 401).
    pub reauth: bool,
    /// Forwarded to the provider as `login_hint` (the development IdP signs that user in).
    pub login_hint: Option<&'a str>,
}

/// The caller's live session for a re-authentication, from the `aurora_session` cookie.
async fn current_session(
    state: &AppState,
    session_token: Option<&str>,
) -> Result<(Uuid, Uuid), ApiError> {
    let unauth = || ApiError::unauthenticated("Sign in before re-authenticating.");
    let tok = session_token
        .filter(|t| t.starts_with(aurora_core::token::TokenPrefix::Session.as_str()))
        .ok_or_else(unauth)?;
    let rec = sessions::find_by_hash(
        &state.pool,
        &aurora_core::token::hash(tok),
        state.config.auth.session_idle_secs,
    )
    .await?
    .filter(|r| r.live)
    .ok_or_else(unauth)?;
    Ok((rec.id, rec.user_id))
}

pub async fn start(
    state: &AppState,
    client: &Client,
    provider_id: &str,
    req: StartRequest<'_>,
    session_token: Option<&str>,
) -> Result<Started, ApiError> {
    crate::http::counters::check_security_bucket(state, Bucket::OidcStartIp, &client.ip_key())
        .await?;
    let p = provider(state, provider_id)?;
    let return_to = validate_return_to(req.return_to)?;
    let ra = if req.reauth {
        Some(current_session(state, session_token).await?)
    } else {
        None
    };
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
        ra,
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
    let mut authz = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            move || CsrfToken::new(st),
            move || n,
        )
        .set_pkce_challenge(challenge);
    for s in &p.scopes {
        if s != "openid" {
            authz = authz.add_scope(Scope::new(s.clone()));
        }
    }
    if req.reauth {
        // Aurora never assumes an `acr`/`amr`: it forces a fresh login and checks `auth_time`.
        authz = authz
            .add_prompt(openidconnect::core::CoreAuthPrompt::Login)
            .set_max_age(std::time::Duration::ZERO);
    }
    if let Some(h) = req.login_hint.filter(|h| !h.is_empty() && h.len() <= 254) {
        authz = authz.set_login_hint(LoginHint::new(h.to_owned()));
    }
    let (url, _, _) = authz.url();
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
    /// `None` for a re-authentication (the existing session is stamped, no new one is issued).
    pub session_token: Option<String>,
    pub return_to: String,
    pub reauth: bool,
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
    /// The IdP's `auth_time` (unix seconds), when it returned one.
    auth_time: Option<i64>,
    /// (P4-IDP-1) Role values read from the validated token (or userinfo); empty when the provider
    /// has no `role_claim` or the claim is absent.
    role_values: Vec<String>,
}

/// The claims object of the **already verified** ID token (its payload segment).
fn verified_payload(id_token: &impl Serialize) -> Option<serde_json::Value> {
    use base64::Engine as _;
    let jwt = serde_json::to_value(id_token).ok()?;
    let seg = jwt.as_str()?.split('.').nth(1)?.to_owned();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(seg.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
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
    // D-118: a provider without the claim may be declared authoritative for some domains.
    let email_verified =
        claims.email_verified() == Some(true) || domain_in(&p.trusted_email_domains, &email);
    let sub = claims.subject().as_str().to_owned();
    // SEC-78: only this provider's validated token (or its userinfo, fetched with this flow's
    // access token and bound to the same `sub`) is ever read for roles.
    let role_values = match p.role_claim.as_deref() {
        None => vec![],
        Some(path) if p.role_claim_from_userinfo() => {
            let info = state
                .oidc
                .userinfo(p, OAuth2TokenResponse::access_token(&tokens).secret())
                .await
                .map_err(|e| {
                    tracing::warn!("oidc: userinfo failed: {e}");
                    FAILED
                })?;
            if info.get("sub").and_then(|s| s.as_str()) == Some(sub.as_str()) {
                roles::extract_values(&p.id, &info, path)
            } else {
                tracing::warn!("oidc: userinfo sub does not match the ID token; roles ignored");
                vec![]
            }
        }
        Some(path) => verified_payload(id_token)
            .map(|pl| roles::extract_values(&p.id, &pl, path))
            .unwrap_or_default(),
    };
    Ok(Verified {
        role_values,
        sub,
        email,
        email_verified,
        name: claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.as_str().to_owned()),
        auth_time: claims.auth_time().map(|t| t.timestamp()),
    })
}

fn domain_allowed(p: &OidcProvider, email: &str) -> bool {
    p.allowed_email_domains.is_empty() || domain_in(&p.allowed_email_domains, email)
}

/// Exact, case-insensitive match of the e-mail's domain (no subdomains).
fn domain_in(domains: &[String], email: &str) -> bool {
    let Some((_, domain)) = email.rsplit_once('@') else {
        return false;
    };
    !domain.is_empty()
        && domains.iter().any(|d| {
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
    // Phase 1 (RFE-004): there is no local credential, so the provider's verification of the
    // e-mail is the only proof of it. Always required (`require_email_verified` is gone).
    if !v.email_verified {
        deny(state, client, &v.email, p, "email_unverified");
        return Err(Fail("email_unverified"));
    }
    if !domain_allowed(p, &v.email) {
        deny(state, client, &v.email, p, "email_domain_not_allowed");
        return Err(Fail("email_domain_not_allowed"));
    }
    if let Some((session, user)) = flow.ra {
        return match reauthenticate(state, client, p, &v, session, user).await {
            Ok(()) => Ok(SignedIn {
                session_token: None,
                return_to: flow.r,
                reauth: true,
            }),
            Err(Fail(code)) => {
                deny(state, client, &v.email, p, code);
                Err(Fail(code))
            }
        };
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
                session_token: Some(session_token),
                return_to: flow.r,
                reauth: false,
            })
        }
        Err(Fail(code)) => {
            deny(state, client, &v.email, p, code);
            Err(Fail(code))
        }
    }
}

/// Re-authentication callback (docs/04 §OIDC): the same user, a fresh `auth_time` when the IdP
/// returns one, then `sessions.reauth_at = now()`.
async fn reauthenticate(
    state: &AppState,
    client: &Client,
    p: &OidcProvider,
    v: &Verified,
    session: Uuid,
    user_id: Uuid,
) -> Result<(), Fail> {
    let user = identities::find_user(&state.pool, &p.id, &v.sub)
        .await?
        .ok_or(Fail("reauth_user_mismatch"))?;
    if user.id != user_id {
        return Err(Fail("reauth_user_mismatch"));
    }
    if user.disabled_at.is_some() {
        return Err(Fail("user_disabled"));
    }
    if let Some(t) = v.auth_time
        && Utc::now().timestamp() - t > REAUTH_AUTH_TIME_MAX_AGE_SECS
    {
        return Err(Fail("reauth_stale"));
    }
    if !sessions::stamp_reauth(&state.pool, session, user_id).await? {
        return Err(Fail("oidc_state_invalid"));
    }
    audit::record(
        state,
        AuditEvent::new(auth::actor(&user, client), "user.reauth", Outcome::Success)
            .target("user", user.id)
            .metadata(json!({"method": "oidc", "provider": p.id})),
    );
    Ok(())
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
    // D-139: what this provider's mappings grant for the validated claim values.
    let desired = roles::compute(p, &v.role_values);
    if p.require_role && !desired.grants_anything() {
        return Err(Fail("no_mapped_role"));
    }
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
                    create_user(state, &mut tx, p, v, listed, &desired).await?
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
        return Err(Fail("user_disabled"));
    }

    // The e-mail list is a manual source: it takes the flag over from a claim and is granted
    // before the claim sync so that the sync never revokes it.
    if listed {
        if !user.is_instance_admin {
            user = users::grant_instance_admin(&mut *tx, user.id).await?;
        }
        aurora_db::repo::idp_roles::clear_admin_source(&mut *tx, user.id).await?;
    }
    // Precedence: invitation (in `create_user`) → role claims → provider `jit` → placement.
    roles::sync(&mut tx, state, p, &mut user, &v.role_values, &desired).await?;
    apply_jit(&mut tx, p, &mut user).await?;

    user = users::record_login(&mut *tx, user.id).await?;
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
    desired: &roles::Desired,
) -> Result<UserRow, Fail> {
    let jit_org = match &p.jit {
        Some(j) => orgs::get_by_slug(&mut *tx, &j.org_slug).await?,
        None => None,
    };
    // docs/04 §Sign-up semantics under OIDC-only. (A pre-provisioned `bootstrap-admin` user
    // exists already and never reaches here.)
    let invited = inv_repo::has_pending_for_email(&mut *tx, &v.email).await?;
    // D-143: the provider's own role mapping admits a new user when the validated claim maps to
    // at least one actual grant (all other sign-in checks have already passed).
    let claimed = desired.grants_anything();
    let allowed = match state.config.signup_mode {
        SignupMode::Open => true,
        SignupMode::SsoJit => jit_org.is_some() || invited || claimed,
        SignupMode::InviteOnly => listed || invited || claimed,
    };
    if !allowed {
        return Err(Fail("signup_closed"));
    }
    let admitted_by_claim = claimed
        && match state.config.signup_mode {
            SignupMode::Open => false,
            SignupMode::SsoJit => jit_org.is_none() && !invited,
            SignupMode::InviteOnly => !listed && !invited,
        };
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
    if admitted_by_claim {
        // Audited in the user-creating transaction; names the provider and the matched values.
        let matched: Vec<&str> = desired
            .granted
            .iter()
            .filter_map(|g| g["value"].as_str())
            .collect();
        let ev = AuditEvent::new(
            AuditActor {
                kind: "service",
                id: None,
                label: format!("idp:{}", p.id),
                source: "system",
                ip: None,
            },
            "user.admit_by_claim",
            Outcome::Success,
        )
        .target("user", user.id)
        .metadata(json!({"provider": p.id, "claim_values": matched}));
        audit::insert_tx(&mut *tx, &ev).await.map_err(|e| {
            tracing::error!("oidc: admission audit: {e}");
            Fail("oidc_failed")
        })?;
    }
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
    } else {
        // `invite_only` / `sso_jit`: a pending invitation is accepted as part of the sign-in.
        invitations::service::accept_pending_for_email(tx, &user)
            .await
            .map_err(|e| {
                tracing::error!("oidc: accept invitations: {e:?}");
                Fail("oidc_failed")
            })?;
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
    // RFE-008: a membership created by JIT provisioning is audited in the sign-in transaction.
    let actor = AuditActor {
        kind: "user",
        id: Some(user.id),
        label: user.email.clone(),
        source: "dashboard",
        ip: None,
    };
    let ev = AuditEvent::new(actor, "member.add", Outcome::Success)
        .org(org_id)
        .target("user", user.id)
        .metadata(serde_json::json!({"role": role, "via": "sso_jit"}));
    audit::insert_tx(&mut *tx, &ev).await.map_err(|e| {
        tracing::error!("oidc: jit audit: {e}");
        Fail("oidc_failed")
    })?;
    // Phase 2, RFE-012: JIT may bind as `approver`, never `owner`.
    let ws_role = if j.workspace_role == "approver" {
        "approver"
    } else {
        "member"
    };
    for slug in &j.workspace_slugs {
        match workspaces::get_by_slug(&mut *tx, org_id, slug).await? {
            Some(ws) => workspaces::bind_user(&mut *tx, ws.id, user.id, ws_role).await?,
            None => tracing::warn!("oidc: jit workspace `{slug}` does not exist"),
        }
    }
    if user.default_org_id.is_none() {
        users::set_default_org(&mut *tx, user.id, org_id).await?;
        user.default_org_id = Some(org_id);
    }
    Ok(())
}
