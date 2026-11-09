//! Client of the host plane `/v1/connector-host/*` (C-455..C-457), authenticated with `auh_…`.
//!
//! Assignment bodies hold decrypted secrets: they are only ever deserialized into
//! [`AssignmentsResponse`] (whose `Debug` redacts them) and never logged.

use std::time::Duration;

use aurora_protocol::host::{
    AssignmentsResponse, HostRegisterRequest, HostRegisterResponse, HostStatusRequest,
};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("the API answered {0}")]
    Status(u16),
    #[error("undecodable response: {0}")]
    Decode(String),
}

impl ApiError {
    /// The token is not accepted: retrying will not help.
    pub fn is_auth(&self) -> bool {
        matches!(self, ApiError::Status(401 | 403))
    }
}

/// Request timeout of the short calls.
const SHORT: Duration = Duration::from_secs(20);
/// Margin added to the long poll's `wait`.
const POLL_MARGIN: Duration = Duration::from_secs(15);

pub struct HostApi {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl HostApi {
    pub fn new(base: &str, token: &str) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1/connector-host{path}", self.base)
    }

    async fn send<T: DeserializeOwned>(rb: reqwest::RequestBuilder) -> Result<T, ApiError> {
        let resp = rb
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.without_url().to_string()))?;
        if !resp.status().is_success() {
            return Err(ApiError::Status(resp.status().as_u16()));
        }
        resp.json::<T>()
            .await
            .map_err(|e| ApiError::Decode(e.without_url().to_string()))
    }

    pub async fn register(
        &self,
        req: &HostRegisterRequest,
    ) -> Result<HostRegisterResponse, ApiError> {
        Self::send(
            self.http
                .post(self.url("/register"))
                .bearer_auth(&self.token)
                .timeout(SHORT)
                .json(req),
        )
        .await
    }

    /// Long poll: returns at once when the current revision differs from `revision`.
    pub async fn assignments(
        &self,
        revision: Option<i64>,
        wait_secs: u32,
        instance_id: Uuid,
    ) -> Result<AssignmentsResponse, ApiError> {
        let mut q = vec![
            ("wait", wait_secs.to_string()),
            ("instance_id", instance_id.to_string()),
        ];
        if let Some(r) = revision {
            q.push(("revision", r.to_string()));
        }
        Self::send(
            self.http
                .get(self.url("/assignments"))
                .bearer_auth(&self.token)
                .timeout(Duration::from_secs(u64::from(wait_secs)) + POLL_MARGIN)
                .query(&q),
        )
        .await
    }

    pub async fn status(&self, req: &HostStatusRequest) -> Result<(), ApiError> {
        let resp = self
            .http
            .post(self.url("/status"))
            .bearer_auth(&self.token)
            .timeout(SHORT)
            .json(req)
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.without_url().to_string()))?;
        if resp.status() == StatusCode::NO_CONTENT || resp.status().is_success() {
            Ok(())
        } else {
            Err(ApiError::Status(resp.status().as_u16()))
        }
    }
}
