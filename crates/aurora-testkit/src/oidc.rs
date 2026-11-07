//! In-process mock OpenID Provider for sign-in tests (BE-015).
//!
//! Serves discovery, JWKS (ES256 key generated per instance), an auto-approving
//! authorize endpoint (302 to `redirect_uri` with `code` + `state`) and a token
//! endpoint that verifies the PKCE S256 verifier and returns an ID token whose
//! claims are configurable through [`MockIdp::set_claims`] / [`MockIdp::set`].
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const CLIENT_ID: &str = "aurora-test-client";
pub const CLIENT_SECRET: &str = "aurora-test-secret";
const KID: &str = "mock-key-1";

/// Claims placed in the next ID tokens. `nonce: None` echoes the nonce sent to `/authorize`.
#[derive(Debug, Clone)]
pub struct Claims {
    pub sub: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub name: Option<String>,
    pub nonce: Option<String>,
    /// Seconds from now until `exp` (negative => already expired).
    pub exp_in: i64,
}

impl Default for Claims {
    fn default() -> Self {
        Self {
            sub: "sub-1".into(),
            email: Some("user@example.com".into()),
            email_verified: Some(true),
            name: Some("Test User".into()),
            nonce: None,
            exp_in: 300,
        }
    }
}

struct Pending {
    nonce: String,
    challenge: String,
    redirect_uri: String,
}

struct Inner {
    issuer: String,
    key: EcdsaKeyPair,
    claims: Mutex<Claims>,
    codes: Mutex<HashMap<String, Pending>>,
}

pub struct MockIdp {
    inner: Arc<Inner>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockIdp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockIdp {
    /// Start on `127.0.0.1:0`.
    pub async fn start() -> MockIdp {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .expect("generate ES256 key");
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref())
            .expect("load ES256 key");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock idp");
        let issuer = format!("http://{}", listener.local_addr().expect("addr"));
        let inner = Arc::new(Inner {
            issuer,
            key,
            claims: Mutex::new(Claims::default()),
            codes: Mutex::new(HashMap::new()),
        });
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .with_state(inner.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        MockIdp { inner, task }
    }

    /// Issuer URL (no trailing slash), for `auth.oidc_providers[].issuer`.
    pub fn issuer(&self) -> &str {
        &self.inner.issuer
    }

    pub fn client_id(&self) -> &'static str {
        CLIENT_ID
    }

    pub fn client_secret(&self) -> &'static str {
        CLIENT_SECRET
    }

    /// Replace all claims.
    pub fn set_claims(&self, c: Claims) {
        *self.inner.claims.lock().expect("claims") = c;
    }

    /// Mutate the current claims.
    pub fn set(&self, f: impl FnOnce(&mut Claims)) {
        f(&mut self.inner.claims.lock().expect("claims"));
    }

    /// Convenience: `sub`, `email`, `email_verified`.
    pub fn identity(&self, sub: &str, email: &str, verified: bool) {
        self.set(|c| {
            c.sub = sub.into();
            c.email = Some(email.into());
            c.email_verified = Some(verified);
        });
    }
}

async fn discovery(State(s): State<Arc<Inner>>) -> Json<Value> {
    let i = &s.issuer;
    Json(json!({
        "issuer": i,
        "authorization_endpoint": format!("{i}/authorize"),
        "token_endpoint": format!("{i}/token"),
        "jwks_uri": format!("{i}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["ES256"],
        "token_endpoint_auth_methods_supported": ["client_secret_post", "client_secret_basic"],
        "code_challenge_methods_supported": ["S256"],
        "scopes_supported": ["openid", "email", "profile"],
        "claims_supported": ["sub", "email", "email_verified", "name", "nonce"],
    }))
}

async fn jwks(State(s): State<Arc<Inner>>) -> Json<Value> {
    let pk = s.key.public_key().as_ref(); // 0x04 || X || Y
    Json(json!({"keys": [{
        "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": KID,
        "x": B64.encode(&pk[1..33]), "y": B64.encode(&pk[33..65]),
    }]}))
}

fn random_code() -> String {
    let mut b = [0u8; 18];
    aws_lc_rs::rand::fill(&mut b).expect("rand");
    B64.encode(b)
}

fn enc(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

async fn authorize(
    State(s): State<Arc<Inner>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let get = |k: &str| q.get(k).cloned().unwrap_or_default();
    if get("client_id") != CLIENT_ID
        || get("response_type") != "code"
        || get("code_challenge_method") != "S256"
        || get("code_challenge").is_empty()
        || get("redirect_uri").is_empty()
    {
        return (StatusCode::BAD_REQUEST, "invalid authorization request").into_response();
    }
    let code = random_code();
    let redirect_uri = get("redirect_uri");
    s.codes.lock().expect("codes").insert(
        code.clone(),
        Pending {
            nonce: get("nonce"),
            challenge: get("code_challenge"),
            redirect_uri: redirect_uri.clone(),
        },
    );
    let sep = if redirect_uri.contains('?') { '&' } else { '?' };
    Redirect::to(&format!(
        "{redirect_uri}{sep}code={}&state={}",
        enc(&code),
        enc(&get("state"))
    ))
    .into_response()
}

fn oauth_error(code: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": code}))).into_response()
}

fn basic_credentials(h: &axum::http::HeaderMap) -> Option<(String, String)> {
    let v = h.get(header::AUTHORIZATION)?.to_str().ok()?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(v.strip_prefix("Basic ")?)
        .ok()?;
    let raw = String::from_utf8(raw).ok()?;
    let (id, secret) = raw.split_once(':')?;
    let dec = |x: &str| {
        percent_encoding::percent_decode_str(x)
            .decode_utf8_lossy()
            .into_owned()
    };
    Some((dec(id), dec(secret)))
}

async fn token(
    State(s): State<Arc<Inner>>,
    headers: axum::http::HeaderMap,
    Form(f): Form<HashMap<String, String>>,
) -> Response {
    let (id, secret) = basic_credentials(&headers).unwrap_or_else(|| {
        (
            f.get("client_id").cloned().unwrap_or_default(),
            f.get("client_secret").cloned().unwrap_or_default(),
        )
    });
    if id != CLIENT_ID || secret != CLIENT_SECRET {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_client"})),
        )
            .into_response();
    }
    if f.get("grant_type").map(String::as_str) != Some("authorization_code") {
        return oauth_error("unsupported_grant_type");
    }
    let Some(pending) = f
        .get("code")
        .and_then(|c| s.codes.lock().expect("codes").remove(c))
    else {
        return oauth_error("invalid_grant");
    };
    let verifier = f.get("code_verifier").cloned().unwrap_or_default();
    let computed = B64.encode(Sha256::digest(verifier.as_bytes()));
    if computed != pending.challenge
        || f.get("redirect_uri").map(String::as_str) != Some(pending.redirect_uri.as_str())
    {
        return oauth_error("invalid_grant");
    }
    let c = s.claims.lock().expect("claims").clone();
    let now = chrono_now();
    let mut payload = json!({
        "iss": s.issuer, "aud": CLIENT_ID, "sub": c.sub,
        "iat": now, "exp": now + c.exp_in,
        "nonce": c.nonce.clone().unwrap_or(pending.nonce),
    });
    if let Some(v) = &c.email {
        payload["email"] = json!(v);
    }
    if let Some(v) = c.email_verified {
        payload["email_verified"] = json!(v);
    }
    if let Some(v) = &c.name {
        payload["name"] = json!(v);
    }
    let head = json!({"alg": "ES256", "typ": "JWT", "kid": KID});
    let signing_input = format!(
        "{}.{}",
        B64.encode(head.to_string()),
        B64.encode(payload.to_string())
    );
    let sig = s
        .key
        .sign(&SystemRandom::new(), signing_input.as_bytes())
        .expect("sign");
    let id_token = format!("{signing_input}.{}", B64.encode(sig.as_ref()));
    Json(json!({
        "access_token": random_code(),
        "token_type": "Bearer",
        "expires_in": 300,
        "id_token": id_token,
    }))
    .into_response()
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}
