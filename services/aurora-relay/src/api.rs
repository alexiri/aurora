//! Client of the relay plane `/v1/relay/*` (`16 §W17`), authenticated with `aul_…`.
//!
//! One short-lived JSON client; every call carries `Authorization: Bearer aul_…` and, after
//! registration, `X-Aurora-Replica` + `X-Aurora-Protocol: relay/1`. Failures of `resolve` are
//! answers (the relay renders the decision), failures of the rest are retried by the callers.

use std::time::Duration;

use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("the API answered {0}")]
    Status(u16),
    #[error("undecodable response: {0}")]
    Decode(String),
}

impl ApiError {
    /// The anchor token is not accepted: re-register (which re-authenticates).
    pub fn is_auth(&self) -> bool {
        matches!(self, ApiError::Status(401 | 403))
    }
}

const SHORT: Duration = Duration::from_secs(20);

pub struct RelayApi {
    http: reqwest::Client,
    base: String,
    token: String,
    replica: std::sync::Mutex<Option<Uuid>>,
}

/// `POST /v1/relay/register` answer (`16 §W17`).
#[derive(Debug, serde::Deserialize)]
pub struct RegisterResponse {
    pub replica_id: Uuid,
    pub protocol: u32,
    pub config: RegisterConfig,
}

#[derive(Debug, serde::Deserialize)]
pub struct RegisterConfig {
    pub previews_domain: String,
    pub external_url: String,
    pub auth_cache_secs: u32,
    pub wake_wait_secs: u32,
}

/// `POST /v1/relay/resolve` request/answer (`16 §W17`).
#[derive(Debug, serde::Deserialize)]
pub struct ResolveAnswer {
    pub decision: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub share: Option<ShareRef>,
    #[serde(default)]
    pub route: Option<Route>,
    /// The session's user (relay-plane only; keys the per-user stream cap). Never rendered
    /// to the browser.
    #[serde(default)]
    pub user: Option<Uuid>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ShareRef {
    pub id: Uuid,
    pub port: i32,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Route {
    pub sandbox: String,
    pub listening: bool,
    pub start: String,
    #[serde(default)]
    pub runner_endpoint: Option<String>,
    #[serde(default)]
    pub retry_after_secs: Option<u32>,
}

/// `POST /v1/relay/grants/redeem`.
#[derive(Debug, serde::Deserialize)]
pub struct RedeemAnswer {
    pub token: String,
    pub expires_at: String,
    pub idle_secs: u32,
}

/// `POST /v1/relay/streams`.
#[derive(Debug, serde::Deserialize)]
pub struct StreamsAnswer {
    pub grant: String,
    pub expires_in: u32,
    pub runner_endpoint: String,
}

/// `POST /v1/relay/report` entry.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReportShare {
    pub share_id: Uuid,
    #[serde(default)]
    pub streams_open: i32,
    #[serde(default)]
    pub streams_total: u64,
    #[serde(default)]
    pub bytes_to_app: u64,
    #[serde(default)]
    pub bytes_from_app: u64,
    #[serde(default)]
    pub last_request_at: Option<String>,
}

/// A live `(label, session_hash)` pair of open streams (the heartbeat's validate input).
pub type LivePair = (String, String);

/// SHA-256 of the preview cookie token as `hex` — never the token itself (`16 §W17`).
pub fn session_hash(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    let d = h.finalize();
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

impl RelayApi {
    pub fn new(base: &str, token: &str) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            replica: std::sync::Mutex::new(None),
        })
    }

    pub fn replica(&self) -> Option<Uuid> {
        *self.replica.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1/relay{path}", self.base)
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        let b = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.token)
            .timeout(SHORT)
            .header("x-aurora-protocol", "relay/1");
        match self.replica() {
            Some(r) => b.header("x-aurora-replica", r.to_string()),
            None => b,
        }
    }

    async fn send<T: DeserializeOwned>(rb: reqwest::RequestBuilder) -> Result<T, ApiError> {
        let resp = rb
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.without_url().to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ApiError::Status(status.as_u16()));
        }
        resp.json::<T>()
            .await
            .map_err(|e| ApiError::Decode(e.without_url().to_string()))
    }

    /// Registers (or re-registers after an auth failure); sets the replica id on success.
    pub async fn register(&self, version: &str) -> Result<RegisterResponse, ApiError> {
        let r: RegisterResponse = Self::send(
            self.post("/register")
                .json(&serde_json::json!({ "version": version })),
        )
        .await?;
        *self.replica.lock().unwrap_or_else(|p| p.into_inner()) = Some(r.replica_id);
        Ok(r)
    }

    /// `resolve`; a long-poll only when `wait_secs > 0`.
    pub async fn resolve(
        &self,
        label: &str,
        session_hash: Option<&str>,
        wait_secs: u32,
        navigation: bool,
    ) -> Result<ResolveAnswer, ApiError> {
        let timeout = Duration::from_secs(u64::from(wait_secs) + 15);
        let mut body = serde_json::json!({ "label": label, "navigation": navigation });
        if let Some(h) = session_hash {
            body["session_hash"] = serde_json::json!(h);
        }
        if wait_secs > 0 {
            body["wait_secs"] = serde_json::json!(wait_secs);
        }
        let resp = self
            .post("/resolve")
            .timeout(timeout)
            .json(&body)
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.without_url().to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ApiError::Status(status.as_u16()));
        }
        resp.json::<ResolveAnswer>()
            .await
            .map_err(|e| ApiError::Decode(e.without_url().to_string()))
    }

    pub async fn redeem(
        &self,
        grant: &str,
        label: &str,
        nonce: &str,
    ) -> Result<RedeemAnswer, ApiError> {
        Self::send(
            self.post("/grants/redeem")
                .json(&serde_json::json!({ "grant": grant, "label": label, "nonce": nonce })),
        )
        .await
    }

    pub async fn streams(&self, label: &str, hash: &str) -> Result<StreamsAnswer, ApiError> {
        Self::send(
            self.post("/streams")
                .json(&serde_json::json!({ "label": label, "session_hash": hash })),
        )
        .await
    }

    /// Batch re-validation of open streams' credentials; the relay closes the returned
    /// entries with 4403 (`docs/04` step 5).
    pub async fn validate(
        &self,
        sessions: &[(String, String)],
    ) -> Result<Vec<(String, String)>, ApiError> {
        #[derive(serde::Deserialize)]
        struct Resp {
            #[serde(default)]
            revoked: Vec<Entry>,
        }
        #[derive(serde::Deserialize)]
        struct Entry {
            label: String,
            session_hash: String,
        }
        let body: Vec<_> = sessions
            .iter()
            .map(|(l, h)| serde_json::json!({ "label": l, "session_hash": h }))
            .collect();
        let r: Resp = Self::send(
            self.post("/validate")
                .json(&serde_json::json!({ "sessions": body })),
        )
        .await?;
        Ok(r.revoked
            .into_iter()
            .map(|e| (e.label, e.session_hash))
            .collect())
    }

    /// The 15 s heartbeat; refreshes last-seen and the awake stamps.
    pub async fn report(&self, shares: &[ReportShare]) -> Result<(), ApiError> {
        let resp = self
            .post("/report")
            .json(&serde_json::json!({ "shares": shares }))
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.without_url().to_string()))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ApiError::Status(resp.status().as_u16()))
        }
    }
}
