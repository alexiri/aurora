//! `gcp_service_account` (C-801, C-807, D-261(d)): an RS256 JWT-bearer assertion signed with the service-account key,
//! exchanged at Google's pinned token endpoint for an OAuth2 access token (`cloud-platform` scope).
use super::{MintCtx, MintError, MintSpec, Minted};
use crate::{AuthMode, Endpoint, KeyOrigin};
use aurora_core::Secret;
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::KeyPair;
use aws_lc_rs::signature::RSA_PKCS1_SHA256;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use http::HeaderMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The only token endpoint a key may name (C-801 `token_uri_not_google`); also the assertion's `aud`.
pub const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
/// The pinned origin `llm.dev_upstream_overrides` may replace.
pub const TOKEN_ORIGIN: &str = "https://oauth2.googleapis.com";
pub const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
/// Lifetime of the signed assertion (Google allows 1 h; D-261 says ≤ 10 min).
pub const ASSERTION_LIFETIME: Duration = Duration::from_secs(600);

/// Fields of a Google key file; anything else is `unknown_field`.
const KNOWN: &[&str] = &[
    "type",
    "project_id",
    "private_key_id",
    "private_key",
    "client_email",
    "client_id",
    "auth_uri",
    "token_uri",
    "auth_provider_x509_cert_url",
    "client_x509_cert_url",
    "universe_domain",
];

/// Why a key file is refused at save (the `details.reason` of C-801), with the offending field when there is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueError {
    pub reason: &'static str,
    pub field: Option<&'static str>,
}

const fn bad(reason: &'static str, field: Option<&'static str>) -> ValueError {
    ValueError { reason, field }
}

/// A checked key file. The private key is held as DER in zeroizing memory.
pub struct GcpKey {
    pub client_email: String,
    pub project_id: String,
    der: Secret<Vec<u8>>,
}

/// The shape checks of C-801, before any call: JSON object, known fields only, `type = service_account`,
/// `token_uri` pinned, `client_email`, `project_id` and a PKCS#8 RSA `private_key` that parses.
///
/// # Errors
/// [`ValueError`] naming the C-801 reason; never any part of the value.
pub fn parse_value(value: &[u8]) -> Result<GcpKey, ValueError> {
    let v: serde_json::Value =
        serde_json::from_slice(value).map_err(|_| bad("not_service_account", None))?;
    let o = v
        .as_object()
        .ok_or_else(|| bad("not_service_account", None))?;
    if o.keys().any(|k| !KNOWN.contains(&k.as_str())) {
        return Err(bad("unknown_field", None));
    }
    if o.get("type").and_then(|t| t.as_str()) != Some("service_account") {
        return Err(bad("not_service_account", Some("type")));
    }
    let get = |f: &'static str| {
        o.get(f)
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("missing_field", Some(f)))
    };
    let client_email = get("client_email")?;
    let project_id = get("project_id")?;
    let pem = get("private_key")?;
    match o.get("token_uri").and_then(|t| t.as_str()) {
        None => return Err(bad("missing_field", Some("token_uri"))),
        Some(t) if t != TOKEN_URI => return Err(bad("token_uri_not_google", Some("token_uri"))),
        Some(_) => {}
    }
    let der = pem_to_der(pem).ok_or_else(|| bad("missing_field", Some("private_key")))?;
    if KeyPair::from_pkcs8(&der).is_err() {
        return Err(bad("missing_field", Some("private_key")));
    }
    Ok(GcpKey {
        client_email: client_email.to_owned(),
        project_id: project_id.to_owned(),
        der: Secret::new(der),
    })
}

/// A PKCS#8 `PRIVATE KEY` PEM block to DER (`RSA PRIVATE KEY`, PKCS#1, is not what Google issues).
fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    let body = pem
        .trim()
        .strip_prefix("-----BEGIN PRIVATE KEY-----")?
        .strip_suffix("-----END PRIVATE KEY-----")?;
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD.decode(b64).ok()
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl GcpKey {
    /// The signed assertion (`header.claims.signature`).
    ///
    /// # Errors
    /// `Rejected` when the key cannot sign.
    pub fn assertion(&self, now: SystemTime) -> Result<Secret<String>, MintError> {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = serde_json::json!({
            "iss": self.client_email,
            "scope": SCOPE,
            "aud": TOKEN_URI,
            "iat": secs(now),
            "exp": secs(now + ASSERTION_LIFETIME),
        });
        let signing_input = format!("{header}.{}", URL_SAFE_NO_PAD.encode(claims.to_string()));
        let key = KeyPair::from_pkcs8(self.der.expose()).map_err(|_| MintError::rejected(None))?;
        let mut sig = vec![0u8; key.public_modulus_len()];
        key.sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            signing_input.as_bytes(),
            &mut sig,
        )
        .map_err(|_| MintError::rejected(None))?;
        Ok(Secret::new(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(sig)
        )))
    }
}

pub struct GcpSpec;

impl MintSpec for GcpSpec {
    fn mode(&self) -> AuthMode {
        AuthMode::GcpServiceAccount
    }

    fn region(&self, ep: &Endpoint) -> String {
        ep.location.clone().unwrap_or_default()
    }

    fn build(&self, ctx: &MintCtx<'_>) -> Result<http::Request<Vec<u8>>, MintError> {
        let key = parse_value(ctx.value).map_err(|_| MintError::rejected(None))?;
        let assertion = key.assertion(ctx.now)?;
        let body = format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={}",
            assertion.expose()
        );
        let url = format!("{}/token", ctx.overrides.origin(TOKEN_ORIGIN));
        http::Request::builder()
            .method(http::Method::POST)
            .uri(url)
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .header(http::header::ACCEPT, "application/json")
            .body(body.into_bytes())
            .map_err(|_| MintError::unavailable())
    }

    fn parse(
        &self,
        ctx: &MintCtx<'_>,
        status: u16,
        _headers: &HeaderMap,
        body: &[u8],
    ) -> Result<Minted, MintError> {
        if status != 200 {
            return Err(super::failure(status, body));
        }
        let v: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| MintError::unavailable())?;
        let token = v
            .get("access_token")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .ok_or_else(MintError::unavailable)?;
        let expires_in = v
            .get("expires_in")
            .and_then(serde_json::Value::as_u64)
            .filter(|e| *e > 0)
            .ok_or_else(MintError::unavailable)?;
        // The account the token proves: the key's own project and e-mail (C-804).
        let key = parse_value(ctx.value).map_err(|_| MintError::rejected(None))?;
        Ok(Minted {
            material: Secret::new(token.to_owned()),
            expires_at: ctx.now + Duration::from_secs(expires_in),
            origin: Some(KeyOrigin {
                workspace_id: Some(key.project_id),
                user_id: Some(key.client_email),
                ..KeyOrigin::default()
            }),
        })
    }
}
