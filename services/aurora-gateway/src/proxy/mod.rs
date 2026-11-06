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
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(25);

pub struct ProxyParts {
    pub ca: Arc<Ca>,
    pub resolver: Arc<dyn CredentialResolver>,
    pub handler: Arc<dyn RequestHandler>,
    pub guard: Arc<Guard>,
    pub header_timeout: Duration,
    /// PEM bundle trusted for upstream TLS in addition to the native roots.
    pub extra_ca_pem: Option<Vec<u8>>,
    /// `gateway.egress_proxy`
    pub egress_proxy: Option<String>,
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
    resolver: Arc<dyn CredentialResolver>,
    handler: Arc<dyn RequestHandler>,
    guard: Arc<Guard>,
    upstream: Upstream,
    egress: Option<EgressProxy>,
    stop: watch::Sender<bool>,
    live: Arc<Live>,
}

/// Per-connection routing context.
#[derive(Clone)]
struct Target {
    identity: Arc<RequestIdentity>,
    lane: Lane,
    scheme: Scheme,
    host: String,
    port: u16,
}

impl Proxy {
    pub fn new(p: ProxyParts) -> Result<Arc<Self>, String> {
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
            resolver: p.resolver,
            handler: p.handler,
            guard: p.guard,
            upstream,
            egress,
            stop: watch::channel(false).0,
            live: Arc::default(),
        }))
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
        // 13. forward
        let mut resp = match self.upstream.send(prepared).await {
            Ok(r) => r,
            Err(e) => {
                if let Some(h) = hook {
                    h.on_error(&e);
                }
                return guidance::response(&e);
            }
        };
        // 14-15. guidance replacement and request log
        if let Some(h) = hook {
            if let Some(replacement) = h.on_response(resp.status(), resp.headers()) {
                return replacement;
            }
        }
        if resp.status() == http::StatusCode::SWITCHING_PROTOCOLS {
            if let Some(client_up) = upgrade_fut {
                let upstream_up = hyper::upgrade::on(&mut resp);
                let stop = self.stop.subscribe();
                let live = self.track();
                tokio::spawn(async move {
                    let _live = live;
                    if let (Ok(c), Ok(u)) = (client_up.await, upstream_up.await) {
                        let idle = aurora_core::limits::WEBSOCKET_IDLE_TIMEOUT;
                        bridge(
                            hyper_util::rt::TokioIo::new(c),
                            hyper_util::rt::TokioIo::new(u),
                            Some(idle),
                            stop,
                        )
                        .await;
                    }
                });
                let (p, _) = resp.into_parts();
                return Response::from_parts(p, empty_body());
            }
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
