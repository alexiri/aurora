//! Proxy listener (4102): CONNECT (MITM / open lane) and absolute-form HTTP.

mod absolute;
mod connect;
mod database;
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
    /// (phase 3, RFE-014) Outage state: new connections are refused while `down`.
    pub health: Option<Arc<crate::health::Health>>,
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
    /// (FIX-086) Rate limit of the CONNECT-authentication refusal log line, per source address.
    connect_log: SourceLimiter,
    /// (phase 2, P2-DBL-1) Cancel keys of live database sessions.
    db_cancel: crate::db::pg::CancelRegistry,
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
    /// The answer owed to in-flight requests when the tunnel is closed by a freeze or a BYO
    /// tightening (P2-FU-2); set before `kill` fires.
    closed_by: Option<crate::byo::ClosedByRx>,
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
            connect_log: SourceLimiter::default(),
            db_cancel: crate::db::pg::CancelRegistry::default(),
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

    /// (P2-FU-2) The answer of a request caught in the pipeline by a freeze or BYO tightening:
    /// the refusal a new request would get, counted in the same refusal counter, then the
    /// connection closes after it (`Connection: close`; a tunnel is shut by its task).
    fn answer_closed(
        &self,
        t: &Target,
        c: &crate::byo::ClosedBy,
        method: &str,
        http1: bool,
    ) -> Response<Body> {
        match c.reason {
            None => metrics::counter!("aurora_gateway_frozen_refused_total").increment(1),
            Some(reason) => {
                metrics::counter!("aurora_gateway_byo_refused_total", "reason" => reason.label())
                    .increment(1);
                if self.refusals.should_log(t.identity.credential_id) {
                    self.handler
                        .byo_refused(&t.identity, method, &t.host, reason, &c.error);
                }
            }
        }
        let mut res = guidance::response(&c.error);
        if http1 {
            res.headers_mut().insert(
                http::header::CONNECTION,
                http::HeaderValue::from_static("close"),
            );
        }
        res
    }

    fn track(&self) -> LiveGuard {
        self.live.count.fetch_add(1, Ordering::SeqCst);
        LiveGuard(self.live.clone())
    }

    async fn authenticate(
        &self,
        headers: &HeaderMap,
        client: std::net::IpAddr,
    ) -> Result<(Arc<RequestIdentity>, Lane), ApiError> {
        // (RFE-014, `down`) No snapshot loaded or draining: new connections are refused first,
        // whatever the credential.
        if let Some(r) = self.byo.health.as_ref().and_then(|h| h.down_reason()) {
            return Err(match r {
                crate::health::Reason::Draining => guidance::restarting_error(),
                _ => guidance::gateway_degraded("snapshot_not_loaded"),
            });
        }
        let Some((lane, token)) = parse_proxy_authorization(headers) else {
            self.connect_refused(client, "unknown", None);
            return Err(guidance::proxy_auth_required());
        };
        match self.resolver.resolve(&token).await {
            Ok(Some(id)) => Ok((Arc::new(id), lane)),
            // (FIX-050, C-630) A credential that is present but unusable (revoked, expired
            // non-agent kind, organization without an active owner) is answered exactly like an
            // unknown one, and classified in the log and the metric. `Ok(None)` stays the
            // unclassified `unknown`.
            Ok(None) => {
                self.connect_refused(client, "unknown", None);
                Err(guidance::proxy_auth_required())
            }
            Err(e @ crate::auth::ResolveError::Refused { .. }) => {
                self.connect_refused(client, e.reason(), e.credential_kind());
                Err(guidance::proxy_auth_required())
            }
            Err(crate::auth::ResolveError::Expired) => {
                metrics::counter!("aurora_gateway_agent_token_expired_total").increment(1);
                self.connect_refused(client, "expired", Some("agent"));
                Err(guidance::agent_token_expired())
            }
            Err(crate::auth::ResolveError::Frozen { level }) => {
                metrics::counter!("aurora_gateway_frozen_refused_total").increment(1);
                self.connect_refused(client, "frozen", None);
                Err(guidance::frozen(level))
            }
            // (RFE-014) The gateway cannot tell an invalid token from an unreadable store: 503,
            // never 407.
            Err(
                e @ (crate::auth::ResolveError::DatabaseUnavailable
                | crate::auth::ResolveError::Unavailable(_)),
            ) => {
                self.connect_refused(client, "store_unavailable", None);
                match e {
                    crate::auth::ResolveError::DatabaseUnavailable => {
                        Err(guidance::gateway_degraded("database_unavailable"))
                    }
                    _ => {
                        tracing::warn!(error = %e, "credential resolution failed");
                        Err(guidance::credential_store_unavailable())
                    }
                }
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
        let method_name = parts.method.as_str().to_owned();
        let http1 = parts.version <= http::Version::HTTP_11;
        let mut closed_by = t.closed_by.clone();
        // (P2-FU-2) A freeze or BYO tightening that arrives while the request is in the pipeline
        // (held for approval, among others) refuses it with the admission answer. Dropping the
        // pipeline future abandons its approval and never releases it upstream.
        let outcome = tokio::select! {
            biased;
            c = crate::byo::closed_by(&mut closed_by) => {
                return self.answer_closed(t, &c, &method_name, http1);
            }
            o = self.handler.handle(PipelineRequest {
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
            }) => o,
        };
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
                        return self
                            .finish(r2, hook, response_headers, upgrade_fut, kill)
                            .await;
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
            if let Some(h) = &hook {
                match echo_check(&h.echo_needles(), resp).await {
                    Ok(r) => resp = r,
                    Err(e) => {
                        if let Some(h) = hook {
                            h.on_error(&e);
                        }
                        return guidance::response(&e);
                    }
                }
            }
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
            .await
    }

    /// 14-15. guidance replacement, request log, upgrade bridging, response hygiene.
    async fn finish(
        &self,
        mut resp: Response<hyper::body::Incoming>,
        hook: Option<Box<dyn crate::pipeline::PostHook>>,
        response_headers: http::HeaderMap,
        upgrade_fut: Option<hyper::upgrade::OnUpgrade>,
        kill: Option<watch::Receiver<bool>>,
    ) -> Response<Body> {
        // (FIX-086) A `101` to a request that asked for no (honoured) upgrade is a protocol error:
        // never bridged. The response is dropped, which closes that upstream connection; the agent
        // gets 502 and the exchange is logged once through the hook.
        if resp.status() == http::StatusCode::SWITCHING_PROTOCOLS && upgrade_fut.is_none() {
            tracing::warn!("upstream answered 101 to a request that asked for no upgrade");
            let e = guidance::bad_gateway();
            if let Some(h) = hook {
                h.on_error(&e);
            }
            return guidance::response(&e);
        }
        // (FIX-104, SEC-01) A credential placed in the URL never comes back to the agent.
        let needles = hook.as_ref().map(|h| h.echo_needles()).unwrap_or_default();
        if !needles.is_empty() && resp.status() != http::StatusCode::SWITCHING_PROTOCOLS {
            let (parts, inc) = resp.into_parts();
            let body: Body = inc.map_err(|e| -> BoxError { Box::new(e) }).boxed_unsync();
            let mut checked = match echo_check(&needles, Response::from_parts(parts, body)).await {
                Ok(r) => r,
                Err(e) => {
                    if let Some(h) = hook {
                        h.on_error(&e);
                    }
                    return guidance::response(&e);
                }
            };
            if let Some(h) = hook
                && let Some(replacement) = h.on_response(checked.status(), checked.headers())
            {
                return replacement;
            }
            for (k, v) in &response_headers {
                checked.headers_mut().insert(k.clone(), v.clone());
            }
            hygiene::strip_hop_by_hop(checked.headers_mut());
            return checked;
        }
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

/// Bytes of a response body searched for an injected value (docs/17 SEC-01); what follows is
/// streamed on unchecked.
const ECHO_BODY_CHECK: usize = 64 * 1024;

/// (FIX-104, SEC-01) 502 `upstream_echoed_credential` when the response headers or the first
/// 64 KiB of its body carry one of `needles`; otherwise the response with its body intact.
async fn echo_check(needles: &[Vec<u8>], resp: Response<Body>) -> Result<Response<Body>, ApiError> {
    if needles.is_empty() {
        return Ok(resp);
    }
    let echoed = || {
        ApiError::new(
            http::StatusCode::BAD_GATEWAY,
            "upstream_echoed_credential",
            "The upstream answered with a credential Aurora injected into the request URL, so the response was withheld.",
        )
    };
    if resp
        .headers()
        .values()
        .any(|v| crate::inject::echoes(needles, v.as_bytes()))
    {
        return Err(echoed());
    }
    let (parts, body) = resp.into_parts();
    let (prefix, _, rest) = crate::decide::buffer_prefix(body, ECHO_BODY_CHECK).await;
    if crate::inject::echoes(needles, &prefix[..prefix.len().min(ECHO_BODY_CHECK)]) {
        return Err(echoed());
    }
    Ok(Response::from_parts(parts, rest))
}

fn dial_error(e: &DialError) -> ApiError {
    match e {
        DialError::Refused(h) => guidance::destination_refused(h),
        DialError::Resolution(_) => guidance::resolution_failed(),
        DialError::Connect(_) => guidance::bad_gateway(),
    }
}

/// (FIX-086) At most one `connect_auth_refused` log line per source address per
/// [`aurora_core::limits::BYO_REFUSAL_LOG_INTERVAL`]; the metric counts every refusal.
#[derive(Default)]
pub struct SourceLimiter {
    /// Keyed by source address and reason class: every class stays observable (FIX-050, C-630)
    /// while a flood of one class from one address writes one line per interval.
    last: std::sync::Mutex<
        std::collections::HashMap<(std::net::IpAddr, &'static str), std::time::Instant>,
    >,
    counted: std::sync::atomic::AtomicU64,
    logged: std::sync::atomic::AtomicU64,
}

impl SourceLimiter {
    fn should_log(&self, source: std::net::IpAddr, reason: &'static str) -> bool {
        let key = (source, reason);
        self.counted.fetch_add(1, Ordering::Relaxed);
        let interval = aurora_core::limits::BYO_REFUSAL_LOG_INTERVAL;
        let now = std::time::Instant::now();
        let log = match self.last.lock() {
            Ok(mut m) => {
                if m.len() > 10_000 {
                    m.retain(|_, t| now.duration_since(*t) < interval);
                }
                match m.get(&key) {
                    Some(t) if now.duration_since(*t) < interval => false,
                    _ => {
                        m.insert(key, now);
                        true
                    }
                }
            }
            Err(_) => true,
        };
        if log {
            self.logged.fetch_add(1, Ordering::Relaxed);
        }
        log
    }

    /// `(refusals counted, log lines written)`.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.counted.load(Ordering::Relaxed),
            self.logged.load(Ordering::Relaxed),
        )
    }
}

impl Proxy {
    /// (FIX-086) `(CONNECT-stage refusals, log lines written for them)`.
    pub fn connect_refusal_stats(&self) -> (u64, u64) {
        self.connect_log.stats()
    }

    /// (FIX-050, C-630) One metric per CONNECT-stage authentication refusal
    /// (`unknown | revoked | expired | no_active_owner | frozen | store_unavailable`), and (FIX-086)
    /// a warn line with the reason class, the credential kind when known and the source address
    /// — never the token, a prefix or its hash — at most once per source per interval. The answer
    /// itself is the caller's, unchanged.
    fn connect_refused(
        &self,
        client: std::net::IpAddr,
        reason: &'static str,
        kind: Option<&'static str>,
    ) {
        metrics::counter!("aurora_gateway_connect_refused_total", "reason" => reason).increment(1);
        if self.connect_log.should_log(client, reason) {
            tracing::warn!(
                reason,
                kind = kind.unwrap_or(""),
                source = %client,
                "connect_auth_refused"
            );
        }
    }
}
