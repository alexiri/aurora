//! Authentication extractors and request scoping (docs/04 §Sessions, §API keys, §Authorization).
//!
//! - [`Caller`]: session (S) or API key (K).
//! - [`SessionCaller`]: session only (SO); keys get 403 `session_required`.
//! - [`InstanceAdmin`]: session caller with `is_instance_admin`.
//! - `Caller::workspace` / `Caller::org`: resource scoping (404 when invisible, key confinement).
//!   Use [`require_own`] / [`OrgAuth::require_admin`] / [`OrgAuth::require_owner`] for 403 `forbidden`.

use crate::app::AppState;
use crate::http::client_ip::ClientIp;
use aurora_core::ApiError;
use aurora_core::token::{self, TokenPrefix};
use aurora_db::authz::{self, OrgRole, WorkspaceAuth};
use aurora_db::bus::{BusEvent, Channel, Listener};
use aurora_db::repo::{api_keys, sessions};
use axum::extract::FromRequestParts;
use http::{HeaderMap, StatusCode, header, request::Parts};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const SESSION_COOKIE: &str = "aurora_session";
/// Maximum age of a cached authentication result.
pub const CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    User,
    Service,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    Session {
        id: Uuid,
    },
    ApiKey {
        id: Uuid,
        org_id: Uuid,
        /// `None` => org key (`auo_`).
        workspace_id: Option<Uuid>,
        kind: KeyKind,
    },
}

/// An authenticated request principal.
#[derive(Debug, Clone)]
pub struct Caller {
    pub user_id: Uuid,
    /// Owner email, used as the audit actor label.
    pub email: String,
    pub is_instance_admin: bool,
    pub principal: Principal,
    pub ip: Option<IpAddr>,
}

impl Caller {
    /// `dashboard` for sessions, `api` for keys.
    pub fn audit_source(&self) -> &'static str {
        match self.principal {
            Principal::Session { .. } => "dashboard",
            Principal::ApiKey { .. } => "api",
        }
    }

    pub fn is_session(&self) -> bool {
        matches!(self.principal, Principal::Session { .. })
    }

    /// Resolve access to a workspace: key confinement, then the access law.
    /// Invisible => 404; workspace key on another workspace => 403 `workspace_mismatch`;
    /// org key on another org's workspace => 404.
    pub async fn workspace(&self, state: &AppState, ws: Uuid) -> Result<WorkspaceAuth, ApiError> {
        if let Principal::ApiKey {
            org_id,
            workspace_id,
            ..
        } = &self.principal
        {
            match workspace_id {
                Some(k) if *k != ws => {
                    return Err(ApiError::new(
                        StatusCode::FORBIDDEN,
                        "workspace_mismatch",
                        "This API key is confined to a different workspace.",
                    ));
                }
                Some(_) => {}
                None => {
                    if authz::workspace_org(&state.pool, ws).await? != Some(*org_id) {
                        return Err(not_found_ws());
                    }
                }
            }
        }
        authz::workspace_access(&state.pool, self.user_id, ws)
            .await?
            .ok_or_else(not_found_ws)
    }

    /// Resolve the caller's active membership in `org`. Suspended/non-member => 404.
    /// Workspace keys cannot use organization routes (403 `forbidden`);
    /// org keys of another org => 404.
    pub async fn org(&self, state: &AppState, org: Uuid) -> Result<OrgAuth, ApiError> {
        if let Principal::ApiKey {
            org_id,
            workspace_id,
            ..
        } = &self.principal
        {
            if workspace_id.is_some() {
                return Err(ApiError::forbidden(
                    "Workspace API keys cannot access organization resources.",
                ));
            }
            if *org_id != org {
                return Err(ApiError::not_found("Organization not found."));
            }
        }
        match authz::org_membership(&state.pool, self.user_id, org).await? {
            Some(m) if m.is_active() => Ok(OrgAuth {
                org_id: org,
                role: m.role,
            }),
            _ => Err(ApiError::not_found("Organization not found.")),
        }
    }
}

impl Caller {
    /// Org-scope credential *metadata* reads (I-27 amendment): org owner/admin, or an active
    /// org member who is `can_own` on at least one workspace of the org (so they can discover
    /// org secrets/connections to grant to their agents). Same rules as [`Caller::org`] for
    /// keys: a workspace key is 403 `forbidden`, an org key answers like its owner, another
    /// org's key is 404. Plain members get 403 `forbidden`.
    pub async fn org_credential_reader(
        &self,
        state: &AppState,
        org: Uuid,
    ) -> Result<OrgAuth, ApiError> {
        let a = self.org(state, org).await?;
        if a.role.is_admin() || authz::owns_any_workspace(&state.pool, self.user_id, org).await? {
            Ok(a)
        } else {
            Err(ApiError::forbidden(
                "Organization owner/admin or workspace owner role required.",
            ))
        }
    }
}

fn not_found_ws() -> ApiError {
    ApiError::not_found("Workspace not found.")
}

#[derive(Debug, Clone, Copy)]
pub struct OrgAuth {
    pub org_id: Uuid,
    pub role: OrgRole,
}

impl OrgAuth {
    pub fn require_admin(&self) -> Result<(), ApiError> {
        if self.role.is_admin() {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "Organization owner or admin role required.",
            ))
        }
    }
    pub fn require_owner(&self) -> Result<(), ApiError> {
        if self.role == OrgRole::Owner {
            Ok(())
        } else {
            Err(ApiError::forbidden("Organization owner role required."))
        }
    }
}

/// 403 `forbidden` unless the caller can own the workspace (`can_own`).
pub fn require_own(a: &WorkspaceAuth) -> Result<(), ApiError> {
    if a.can_own() {
        Ok(())
    } else {
        Err(ApiError::forbidden("Workspace owner role required."))
    }
}

// ---- cache -----------------------------------------------------------------

/// <=30 s cache of authentication results keyed by token hash.
pub struct AuthCache {
    ttl: Duration,
    map: Mutex<HashMap<[u8; 32], (Instant, Caller)>>,
}

impl Default for AuthCache {
    fn default() -> Self {
        Self::new(CACHE_TTL)
    }
}

impl AuthCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl: ttl.min(CACHE_TTL),
            map: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<[u8; 32], (Instant, Caller)>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn get(&self, h: &[u8; 32]) -> Option<Caller> {
        let mut m = self.lock();
        match m.get(h) {
            Some((at, c)) if at.elapsed() < self.ttl => Some(c.clone()),
            Some(_) => {
                m.remove(h);
                None
            }
            None => None,
        }
    }

    fn put(&self, h: [u8; 32], c: Caller) {
        let mut m = self.lock();
        if m.len() > 10_000 {
            let ttl = self.ttl;
            m.retain(|_, (at, _)| at.elapsed() < ttl);
        }
        m.insert(h, (Instant::now(), c));
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Apply an `aurora_auth_revoked` payload `{kind, id}`; unknown shapes flush everything.
    pub fn invalidate(&self, payload: &serde_json::Value) {
        let kind = payload.get("kind").and_then(|v| v.as_str());
        let id = payload
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());
        let (Some(kind), Some(id)) = (kind, id) else {
            return self.clear();
        };
        let mut m = self.lock();
        match kind {
            "session" => m.retain(
                |_, (_, c)| !matches!(c.principal, Principal::Session { id: s } if s == id),
            ),
            "api_key" | "key" => m.retain(
                |_, (_, c)| !matches!(c.principal, Principal::ApiKey { id: k, .. } if k == id),
            ),
            "user" => m.retain(|_, (_, c)| c.user_id != id),
            _ => m.clear(),
        }
    }
}

/// Background task: flush the cache on `aurora_auth_revoked` and on listener resync.
pub fn spawn_revocation_listener(
    pool: aurora_db::PgPool,
    cache: std::sync::Arc<AuthCache>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match Listener::connect(&pool, &[Channel::AuthRevoked]).await {
                Ok(mut l) => loop {
                    match l.next().await {
                        BusEvent::Resync => cache.clear(),
                        BusEvent::Notification { payload, .. } => cache.invalidate(&payload),
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "auth revocation listener connect failed; retrying");
                    cache.clear();
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    })
}

// ---- cookies / headers -------------------------------------------------------

/// Value of cookie `name`, if present.
pub fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// `Set-Cookie` value establishing the session (docs/04 §Sessions).
pub fn session_cookie(state: &AppState, token: &str) -> String {
    format!(
        "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{}",
        state.config.auth.session_absolute_secs,
        if state.net.cookie_secure {
            "; Secure"
        } else {
            ""
        }
    )
}

/// `Set-Cookie` value clearing the session.
pub fn clear_session_cookie(state: &AppState) -> String {
    format!(
        "{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{}",
        if state.net.cookie_secure {
            "; Secure"
        } else {
            ""
        }
    )
}

fn unauth() -> ApiError {
    ApiError::unauthenticated("Authentication required.")
}

// ---- extractors --------------------------------------------------------------

async fn authenticate_key(state: &AppState, tok: &str) -> Result<Caller, ApiError> {
    if !(tok.starts_with(TokenPrefix::WorkspaceKey.as_str())
        || tok.starts_with(TokenPrefix::OrgKey.as_str()))
    {
        return Err(unauth());
    }
    let h = token::hash(tok);
    if let Some(c) = state.session_cache.get(&h) {
        return Ok(c);
    }
    let rec = api_keys::find_by_hash(&state.pool, &h)
        .await?
        .filter(|r| r.live)
        .ok_or_else(unauth)?;
    if let Err(e) = api_keys::touch(&state.pool, rec.id).await {
        tracing::warn!(error = %e, "api key touch failed");
    }
    let c = Caller {
        user_id: rec.user_id,
        email: rec.user_email,
        is_instance_admin: rec.is_instance_admin,
        principal: Principal::ApiKey {
            id: rec.id,
            org_id: rec.org_id,
            workspace_id: rec.workspace_id,
            kind: if rec.kind == "service" {
                KeyKind::Service
            } else {
                KeyKind::User
            },
        },
        ip: None,
    };
    state.session_cache.put(h, c.clone());
    Ok(c)
}

async fn authenticate_session(state: &AppState, tok: &str) -> Result<Caller, ApiError> {
    if !tok.starts_with(TokenPrefix::Session.as_str()) {
        return Err(unauth());
    }
    let h = token::hash(tok);
    if let Some(c) = state.session_cache.get(&h) {
        return Ok(c);
    }
    let rec = sessions::find_by_hash(&state.pool, &h, state.config.auth.session_idle_secs)
        .await?
        .filter(|r| r.live)
        .ok_or_else(unauth)?;
    if let Err(e) = sessions::touch(&state.pool, rec.id).await {
        tracing::warn!(error = %e, "session touch failed");
    }
    let c = Caller {
        user_id: rec.user_id,
        email: rec.user_email,
        is_instance_admin: rec.is_instance_admin,
        principal: Principal::Session { id: rec.id },
        ip: None,
    };
    state.session_cache.put(h, c.clone());
    Ok(c)
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if let Some(c) = parts.extensions.get::<Caller>() {
            return Ok(c.clone());
        }
        // Any Authorization header commits to header auth: never fall back to
        // cookies (so the CSRF exemption for header-authenticated requests in
        // http::csrf can never be combined with cookie authentication).
        let bearer = match parts.headers.get(header::AUTHORIZATION) {
            None => None,
            Some(v) => {
                let tok = v
                    .to_str()
                    .ok()
                    .and_then(|v| v.split_once(' '))
                    .and_then(|(scheme, rest)| {
                        scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
                    })
                    .ok_or_else(unauth)?;
                Some(tok)
            }
        };
        let mut caller = match bearer {
            Some(tok) => authenticate_key(state, tok).await?,
            None => {
                let tok = cookie_value(&parts.headers, SESSION_COOKIE).ok_or_else(unauth)?;
                authenticate_session(state, tok).await?
            }
        };
        caller.ip = parts.extensions.get::<ClientIp>().and_then(|c| c.0);
        parts.extensions.insert(caller.clone());
        Ok(caller)
    }
}

/// Session-only (SO) caller.
#[derive(Debug, Clone)]
pub struct SessionCaller(pub Caller);

impl std::ops::Deref for SessionCaller {
    type Target = Caller;
    fn deref(&self) -> &Caller {
        &self.0
    }
}

impl FromRequestParts<AppState> for SessionCaller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let c = Caller::from_request_parts(parts, state).await?;
        if c.is_session() {
            Ok(Self(c))
        } else {
            Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "session_required",
                "This action requires a signed-in session, not an API key.",
            ))
        }
    }
}

/// Instance admin (IA): session caller with `is_instance_admin`.
#[derive(Debug, Clone)]
pub struct InstanceAdmin(pub Caller);

impl std::ops::Deref for InstanceAdmin {
    type Target = Caller;
    fn deref(&self) -> &Caller {
        &self.0
    }
}

impl FromRequestParts<AppState> for InstanceAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let SessionCaller(c) = SessionCaller::from_request_parts(parts, state).await?;
        if c.is_instance_admin {
            Ok(Self(c))
        } else {
            Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "instance_admin_required",
                "Instance administrator access required.",
            ))
        }
    }
}

/// Agent-token (A) caller: a live `aug_` gateway credential of kind `agent`.
/// Use only on `/v1/agent/*` routes; every other extractor rejects `aug_` tokens.
/// Not cached, so regeneration/deletion take effect immediately.
#[derive(Debug, Clone)]
pub struct AgentCaller {
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub org_id: Uuid,
    pub credential_id: Uuid,
}

impl FromRequestParts<AppState> for AgentCaller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let tok = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split_once(' '))
            .and_then(|(scheme, rest)| scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim()))
            .filter(|t| t.starts_with(TokenPrefix::Gateway.as_str()))
            .ok_or_else(unauth)?;
        let rec =
            aurora_db::repo::gateway::find_credential_identity(&state.pool, &token::hash(tok))
                .await?
                .filter(|r| r.kind == "agent")
                .ok_or_else(unauth)?;
        Ok(Self {
            agent_id: rec.agent_id,
            workspace_id: rec.workspace_id,
            org_id: rec.org_id,
            credential_id: rec.credential_id,
        })
    }
}
