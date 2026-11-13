//! In-process mock OpenID Provider for sign-in tests (BE-015).
//!
//! Serves discovery, JWKS (ES256 key generated per instance), an auto-approving
//! authorize endpoint (302 to `redirect_uri` with `code` + `state`) and a token
//! endpoint that verifies the PKCE S256 verifier and returns an ID token whose
//! claims are configurable through [`MockIdp::set_claims`] / [`MockIdp::set`].
//!
//! Phase 1 (RFE-004) the same code is the development IdP (`aurora-dev-idp`, `src/bin`): with
//! [`Options::users`] it serves a login page listing fixture users and signs a user in
//! non-interactively when the authorize request carries `login_hint=<email>`.
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

/// The fixed development client of the `dev` provider (`aurora_config::DEV_IDP_CLIENT_*`; the
/// aurora-api suite asserts the two stay equal).
pub const CLIENT_ID: &str = "aurora-dev-client";
pub const CLIENT_SECRET: &str = "aurora-dev-secret";
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
    /// `aud` claim; `None` = the client id.
    pub aud: Option<String>,
    /// Seconds ago the user authenticated. `Some(n)` always emits `auth_time = now - n`; `None`
    /// emits `auth_time = now` only when the authorize request carried `max_age`.
    pub auth_time_ago: Option<i64>,
    /// (FIX-108) Never emit `auth_time` (an IdP that ignores `max_age`); wins over the above.
    pub omit_auth_time: bool,
    /// (P4-IDP-1) Extra claims placed in the ID token, e.g. `{"cern_roles": ["aurora-admins"]}`
    /// (the configurable roles claim). A nested path is written as nested JSON.
    pub extra: serde_json::Map<String, Value>,
    /// (P4-IDP-1) Claims served **only** by `/userinfo` (for `role_claim_source = userinfo`).
    pub userinfo_extra: serde_json::Map<String, Value>,
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
            aud: None,
            auth_time_ago: None,
            omit_auth_time: false,
            extra: serde_json::Map::new(),
            userinfo_extra: serde_json::Map::new(),
        }
    }
}

struct Pending {
    nonce: String,
    challenge: String,
    redirect_uri: String,
    /// The fixture user chosen through `login_hint` (overrides the configured claims).
    user: Option<DevUser>,
    max_age_requested: bool,
}

/// A fixture user of the development IdP (`deploy/dev/dev-idp-users.json`).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DevUser {
    pub sub: String,
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "yes")]
    pub email_verified: bool,
    /// (P4-IDP-1) Extra ID-token claims of this fixture user, e.g. the roles claim.
    #[serde(default)]
    pub extra: serde_json::Map<String, Value>,
}

fn yes() -> bool {
    true
}

/// Startup options. `Options::default()` is the in-process test mock.
#[derive(Debug, Clone)]
pub struct Options {
    /// Listen address (default `127.0.0.1:0`).
    pub bind: String,
    /// Public issuer URL as the browser reaches it (default `http://<bound address>`).
    pub issuer: Option<String>,
    /// URL path the IdP is served under (e.g. `/dev-idp` behind an ingress); default none.
    pub path_prefix: String,
    /// Fixture users. `Some`: only listed users sign in and the authorize endpoint without a
    /// `login_hint` shows a login page. `None`: the mock auto-approves with its configured claims.
    pub users: Option<Vec<DevUser>>,
    /// With `users`, also accept an unlisted `login_hint` (e2e harnesses mint their own e-mails).
    pub allow_any_hint: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:0".into(),
            issuer: None,
            path_prefix: String::new(),
            users: None,
            allow_any_hint: false,
        }
    }
}

struct Inner {
    issuer: String,
    key: EcdsaKeyPair,
    claims: Mutex<Claims>,
    codes: Mutex<HashMap<String, Pending>>,
    users: Option<Vec<DevUser>>,
    allow_any_hint: bool,
    last_authorize: Mutex<HashMap<String, String>>,
    /// Access token → the claims `/userinfo` returns for it.
    userinfo: Mutex<HashMap<String, Value>>,
    /// (P4-IDP-1) Per-e-mail (lower case) extra ID-token claims set at run time through
    /// `PUT /dev/claims/{email}` or [`MockIdp::set_user_claims`]; they override the fixture's.
    user_claims: Mutex<HashMap<String, serde_json::Map<String, Value>>>,
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
        Self::start_with(Options::default()).await
    }

    /// Start with explicit [`Options`].
    pub async fn start_with(opts: Options) -> MockIdp {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .expect("generate ES256 key");
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref())
            .expect("load ES256 key");
        let listener = tokio::net::TcpListener::bind(&opts.bind)
            .await
            .expect("bind mock idp");
        let prefix = opts.path_prefix.trim_end_matches('/').to_owned();
        let issuer = opts.issuer.as_deref().map_or_else(
            || format!("http://{}{prefix}", listener.local_addr().expect("addr")),
            |i| i.trim_end_matches('/').to_owned(),
        );
        let inner = Arc::new(Inner {
            issuer,
            key,
            claims: Mutex::new(Claims::default()),
            codes: Mutex::new(HashMap::new()),
            users: opts.users,
            allow_any_hint: opts.allow_any_hint,
            last_authorize: Mutex::new(HashMap::new()),
            userinfo: Mutex::new(HashMap::new()),
            user_claims: Mutex::new(HashMap::new()),
        });
        let routes = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/userinfo", get(userinfo))
            .route("/dev/claims/{email}", axum::routing::put(put_user_claims))
            .route("/jwks", get(jwks))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .with_state(inner.clone());
        let app = if prefix.is_empty() {
            routes
        } else {
            Router::new().nest(&prefix, routes)
        };
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

    /// The query of the most recent `/authorize` request (e.g. to assert `prompt=login&max_age=0`).
    pub fn last_authorize(&self) -> HashMap<String, String> {
        self.inner.last_authorize.lock().expect("authorize").clone()
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
    /// Set the extra ID-token claims of the user with this e-mail (replaces earlier ones).
    pub fn set_user_claims(&self, email: &str, claims: serde_json::Map<String, Value>) {
        self.inner
            .user_claims
            .lock()
            .expect("user claims")
            .insert(email.to_ascii_lowercase(), claims);
    }

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
        "userinfo_endpoint": format!("{i}/userinfo"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["ES256"],
        "token_endpoint_auth_methods_supported": ["client_secret_post", "client_secret_basic"],
        "code_challenge_methods_supported": ["S256"],
        "scopes_supported": ["openid", "email", "profile"],
        "claims_supported": ["sub", "email", "email_verified", "name", "nonce", "auth_time"],
    }))
}

/// `/userinfo`: the standard claims plus `userinfo_extra`, for a Bearer token this IdP issued.
async fn userinfo(State(s): State<Arc<Inner>>, headers: axum::http::HeaderMap) -> Response {
    let tok = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    match s.userinfo.lock().expect("userinfo").get(tok) {
        Some(v) => Json(v.clone()).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// Dev/e2e only: `PUT /dev/claims/{email}` with a JSON object sets that user's extra ID-token
/// claims for the next sign-ins (e.g. `{"cern_roles": ["aurora-admins"]}`).
async fn put_user_claims(
    State(s): State<Arc<Inner>>,
    axum::extract::Path(email): axum::extract::Path<String>,
    Json(claims): Json<serde_json::Map<String, Value>>,
) -> StatusCode {
    s.user_claims
        .lock()
        .expect("user claims")
        .insert(email.to_ascii_lowercase(), claims);
    StatusCode::NO_CONTENT
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

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn login_page(users: &[DevUser], q: &HashMap<String, String>) -> Response {
    let base = |email: &str| {
        let mut pairs: Vec<String> = q
            .iter()
            .filter(|(k, _)| k.as_str() != "login_hint")
            .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
            .collect();
        pairs.push(format!("login_hint={}", enc(email)));
        format!("authorize?{}", pairs.join("&"))
    };
    let mut items = String::new();
    for u in users {
        items.push_str(&format!(
            "<li><a href=\"{}\">{} <small>{}</small></a></li>",
            html_escape(&base(&u.email)),
            html_escape(u.name.as_deref().unwrap_or(&u.email)),
            html_escape(&u.email),
        ));
    }
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Aurora development sign-in</title>\
         <style>body{{font:16px system-ui;max-width:28rem;margin:4rem auto;padding:0 1rem}}li{{margin:.5rem 0}}small{{color:#666}}</style></head>\
         <body><h1>Development sign-in</h1><p>Development only. Pick a user:</p><ul>{items}</ul></body></html>"
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
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
    *s.last_authorize.lock().expect("authorize") = q.clone();
    let hint = get("login_hint");
    let user = if hint.is_empty() {
        if let Some(users) = &s.users {
            return login_page(users, &q);
        }
        None
    } else {
        let listed = s
            .users
            .as_ref()
            .and_then(|us| us.iter().find(|u| u.email.eq_ignore_ascii_case(&hint)))
            .cloned();
        match (listed, &s.users) {
            (Some(u), _) => Some(u),
            (None, Some(_)) if !s.allow_any_hint => {
                return (StatusCode::FORBIDDEN, "unknown development user").into_response();
            }
            // In-process mock (no fixture list) or `allow_any_hint`: mint a user from the hint.
            (None, _) => Some(DevUser {
                sub: format!("dev-{}", hint.to_ascii_lowercase()),
                email: hint.clone(),
                name: None,
                email_verified: true,
                extra: serde_json::Map::new(),
            }),
        }
    };
    let code = random_code();
    let redirect_uri = get("redirect_uri");
    s.codes.lock().expect("codes").insert(
        code.clone(),
        Pending {
            nonce: get("nonce"),
            challenge: get("code_challenge"),
            redirect_uri: redirect_uri.clone(),
            user,
            max_age_requested: q.contains_key("max_age"),
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
    let mut c = s.claims.lock().expect("claims").clone();
    if let Some(u) = &pending.user {
        c.sub = u.sub.clone();
        c.email = Some(u.email.clone());
        c.email_verified = Some(u.email_verified);
        c.name = u.name.clone().or_else(|| Some(u.email.clone()));
    }
    let now = chrono_now();
    let mut payload = json!({
        "iss": s.issuer, "aud": c.aud.clone().unwrap_or_else(|| CLIENT_ID.to_owned()), "sub": c.sub,
        "iat": now, "exp": now + c.exp_in,
        "nonce": c.nonce.clone().unwrap_or(pending.nonce),
    });
    if !c.omit_auth_time {
        if let Some(ago) = c.auth_time_ago {
            payload["auth_time"] = json!(now - ago);
        } else if pending.max_age_requested {
            payload["auth_time"] = json!(now);
        }
    }
    if let Some(v) = &c.email {
        payload["email"] = json!(v);
    }
    if let Some(v) = c.email_verified {
        payload["email_verified"] = json!(v);
    }
    if let Some(v) = &c.name {
        payload["name"] = json!(v);
    }
    let mut extra = c.extra.clone();
    if let Some(u) = &pending.user {
        extra.extend(u.extra.clone());
    }
    if let Some(e) = &c.email
        && let Some(per_user) = s
            .user_claims
            .lock()
            .expect("user claims")
            .get(&e.to_ascii_lowercase())
    {
        extra.extend(per_user.clone());
    }
    for (k, v) in &extra {
        // Registered claims stay the IdP's own.
        if !matches!(k.as_str(), "iss" | "aud" | "sub" | "exp" | "iat" | "nonce") {
            payload[k] = v.clone();
        }
    }
    let access_token = random_code();
    let mut info = json!({"sub": c.sub});
    if let Some(v) = &c.email {
        info["email"] = json!(v);
    }
    if let Some(v) = c.email_verified {
        info["email_verified"] = json!(v);
    }
    for (k, v) in c.userinfo_extra.iter().chain(extra.iter()) {
        if k != "sub" {
            info[k] = v.clone();
        }
    }
    s.userinfo
        .lock()
        .expect("userinfo")
        .insert(access_token.clone(), info);
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
        "access_token": access_token,
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

/// Sign in through a running Aurora API over HTTP using the development IdP's `login_hint`
/// (phase 1, RFE-004; the e2e suites use this instead of the removed password sign-up).
///
/// Drives the real flow with redirects followed by hand: `GET {base}/v1/auth/oidc/{provider}/start`
/// → the IdP's authorize URL (with `login_hint`) → the callback on `{base}` with the
/// `__Host-aurora_oidc` flow cookie. Returns the `Cookie` header value (`aurora_session=aus_…`).
///
/// `base` is where the *test* reaches the API (it may differ from the API's `server.external_url`;
/// the callback path is replayed against `base`). The IdP URL is used exactly as the API returned it.
pub async fn sign_in_over_http(base: &str, provider: &str, email: &str) -> Result<String, String> {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let base = base.trim_end_matches('/');
    let start = format!(
        "{base}/v1/auth/oidc/{provider}/start?login_hint={}",
        enc(email)
    );
    let r = http.get(&start).send().await.map_err(|e| e.to_string())?;
    if r.status().as_u16() != 302 {
        return Err(format!(
            "start: {} {}",
            r.status(),
            r.text().await.unwrap_or_default()
        ));
    }
    let flow = r
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("__Host-aurora_oidc="))
        .and_then(|c| c.split(';').next())
        .map(str::to_owned)
        .ok_or("start: no __Host-aurora_oidc cookie")?;
    let authorize = r
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or("start: no location")?
        .to_owned();
    let r = http
        .get(&authorize)
        .send()
        .await
        .map_err(|e| format!("authorize {authorize}: {e}"))?;
    if !r.status().is_redirection() {
        return Err(format!(
            "authorize: {} {}",
            r.status(),
            r.text().await.unwrap_or_default()
        ));
    }
    let back = r
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or("authorize: no location")?
        .to_owned();
    let path = back
        .find("/v1/auth/oidc/")
        .map(|i| &back[i..])
        .ok_or_else(|| format!("authorize: unexpected redirect {back}"))?;
    let r = http
        .get(format!("{base}{path}"))
        .header("cookie", &flow)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let loc = r
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    r.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("aurora_session=aus_"))
        .and_then(|c| c.split(';').next())
        .map(str::to_owned)
        .ok_or_else(|| format!("callback refused: {loc}"))
}
