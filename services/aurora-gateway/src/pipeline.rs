//! The hook where policy, injection and approvals plug in (GW-003+).

use crate::Body;
use crate::auth::{Lane, RequestIdentity};
use crate::hygiene::AuroraHeaders;
use aurora_core::ApiError;
use http::{HeaderMap, HeaderValue, Method, Response};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
    pub fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }
}

/// A request after steps 1-3, as seen by the handler.
pub struct PipelineRequest {
    pub identity: Arc<RequestIdentity>,
    pub lane: Lane,
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
    pub method: Method,
    /// Normalized (step 3).
    pub path: String,
    pub query: Option<String>,
    /// Hygiene-stripped client headers (no Host).
    pub headers: HeaderMap,
    pub aurora_headers: AuroraHeaders,
    /// `Upgrade` value when the client asked for a protocol upgrade.
    pub upgrade: Option<HeaderValue>,
    pub body: Body,
}

/// What is sent upstream.
pub struct PreparedRequest {
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
    pub method: Method,
    pub path: String,
    pub query: Option<String>,
    pub headers: HeaderMap,
    pub upgrade: Option<HeaderValue>,
    pub body: Body,
    /// Step 14/15 callback, invoked once with the upstream outcome.
    pub hook: Option<Box<dyn PostHook>>,
    /// Headers added to the response the client receives (`x-aurora-connections`).
    pub response_headers: HeaderMap,
    /// Stale-token resend (step 14): body-less requests only.
    pub retry: Option<Box<dyn Retry>>,
}

/// Rebuilds a request with fresh credentials after the upstream answered with a
/// `token.stale_codes` match (docs/06 step 14); used at most once.
#[async_trait::async_trait]
pub trait Retry: Send + 'static {
    fn stale_codes(&self) -> &[aurora_registry::manifest::StaleCode];
    /// `None` when the credentials could not be refreshed (the first response stands).
    async fn rebuild(self: Box<Self>) -> Option<PreparedRequest>;
}

/// Post-response hook (pipeline steps 14-15): guidance replacement and request log.
pub trait PostHook: Send + 'static {
    /// Upstream answered. Return `Some` to replace the response (status is the
    /// caller's to preserve).
    fn on_response(
        self: Box<Self>,
        status: http::StatusCode,
        headers: &HeaderMap,
    ) -> Option<Response<Body>>;
    /// The upstream exchange failed with this refusal.
    fn on_error(self: Box<Self>, err: &ApiError);
}

impl PipelineRequest {
    /// Pass-through: forward as received.
    pub fn into_prepared(self) -> PreparedRequest {
        PreparedRequest {
            scheme: self.scheme,
            host: self.host,
            port: self.port,
            method: self.method,
            path: self.path,
            query: self.query,
            headers: self.headers,
            upgrade: self.upgrade,
            body: self.body,
            hook: None,
            response_headers: HeaderMap::new(),
            retry: None,
        }
    }
}

#[allow(clippy::large_enum_variant)]
pub enum Outcome {
    /// Answer the client directly (guidance).
    Respond(Response<Body>),
    Forward(PreparedRequest),
}

pub enum OpenLaneDecision {
    Allow,
    Deny(ApiError),
}

/// Byte accounting for a finished open-lane tunnel (request-log hook).
#[derive(Debug, Clone)]
pub struct TunnelReport {
    pub identity: Arc<RequestIdentity>,
    pub host: String,
    pub port: u16,
    /// client -> upstream
    pub bytes_up: u64,
    /// upstream -> client
    pub bytes_down: u64,
    pub duration: Duration,
}

#[async_trait::async_trait]
pub trait RequestHandler: Send + Sync + 'static {
    /// Invoked after hygiene (steps 1-3).
    async fn handle(&self, req: PipelineRequest) -> Outcome {
        Outcome::Forward(req.into_prepared())
    }

    /// Host-level decision for the open lane, before the tunnel is dialled.
    async fn open_lane_check(
        &self,
        _identity: &RequestIdentity,
        _host: &str,
        _port: u16,
    ) -> OpenLaneDecision {
        OpenLaneDecision::Allow
    }

    /// Called once per finished open-lane tunnel.
    fn tunnel_closed(&self, _report: TunnelReport) {}
}

/// Default handler: forwards everything unchanged.
pub struct PassThrough;

#[async_trait::async_trait]
impl RequestHandler for PassThrough {}
