//! aurora-client: typed HTTP client for the Aurora control-plane API
//! (`docs/API_CONTRACTS.md`), used by the CLI and tests.
#![forbid(unsafe_code)]

pub mod config;
pub mod types;

use std::time::Duration;

use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use types::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{code}: {message} (HTTP {status})")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn code(&self) -> Option<&str> {
        match self {
            Error::Api { code, .. } => Some(code),
            _ => None,
        }
    }
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// `https://` always; `http://` only to `localhost`, `127.0.0.0/8` or `::1`.
pub fn require_secure_url(url: &str) -> Result<()> {
    let u = reqwest::Url::parse(url)
        .map_err(|_| Error::Other(format!("not a valid server URL: {}", clean(url))))?;
    let host = u.host_str().unwrap_or("").trim_matches(['[', ']']);
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|a| a.is_loopback());
    match u.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => Err(Error::Other(format!(
            "refusing to talk to {} over cleartext: the server URL must be https:// (http:// is only allowed for localhost)",
            clean(url)
        ))),
    }
}

/// Strips control characters (terminal escapes, carriage returns, bells) from a string the
/// server supplied before it is printed (FIX-118, PKG-7).
pub fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    bearer: Option<String>,
}

impl Client {
    /// `bearer`: API key (`auk_…`/`auo_…`) or agent token.
    ///
    /// (FIX-118, PKG-6) The base URL must be `https://`, or `http://` to a loopback host: the
    /// device-flow response carries the API key and every request carries a bearer token.
    pub fn new(base_url: &str, bearer: Option<String>) -> Result<Client> {
        require_secure_url(base_url)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Client {
            http,
            base: base_url.trim_end_matches('/').to_string(),
            bearer,
        })
    }

    fn req(&self, m: Method, path: &str) -> RequestBuilder {
        let rb = self.http.request(m, format!("{}{}", self.base, path));
        match &self.bearer {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    async fn send<T: DeserializeOwned>(&self, rb: RequestBuilder) -> Result<T> {
        let resp = check(rb.send().await?).await?;
        Ok(resp.json().await?)
    }

    pub async fn auth_cli_start(&self, hostname: Option<&str>) -> Result<DeviceStart> {
        self.send(
            self.req(Method::POST, "/v1/auth/cli")
                .json(&json!({ "hostname": hostname })),
        )
        .await
    }

    /// One poll. Pending states surface as `Error::Api` with code
    /// `authorization_pending` / `slow_down`.
    pub async fn auth_cli_token(&self, device_code: &str) -> Result<CliToken> {
        self.send(
            self.req(Method::POST, "/v1/auth/cli/token")
                .json(&json!({ "device_code": device_code })),
        )
        .await
    }

    /// Poll until approved. `interval` is the starting delay; `slow_down`
    /// adds `slow_step` (RFC 8628 uses 5 s). Fails after `expires_in`.
    pub async fn poll_cli_token(
        &self,
        device_code: &str,
        mut interval: Duration,
        slow_step: Duration,
        expires_in: Duration,
    ) -> Result<CliToken> {
        let deadline = tokio::time::Instant::now() + expires_in;
        loop {
            tokio::time::sleep(interval).await;
            match self.auth_cli_token(device_code).await {
                Ok(t) => return Ok(t),
                Err(e) => match e.code() {
                    Some("authorization_pending") => {}
                    Some("slow_down") => interval += slow_step,
                    _ => return Err(e),
                },
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Api {
                    status: 400,
                    code: "expired_token".into(),
                    message: "login request expired".into(),
                });
            }
        }
    }

    pub async fn me(&self) -> Result<Me> {
        self.send(self.req(Method::GET, "/v1/me")).await
    }

    /// Revoke the API key this client authenticates with (`DELETE /v1/me/api-key`, C-720).
    pub async fn revoke_my_api_key(&self) -> Result<()> {
        check(self.req(Method::DELETE, "/v1/me/api-key").send().await?).await?;
        Ok(())
    }

    pub async fn resolve(&self, org: &str, workspace: Option<&str>) -> Result<Resolved> {
        let mut q = vec![("org", org)];
        if let Some(w) = workspace {
            q.push(("workspace", w));
        }
        self.send(self.req(Method::GET, "/v1/resolve").query(&q))
            .await
    }

    pub async fn list_agents(&self, workspace_id: &str) -> Result<Vec<Agent>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut rb = self
                .req(
                    Method::GET,
                    &format!("/v1/workspaces/{workspace_id}/agents"),
                )
                .query(&[("limit", "200")]);
            if let Some(c) = &cursor {
                rb = rb.query(&[("cursor", c.as_str())]);
            }
            let page: Page<Agent> = self.send(rb).await?;
            out.extend(page.items);
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => return Ok(out),
            }
        }
    }

    pub async fn regenerate_agent_token(&self, agent_id: &str) -> Result<AgentToken> {
        self.send(self.req(Method::POST, &format!("/v1/agents/{agent_id}/token")))
            .await
    }

    /// Session/key variant: `proxy_url` contains `<AGENT_TOKEN>`.
    pub async fn container_config(&self, agent_id: &str) -> Result<ContainerConfig> {
        self.send(self.req(
            Method::GET,
            &format!("/v1/agents/{agent_id}/container-config"),
        ))
        .await
    }

    /// Agent-token variant (client must be built with the agent token).
    pub async fn agent_container_config(&self) -> Result<ContainerConfig> {
        self.send(self.req(Method::GET, "/v1/agent/container-config"))
            .await
    }

    /// Mints a labelled BYO context credential (`auc_`) for attribution (client must be built
    /// with the agent token; RFE-025).
    pub async fn create_context_credential(
        &self,
        label: &str,
        ttl_secs: u32,
    ) -> Result<types::ContextCredential> {
        self.send(
            self.req(Method::POST, "/v1/agent/context-credentials")
                .json(&json!({ "label": label, "ttl_secs": ttl_secs })),
        )
        .await
    }

    /// Long-poll pending approvals decidable by the caller (`wait` ≤ 30).
    pub async fn pending_approvals(&self, wait_secs: u64) -> Result<Vec<Approval>> {
        let wait = wait_secs.min(30);
        #[derive(serde::Deserialize)]
        struct Items {
            items: Vec<Approval>,
        }
        let r: Items = self
            .send(
                self.req(Method::GET, "/v1/approvals/pending")
                    .query(&[("wait", wait)])
                    .timeout(Duration::from_secs(wait + 15)),
            )
            .await?;
        Ok(r.items)
    }

    pub async fn decide(&self, approval_id: &str, decision: Decision) -> Result<Approval> {
        #[derive(Serialize)]
        struct Body<'a> {
            decision: &'a str,
        }
        self.send(
            self.req(
                Method::POST,
                &format!("/v1/approvals/{approval_id}/decision"),
            )
            .json(&Body {
                decision: decision.as_str(),
            }),
        )
        .await
    }
}

async fn check(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    Err(api_error(status, resp.bytes().await.ok().as_deref()))
}

fn api_error(status: StatusCode, body: Option<&[u8]>) -> Error {
    let v: Option<serde_json::Value> = body.and_then(|b| serde_json::from_slice(b).ok());
    let e = v.as_ref().and_then(|v| v.get("error"));
    let s = |k: &str| e.and_then(|e| e.get(k)).and_then(|x| x.as_str());
    Error::Api {
        status: status.as_u16(),
        code: s("code").unwrap_or("http_error").to_string(),
        message: s("message").unwrap_or("request failed").to_string(),
    }
}

#[cfg(test)]
mod tests;
