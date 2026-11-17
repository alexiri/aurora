//! Web Push for approvals (phase 5, P4-011B-3, D-134, D-266, D-267(b); C-830..C-834, C-837,
//! C-838; SEC-75, SEC-76). A decider's opted-in browser is woken when an approval waits and no
//! Aurora tab is open: the instance holds one VAPID key, each subscription is bound to the session
//! that registered it, and what is pushed is an end-to-end encrypted, fixed-field payload sent
//! through the destination guard to an allow-listed push service only. Every route answers 404
//! `push_unavailable` while `push.enabled` is off, and no outbound call is made.
//!
//! No endpoint URL, key, JWT or plaintext payload is ever logged, returned, put in a metric
//! label, an error, an audit row or a snapshot.
pub mod cli;
pub mod crypto;
pub mod deliver;
mod dto;
pub mod jobs;
mod routes;
pub mod service;
pub mod vapid;

use crate::app::AppState;
use aurora_config::ApiConfig;
use aurora_core::ApiError;
use aurora_core::limits::{PUSH_ENDPOINT_MAX, PUSH_TEST_PER_HOUR};
use aurora_db::repo::push::SubRow;
use axum::http::StatusCode;
use chrono::Utc;
use crypto::VapidKey;
use dto::PushState;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub use routes::router;

/// Per-process Web Push state: the operator key (when configured), the loaded generated keys, the
/// JWT cache and the per-user test-push limiter.
pub struct PushRuntime {
    pub enabled: bool,
    /// `sub` claim of every JWT.
    pub subject: String,
    operator: Option<Arc<VapidKey>>,
    loaded: Mutex<HashMap<Uuid, Arc<VapidKey>>>,
    jwts: Mutex<HashMap<(String, String), (String, i64)>>,
    tests: Mutex<HashMap<Uuid, VecDeque<Instant>>>,
    /// The outbound side (C-834): destination-guarded client, the loopback-only test client and
    /// the pool bound. Built only with push on.
    pub(crate) sender: deliver::Sender,
}

impl PushRuntime {
    /// Build from the validated configuration; refuses an unreadable operator key, naming the
    /// variable (C-833). With push off nothing is read.
    pub fn from_config(cfg: &ApiConfig) -> Result<Self, String> {
        let mut operator = None;
        if cfg.push.enabled
            && let Some(var) = cfg.push.vapid.private_key_env.as_deref()
        {
            let raw = std::env::var(var).map_err(|_| format!("{var}: not set"))?;
            operator = Some(Arc::new(VapidKey::from_operator_value(&raw).map_err(
                |_| format!("{var}: not a PKCS#8 PEM or base64url raw 32-byte P-256 private key"),
            )?));
        }
        let subject = cfg
            .push
            .vapid
            .subject
            .clone()
            .unwrap_or_else(|| cfg.external_url.clone());
        Ok(Self {
            enabled: cfg.push.enabled,
            subject,
            operator,
            loaded: Mutex::default(),
            jwts: Mutex::default(),
            tests: Mutex::default(),
            sender: deliver::Sender::from_config(cfg)?,
        })
    }

    /// `disabled` | `operator` | `generated`.
    pub fn mode(&self) -> &'static str {
        if !self.enabled {
            "disabled"
        } else if self.operator.is_some() {
            "operator"
        } else {
            "generated"
        }
    }

    pub fn operator_key(&self) -> Option<Arc<VapidKey>> {
        self.operator.clone()
    }

    pub(crate) fn loaded_key(&self, id: Uuid) -> Option<Arc<VapidKey>> {
        self.loaded.lock().ok()?.get(&id).cloned()
    }

    pub(crate) fn remember_key(&self, id: Uuid, key: Arc<VapidKey>) {
        if let Ok(mut m) = self.loaded.lock() {
            m.insert(id, key);
        }
    }

    pub(crate) fn forget_key(&self, id: Uuid) {
        if let Ok(mut m) = self.loaded.lock() {
            m.remove(&id);
        }
        if let Ok(mut j) = self.jwts.lock() {
            j.clear();
        }
    }

    /// The VAPID `Authorization` header value for `aud` under `key`, cached per (key, `aud`)
    /// until 5 minutes before `exp` (C-833).
    pub fn authorization(&self, key: &VapidKey, aud: &str) -> String {
        let now = Utc::now().timestamp();
        let cache_key = (key.public_b64u(), aud.to_owned());
        let jwt = {
            let mut cache = self.jwts.lock().unwrap_or_else(|e| e.into_inner());
            match cache.get(&cache_key) {
                Some((jwt, exp)) if *exp - crypto::JWT_REUSE_MARGIN_SECS > now => jwt.clone(),
                _ => {
                    let exp = crypto::jwt_exp(now);
                    let jwt = key.jwt(aud, exp, &self.subject);
                    cache.insert(cache_key, (jwt.clone(), exp));
                    jwt
                }
            }
        };
        format!("vapid t={jwt}, k={}", key.public_b64u())
    }

    /// One test push per `PUSH_TEST_PER_HOUR` window per user; returns the seconds until the next
    /// is allowed when the limit is reached. (Per replica: the window lives in this process.)
    pub fn take_test_slot(&self, user: Uuid) -> Result<(), u64> {
        let hour = Duration::from_secs(3600);
        let now = Instant::now();
        let mut m = self.tests.lock().unwrap_or_else(|e| e.into_inner());
        let q = m.entry(user).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= hour) {
            q.pop_front();
        }
        if q.len() >= PUSH_TEST_PER_HOUR as usize {
            let wait = q
                .front()
                .map(|t| hour.saturating_sub(now.duration_since(*t)).as_secs() + 1)
                .unwrap_or(1);
            return Err(wait);
        }
        q.push_back(now);
        Ok(())
    }
}

/// 404 `push_unavailable`: Web Push is off on this instance.
pub fn push_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "push_unavailable",
        "Web Push is not enabled on this instance.",
    )
}

/// The derived state of a subscription (C-831): disabled, then expired, paused, a key that was
/// rotated out (the browser must re-subscribe), else active.
pub fn state_of(r: &SubRow) -> PushState {
    if r.disabled_at.is_some() {
        PushState::Disabled
    } else if r.expires_at.is_some_and(|e| e <= Utc::now()) {
        PushState::Expired
    } else if r.paused_at.is_some() {
        PushState::Paused
    } else if r.key_status.as_deref() == Some("retired") {
        PushState::StaleKey
    } else {
        PushState::Active
    }
}

/// Why an endpoint was refused (`push_endpoint_not_allowed` `details.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointRefusal {
    NotHttps,
    HostNotAllowed,
    TooLong,
    Userinfo,
    IpLiteral,
}

impl EndpointRefusal {
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotHttps => "not_https",
            Self::HostNotAllowed => "host_not_allowed",
            Self::TooLong => "too_long",
            Self::Userinfo => "userinfo",
            Self::IpLiteral => "ip_literal",
        }
    }
}

/// `host` matches one allow-list entry: exact, or `*.suffix` (at least one label before the
/// suffix; the dot is part of the match, so `evilfcm.googleapis.com` never matches
/// `*.fcm.googleapis.com`).
pub fn host_allowed(patterns: &[String], host: &str) -> bool {
    patterns.iter().any(|p| match p.strip_prefix("*.") {
        Some(suffix) => host
            .strip_suffix(suffix)
            .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
        None => p == host,
    })
}

/// Validate a subscription endpoint against the allow-list: `https`, no credentials, a DNS name
/// (not an IP literal), the default port, within `PUSH_ENDPOINT_MAX`, on an allow-listed host.
/// Used at registration and again before every send (SEC-76).
pub fn check_endpoint(
    allowed: &[String],
    endpoint: &str,
) -> Result<(url::Url, String), EndpointRefusal> {
    if endpoint.chars().count() > PUSH_ENDPOINT_MAX {
        return Err(EndpointRefusal::TooLong);
    }
    let url = url::Url::parse(endpoint).map_err(|_| EndpointRefusal::NotHttps)?;
    if url.scheme() != "https" {
        return Err(EndpointRefusal::NotHttps);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(EndpointRefusal::Userinfo);
    }
    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.to_owned(),
        Some(_) => return Err(EndpointRefusal::IpLiteral),
        None => return Err(EndpointRefusal::NotHttps),
    };
    if url.port().is_some_and(|p| p != 443) || !host_allowed(allowed, &host) {
        return Err(EndpointRefusal::HostNotAllowed);
    }
    Ok((url, host))
}

/// Entry point for the API's startup: with push on and generated keys, make sure an active key
/// exists (the first replica generates it under the advisory lock).
pub async fn ensure_ready(state: &AppState) -> Result<(), ApiError> {
    if state.push.enabled && state.push.operator_key().is_none() {
        vapid::active(state).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow() -> Vec<String> {
        [
            "fcm.googleapis.com",
            "*.push.services.mozilla.com",
            "*.notify.windows.com",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn endpoint_rules() {
        let a = allow();
        assert!(check_endpoint(&a, "https://fcm.googleapis.com/fcm/send/abc").is_ok());
        assert!(check_endpoint(&a, "https://updates.push.services.mozilla.com/wpush/v2/x").is_ok());
        let refused = |e: &str| check_endpoint(&a, e).err();
        assert_eq!(
            refused("http://fcm.googleapis.com/x"),
            Some(EndpointRefusal::NotHttps)
        );
        assert_eq!(
            refused("https://u:p@fcm.googleapis.com/x"),
            Some(EndpointRefusal::Userinfo)
        );
        assert_eq!(
            refused("https://127.0.0.1/x"),
            Some(EndpointRefusal::IpLiteral)
        );
        assert_eq!(refused("https://[::1]/x"), Some(EndpointRefusal::IpLiteral));
        assert_eq!(
            refused("https://fcm.googleapis.com.evil.example/x"),
            Some(EndpointRefusal::HostNotAllowed)
        );
        assert_eq!(
            refused("https://evilfcm.googleapis.com/x"),
            Some(EndpointRefusal::HostNotAllowed)
        );
        assert_eq!(
            refused("https://push.services.mozilla.com/x"),
            Some(EndpointRefusal::HostNotAllowed)
        );
        assert_eq!(
            refused("https://xpush.services.mozilla.com/x"),
            Some(EndpointRefusal::HostNotAllowed)
        );
        assert_eq!(
            refused("https://fcm.googleapis.com:8443/x"),
            Some(EndpointRefusal::HostNotAllowed)
        );
        assert_eq!(
            refused("https://fcm.googleapis.com./x"),
            Some(EndpointRefusal::HostNotAllowed)
        );
        let long = format!(
            "https://fcm.googleapis.com/{}",
            "a".repeat(PUSH_ENDPOINT_MAX)
        );
        assert_eq!(refused(&long), Some(EndpointRefusal::TooLong));
        assert_eq!(refused("not a url"), Some(EndpointRefusal::NotHttps));
    }
}
