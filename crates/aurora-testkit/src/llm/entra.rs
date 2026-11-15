//! In-process mock of the Microsoft Entra authority (`POST /<tenant>/oauth2/v2.0/token`, client-credentials grant).
//! It verifies what the real authority does (path tenant, `grant_type`, `client_id`, `client_secret`, `scope`) before it
//! issues an access token: an **unsigned** test JWT whose `tid` and `appid` claims name the tenant and the application.
//! It can answer `400 invalid_client`, `503`, or after a delay, and records every request body it received.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};

pub const SCOPE: &str = "https://cognitiveservices.azure.com/.default";
/// Every issued token starts with this, so a mock upstream can tell them from a key.
pub const TOKEN_PREFIX: &str = "eyJ";

/// The application the mock knows.
#[derive(Clone, Debug)]
pub struct TestApp {
    pub tenant_id: String,
    pub client_id: String,
    pub client_secret: String,
}

impl TestApp {
    /// The secret's value as the API takes it.
    pub fn json(&self) -> String {
        json!({"tenant_id": self.tenant_id, "client_id": self.client_id, "client_secret": self.client_secret})
            .to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    Ok,
    /// `401 {"error":"invalid_client"}` (a revoked secret or a deleted application).
    InvalidClient,
    /// `503`.
    Unavailable,
}

struct State_ {
    app: TestApp,
    behaviour: Mutex<Behaviour>,
    delay: Mutex<Duration>,
    expires_in: AtomicU64,
    requests: AtomicUsize,
    issued: Mutex<Vec<String>>,
    /// Requests that did not verify (reason).
    refused: Mutex<Vec<String>>,
    /// The `tid` claim to issue instead of the application's (a token for another tenant).
    tid_override: Mutex<Option<String>>,
}

#[derive(Clone)]
pub struct MockEntra {
    st: Arc<State_>,
    base: String,
}

fn respond(status: u16, v: &Value) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(v.to_string()))
        .unwrap()
}

async fn token(
    State(st): State<Arc<State_>>,
    Path(tenant): Path<String>,
    body: String,
) -> Response {
    st.requests.fetch_add(1, Ordering::SeqCst);
    let delay = *st.delay.lock().unwrap();
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    let behaviour = *st.behaviour.lock().unwrap();
    match behaviour {
        Behaviour::InvalidClient => {
            return respond(
                401,
                &json!({"error": "invalid_client", "error_description": "AADSTS7000215: Invalid client secret provided."}),
            );
        }
        Behaviour::Unavailable => {
            return respond(503, &json!({"error": "temporarily_unavailable"}));
        }
        Behaviour::Ok => {}
    }
    let form: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();
    let get = |k: &str| form.get(k).map_or("", String::as_str);
    let why = if form.get("grant_type").map(String::as_str) != Some("client_credentials") {
        Some("grant_type")
    } else if tenant != st.app.tenant_id {
        Some("tenant")
    } else if get("client_id") != st.app.client_id {
        Some("client_id")
    } else if get("client_secret") != st.app.client_secret {
        Some("client_secret")
    } else if get("scope") != SCOPE {
        Some("scope")
    } else {
        None
    };
    if let Some(why) = why {
        st.refused.lock().unwrap().push(why.to_owned());
        return respond(400, &json!({"error": "invalid_client"}));
    }
    let n = st.issued.lock().unwrap().len() + 1;
    let tid = st
        .tid_override
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| st.app.tenant_id.clone());
    let claims = json!({"tid": tid, "appid": st.app.client_id, "aud": "https://cognitiveservices.azure.com", "n": n});
    let tok = format!(
        "{}.{}.mock-signature-{n}",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    st.issued.lock().unwrap().push(tok.clone());
    respond(
        200,
        &json!({"access_token": tok, "expires_in": st.expires_in.load(Ordering::SeqCst), "token_type": "Bearer", "ext_expires_in": 3600}),
    )
}

impl MockEntra {
    pub async fn start(app: &TestApp) -> Self {
        let st = Arc::new(State_ {
            app: app.clone(),
            behaviour: Mutex::new(Behaviour::Ok),
            delay: Mutex::new(Duration::ZERO),
            expires_in: AtomicU64::new(3599),
            requests: AtomicUsize::new(0),
            issued: Mutex::new(vec![]),
            refused: Mutex::new(vec![]),
            tid_override: Mutex::new(None),
        });
        let router = Router::new()
            .route("/{tenant}/oauth2/v2.0/token", post(token))
            .with_state(st.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(l, router).await;
        });
        Self { st, base }
    }

    /// `http://127.0.0.1:PORT`: the target of `llm.dev_upstream_overrides["https://login.microsoftonline.com"]`.
    pub fn base(&self) -> &str {
        &self.base
    }
    pub fn set_behaviour(&self, b: Behaviour) {
        *self.st.behaviour.lock().unwrap() = b;
    }
    pub fn set_delay(&self, d: Duration) {
        *self.st.delay.lock().unwrap() = d;
    }
    pub fn set_expires_in(&self, secs: u64) {
        self.st.expires_in.store(secs, Ordering::SeqCst);
    }
    /// Issue tokens whose `tid` is `tid` (for the application's own tenant: `None`).
    pub fn set_tid(&self, tid: Option<&str>) {
        *self.st.tid_override.lock().unwrap() = tid.map(str::to_owned);
    }
    /// Requests received (all outcomes).
    pub fn requests(&self) -> usize {
        self.st.requests.load(Ordering::SeqCst)
    }
    /// Tokens handed out, oldest first.
    pub fn issued(&self) -> Vec<String> {
        self.st.issued.lock().unwrap().clone()
    }
    /// Why requests were refused (empty when every request verified).
    pub fn refused(&self) -> Vec<String> {
        self.st.refused.lock().unwrap().clone()
    }
}
