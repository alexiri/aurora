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
    /// (D-251) Set for a request on an LDAP app's synthetic host: the approved `ldap` block the
    /// request is translated with instead of being forwarded.
    pub ldap: Option<aurora_registry::manifest::Ldap>,
    /// (P5-ICS, D-273, C-843) Set for an admitted request on a secret-link connection's synthetic
    /// host: answered by the secret-link lane (`seclink`), never forwarded as received. It carries
    /// its own request-log report, so `hook` is `None` for it.
    pub link: Option<crate::seclink::LinkFetch>,
    /// (FIX-290, D-511) Set only for a request that won a granted connection of an
    /// instance-registered self-hosted app and goes to that registration's host: this request
    /// alone may reach the host on a private address.
    pub app_host: Option<aurora_core::ranges::AppHost>,
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
    /// (FIX-104, SEC-01) Forms of every injected value that was placed in the request URL: a
    /// response carrying one (headers, or the first 64 KiB of the body) is replaced by 502
    /// `upstream_echoed_credential`. Empty when nothing was placed in the URL.
    fn echo_needles(&self) -> Vec<Vec<u8>> {
        Vec::new()
    }
    /// (lane review 2026-11-25) A matched tool's response projection: the `$`-paths every item
    /// of the tool's JSON answer keeps, applied by the gateway on the response path for every
    /// caller. `None` = the answer streams through untouched.
    fn response_project(&self) -> Option<&[String]> {
        None
    }
    /// (FIX-166, D-368) Ids and the decision for the wire log's `forwarded` event; never values.
    fn wire_context(&self) -> crate::wire::Decided {
        crate::wire::Decided::default()
    }
    /// Upstream answered. Return `Some` to replace the response (status is the
    /// caller's to preserve).
    fn on_response(
        self: Box<Self>,
        status: http::StatusCode,
        headers: &HeaderMap,
    ) -> Option<Response<Body>>;
    /// (FIX-153, C-892) The refusal for a request on an LDAP synthetic host that reached the
    /// lane without an approved `ldap` block, derived from the decision's fallback.
    fn ldap_refusal(&self) -> Option<ApiError> {
        None
    }
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
            ldap: None,
            link: None,
            app_host: None,
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

    /// (phase 2, P2-DBL-1) Admits a database login: resolves connection `connection_id` for
    /// `identity` (granted, active, a database app), reads its stored credentials and returns
    /// the endpoint plus the per-statement authorizer. Default: the lane is not available.
    async fn db_open(
        self: Arc<Self>,
        _identity: &Arc<RequestIdentity>,
        _connection_id: uuid::Uuid,
    ) -> Result<crate::db::DbOpened, crate::db::DbOpenError> {
        Err(crate::db::DbOpenError::Refused(ApiError::new(
            http::StatusCode::FORBIDDEN,
            "database_lane_unavailable",
            "The database lane is not enabled on this gateway.",
        )))
    }

    /// (RFE-025) A BYO admission refusal that is due for a request-log row (the proxy rate
    /// limits to one per credential per minute and counts every refusal itself).
    fn byo_refused(
        &self,
        _identity: &Arc<RequestIdentity>,
        _method: &str,
        _host: &str,
        _reason: crate::byo::Refusal,
        _err: &ApiError,
    ) {
    }

    /// (FIX-180) A request body stalled before the decision produced a hook: the proxy answered
    /// `408 request_body_timeout`; this writes its request-log row.
    fn body_stalled(
        &self,
        _identity: &Arc<RequestIdentity>,
        _lane: &'static str,
        _method: &str,
        _host: &str,
        _err: &ApiError,
    ) {
    }
}

/// Default handler: forwards everything unchanged.
pub struct PassThrough;

#[async_trait::async_trait]
impl RequestHandler for PassThrough {}
