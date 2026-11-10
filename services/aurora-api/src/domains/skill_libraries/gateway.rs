//! The API's side of `POST /internal/v1/library-fetch` (docs/06 §Library fetch; phase 4, P4-GSL,
//! D-149, SEC-84). The API never holds a git credential: every repository read goes through the
//! gateway, which builds the request from the package's `library` templates. A trait so tests can
//! stand in for the gateway (a fake that builds the same request with `aurora_registry::library`
//! and calls the fake git host).

use async_trait::async_trait;
use aurora_registry::library::Step;
use base64::Engine as _;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct FetchParams {
    pub repo: String,
    pub reference: Option<String>,
    pub sha: Option<String>,
    pub blob: Option<String>,
    pub page: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub org_id: Uuid,
    pub connection_id: Uuid,
    pub step: Step,
    pub params: FetchParams,
    pub max_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct FetchOk {
    /// The git host's HTTP status (the gateway passes 4xx/5xx through for the API to interpret).
    pub status: u16,
    pub link: Option<String>,
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
}

/// Why a fetch did not produce a response; `reason()` is safe to show (never a token or URL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayError {
    /// 400 `library_param`.
    Param,
    /// 404: the connection is not in this organization.
    ConnectionNotFound,
    /// 409 `library_unavailable` (workspace-scoped, disabled, no library block, instance gone).
    Unavailable,
    /// 409 `reauth_required`.
    Reauth,
    /// 429: the gateway's own rate limit; the caller may retry shortly.
    Busy,
    /// 413 / 502 / 504 / 503 and transport errors.
    Upstream(&'static str),
}

impl GatewayError {
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Param => "the repository, ref or path was refused",
            Self::ConnectionNotFound => "connection_deleted",
            Self::Unavailable => "the connection cannot serve a skill library",
            Self::Reauth => "the connection needs to be reconnected",
            Self::Busy => "too many library requests; retry shortly",
            Self::Upstream(r) => r,
        }
    }
}

#[async_trait]
pub trait LibraryGateway: Send + Sync {
    async fn fetch(&self, r: &FetchRequest) -> Result<FetchOk, GatewayError>;
}

/// The production client: HTTP to the gateway's internal listener with the internal secret.
pub struct HttpGateway {
    base: String,
    bearer: String,
    key_id: String,
    http: reqwest::Client,
}

impl HttpGateway {
    pub fn new(base: &str, ring: &aurora_crypto::SecretRing) -> Result<Self, String> {
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            bearer: ring.active_bearer(),
            key_id: ring.active_id().to_owned(),
            http: reqwest::Client::builder()
                // The gateway's own cap is 20 s; a little more for the hop.
                .timeout(
                    aurora_core::limits::LIBRARY_FETCH_TIMEOUT + std::time::Duration::from_secs(5),
                )
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| format!("library gateway client: {e}"))?,
        })
    }
}

#[async_trait]
impl LibraryGateway for HttpGateway {
    async fn fetch(&self, r: &FetchRequest) -> Result<FetchOk, GatewayError> {
        let mut params = serde_json::Map::new();
        params.insert("repo".into(), json!(r.params.repo));
        for (k, v) in [
            ("ref", &r.params.reference),
            ("sha", &r.params.sha),
            ("blob", &r.params.blob),
        ] {
            if let Some(v) = v {
                params.insert(k.into(), json!(v));
            }
        }
        if let Some(p) = r.params.page {
            params.insert("page".into(), json!(p));
        }
        let body = json!({
            "org_id": r.org_id, "connection_id": r.connection_id, "step": r.step.as_str(),
            "params": params, "max_bytes": r.max_bytes,
        });
        let resp = self
            .http
            .post(format!("{}/internal/v1/library-fetch", self.base))
            .bearer_auth(&self.bearer)
            .header(aurora_core::limits::KEY_ID_HEADER, &self.key_id)
            .headers(aurora_core::telemetry::headers())
            .json(&body)
            .send()
            .await
            .map_err(|_| GatewayError::Upstream("the gateway is unreachable"))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .map_err(|_| GatewayError::Upstream("the gateway answered unexpectedly"))?;
        if !status.is_success() {
            let code = v
                .pointer("/error/code")
                .and_then(Value::as_str)
                .unwrap_or("");
            return Err(match (status.as_u16(), code) {
                (400, "library_param") => GatewayError::Param,
                (404, _) => GatewayError::ConnectionNotFound,
                (409, "reauth_required") => GatewayError::Reauth,
                (409, _) => GatewayError::Unavailable,
                (413, _) => GatewayError::Upstream("the git host's response was too large"),
                (429, _) => GatewayError::Busy,
                (504, _) => GatewayError::Upstream("the git host did not answer in time"),
                _ => GatewayError::Upstream("the git host could not be reached"),
            });
        }
        let host_status = v
            .get("status")
            .and_then(Value::as_u64)
            .and_then(|s| u16::try_from(s).ok())
            .ok_or(GatewayError::Upstream("the gateway answered unexpectedly"))?;
        let body = v
            .get("body")
            .and_then(Value::as_str)
            .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
            .ok_or(GatewayError::Upstream("the gateway answered unexpectedly"))?;
        let header = |n: &str| {
            v.pointer(&format!("/headers/{n}"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        Ok(FetchOk {
            status: host_status,
            link: header("link"),
            retry_after: header("retry-after"),
            body,
        })
    }
}
