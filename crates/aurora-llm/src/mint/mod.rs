//! (C-807, D-261(e), SEC-97) The shared half of the mint service: per minted auth mode a pinned endpoint, a pure request
//! builder and a pure response parser. The gateway's `LlmMint` (cache, single-flight, hold) and the API's validation
//! both run these, so the two see the same request and the same errors. A [`MintError`] never carries the provider's
//! message, only its error *code*.
pub mod entra;
pub mod gcp;
pub mod sts;

use crate::{AuthMode, Endpoint, KeyOrigin, Overrides};
use aurora_core::Secret;
use http::HeaderMap;
use std::time::{Duration, SystemTime};

/// How a mint failed: the secret or the provider refused it (`Rejected`: nothing to retry until the value changes), or
/// the provider could not be reached or answered with an error that says nothing about the secret (`Unavailable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintClass {
    Rejected,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintError {
    pub class: MintClass,
    /// The provider's error code (`invalid_grant`), when it is a short token of `[A-Za-z0-9_.-]`; never a message.
    pub provider_code: Option<String>,
}

impl MintError {
    #[must_use]
    pub fn rejected(code: Option<String>) -> Self {
        Self {
            class: MintClass::Rejected,
            provider_code: code,
        }
    }
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            class: MintClass::Unavailable,
            provider_code: None,
        }
    }
    /// The `reason` of C-807's 502 `credential_unavailable` details.
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self.class {
            MintClass::Rejected => "mint_rejected",
            MintClass::Unavailable => "mint_unavailable",
        }
    }
}

/// A minted credential: the material the gateway injects (never stored, logged or returned), when it stops being
/// valid, and the account it proves (the key-origin evidence).
pub struct Minted {
    pub material: Secret<String>,
    pub expires_at: SystemTime,
    pub origin: Option<KeyOrigin>,
}

/// Everything a builder or parser may read.
pub struct MintCtx<'a> {
    pub endpoint: &'a Endpoint,
    /// The secret's value (the key file JSON, the client secret document, the source key).
    pub value: &'a [u8],
    pub org_id: &'a str,
    /// The org's own `external_id` (AssumeRole only; read by the gateway from `organizations`).
    pub org_external_id: Option<&'a str>,
    pub now: SystemTime,
    pub overrides: &'a Overrides,
}

/// One minted mode.
pub trait MintSpec: Send + Sync {
    fn mode(&self) -> AuthMode;
    /// The cache key's `region` part (`location` for Vertex; the tenant for Entra; the region for STS).
    fn region(&self, ep: &Endpoint) -> String;
    /// The request to the pinned endpoint. `Err(Rejected)` when the value cannot be used at all.
    fn build(&self, ctx: &MintCtx<'_>) -> Result<http::Request<Vec<u8>>, MintError>;
    fn parse(
        &self,
        ctx: &MintCtx<'_>,
        status: u16,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<Minted, MintError>;
}

/// The spec of a minted mode; `None` for the modes that are plain keys or signed per request.
#[must_use]
pub fn spec(mode: AuthMode) -> Option<&'static dyn MintSpec> {
    match mode {
        AuthMode::GcpServiceAccount => Some(&gcp::GcpSpec),
        AuthMode::AzureEntra => Some(&entra::EntraSpec),
        AuthMode::AwsAssumeRole => Some(&sts::StsSpec),
        _ => None,
    }
}

/// Longest response body a mint reads.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Run a mint over `http` (the API's guarded client; the gateway sends the built request through its own).
///
/// # Errors
/// [`MintError`] as the parser maps it; a transport failure is `Unavailable`.
pub async fn run(
    http: &reqwest::Client,
    spec: &dyn MintSpec,
    ctx: &MintCtx<'_>,
) -> Result<Minted, MintError> {
    let req = spec.build(ctx)?;
    let (parts, body) = req.into_parts();
    let mut rb = http.request(parts.method, parts.uri.to_string());
    for (k, v) in &parts.headers {
        rb = rb.header(k, v);
    }
    let resp = rb
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| MintError::unavailable())?;
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = resp.bytes().await.map_err(|_| MintError::unavailable())?;
    let body = &bytes[..bytes.len().min(MAX_RESPONSE_BYTES)];
    spec.parse(ctx, status, &headers, body)
}

/// A provider error code from a JSON body (`{"error":"invalid_grant"}` or `{"error":{"code":"…"}}`), only when it is a
/// short token; the provider's description is never kept.
pub(crate) fn error_code(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let e = v.get("error")?;
    let code = e
        .as_str()
        .or_else(|| e.get("code").and_then(|c| c.as_str()))
        .or_else(|| e.get("status").and_then(|c| c.as_str()))?;
    (!code.is_empty()
        && code.len() <= 64
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
    .then(|| code.to_owned())
}

/// The common status mapping of token endpoints: 400/401/403 reject the credential, everything else is unavailable.
pub(crate) fn failure(status: u16, body: &[u8]) -> MintError {
    if matches!(status, 400 | 401 | 403) {
        MintError::rejected(error_code(body))
    } else {
        MintError::unavailable()
    }
}
