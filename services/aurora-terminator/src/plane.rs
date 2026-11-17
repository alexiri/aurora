//! Client of the terminator plane `/v1/terminator/*` (`16 §W20`, `05 §Terminator plane`),
//! authenticated with `aut_…` and, after registration, `X-Aurora-Replica`. Certificates,
//! grants and tokens are never logged; every failure is a typed value the callers map to a
//! hint-free outcome.

use std::sync::Mutex;
use std::time::Duration;

use aurora_protocol::terminator as wire;
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

const SHORT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
pub enum PlaneError {
    #[error("transport: {0}")]
    Transport(String),
    /// A non-success status with the error code of the standard envelope when there is one.
    #[error("the API answered {status}")]
    Status { status: u16, code: String },
    #[error("undecodable response: {0}")]
    Decode(String),
}

impl PlaneError {
    /// The credential or the replica is not accepted: register again.
    pub fn needs_register(&self) -> bool {
        matches!(self, Self::Status { status: 401, .. })
            || matches!(self, Self::Status { status: 403, code } if code == "replica_unknown")
    }
}

/// `sessions`: the API's verdict.
#[derive(Debug)]
pub enum Opened {
    Open(wire::SessionOpenResponse),
    /// 403 `{ reason }`: `denied` for every access refusal, `limit` over a cap.
    Refused(wire::SessionRefusal),
}

/// `channels`.
#[derive(Debug)]
pub enum Granted {
    Grant(wire::ChannelGrantResponse),
    /// 403 `session_closed`.
    SessionClosed,
    /// 409 `not_running`: the sandbox is waking; ask again after this many seconds.
    NotRunning {
        retry_after_secs: u32,
    },
    /// 429 `ssh_channel_limit`.
    ChannelLimit,
    /// 429 `wake_limited`.
    WakeLimited,
}

pub struct Plane {
    http: reqwest::Client,
    base: String,
    token: String,
    replica: Mutex<Option<Uuid>>,
}

impl Plane {
    pub fn new(base: &str, token: &str) -> Result<Self, PlaneError> {
        let http = reqwest::Client::builder()
            .timeout(SHORT)
            .build()
            .map_err(|e| PlaneError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            replica: Mutex::new(None),
        })
    }

    fn replica(&self) -> Option<Uuid> {
        *self.replica.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn call<B: Serialize>(
        &self,
        path: &str,
        body: &B,
        timeout: Option<Duration>,
    ) -> Result<(u16, serde_json::Value), PlaneError> {
        let mut rq = self
            .http
            .post(format!("{}/v1/terminator/{path}", self.base))
            .bearer_auth(&self.token)
            .json(body);
        if let Some(r) = self.replica() {
            rq = rq.header("x-aurora-replica", r.to_string());
        }
        if let Some(t) = timeout {
            rq = rq.timeout(t);
        }
        let resp = rq
            .send()
            .await
            .map_err(|e| PlaneError::Transport(e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        let v = resp.json::<serde_json::Value>().await.unwrap_or_default();
        Ok((status, v))
    }

    fn ok<T: DeserializeOwned>(status: u16, v: serde_json::Value) -> Result<T, PlaneError> {
        if (200..300).contains(&status) {
            serde_json::from_value(v).map_err(|e| PlaneError::Decode(e.to_string()))
        } else {
            Err(Self::status_err(status, &v))
        }
    }

    fn status_err(status: u16, v: &serde_json::Value) -> PlaneError {
        PlaneError::Status {
            status,
            code: v["error"]["code"].as_str().unwrap_or_default().to_owned(),
        }
    }

    /// `register`: also remembers the replica id for every later call.
    pub async fn register(
        &self,
        host_public_key: &str,
    ) -> Result<wire::RegisterResponse, PlaneError> {
        *self.replica.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let req = wire::RegisterRequest {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            host_public_key: host_public_key.to_owned(),
        };
        let (s, v) = self.call("register", &req, None).await?;
        let r: wire::RegisterResponse = Self::ok(s, v)?;
        *self.replica.lock().unwrap_or_else(|e| e.into_inner()) = Some(r.replica_id);
        Ok(r)
    }

    pub async fn open_session(&self, req: &wire::SessionOpenRequest) -> Result<Opened, PlaneError> {
        let (s, v) = self.call("sessions", req, None).await?;
        match s {
            201 | 200 => Ok(Opened::Open(Self::ok(s, v)?)),
            403 if v.get("reason").is_some() => Ok(Opened::Refused(
                serde_json::from_value::<wire::SessionRefusedBody>(v)
                    .map_err(|e| PlaneError::Decode(e.to_string()))?
                    .reason,
            )),
            _ => Err(Self::status_err(s, &v)),
        }
    }

    pub async fn channel(
        &self,
        session: Uuid,
        mode: wire::ChannelMode,
        wait_secs: u32,
    ) -> Result<Granted, PlaneError> {
        let req = wire::ChannelGrantRequest {
            mode,
            wait_secs: Some(wait_secs),
        };
        let timeout = Duration::from_secs(u64::from(wait_secs) + 15);
        let (s, v) = self
            .call(&format!("sessions/{session}/channels"), &req, Some(timeout))
            .await?;
        let code = v["error"]["code"].as_str().unwrap_or_default().to_owned();
        match (s, code.as_str()) {
            (200 | 201, _) => Ok(Granted::Grant(Self::ok(s, v)?)),
            (403, "session_closed") => Ok(Granted::SessionClosed),
            (409, "not_running") => Ok(Granted::NotRunning {
                retry_after_secs: v["error"]["details"]["retry_after_secs"]
                    .as_u64()
                    .unwrap_or(2)
                    .clamp(1, 30) as u32,
            }),
            (429, "ssh_channel_limit") => Ok(Granted::ChannelLimit),
            (429, "wake_limited") => Ok(Granted::WakeLimited),
            _ => Err(Self::status_err(s, &v)),
        }
    }

    pub async fn heartbeat(
        &self,
        beats: Vec<wire::SessionBeat>,
    ) -> Result<wire::HeartbeatResponse, PlaneError> {
        let (s, v) = self
            .call(
                "heartbeat",
                &wire::HeartbeatRequest { sessions: beats },
                None,
            )
            .await?;
        Self::ok(s, v)
    }

    pub async fn close(
        &self,
        session: Uuid,
        req: &wire::SessionCloseRequest,
    ) -> Result<(), PlaneError> {
        let (s, v) = self
            .call(&format!("sessions/{session}/close"), req, None)
            .await?;
        Self::ok::<wire::OkBody>(s, v).map(|_| ())
    }
}
