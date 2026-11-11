//! SCIM authentication (SEC-72): the `aud_` bearer of one org, resolved fresh on every
//! request. Cookies, `Origin` and CSRF do not exist here. Every failure is the one identical
//! 401 SCIM error with `WWW-Authenticate: Bearer`.

use super::error::{ScimError, ScimResult};
use crate::app::AppState;
use crate::http::client_ip::ClientIp;
use aurora_core::token::{self, TokenPrefix};
use aurora_db::repo::scim::{self, TokenRow};
use axum::extract::FromRequestParts;
use http::request::Parts;
use uuid::Uuid;

/// `organizations.settings.scim` resolved for one request (defaults = SCIM off).
#[derive(Debug, Clone)]
pub struct OrgScim {
    pub org_id: Uuid,
    pub enabled: bool,
    /// Exact, lower-cased domains; at least one when `enabled`.
    pub email_domains: Vec<String>,
    pub exclusive: bool,
}

impl OrgScim {
    pub fn from_settings(org_id: Uuid, v: &serde_json::Value) -> Self {
        let domains = v
            .get("email_domains")
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(|s| s.trim().to_ascii_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            org_id,
            enabled: v
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            email_domains: domains,
            exclusive: v
                .get("exclusive")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        }
    }

    /// Is this e-mail's domain one of the org's domains (exact, case-insensitive, D-118)?
    pub fn domain_allowed(&self, email: &str) -> bool {
        domain_in(&self.email_domains, email)
    }

    /// The instance allow-list (`scim.allowed_email_domains`; empty = unrestricted).
    pub fn instance_domain_allowed(domains: &[String], email: &str) -> bool {
        domains.is_empty() || domain_in(domains, email)
    }
}

fn domain_in(domains: &[String], email: &str) -> bool {
    let Some(domain) = email.rsplit('@').next() else {
        return false;
    };
    if domain.is_empty() || !email.contains('@') {
        return false;
    }
    domains.iter().any(|d| d.eq_ignore_ascii_case(domain))
}

fn bearer(headers: &http::HeaderMap) -> Option<&str> {
    let v = headers.get(http::header::AUTHORIZATION)?;
    let s = v.to_str().ok()?;
    let (scheme, rest) = s.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| rest.trim())
        .filter(|t| !t.is_empty())
}

/// A token proven live for this request: its row (for the audit label and the rate bucket)
/// and the org's settings snapshot. `enabled` (org and instance) is checked here.
pub struct ScimToken {
    pub token: TokenRow,
    pub org: OrgScim,
}

impl ScimToken {
    /// Resolve the bearer of a `/scim/v2` request. Every failure is
    /// [`ScimError::unauthorized`] — the one 401 body.
    pub async fn authenticate(state: &AppState, headers: &http::HeaderMap) -> ScimResult<Self> {
        let Some(tok) = bearer(headers) else {
            return Err(unauthorized());
        };
        // A request that presents a non-`aud_` bearer commits to it: never a session
        // fall-through (04 §SCIM).
        if token::parse_prefix(tok) != Some(TokenPrefix::Scim) {
            return Err(unauthorized());
        }
        let row = scim::token_by_hash(&state.pool, &token::hash(tok))
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "scim: token lookup failed");
                ScimError::internal()
            })?;
        let Some(row) = row else {
            return Err(unauthorized());
        };
        if !row.is_live(chrono::Utc::now()) {
            return Err(unauthorized());
        }
        // Org switch + instance switch, read fresh on every request (no cache).
        let org = org_scim(state, row.org_id).await?;
        if !org.enabled || !state.config.scim.enabled {
            return Err(unauthorized());
        }
        Ok(Self { token: row, org })
    }
}

/// The org's SCIM settings; an unknown org is the same 401.
pub async fn org_scim(state: &AppState, org: Uuid) -> ScimResult<OrgScim> {
    let v: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT settings -> 'scim' FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "scim: org settings read failed");
                ScimError::internal()
            })?;
    Ok(OrgScim::from_settings(
        org,
        v.as_ref().unwrap_or(&serde_json::Value::Null),
    ))
}

fn unauthorized() -> ScimError {
    ScimError::unauthorized()
}

/// Client IP of a request (resolved by the outer client-IP middleware).
pub fn client_ip(ext: &http::Extensions) -> Option<std::net::IpAddr> {
    ext.get::<ClientIp>().and_then(|c| c.0)
}

/// Axum extractor: the live token of the request, or the one 401. Rate limiting has been
/// applied by the router's middleware before this runs.
pub struct ScimCaller(pub ScimToken);

impl FromRequestParts<AppState> for ScimCaller {
    type Rejection = ScimError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let tok = ScimToken::authenticate(state, &parts.headers).await?;
        Ok(Self(tok))
    }
}
