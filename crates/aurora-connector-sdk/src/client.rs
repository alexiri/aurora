//! HTTP client for `/v1/connector/*` with retry and exponential backoff.

use std::sync::RwLock;
use std::time::Duration;

use aurora_core::limits;
use aurora_protocol::common::{Validate, ValidationError};
use aurora_protocol::connector::*;
use aurora_protocol::version::HEADER as PROTOCOL_HEADER;
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("core answered {status} {code}: {message}")]
    Status {
        status: u16,
        code: String,
        message: String,
    },
    #[error("undecodable response: {0}")]
    Decode(String),
    #[error("invalid request: {0}")]
    Invalid(#[from] ValidationError),
}

impl ClientError {
    /// Worth retrying: transport failures, 408, 425, 429 and 5xx.
    pub fn is_retryable(&self) -> bool {
        match self {
            ClientError::Transport(_) => true,
            ClientError::Status { status, .. } => {
                matches!(*status, 408 | 425 | 429) || *status >= 500
            }
            _ => false,
        }
    }

    /// The token is not (or no longer) accepted.
    pub fn is_auth(&self) -> bool {
        matches!(
            self,
            ClientError::Status {
                status: 401 | 403,
                ..
            }
        )
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            ClientError::Status { status, .. } => Some(*status),
            _ => None,
        }
    }
}

/// Retry policy: `max_attempts` tries, delay doubling from `initial` up to `max`.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial: Duration,
    pub max: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            initial: limits::OP_BACKOFF_INITIAL,
            max: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// Delay before retry number `n` (0-based).
    pub fn delay(&self, n: u32) -> Duration {
        let factor = 1u32.checked_shl(n.min(20)).unwrap_or(u32::MAX);
        self.initial.saturating_mul(factor).min(self.max)
    }
}

/// Outcome of `GET /config`.
#[derive(Debug)]
pub enum ConfigPoll {
    NotModified,
    Changed(Box<ConfigResponse>),
}

pub struct CoreClient {
    http: reqwest::Client,
    base: String,
    token: String,
    replica: RwLock<Option<Uuid>>,
    retry: RetryPolicy,
}

impl CoreClient {
    /// `base` is the API origin (without `/v1/connector`).
    pub fn new(base: &str, token: &str, retry: RetryPolicy) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            replica: RwLock::new(None),
            retry,
        })
    }

    pub fn retry_policy(&self) -> RetryPolicy {
        self.retry
    }

    pub fn replica_id(&self) -> Option<Uuid> {
        self.replica.read().ok().and_then(|g| *g)
    }

    pub fn set_replica(&self, id: Uuid) {
        if let Ok(mut g) = self.replica.write() {
            *g = Some(id);
        }
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let mut rb = self
            .http
            .request(method, format!("{}/v1/connector{path}", self.base))
            .bearer_auth(&self.token)
            .header(PROTOCOL_HEADER, PROTOCOL_HEADER_VALUE);
        if let Some(r) = self.replica_id() {
            rb = rb.header(HDR_REPLICA, r.to_string());
        }
        rb
    }

    async fn into_error(resp: reqwest::Response) -> ClientError {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        // The core wraps errors in the API envelope `{"error": {code, message}}`; accept a bare
        // `{code, message}` as well.
        #[derive(serde::Deserialize)]
        struct Envelope {
            error: ErrorBody,
        }
        let parsed: Option<ErrorBody> = serde_json::from_str::<Envelope>(&body)
            .map(|e| e.error)
            .or_else(|_| serde_json::from_str::<ErrorBody>(&body))
            .ok();
        let (code, message) = match parsed {
            Some(b) => (b.code.as_str().to_owned(), b.message),
            None => (String::new(), body.chars().take(200).collect()),
        };
        ClientError::Status {
            status,
            code,
            message,
        }
    }

    /// One attempt with a per-request timeout.
    async fn once<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        timeout: Duration,
    ) -> Result<reqwest::Response, ClientError> {
        let mut rb = self.request(method, path).timeout(timeout);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let resp = rb
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        if resp.status().is_success() || resp.status() == StatusCode::NOT_MODIFIED {
            Ok(resp)
        } else {
            Err(Self::into_error(resp).await)
        }
    }

    async fn call<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        timeout: Duration,
    ) -> Result<reqwest::Response, ClientError> {
        let mut n = 0;
        loop {
            match self.once(method.clone(), path, body, timeout).await {
                Err(e) if e.is_retryable() && n + 1 < self.retry.max_attempts => {
                    tracing::warn!(error = %e, path, attempt = n + 1, "connector call failed; retrying");
                    tokio::time::sleep(self.retry.delay(n)).await;
                    n += 1;
                }
                other => return other,
            }
        }
    }

    async fn json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        timeout: Duration,
    ) -> Result<T, ClientError> {
        let resp = self.call(method, path, body, timeout).await?;
        resp.json::<T>()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))
    }

    const SHORT: Duration = Duration::from_secs(30);

    /// `POST /register`; stores the returned replica id for later calls.
    pub async fn register(&self, req: &RegisterRequest) -> Result<RegisterResponse, ClientError> {
        req.validate()?;
        let r: RegisterResponse = self
            .json(Method::POST, "/register", Some(req), Self::SHORT)
            .await?;
        self.set_replica(r.replica_id);
        Ok(r)
    }

    /// `GET /config` with `If-None-Match`.
    pub async fn config(&self, etag: Option<&str>) -> Result<ConfigPoll, ClientError> {
        let mut n = 0;
        loop {
            let mut rb = self.request(Method::GET, "/config").timeout(Self::SHORT);
            if let Some(e) = etag {
                rb = rb.header("if-none-match", e);
            }
            let res = match rb.send().await {
                Ok(resp) if resp.status() == StatusCode::NOT_MODIFIED => {
                    return Ok(ConfigPoll::NotModified);
                }
                Ok(resp) if resp.status().is_success() => resp
                    .json::<ConfigResponse>()
                    .await
                    .map(|c| ConfigPoll::Changed(Box::new(c)))
                    .map_err(|e| ClientError::Decode(e.to_string())),
                Ok(resp) => Err(Self::into_error(resp).await),
                Err(e) => Err(ClientError::Transport(e.to_string())),
            };
            match res {
                Err(e) if e.is_retryable() && n + 1 < self.retry.max_attempts => {
                    tokio::time::sleep(self.retry.delay(n)).await;
                    n += 1;
                }
                other => return other,
            }
        }
    }

    /// `POST /events`. Retries the same batch (event ids are the idempotency keys).
    pub async fn post_events(&self, req: &EventsRequest) -> Result<EventsResponse, ClientError> {
        req.validate()?;
        self.json(Method::POST, "/events", Some(req), Self::SHORT)
            .await
    }

    /// `POST /ops/claim` (long poll).
    pub async fn claim_ops(&self, req: &ClaimRequest) -> Result<ClaimResponse, ClientError> {
        req.validate()?;
        let timeout = Duration::from_secs(u64::from(req.wait_secs) + 15);
        self.json(Method::POST, "/ops/claim", Some(req), timeout)
            .await
    }

    /// `POST /ops/results`.
    pub async fn post_results(&self, req: &ResultsRequest) -> Result<(), ClientError> {
        req.validate()?;
        let _: OkResponse = self
            .json(Method::POST, "/ops/results", Some(req), Self::SHORT)
            .await?;
        Ok(())
    }

    /// `POST /credentials` (compare-and-set); a 409 `stale_version` is `ClientError::Status`.
    pub async fn write_credentials(
        &self,
        req: &CredentialsRequest,
    ) -> Result<CredentialsResponse, ClientError> {
        req.validate()?;
        self.json(Method::POST, "/credentials", Some(req), Self::SHORT)
            .await
    }

    /// `POST /link-start`.
    pub async fn link_start(
        &self,
        req: &LinkStartRequest,
    ) -> Result<LinkStartResponse, ClientError> {
        self.json(Method::POST, "/link-start", Some(req), Self::SHORT)
            .await
    }

    /// `PUT /files/{op_id}`: uploads the bytes of a fetched inbound file.
    pub async fn put_file(
        &self,
        op_id: Uuid,
        filename: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<FilePutResponse, ClientError> {
        let mut n = 0;
        loop {
            let resp = self
                .request(Method::PUT, &format!("/files/{op_id}"))
                .timeout(Duration::from_secs(120))
                .header("content-type", content_type)
                .header(HDR_FILENAME, filename)
                .body(bytes.clone())
                .send()
                .await
                .map_err(|e| ClientError::Transport(e.to_string()));
            let res = match resp {
                Ok(r) if r.status().is_success() => r
                    .json::<FilePutResponse>()
                    .await
                    .map_err(|e| ClientError::Decode(e.to_string())),
                Ok(r) => Err(Self::into_error(r).await),
                Err(e) => Err(e),
            };
            match res {
                Err(e) if e.is_retryable() && n + 1 < self.retry.max_attempts => {
                    tokio::time::sleep(self.retry.delay(n)).await;
                    n += 1;
                }
                other => return other,
            }
        }
    }

    /// `GET /files/{attachment_id}`: bytes of an outbound attachment.
    pub async fn get_file(&self, attachment_id: Uuid) -> Result<bytes::Bytes, ClientError> {
        let resp = self
            .call::<()>(
                Method::GET,
                &format!("/files/{attachment_id}"),
                None,
                Duration::from_secs(120),
            )
            .await?;
        resp.bytes()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))
    }

    /// `POST /retire`: clean shutdown; releases owned integrations.
    pub async fn retire(&self) -> Result<(), ClientError> {
        self.call::<()>(Method::POST, "/retire", None, Self::SHORT)
            .await
            .map(|_| ())
    }
}
