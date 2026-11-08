//! Encrypted connect state (`oauth-state` purpose, SEC-23) and the browser-binding cookie.
use crate::app::AppState;
use aurora_crypto::Purpose;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const COOKIE: &str = "__Host-aurora_oauth";
const AAD: &[u8] = b"connect-flow";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flow {
    pub nonce: String,
    pub app_id: String,
    pub method_id: String,
    /// `organization` | `workspace`.
    pub scope: String,
    pub workspace_id: Option<Uuid>,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub client_id: Option<Uuid>,
    /// The registered app instance this flow connects (D-74); re-checked at the callback.
    #[serde(default)]
    pub instance_id: Option<Uuid>,
    pub access_level: Option<String>,
    pub pkce_verifier: Option<String>,
    pub reconnect: Option<Uuid>,
    pub continuation: Option<String>,
    /// Scopes requested at authorize time.
    pub scopes: Vec<String>,
    /// Validated tenant host from the connect form (S-14), for templated OAuth URLs.
    #[serde(default)]
    pub tenant_host: Option<String>,
    /// Non-secret form values stored as connection metadata.
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    pub exp: i64,
}

pub fn random(n: usize) -> String {
    let mut b = vec![0u8; n];
    if aws_lc_rs::rand::fill(&mut b).is_err() {
        // Fall back to a time-seeded value rather than panicking; the sealed state is
        // still authenticated, so predictability only affects the nonce.
        b = Uuid::now_v7().as_bytes().to_vec();
    }
    B64.encode(b)
}

pub fn challenge(verifier: &str) -> String {
    B64.encode(Sha256::digest(verifier.as_bytes()))
}

impl Flow {
    pub fn expiry() -> i64 {
        Utc::now().timestamp()
            + i64::try_from(aurora_core::limits::OAUTH_STATE_TTL.as_secs()).unwrap_or(600)
    }

    pub fn seal(&self, state: &AppState) -> Option<String> {
        state
            .derived
            .seal(Purpose::OauthState, AAD, &serde_json::to_vec(self).ok()?)
            .ok()
    }

    pub fn open(state: &AppState, sealed: &str) -> Option<Self> {
        let plain = state.derived.open(Purpose::OauthState, AAD, sealed).ok()?;
        let f: Self = serde_json::from_slice(plain.expose()).ok()?;
        (f.exp >= Utc::now().timestamp()).then_some(f)
    }
}

pub fn cookie(value: &str, max_age: u64) -> String {
    format!("{COOKIE}={value}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={max_age}")
}

pub fn cookie_ttl() -> u64 {
    aurora_core::limits::OAUTH_STATE_TTL.as_secs()
}
