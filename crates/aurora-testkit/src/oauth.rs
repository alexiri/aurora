//! In-process mock OAuth2 provider for connect-flow tests (BE-043): auto-approving
//! `/authorize` (302 to `redirect_uri` with `code` + `state`), `/token` with PKCE S256 check
//! and refresh, and a record of received token requests.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Default)]
struct Inner {
    /// code -> (code_challenge, redirect_uri)
    codes: HashMap<String, (Option<String>, String)>,
    /// Token request forms received, in order.
    requests: Vec<HashMap<String, String>>,
    /// Extra fields merged into token responses (e.g. `scope`).
    extra: serde_json::Map<String, Value>,
    n: u32,
}

#[derive(Clone)]
pub struct MockOAuth {
    inner: Arc<Mutex<Inner>>,
    base: String,
}

impl MockOAuth {
    pub async fn start() -> Self {
        let inner = Arc::new(Mutex::new(Inner::default()));
        let app = Router::new()
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route("/me", get(me))
            .with_state(inner.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        Self { inner, base }
    }

    /// `http://127.0.0.1:PORT`.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Fields (e.g. `scope`, `expires_in`) merged into every token response.
    pub fn set_extra(&self, extra: Value) {
        self.inner.lock().unwrap().extra = extra.as_object().cloned().unwrap_or_default();
    }

    /// Token requests received so far.
    pub fn token_requests(&self) -> Vec<HashMap<String, String>> {
        self.inner.lock().unwrap().requests.clone()
    }

    /// Drive `/authorize` for a provider URL's query string; returns the redirect target.
    pub async fn approve(&self, authorize_url: &str) -> String {
        let q = authorize_url.split_once('?').map_or("", |(_, q)| q);
        let c = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let r = c
            .get(format!("{}/authorize?{q}", self.base))
            .send()
            .await
            .unwrap();
        r.headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }
}

async fn authorize(
    State(s): State<Arc<Mutex<Inner>>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let (Some(ru), Some(state)) = (q.get("redirect_uri"), q.get("state")) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mut g = s.lock().unwrap();
    g.n += 1;
    let code = format!("code-{}", g.n);
    g.codes
        .insert(code.clone(), (q.get("code_challenge").cloned(), ru.clone()));
    let sep = if ru.contains('?') { '&' } else { '?' };
    let enc = |v: &str| {
        percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC).to_string()
    };
    Redirect::to(&format!("{ru}{sep}code={code}&state={}", enc(state))).into_response()
}

async fn token(
    State(s): State<Arc<Mutex<Inner>>>,
    Form(f): Form<HashMap<String, String>>,
) -> Response {
    let mut g = s.lock().unwrap();
    g.requests.push(f.clone());
    let err = |e: &str| (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
    let mut body = match f.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let Some((chal, ru)) = f.get("code").and_then(|c| g.codes.remove(c)) else {
                return err("invalid_grant");
            };
            if f.get("redirect_uri") != Some(&ru) {
                return err("invalid_grant");
            }
            if let Some(ch) = chal {
                let v = f.get("code_verifier").map_or("", String::as_str);
                if B64.encode(Sha256::digest(v.as_bytes())) != ch {
                    return err("invalid_grant");
                }
            }
            json!({"access_token": "at-1", "refresh_token": "rt-1", "expires_in": 3600, "token_type": "Bearer"})
        }
        Some("refresh_token") if f.get("refresh_token").map(String::as_str) == Some("rt-1") => {
            json!({"access_token": "at-2", "expires_in": 3600, "token_type": "Bearer"})
        }
        _ => return err("invalid_grant"),
    };
    for (k, v) in &g.extra {
        body[k] = v.clone();
    }
    Json(body).into_response()
}

/// Probe upstream: `Bearer bad` => 403, anything else => 200 with an account document.
async fn me(headers: axum::http::HeaderMap) -> Response {
    let a = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if a == "Bearer bad" {
        return StatusCode::FORBIDDEN.into_response();
    }
    Json(json!({"email": "me@acme.io", "org": {"name": "Acme"}, "auth": a})).into_response()
}
