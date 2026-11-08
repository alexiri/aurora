//! Proxy credential authentication (06 §Credential authentication).

use base64::{Engine, engine::general_purpose::STANDARD};
use http::HeaderMap;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Mitm,
    Open,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialKind {
    /// `aug_`
    Agent,
    /// `aub_`
    Boot,
    /// `auc_`
    Context,
}

/// Who is behind a proxy credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestIdentity {
    pub credential_id: Uuid,
    pub kind: CredentialKind,
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub org_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub context_label: Option<String>,
    /// (phase 2, RFE-005, D-49) The person who authored the turn this credential is bound to.
    /// Set only for hosted `context` credentials of person-authored turns, and only while that
    /// user still holds `can_use` on the workspace; `None` for every other credential (boot,
    /// BYO agent, BYO context, non-person turns). Policy `user` identities match only this.
    pub speaker_user_id: Option<Uuid>,
    /// Credential capabilities (`gateway_credentials.capabilities`, 08 §7); only `boot`
    /// credentials can hold any. Phase 1: `browser` (open-lane use, RFE-003).
    pub capabilities: Vec<String>,
    /// (phase 2, RFE-025) Effective BYO controls of the credential's agent; `Some` only for the
    /// `agent` and BYO `context` credentials of a BYO agent. Computed with the identity and
    /// cached with it (invalidated by the `agent`, `workspace` and `org` scopes).
    pub byo: Option<ByoAdmission>,
}

/// The BYO inputs of admission (`06 §BYO admission`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByoAdmission {
    pub effective: aurora_llm::governance::EffectiveByo,
    pub org_slug: String,
    pub ws_slug: String,
}

/// Capability that admits the open lane (RFE-003).
pub const CAP_BROWSER: &str = "browser";

impl RequestIdentity {
    pub fn has_capability(&self, cap: &str) -> bool {
        self.kind == CredentialKind::Boot && self.capabilities.iter().any(|c| c == cap)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// Backing store unreachable; the request is answered 503.
    #[error("credential store unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait CredentialResolver: Send + Sync + 'static {
    /// `Ok(None)` = unknown, revoked or expired credential.
    async fn resolve(&self, token: &str) -> Result<Option<RequestIdentity>, ResolveError>;

    /// Fresh identity of a credential by id, bypassing caches (RFE-025: re-evaluating live BYO
    /// tunnels after an invalidation). `Ok(None)` = revoked, expired or gone. The default is
    /// `None`, i.e. a live tunnel is closed.
    async fn resolve_credential(&self, _id: Uuid) -> Result<Option<RequestIdentity>, ResolveError> {
        Ok(None)
    }
}

/// Every credential is rejected (production wiring until GW-002).
pub struct DenyAllResolver;

#[async_trait::async_trait]
impl CredentialResolver for DenyAllResolver {
    async fn resolve(&self, _token: &str) -> Result<Option<RequestIdentity>, ResolveError> {
        Ok(None)
    }
}

/// Parses `Proxy-Authorization: Basic base64(user:token)`.
pub fn parse_proxy_authorization(headers: &HeaderMap) -> Option<(Lane, String)> {
    let v = headers
        .get(http::header::PROXY_AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, rest) = v.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let raw = STANDARD.decode(rest.trim()).ok()?;
    let raw = String::from_utf8(raw).ok()?;
    let (user, token) = raw.split_once(':')?;
    if token.is_empty() {
        return None;
    }
    let lane = if user == "open" {
        Lane::Open
    } else {
        Lane::Mitm
    };
    Some((lane, token.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(v: &str) -> HeaderMap {
        let mut m = HeaderMap::new();
        m.insert("proxy-authorization", v.parse().unwrap());
        m
    }

    #[test]
    fn parses_lanes() {
        let b = STANDARD.encode("agent:aug_x");
        assert_eq!(
            parse_proxy_authorization(&h(&format!("Basic {b}"))),
            Some((Lane::Mitm, "aug_x".into()))
        );
        let b = STANDARD.encode("open:aug_x:y");
        assert_eq!(
            parse_proxy_authorization(&h(&format!("basic {b}"))),
            Some((Lane::Open, "aug_x:y".into()))
        );
    }

    #[test]
    fn rejects_bad() {
        assert!(parse_proxy_authorization(&HeaderMap::new()).is_none());
        assert!(parse_proxy_authorization(&h("Bearer abc")).is_none());
        assert!(parse_proxy_authorization(&h("Basic !!!")).is_none());
        let b = STANDARD.encode("nocolon");
        assert!(parse_proxy_authorization(&h(&format!("Basic {b}"))).is_none());
    }
}
