//! API client for the W5 runner plane (`16 §W5`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aurora_protocol::runner::{
    AttachmentUploadMeta, AttachmentUploadResponse, ChannelClaimRequest, ChannelClaimResponse,
    ChannelRef, HDR_BOOT, HDR_CAPTION, HDR_CONVERSATION, HDR_FILENAME, HDR_REPLICA, HDR_SANDBOX,
    HDR_TURN, MemoryWriteRequest, MemoryWriteResponse, PreviewStreamClosedRequest,
    PreviewStreamOpenRequest, PreviewStreamOpenResponse, RegisterRequest, RegisterResponse,
    ReportResult, ReportsRequest, ReportsResponse, SandboxAssignment, SandboxesCheckRequest,
    SandboxesCheckResponse, SandboxesResponse, StraysRequest, WorkRequest, WorkResponse,
};
use aurora_protocol::tools::{ToolCall, ToolResult};
use reqwest::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("api answered {status}: {body}")]
    Status { status: u16, body: String },
    #[error("undecodable response: {0}")]
    Decode(String),
}

impl ApiError {
    pub fn retryable(&self) -> bool {
        match self {
            Self::Transport(_) => true,
            Self::Status { status, .. } => *status >= 500 || matches!(*status, 408 | 429),
            Self::Decode(_) => false,
        }
    }

    /// Credentials rejected: retrying is pointless.
    pub fn is_auth(&self) -> bool {
        matches!(self, Self::Status { status, .. } if matches!(*status, 401 | 403))
    }
}

/// (P4-PRV-2, W17) HTTP timeout of the preview-stream plane calls (grant redemption plus the
/// `stream.open` budget; `16 §W18` connect timeout 10 s).
const PREVIEW_HTTP_TIMEOUT: Duration = aurora_core::limits::PREVIEW_CONNECT_TIMEOUT;

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub attempts: u32,
    pub initial: Duration,
    pub max: Duration,
}

impl RetryPolicy {
    pub fn delay(&self, attempt: u32) -> Duration {
        self.initial
            .saturating_mul(1u32 << attempt.min(16))
            .min(self.max)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 5,
            initial: Duration::from_millis(250),
            max: Duration::from_secs(10),
        }
    }
}

pub struct ApiClient {
    http: reqwest::Client,
    base: String,
    token: String,
    replica: Mutex<Option<Uuid>>,
    retry: RetryPolicy,
    reconciler: AtomicBool,
}

impl ApiClient {
    /// `api_url` is the API origin; `/v1/runner` is appended.
    pub fn new(api_url: &str, token: &str, retry: RetryPolicy) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("aurora-runner/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base: format!("{}/v1/runner", api_url.trim_end_matches('/')),
            token: token.to_owned(),
            replica: Mutex::new(None),
            retry,
            reconciler: AtomicBool::new(false),
        })
    }

    pub fn replica_id(&self) -> Option<Uuid> {
        self.replica.lock().ok().and_then(|r| *r)
    }

    /// `reconciler` flag of the most recent successful `/work`; false if absent or after an error (S-08).
    pub fn is_reconciler(&self) -> bool {
        self.reconciler.load(Ordering::Relaxed)
    }

    async fn once<B: Serialize, R: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        timeout: Option<Duration>,
    ) -> Result<(R, reqwest::header::HeaderMap), ApiError> {
        let mut rb = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .header(aurora_protocol::version::HEADER, "runner/1");
        if let Some(id) = self.replica_id() {
            rb = rb.header(HDR_REPLICA, id.to_string());
        }
        if let Some(t) = timeout {
            rb = rb.timeout(t);
        }
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let resp = rb
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.without_url().to_string()))?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ApiError::Transport(e.without_url().to_string()))?;
        if !status.is_success() {
            let snippet: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
            return Err(ApiError::Status {
                status: status.as_u16(),
                body: snippet,
            });
        }
        let raw: &[u8] = if bytes.is_empty() { b"null" } else { &bytes };
        let value = serde_json::from_slice(raw).map_err(|e| ApiError::Decode(e.to_string()))?;
        Ok((value, headers))
    }

    async fn call<B: Serialize, R: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<R, ApiError> {
        let mut attempt = 0;
        loop {
            match self.once(method.clone(), path, body, None).await {
                Ok((v, _)) => return Ok(v),
                Err(e) if e.retryable() && attempt + 1 < self.retry.attempts => {
                    tracing::warn!(path, attempt, error = %e, "api call failed; retrying");
                    tokio::time::sleep(self.retry.delay(attempt)).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub async fn register(&self, req: &RegisterRequest) -> Result<RegisterResponse, ApiError> {
        let r: RegisterResponse = self.call(Method::POST, "/register", Some(req)).await?;
        if let Ok(mut g) = self.replica.lock() {
            *g = Some(r.replica_id);
        }
        Ok(r)
    }

    /// One long poll (no internal retry; the runner loop owns the backoff).
    pub async fn work(&self, req: &WorkRequest) -> Result<WorkResponse, ApiError> {
        let timeout = Duration::from_secs(u64::from(req.wait_secs) + 15);
        let result = self
            .once::<_, WorkResponse>(Method::POST, "/work", Some(req), Some(timeout))
            .await;
        // Fail safe: only a successful response with `reconciler: true` enables reconcile.
        self.reconciler.store(
            result.as_ref().is_ok_and(|(r, _)| r.reconciler),
            Ordering::Relaxed,
        );
        let (r, _) = result?;
        Ok(r)
    }

    pub async fn post_reports(
        &self,
        reports: &ReportsRequest,
    ) -> Result<Vec<ReportResult>, ApiError> {
        let r: ReportsResponse = self.call(Method::POST, "/reports", Some(reports)).await?;
        Ok(r.results)
    }

    pub async fn list_sandboxes(&self) -> Result<Vec<SandboxAssignment>, ApiError> {
        let r: SandboxesResponse = self.call(Method::GET, "/sandboxes", None::<&()>).await?;
        Ok(r.sandboxes)
    }

    pub async fn check_sandboxes(&self, ids: Vec<Uuid>) -> Result<Vec<Uuid>, ApiError> {
        let req = SandboxesCheckRequest { sandbox_ids: ids };
        let r: SandboxesCheckResponse = self
            .call(Method::POST, "/sandboxes/check", Some(&req))
            .await?;
        Ok(r.known)
    }

    pub async fn strays(&self, req: &StraysRequest) -> Result<(), ApiError> {
        let _: serde_json::Value = self.call(Method::POST, "/strays", Some(req)).await?;
        Ok(())
    }

    pub async fn retire(&self) -> Result<(), ApiError> {
        let _: serde_json::Value = self
            .call(Method::POST, "/retire", Some(&serde_json::json!({})))
            .await?;
        Ok(())
    }

    /// Claim a bootstrap token (single use). No retry on 409/404.
    pub async fn claim(&self, token: &str) -> Result<ChannelClaimResponse, ApiError> {
        let req = ChannelClaimRequest {
            bootstrap_token: aurora_protocol::common::SecretString::new(token),
        };
        self.call(Method::POST, "/channel/claim", Some(&req)).await
    }

    pub async fn channel_hold(&self, r: &ChannelRef) -> Result<(), ApiError> {
        let _: serde_json::Value = self.call(Method::POST, "/channel/hold", Some(r)).await?;
        Ok(())
    }

    pub async fn channel_release(&self, r: &ChannelRef) -> Result<(), ApiError> {
        let _: serde_json::Value = self.call(Method::POST, "/channel/release", Some(r)).await?;
        Ok(())
    }

    /// (P4-PRV-2, W17) Redeems one single-use `aux_` stream grant. No retry: the grant is
    /// single-use and a replay would answer 403.
    pub async fn preview_stream_open(
        &self,
        req: &PreviewStreamOpenRequest,
    ) -> Result<PreviewStreamOpenResponse, ApiError> {
        let (r, _) = self
            .once(
                Method::POST,
                "/preview-streams/open",
                Some(req),
                Some(PREVIEW_HTTP_TIMEOUT),
            )
            .await?;
        Ok(r)
    }

    /// (P4-PRV-2, W17) Advisory stream accounting; best effort, never blocks a stream.
    pub async fn preview_stream_closed(&self, req: &PreviewStreamClosedRequest) {
        let _ = self
            .once::<_, serde_json::Value>(
                Method::POST,
                "/preview-streams/closed",
                Some(req),
                Some(PREVIEW_HTTP_TIMEOUT),
            )
            .await;
    }

    /// Single attempt: tool calls are not idempotent.
    pub async fn tool_call(&self, call: &ToolCall) -> Result<ToolResult, ApiError> {
        let t = aurora_core::limits::TOOL_CALL_TIMEOUT + Duration::from_secs(5);
        let (r, _) = self
            .once(Method::POST, "/tool-calls", Some(call), Some(t))
            .await?;
        Ok(r)
    }

    /// Single attempt: writes are not retried blindly.
    pub async fn memory_write(
        &self,
        req: &MemoryWriteRequest,
    ) -> Result<MemoryWriteResponse, ApiError> {
        let (r, _) = self
            .once(
                Method::POST,
                "/memory-writes",
                Some(req),
                Some(Duration::from_secs(30)),
            )
            .await?;
        Ok(r)
    }

    /// Download an inbound attachment (`GET /attachments/{id}`, decision S-11). Retries transient
    /// failures; the body is read incrementally and refused once it passes `max_bytes`, so a lying
    /// or oversized response never fills memory.
    pub async fn download_attachment(
        &self,
        id: Uuid,
        max_bytes: u64,
        timeout: Duration,
    ) -> Result<Vec<u8>, ApiError> {
        let mut attempt = 0;
        loop {
            match tokio::time::timeout(timeout, self.download_once(id, max_bytes)).await {
                Ok(Ok(bytes)) => return Ok(bytes),
                Ok(Err(e)) if e.retryable() && attempt + 1 < self.retry.attempts => {
                    tracing::warn!(attachment_id = %id, attempt, error = %e, "attachment download failed; retrying");
                    tokio::time::sleep(self.retry.delay(attempt)).await;
                    attempt += 1;
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(ApiError::Transport("attachment download timed out".into())),
            }
        }
    }

    async fn download_once(&self, id: Uuid, max_bytes: u64) -> Result<Vec<u8>, ApiError> {
        let mut rb = self
            .http
            .get(format!("{}/attachments/{id}", self.base))
            .bearer_auth(&self.token)
            .header(aurora_protocol::version::HEADER, "runner/1");
        if let Some(r) = self.replica_id() {
            rb = rb.header(HDR_REPLICA, r.to_string());
        }
        let mut resp = rb
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.without_url().to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.bytes().await.unwrap_or_default();
            return Err(ApiError::Status {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&body).chars().take(200).collect(),
            });
        }
        let too_large = || ApiError::Decode(format!("attachment exceeds {max_bytes} bytes"));
        if resp.content_length().is_some_and(|n| n > max_bytes) {
            return Err(too_large());
        }
        let mut out = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| ApiError::Transport(e.without_url().to_string()))?
        {
            if (out.len() + chunk.len()) as u64 > max_bytes {
                return Err(too_large());
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    /// Upload an outbound file; metadata travels in headers, the body is the raw bytes.
    pub async fn upload_attachment(
        &self,
        meta: &AttachmentUploadMeta,
        bytes: Vec<u8>,
    ) -> Result<AttachmentUploadResponse, ApiError> {
        let mut rb = self
            .http
            .post(format!("{}/attachments", self.base))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(120))
            .header(aurora_protocol::version::HEADER, "runner/1")
            .header(HDR_SANDBOX, meta.sandbox_id.to_string())
            .header(HDR_BOOT, meta.boot_id.to_string())
            .header(HDR_CONVERSATION, meta.conversation_id.to_string())
            .header(HDR_TURN, meta.turn_id.to_string())
            // Percent-encode: header values must be ASCII.
            .header(HDR_FILENAME, pct(&meta.filename))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream");
        if let Some(c) = &meta.caption {
            rb = rb.header(HDR_CAPTION, pct(c));
        }
        if let Some(id) = self.replica_id() {
            rb = rb.header(HDR_REPLICA, id.to_string());
        }
        let resp = rb
            .body(bytes)
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.without_url().to_string()))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| ApiError::Transport(e.without_url().to_string()))?;
        if !status.is_success() {
            return Err(ApiError::Status {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&body).chars().take(200).collect(),
            });
        }
        serde_json::from_slice(&body).map_err(|e| ApiError::Decode(e.to_string()))
    }
}

/// Percent-encode everything but unreserved ASCII.
fn pct(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}
