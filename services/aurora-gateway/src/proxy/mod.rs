//! Proxy listener (4102): CONNECT (MITM / open lane) and absolute-form HTTP.

mod absolute;
mod connect;
mod listener;
mod tunnel;

use crate::auth::{CredentialResolver, Lane, RequestIdentity, parse_proxy_authorization};
use crate::ca::Ca;
use crate::guard::{DialError, Guard};
use crate::hygiene::{self, PathError};
use crate::pipeline::{Outcome, PipelineRequest, RequestHandler, Scheme};
use crate::proxy_tls::ProxyTls;
use crate::upstream::{EgressProxy, Upstream};
use crate::{Body, BoxError, empty_body, guidance};
use aurora_core::ApiError;
use http::{HeaderMap, Request, Response};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, watch};

pub use tunnel::bridge;

/// In-flight work may finish for this long after SIGTERM.
/// Response-body prefix inspected for `token.stale_codes.body_contains`.
const STALE_BODY_PEEK: usize = 16 * 1024;
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(25);

pub struct ProxyParts {
    pub ca: Arc<Ca>,
    /// (phase 1, RFE-002) The listener's TLS configuration; there is no plaintext variant.
    pub tls: Arc<ProxyTls>,
    pub resolver: Arc<dyn CredentialResolver>,
    pub handler: Arc<dyn RequestHandler>,
    pub guard: Arc<Guard>,
    pub header_timeout: Duration,
    /// PEM bundle trusted for upstream TLS in addition to the native roots.
    pub extra_ca_pem: Option<Vec<u8>>,
    /// `gateway.egress_proxy`
    pub egress_proxy: Option<String>,
}

/// (phase 2, RFE-025) BYO admission inputs of the listener.
#[derive(Default)]
pub struct ByoParts {
    /// `gateway.trusted_proxies`: peers that must send a PROXY protocol header.
    pub trusted_proxies: Vec<aurora_core::cidr::Cidr>,
    /// Base of the `manage_url` links in refusals.
    pub external_url: String,
    /// Live-tunnel registry (closes BYO tunnels when their admission changes).
    pub tunnels: Option<Arc<crate::byo::TunnelRegistry>>,
}

#[derive(Default)]
struct Live {
    count: AtomicUsize,
    notify: Notify,
}

struct LiveGuard(Arc<Live>);
impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }
}

pub struct Proxy {
    ca: Arc<Ca>,
    tls: Arc<ProxyTls>,
    resolver: Arc<dyn CredentialResolver>,
    handler: Arc<dyn RequestHandler>,
    guard: Arc<Guard>,
    upstream: Upstream,
    egress: Option<EgressProxy>,
    stop: watch::Sender<bool>,
    live: Arc<Live>,
    byo: ByoParts,
    refusals: crate::byo::RefusalLimiter,
}

/// Per-connection routing context.
#[derive(Clone)]
struct Target {
    identity: Arc<RequestIdentity>,
    lane: Lane,
    scheme: Scheme,
    host: String,
    port: u16,
    /// Closes the tunnel (and its WebSockets) when its BYO admission changes (RFE-025).
    kill: Option<watch::Receiver<bool>>,
}

impl Proxy {
    pub fn new(p: ProxyParts) -> Result<Arc<Self>, String> {
        Self::with_byo(p, ByoParts::default())
    }

    pub fn with_byo(p: ProxyParts, byo: ByoParts) -> Result<Arc<Self>, String> {
        let egress = p
            .egress_proxy
            .as_deref()
            .map(EgressProxy::parse)
            .transpose()?;
        let upstream = Upstream::new(
            p.guard.clone(),
            egress.clone(),
            p.extra_ca_pem.as_deref(),
            p.header_timeout,
        )?;
        Ok(Arc::new(Self {
            ca: p.ca,
            tls: p.tls,
            resolver: p.resolver,
            handler: p.handler,
            guard: p.guard,
            upstream,
            egress,
            stop: watch::channel(false).0,
            live: Arc::default(),
            byo,
            refusals: crate::byo::RefusalLimiter::default(),
        }))
    }

    /// (RFE-025) BYO admission checks 1 (posture) and 2 (source address): counted, logged at
    /// most once per credential per minute, answered 403.
    fn byo_gate(
        &self,
        identity: &Arc<RequestIdentity>,
        client: std::net::IpAddr,
        method: &str,
        host: &str,
    ) -> Result<(), ApiError> {
        match crate::byo::admit(identity, client, &self.byo.external_url) {
            Ok(()) => Ok(()),
            Err((reason, err)) => {
                metrics::counter!("aurora_gateway_byo_refused_total", "reason" => reason.label())
                    .increment(1);
                if self.refusals.should_log(identity.credential_id) {
                    tracing::warn!(
                        credential = %identity.credential_id,
                        agent = %identity.agent_id,
                        reason = reason.label(),
                        source = %client,
                        "BYO credential refused"
                    );
                    self.handler
                        .byo_refused(identity, method, host, reason, &err);
                }
                Err(err)
            }
        }
    }

    fn track(&self) -> LiveGuard {
        self.live.count.fetch_add(1, Ordering::SeqCst);
        LiveGuard(self.live.clone())
    }

    async fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<(Arc<RequestIdentity>, Lane), ApiError> {
        let Some((lane, token)) = parse_proxy_authorization(headers) else {
            return Err(guidance::proxy_auth_required());
        };
        match self.resolver.resolve(&token).await {
            Ok(Some(id)) => Ok((Arc::new(id), lane)),
            Ok(None) => Err(guidance::proxy_auth_required()),
            Err(crate::auth::ResolveError::Frozen { level }) => {
                metrics::counter!("aurora_gateway_frozen_refused_total").increment(1);
                Err(guidance::frozen(level))
            }
            Err(e) => {
                tracing::warn!(error = %e, "credential resolution failed");
                Err(guidance::credential_store_unavailable())
            }
        }
    }

    /// Pipeline steps 1-3, handler, forwarding (step 13).
    async fn process(&self, t: &Target, mut req: Request<Incoming>) -> Response<Body> {
        let upgrade_fut =
            hygiene::requested_upgrade(req.headers()).map(|_| hyper::upgrade::on(&mut req));
        let (parts, body) = req.into_parts();
        let mut headers = parts.headers;
        let upgrade = hygiene::requested_upgrade(&headers);
        // 1. header hygiene
        let aurora_headers = hygiene::strip_request_headers(&mut headers);
        headers.remove(http::header::HOST);
        // 2. destination guard (IP literals; names are judged at dial time)
        if let Err(e) = self.guard.check_name(&t.host, t.port) {
            return guidance::response(&dial_error(&e));
        }
        // 3. method override and path hygiene
        let path = match hygiene::check_request_line(&headers, parts.uri.path(), parts.uri.query())
        {
            Ok(p) => p,
            Err(PathError::MethodOverride) => {
                return guidance::response(&guidance::method_override_refused());
            }
            Err(PathError::Ambiguous) => {
                return guidance::response(&guidance::ambiguous_path());
            }
        };
        let body: Body = body.map_err(|e| -> BoxError { Box::new(e) }).boxed_unsync();
        let outcome = self
            .handler
            .handle(PipelineRequest {
                identity: t.identity.clone(),
                lane: t.lane,
                scheme: t.scheme,
                host: t.host.clone(),
                port: t.port,
                method: parts.method,
                path,
                query: parts.uri.query().map(str::to_owned),
                headers,
                aurora_headers,
                upgrade,
                body,
            })
            .await;
        let prepared = match outcome {
            Outcome::Respond(r) => return r,
            Outcome::Forward(p) => p,
        };
        let mut prepared = prepared;
        let hook = prepared.hook.take();
        let response_headers = std::mem::take(&mut prepared.response_headers);
        let retry = prepared.retry.take();
        // 13. forward
        let kill = t.kill.clone();
        let resp = match self.upstream.send(prepared).await {
            Ok(r) => r,
            Err(e) => {
                if let Some(h) = hook {
                    h.on_error(&e);
                }
                return guidance::response(&e);
            }
        };
        // 14. stale token: refresh and resend a body-less request once.
        if let Some(r) = retry
            && r.stale_codes()
                .iter()
                .any(|c| c.status == resp.status().as_u16())
        {
            let status = resp.status();
            let need_body = r
                .stale_codes()
                .iter()
                .any(|c| c.status == status.as_u16() && c.body_contains.is_some());
            let (parts, inc) = resp.into_parts();
            let body: Body = inc.map_err(|e| -> BoxError { Box::new(e) }).boxed_unsync();
            let (prefix, replay) = if need_body {
                let (p, _, b) = crate::decide::buffer_prefix(body, STALE_BODY_PEEK).await;
                (p, b)
            } else {
                (bytes::Bytes::new(), body)
            };
            let text = String::from_utf8_lossy(&prefix);
            let stale = r.stale_codes().iter().any(|c| {
                c.status == status.as_u16()
                    && c.body_contains
                        .as_ref()
                        .is_none_or(|n| text.contains(n.as_str()))
            });
            if stale && let Some(next) = r.rebuild().await {
                match self.upstream.send(next).await {
                    Ok(r2) => {
                        // The first response is dropped unread.
                        drop(replay);
                        return self.finish(r2, hook, response_headers, upgrade_fut, kill);
                    }
                    Err(e) => {
                        if let Some(h) = hook {
                            h.on_error(&e);
                        }
                        return guidance::response(&e);
                    }
                }
            }
            let mut resp = Response::from_parts(parts, replay);
            if let Some(h) = hook
                && let Some(replacement) = h.on_response(resp.status(), resp.headers())
            {
                return replacement;
            }
            for (k, v) in &response_headers {
                resp.headers_mut().insert(k.clone(), v.clone());
            }
            hygiene::strip_hop_by_hop(resp.headers_mut());
            return resp;
        }
        self.finish(resp, hook, response_headers, upgrade_fut, kill)
    }

    /// 14-15. guidance replacement, request log, upgrade bridging, response hygiene.
    fn finish(
        &self,
        mut resp: Response<hyper::body::Incoming>,
        hook: Option<Box<dyn crate::pipeline::PostHook>>,
        response_headers: http::HeaderMap,
        upgrade_fut: Option<hyper::upgrade::OnUpgrade>,
        kill: Option<watch::Receiver<bool>>,
    ) -> Response<Body> {
        if let Some(h) = hook
            && let Some(replacement) = h.on_response(resp.status(), resp.headers())
        {
            return replacement;
        }
        for (k, v) in &response_headers {
            resp.headers_mut().insert(k.clone(), v.clone());
        }
        if resp.status() == http::StatusCode::SWITCHING_PROTOCOLS
            && let Some(client_up) = upgrade_fut
        {
            let upstream_up = hyper::upgrade::on(&mut resp);
            let stop = self.stop.subscribe();
            let live = self.track();
            tokio::spawn(async move {
                let _live = live;
                if let (Ok(c), Ok(u)) = (client_up.await, upstream_up.await) {
                    let idle = aurora_core::limits::WEBSOCKET_IDLE_TIMEOUT;
                    let pipe = bridge(
                        hyper_util::rt::TokioIo::new(c),
                        hyper_util::rt::TokioIo::new(u),
                        Some(idle),
                        stop,
                    );
                    // (RFE-025) A WebSocket dies with its BYO tunnel.
                    match kill {
                        Some(mut k) => {
                            tokio::select! {
                                _ = pipe => {}
                                () = crate::byo::killed(&mut k) => {}
                            }
                        }
                        None => {
                            pipe.await;
                        }
                    }
                }
            });
            let (p, _) = resp.into_parts();
            return Response::from_parts(p, empty_body());
        }
        hygiene::strip_hop_by_hop(resp.headers_mut());
        resp.map(|b| b.map_err(|e| -> BoxError { Box::new(e) }).boxed_unsync())
    }
}

fn dial_error(e: &DialError) -> ApiError {
    match e {
        DialError::Refused(h) => guidance::destination_refused(h),
        DialError::Resolution(_) => guidance::resolution_failed(),
        DialError::Connect(_) => guidance::bad_gateway(),
    }
}
