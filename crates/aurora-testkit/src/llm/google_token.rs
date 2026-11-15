//! In-process mock of Google's OAuth2 token endpoint (`POST /token`, JWT-bearer grant) and a generated test
//! service-account key. The mock verifies the assertion (RS256 against the key's public half; `iss`, `scope`, `aud`,
//! `iat`/`exp`) before it hands out a token, and can answer `invalid_grant`, `503` or after a delay.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rsa::{KeyPair, KeySize};
use aws_lc_rs::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde_json::{Value, json};

/// The pinned token URI a real key names, and the assertion's `aud`.
pub const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
pub const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// A throwaway service-account key (2048-bit RSA, generated per call).
#[derive(Clone)]
pub struct TestKey {
    pub client_email: String,
    pub project_id: String,
    pem: String,
    public_der: Vec<u8>,
}

impl TestKey {
    pub fn generate(project_id: &str, client_email: &str) -> Self {
        let key = KeyPair::generate(KeySize::Rsa2048).expect("rsa key");
        let der = AsDer::<aws_lc_rs::encoding::Pkcs8V1Der>::as_der(&key).expect("pkcs8");
        let b64 = STANDARD.encode(der.as_ref());
        let lines: Vec<String> = b64
            .as_bytes()
            .chunks(64)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        use aws_lc_rs::signature::KeyPair as _;
        Self {
            client_email: client_email.to_owned(),
            project_id: project_id.to_owned(),
            pem: format!(
                "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
                lines.join("\n")
            ),
            public_der: key.public_key().as_ref().to_vec(),
        }
    }

    /// The private key as a PEM block (a test greps logs for it).
    pub fn pem(&self) -> &str {
        &self.pem
    }

    /// The key file JSON as Google issues it (the secret's value).
    pub fn json(&self) -> String {
        self.json_with("service_account", TOKEN_URI)
    }

    pub fn json_with(&self, kind: &str, token_uri: &str) -> String {
        json!({
            "type": kind,
            "project_id": self.project_id,
            "private_key_id": "0123456789abcdef",
            "private_key": self.pem,
            "client_email": self.client_email,
            "client_id": "1234567890",
            "auth_uri": "https://accounts.google.com/o/oauth2/auth",
            "token_uri": token_uri,
            "auth_provider_x509_cert_url": "https://www.googleapis.com/oauth2/v1/certs",
            "client_x509_cert_url": "https://www.googleapis.com/robot/v1/metadata/x509/x",
            "universe_domain": "googleapis.com",
        })
        .to_string()
    }
}

/// What the mock answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    Ok,
    /// `400 {"error":"invalid_grant"}`.
    InvalidGrant,
    /// `503`.
    Unavailable,
}

struct State_ {
    key: TestKey,
    behaviour: Mutex<Behaviour>,
    delay: Mutex<Duration>,
    expires_in: AtomicU64,
    requests: AtomicUsize,
    issued: Mutex<Vec<String>>,
    /// Assertions that failed verification (reason).
    refused: Mutex<Vec<String>>,
}

#[derive(Clone)]
pub struct MockGoogleToken {
    st: Arc<State_>,
    base: String,
}

fn verify(st: &State_, assertion: &str) -> Result<(), String> {
    let mut parts = assertion.split('.');
    let (Some(h), Some(c), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("not a JWT".into());
    };
    let dec = |p: &str| {
        URL_SAFE_NO_PAD
            .decode(p)
            .map_err(|_| "bad base64url".to_string())
    };
    let header: Value = serde_json::from_slice(&dec(h)?).map_err(|_| "bad header")?;
    if header["alg"] != "RS256" {
        return Err("alg".into());
    }
    let sig = dec(s)?;
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, &st.key.public_der)
        .verify(format!("{h}.{c}").as_bytes(), &sig)
        .map_err(|_| "signature")?;
    let claims: Value = serde_json::from_slice(&dec(c)?).map_err(|_| "bad claims")?;
    if claims["iss"] != st.key.client_email.as_str() {
        return Err("iss".into());
    }
    if claims["scope"] != SCOPE {
        return Err("scope".into());
    }
    if claims["aud"] != TOKEN_URI {
        return Err("aud".into());
    }
    let (iat, exp) = (
        claims["iat"].as_u64().ok_or("iat")?,
        claims["exp"].as_u64().ok_or("exp")?,
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    if exp <= iat || exp - iat > 600 || iat > now + 60 || exp < now {
        return Err("lifetime".into());
    }
    Ok(())
}

async fn token(State(st): State<Arc<State_>>, body: String) -> Response {
    st.requests.fetch_add(1, Ordering::SeqCst);
    let delay = *st.delay.lock().unwrap();
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    let respond = |status: u16, v: Value| {
        Response::builder()
            .status(StatusCode::from_u16(status).unwrap())
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(v.to_string()))
            .unwrap()
    };
    match *st.behaviour.lock().unwrap() {
        Behaviour::InvalidGrant => {
            return respond(
                400,
                json!({"error": "invalid_grant", "error_description": "Invalid JWT Signature."}),
            );
        }
        Behaviour::Unavailable => return respond(503, json!({"error": "backendError"})),
        Behaviour::Ok => {}
    }
    let form: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();
    if form.get("grant_type").map(String::as_str)
        != Some("urn:ietf:params:oauth:grant-type:jwt-bearer")
    {
        return respond(400, json!({"error": "unsupported_grant_type"}));
    }
    if let Err(why) = verify(&st, form.get("assertion").map_or("", String::as_str)) {
        st.refused.lock().unwrap().push(why);
        return respond(400, json!({"error": "invalid_grant"}));
    }
    let n = st.issued.lock().unwrap().len() + 1;
    let tok = format!("ya29.mock-token-{n}");
    st.issued.lock().unwrap().push(tok.clone());
    respond(
        200,
        json!({"access_token": tok, "expires_in": st.expires_in.load(Ordering::SeqCst), "token_type": "Bearer"}),
    )
}

impl MockGoogleToken {
    pub async fn start(key: &TestKey) -> Self {
        let st = Arc::new(State_ {
            key: key.clone(),
            behaviour: Mutex::new(Behaviour::Ok),
            delay: Mutex::new(Duration::ZERO),
            expires_in: AtomicU64::new(3600),
            requests: AtomicUsize::new(0),
            issued: Mutex::new(vec![]),
            refused: Mutex::new(vec![]),
        });
        let app = Router::new()
            .route("/token", post(token))
            .with_state(st.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        Self { st, base }
    }

    /// `http://127.0.0.1:PORT`: the target of `llm.dev_upstream_overrides["https://oauth2.googleapis.com"]`.
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
    /// Requests received (all outcomes).
    pub fn requests(&self) -> usize {
        self.st.requests.load(Ordering::SeqCst)
    }
    /// Tokens handed out, oldest first.
    pub fn issued(&self) -> Vec<String> {
        self.st.issued.lock().unwrap().clone()
    }
    /// Why assertions were refused (empty when every assertion verified).
    pub fn refused(&self) -> Vec<String> {
        self.st.refused.lock().unwrap().clone()
    }
}
