//! Management-API rate limiting (phase 2, RFE-017, D-57; docs/05 §Conventions "Rate limiting").
//!
//! Per-principal token buckets (a general bucket plus an optional stricter class bucket), a
//! per-organization bucket, a concurrent-stream cap, and per-IP buckets for unauthenticated
//! traffic. In memory per API replica (the same approximation as the gateway, D-05).
//!
//! The pure parts ([`classify`], [`Limiter`]) are independent of axum so they can be unit
//! tested; [`middleware`] wires them into the router after the principal is resolved and
//! before any handler (hence before authorization).

use crate::app::AppState;
use crate::http::auth::{AgentCaller, Caller, Principal};
use crate::http::client_ip::ClientIp;
use aurora_config::ApiRateSection;
use aurora_core::ApiError;
use aurora_core::token::TokenPrefix;
use aurora_db::authz;
use axum::body::Body;
use axum::extract::{FromRequestParts, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use http::{HeaderValue, Method, StatusCode, header};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Buckets idle this long are dropped (they would be full again anyway).
pub const IDLE_EVICT: Duration = Duration::from_secs(600);
/// Hard cap on tracked buckets (LRU beyond it).
pub const MAX_BUCKETS: usize = 100_000;
/// `API_STREAMS_PER_PRINCIPAL`.
pub const STREAMS_PER_PRINCIPAL: u32 = 20;
/// `API_RATE_UNAUTH`: public GETs per minute per IP.
const UNAUTH_GET_PER_MIN: u32 = 120;
/// `API_RATE_UNAUTH`: requests carrying a rejected bearer per minute per IP.
const REJECTED_BEARER_PER_MIN: u32 = 30;
/// How long a resolved "which org does this request charge" answer is reused.
const ORG_CACHE_TTL: Duration = Duration::from_secs(30);

/// A request class (stricter bucket), see the table in docs/05.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    Write,
    Message,
    Upload,
    Simulate,
    Search,
    Export,
}

impl Class {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Message => "message",
            Self::Upload => "upload",
            Self::Simulate => "simulate",
            Self::Search => "search",
            Self::Export => "export",
        }
    }
}

/// Which stricter class a request belongs to (`None`: general bucket only).
/// `path` is the full request path (`/v1/...`).
pub fn classify(method: &Method, path: &str) -> Option<Class> {
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    let Some(("v1", rest)) = segs.split_first().map(|(a, b)| (*a, b)) else {
        return None;
    };
    let unsafe_m = !matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    );
    let is_get = *method == Method::GET;
    let post = *method == Method::POST;
    let last = rest.last().copied().unwrap_or("");
    match rest {
        ["conversations", _, "messages"] if post => return Some(Class::Message),
        ["turns", _, "send-now"] if post => return Some(Class::Message),
        ["conversations", _, "attachments"] if post => return Some(Class::Upload),
        ["agents", _, "avatar"] if *method == Method::PUT => return Some(Class::Upload),
        ["skills", _, "files", ..] if *method == Method::PUT => return Some(Class::Upload),
        ["admin", "plugins"] if post => return Some(Class::Upload),
        ["agents", _, "effective"] if is_get => return Some(Class::Simulate),
        ["conversations", _, "export"] if is_get => return Some(Class::Export),
        // (FIX-126) The Storage pages are cached snapshots read in the `general` bucket; only
        // the deliberate Refresh buttons are `export`-class.
        ["admin", "storage", "refresh"] if post => return Some(Class::Export),
        ["orgs", _, "storage", "refresh"] if post => return Some(Class::Export),
        _ => {}
    }
    if post && last == "simulate" && rest.len() >= 2 && rest[rest.len() - 2] == "policy" {
        return Some(Class::Simulate);
    }
    if is_get && last == "search" {
        return Some(Class::Search);
    }
    unsafe_m.then_some(Class::Write)
}

/// Whether the machine planes / ops endpoints / webhook ingress apply their own limits.
/// `/v1/relay/*` is not exempt but has its own class (a caller is always a relay replica).
pub fn exempt(path: &str) -> bool {
    let p = path.trim_end_matches('/');
    matches!(p, "/healthz" | "/readyz" | "/metrics")
        || path.starts_with("/internal/")
        || path.starts_with("/v1/runner/")
        || path.starts_with("/v1/connector/")
        || path.starts_with("/v1/hooks/")
}

/// Whether this is a relay-plane request (its own per-replica class, no org bucket).
fn is_relay(path: &str) -> bool {
    path.starts_with("/v1/relay/") || path.starts_with("/v1/runner/preview-streams/")
}

/// Whether a request is an SSE connection or a long-poll (counts against the stream cap).
pub fn is_stream(method: &Method, path: &str, query: Option<&str>) -> bool {
    if *method != Method::GET {
        return false;
    }
    let p = path.trim_end_matches('/');
    if p == "/v1/approvals/stream"
        || (p.starts_with("/v1/conversations/") && p.ends_with("/stream"))
    {
        return true;
    }
    query.is_some_and(|q| {
        q.split('&')
            .filter_map(|kv| kv.split_once('='))
            .any(|(k, v)| k == "wait" && v.parse::<u64>().is_ok_and(|n| n > 0))
    })
}

/// Static description of one bucket: capacity (burst) and steady refill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub burst: u32,
    pub limit: u32,
    /// Seconds per `limit` (60 or 3600).
    pub per_secs: u32,
}

impl Spec {
    pub const fn per_sec(limit: u32, burst: u32) -> Self {
        Self {
            burst,
            limit,
            per_secs: 1,
        }
    }
    pub const fn per_min(limit: u32, burst: u32) -> Self {
        Self {
            burst,
            limit,
            per_secs: 60,
        }
    }
    const fn per_hour(limit: u32, burst: u32) -> Self {
        Self {
            burst,
            limit,
            per_secs: 3600,
        }
    }
    fn rate(&self) -> f64 {
        f64::from(self.limit) / f64::from(self.per_secs)
    }
    pub fn per_name(&self) -> &'static str {
        if self.per_secs >= 3600 {
            "hour"
        } else if self.per_secs <= 1 {
            "second"
        } else {
            "minute"
        }
    }
    fn scaled(self, m: f64) -> Self {
        let s = |v: u32| ((f64::from(v) * m).round() as u32).max(1);
        Self {
            burst: s(self.burst),
            limit: s(self.limit),
            per_secs: self.per_secs,
        }
    }
}

/// Resolved bucket table (defaults of docs/05, overrides, multiplier).
#[derive(Debug, Clone)]
pub struct Limits {
    pub enabled: bool,
    pub general: Spec,
    pub write: Spec,
    pub message: Spec,
    pub upload: Spec,
    pub simulate: Spec,
    pub search: Spec,
    pub export: Spec,
    pub preview: Spec,
    pub org: Spec,
    pub unauth_get: Spec,
    pub rejected_bearer: Spec,
    pub streams: u32,
}

impl Limits {
    pub fn from_config(c: &ApiRateSection) -> Self {
        let m = c.multiplier;
        let class = |name: &str, d: Spec| {
            let mut s = d;
            if let Some(o) = c.classes.get(name) {
                if let Some(v) = o.per_minute {
                    s.limit = v;
                    s.per_secs = 60;
                }
                if let Some(v) = o.per_hour {
                    s.limit = v;
                    s.per_secs = 3600;
                }
                if let Some(b) = o.burst {
                    s.burst = b;
                }
            }
            s.scaled(m)
        };
        let org_burst = (c.org_per_minute / 5).max(1);
        Self {
            enabled: c.enabled,
            general: class("general", Spec::per_min(600, 120)),
            write: class("write", Spec::per_min(120, 30)),
            message: class("message", Spec::per_min(30, 10)),
            upload: class("upload", Spec::per_min(20, 5)),
            simulate: class("simulate", Spec::per_min(20, 5)),
            search: class("search", Spec::per_min(30, 10)),
            export: class("export", Spec::per_hour(6, 2)),
            preview: Spec::per_sec(c.preview_per_second, c.preview_burst).scaled(m),
            org: Spec::per_min(c.org_per_minute, org_burst).scaled(m),
            unauth_get: Spec::per_min(UNAUTH_GET_PER_MIN, UNAUTH_GET_PER_MIN).scaled(m),
            rejected_bearer: Spec::per_min(REJECTED_BEARER_PER_MIN, REJECTED_BEARER_PER_MIN)
                .scaled(m),
            streams: STREAMS_PER_PRINCIPAL,
        }
    }

    pub fn class_spec(&self, c: Class) -> Spec {
        match c {
            Class::Write => self.write,
            Class::Message => self.message,
            Class::Upload => self.upload,
            Class::Simulate => self.simulate,
            Class::Search => self.search,
            Class::Export => self.export,
        }
    }
}

/// Which bucket refused a request.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub scope: &'static str,
    pub class: &'static str,
    pub spec: Spec,
    pub retry_after_secs: u64,
    pub remaining: u32,
}

impl Refusal {
    pub fn into_error(self) -> ApiError {
        ApiError::rate_limited(
            self.retry_after_secs,
            "Too many requests. Slow down and retry after the indicated delay.",
        )
        .with_details(json!({
            "scope": self.scope,
            "class": self.class,
            "limit": self.spec.limit,
            "per": self.spec.per_name(),
            "retry_after_secs": self.retry_after_secs,
        }))
    }

    pub fn into_response(self) -> Response {
        let (limit, remaining, reset) = (self.spec.limit, self.remaining, self.retry_after_secs);
        let mut res = self.into_error().into_response();
        let h = res.headers_mut();
        h.insert("ratelimit-limit", HeaderValue::from(limit));
        h.insert("ratelimit-remaining", HeaderValue::from(remaining));
        h.insert("ratelimit-reset", HeaderValue::from(reset));
        h.insert("x-should-retry", HeaderValue::from_static("true"));
        res
    }
}

struct State1 {
    tokens: f64,
    last: Instant,
}

/// One bucket to charge in an atomic multi-bucket check.
pub struct Charge {
    pub key: String,
    pub spec: Spec,
    pub scope: &'static str,
    pub class: &'static str,
}

#[derive(Default)]
struct Inner {
    buckets: HashMap<String, State1>,
    streams: HashMap<String, u32>,
    org_cache: HashMap<(Uuid, String), (Instant, Option<Uuid>)>,
    last_sweep: Option<Instant>,
}

/// The shared limiter state (one per replica).
pub struct Limiter {
    pub limits: Limits,
    inner: Mutex<Inner>,
}

impl Limiter {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Number of tracked buckets (tests, metrics).
    pub fn tracked(&self) -> usize {
        self.lock().buckets.len()
    }

    /// Drop buckets idle for [`IDLE_EVICT`] (an idle bucket is full, so eviction only refills).
    pub fn evict_idle(&self, now: Instant) {
        let mut g = self.lock();
        g.buckets
            .retain(|_, s| now.saturating_duration_since(s.last) < IDLE_EVICT);
        g.last_sweep = Some(now);
    }

    /// Atomically take one token from **every** bucket, or take none and report the bucket
    /// that blocks the longest.
    pub fn take(&self, charges: &[Charge]) -> Result<(), Refusal> {
        self.take_at(Instant::now(), charges)
    }

    pub fn take_at(&self, now: Instant, charges: &[Charge]) -> Result<(), Refusal> {
        let mut g = self.lock();
        if g.last_sweep
            .is_none_or(|t| now.saturating_duration_since(t) > Duration::from_secs(60))
        {
            g.buckets
                .retain(|_, s| now.saturating_duration_since(s.last) < IDLE_EVICT);
            g.last_sweep = Some(now);
        }
        // Refill (creating full buckets) and find the worst blocker.
        let mut worst: Option<Refusal> = None;
        for c in charges {
            if !g.buckets.contains_key(&c.key) {
                if g.buckets.len() >= MAX_BUCKETS {
                    // LRU: drop the least recently used entry (a refill, never a bypass of
                    // the entry cap that bounds memory).
                    if let Some(k) = g
                        .buckets
                        .iter()
                        .min_by_key(|(_, s)| s.last)
                        .map(|(k, _)| k.clone())
                    {
                        g.buckets.remove(&k);
                    }
                }
                g.buckets.insert(
                    c.key.clone(),
                    State1 {
                        tokens: f64::from(c.spec.burst),
                        last: now,
                    },
                );
            }
            let Some(s) = g.buckets.get_mut(&c.key) else {
                continue;
            };
            let rate = c.spec.rate();
            let cap = f64::from(c.spec.burst);
            s.tokens =
                (s.tokens + now.saturating_duration_since(s.last).as_secs_f64() * rate).min(cap);
            s.last = now;
            if s.tokens < 1.0 {
                let wait = ((1.0 - s.tokens) / rate).ceil().max(1.0) as u64;
                if worst.as_ref().is_none_or(|w| wait > w.retry_after_secs) {
                    worst = Some(Refusal {
                        scope: c.scope,
                        class: c.class,
                        spec: c.spec,
                        retry_after_secs: wait,
                        remaining: 0,
                    });
                }
            }
        }
        if let Some(w) = worst {
            return Err(w);
        }
        for c in charges {
            if let Some(s) = g.buckets.get_mut(&c.key) {
                s.tokens -= 1.0;
            }
        }
        Ok(())
    }

    /// Give back the tokens of `charges` (P4-CNT): the shared store refused (the local
    /// pre-filter then did not spend them) or a `closed` class refused with 503.
    pub fn refund(&self, charges: &[Charge]) {
        let mut g = self.lock();
        for c in charges {
            if let Some(s) = g.buckets.get_mut(&c.key) {
                s.tokens = (s.tokens + 1.0).min(f64::from(c.spec.burst));
            }
        }
    }

    /// Reserve a stream slot for `principal`; `None` when at the cap.
    pub fn acquire_stream(self: &Arc<Self>, principal: &str) -> Option<StreamGuard> {
        let mut g = self.lock();
        let n = g.streams.entry(principal.to_owned()).or_insert(0);
        if *n >= self.limits.streams {
            return None;
        }
        *n += 1;
        Some(StreamGuard {
            limiter: self.clone(),
            principal: principal.to_owned(),
        })
    }

    /// Open streams of `principal` (tests).
    pub fn open_streams(&self, principal: &str) -> u32 {
        self.lock().streams.get(principal).copied().unwrap_or(0)
    }

    fn release_stream(&self, principal: &str) {
        let mut g = self.lock();
        if let Some(n) = g.streams.get_mut(principal) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                g.streams.remove(principal);
            }
        }
    }
}

/// Releases a stream slot when dropped (i.e. when the response body is dropped).
pub struct StreamGuard {
    limiter: Arc<Limiter>,
    principal: String,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.limiter.release_stream(&self.principal);
    }
}

// ---- middleware ------------------------------------------------------------------

/// Parse `/v1/orgs/{uuid}/...` or `/v1/workspaces/{uuid}/...` from a path.
fn path_target(path: &str) -> Option<(&'static str, Uuid)> {
    let mut it = path.trim_matches('/').split('/');
    if it.next()? != "v1" {
        return None;
    }
    let kind = match it.next()? {
        "orgs" => "org",
        "workspaces" => "workspace",
        _ => return None,
    };
    Some((kind, Uuid::parse_str(it.next()?).ok()?))
}

/// The organization whose bucket this request charges: the key's own org, or (for sessions)
/// the org named by the path when the user is an active member of it. A non-member never
/// charges (nor can drain) another organization's bucket.
async fn org_to_charge(state: &AppState, c: &Caller, path: &str) -> Option<Uuid> {
    if let Principal::ApiKey { org_id, .. } = &c.principal {
        return Some(*org_id);
    }
    let (kind, id) = path_target(path)?;
    let ck = (c.user_id, format!("{kind}:{id}"));
    let now = Instant::now();
    if let Some((at, v)) = state.api_limiter.lock().org_cache.get(&ck)
        && now.saturating_duration_since(*at) < ORG_CACHE_TTL
    {
        return *v;
    }
    let org = match kind {
        "org" => Some(id),
        _ => match authz::workspace_org(&state.pool, id).await {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(error = %e, "api limiter: workspace org lookup failed");
                return None;
            }
        },
    };
    let resolved = match org {
        Some(o) => match authz::org_membership(&state.pool, c.user_id, o).await {
            Ok(Some(m)) if m.is_active() => Some(o),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(error = %e, "api limiter: membership lookup failed");
                return None;
            }
        },
        None => None,
    };
    let mut g = state.api_limiter.lock();
    if g.org_cache.len() > 10_000 {
        g.org_cache.clear();
    }
    g.org_cache.insert(ck, (now, resolved));
    resolved
}

fn record(refusal: &Refusal) {
    tracing::debug!(
        scope = refusal.scope,
        class = refusal.class,
        retry_after_secs = refusal.retry_after_secs,
        "api rate limit refused a request"
    );
    metrics::counter!(
        "aurora_api_rate_limited_total",
        "class" => refusal.class,
        "scope" => refusal.scope
    )
    .increment(1);
}

fn ip_key(req: &Request) -> String {
    req.extensions()
        .get::<ClientIp>()
        .and_then(|c| c.0)
        .map_or_else(|| "unknown".to_owned(), |ip| ip.to_string())
}

fn bearer(headers: &http::HeaderMap) -> Option<Option<&str>> {
    // Some(None): an Authorization header that is not a usable bearer.
    let v = headers.get(header::AUTHORIZATION)?;
    Some(
        v.to_str()
            .ok()
            .and_then(|v| v.split_once(' '))
            .and_then(|(s, r)| s.eq_ignore_ascii_case("bearer").then(|| r.trim())),
    )
}

enum Who {
    /// Authenticated: principal id, optional resolved caller (for the org bucket).
    Principal(String, Option<Caller>),
    /// No credentials, or credentials that will be rejected.
    Anonymous { rejected: bool },
}

async fn resolve(state: &AppState, headers: &http::HeaderMap, ip: ClientIp) -> Who {
    let had_bearer = bearer(headers);
    let has_cookie =
        crate::http::auth::cookie_value(headers, crate::http::auth::SESSION_COOKIE).is_some();
    if had_bearer.is_none() && !has_cookie {
        return Who::Anonymous { rejected: false };
    }
    // Reuse the production extractors on a detached copy of the request head.
    let (mut parts, _) = http::Request::new(()).into_parts();
    parts.headers = headers.clone();
    parts.extensions.insert(ip);
    if let Some(Some(tok)) = had_bearer
        && tok.starts_with(TokenPrefix::Gateway.as_str())
    {
        return match AgentCaller::from_request_parts(&mut parts, state).await {
            Ok(a) => Who::Principal(format!("agent:{}", a.agent_id), None),
            Err(_) => Who::Anonymous { rejected: true },
        };
    }
    match Caller::from_request_parts(&mut parts, state).await {
        Ok(c) => {
            let id = match &c.principal {
                Principal::ApiKey { id, .. } => format!("key:{id}"),
                Principal::Session { .. } => format!("user:{}", c.user_id),
                Principal::Channel { integration_id } => format!("channel:{integration_id}"),
            };
            Who::Principal(id, Some(c))
        }
        // Extractor errors that are not "unauthenticated" (database trouble) are not the
        // caller's fault: let the handler answer instead of counting a rejected bearer.
        Err(e) => Who::Anonymous {
            rejected: e.status() == StatusCode::UNAUTHORIZED && had_bearer.is_some(),
        },
    }
}

/// Axum middleware (see module docs). Installed inside the client-IP layer.
pub async fn middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let limiter = state.api_limiter.clone();
    let limits = &limiter.limits;
    let path = req.uri().path().to_owned();
    if !limits.enabled || exempt(&path) || !path.starts_with("/v1/") {
        return next.run(req).await;
    }
    let method = req.method().clone();
    let ip = req
        .extensions()
        .get::<ClientIp>()
        .copied()
        .unwrap_or_default();
    let headers = req.headers().clone();
    match resolve(&state, &headers, ip).await {
        Who::Anonymous { rejected } => {
            let ip = ip_key(&req);
            let charge = if rejected {
                Some(Charge {
                    key: format!("ip-rejected:{ip}"),
                    spec: limits.rejected_bearer,
                    scope: "general",
                    class: "unauth",
                })
            } else if method == Method::GET {
                Some(Charge {
                    key: format!("ip-get:{ip}"),
                    spec: limits.unauth_get,
                    scope: "general",
                    class: "unauth",
                })
            } else {
                None
            };
            if let Some(c) = charge {
                let charges = [c];
                if let Err(r) = limiter.take(&charges) {
                    record(&r);
                    return r.into_response();
                }
                // (P4-CNT) unauthenticated and rejected-bearer limits are the `security` class.
                if let Some(cl) = state.counters.security()
                    && let Err(e) = crate::http::counters::take_shared(&state, cl, &charges).await
                {
                    return shared_refusal(e);
                }
            }
            next.run(req).await
        }
        Who::Principal(pid, caller) => {
            let stream = is_stream(&method, &path, req.uri().query());
            let guard = if stream && !is_relay(&path) {
                match limiter.acquire_stream(&pid) {
                    Some(g) => Some(g),
                    None => {
                        metrics::counter!(
                            "aurora_api_rate_limited_total",
                            "class" => "stream",
                            "scope" => "streams"
                        )
                        .increment(1);
                        return too_many_streams(limits.streams);
                    }
                }
            } else {
                None
            };
            let class = classify(&method, &path);
            // (P4-PRV-2) Relay-plane traffic: the caller is a relay replica, not a management
            // principal. It charges the preview class only — no general bucket, no org bucket,
            // no shared management counters (`16 §W17`).
            if is_relay(&path) {
                let charges = [Charge {
                    key: format!("{pid}|preview"),
                    spec: limits.preview,
                    scope: "class",
                    class: "preview",
                }];
                if let Err(r) = limiter.take(&charges) {
                    record(&r);
                    return r.into_response();
                }
                return next.run(req).await;
            }
            let mut charges = vec![Charge {
                key: format!("{pid}|general"),
                spec: limits.general,
                scope: "general",
                class: "general",
            }];
            if let Some(cl) = class {
                charges.push(Charge {
                    key: format!("{pid}|{}", cl.name()),
                    spec: limits.class_spec(cl),
                    scope: "class",
                    class: cl.name(),
                });
            }
            if let Some(c) = &caller
                && let Some(org) = org_to_charge(&state, c, &path).await
            {
                charges.push(Charge {
                    key: format!("org:{org}"),
                    spec: limits.org,
                    scope: "org",
                    class: "org",
                });
            }
            if let Err(r) = limiter.take(&charges) {
                record(&r);
                return r.into_response(); // `guard` drops here, releasing the slot
            }
            // (P4-CNT) authenticated buckets are the `management` class.
            if let Some(cl) = state.counters.management()
                && let Err(e) = crate::http::counters::take_shared(&state, cl, &charges).await
            {
                return shared_refusal(e);
            }
            let res = next.run(req).await;
            match guard {
                Some(g) if res.status().is_success() => hold_until_dropped(res, g),
                _ => res,
            }
        }
    }
}

/// The answer for a refusal by the shared store: 429 from the blocking bucket, or the 503 of a
/// `closed` class (never a 429: a client must not mistake an outage for its own rate).
fn shared_refusal(e: Result<Refusal, ApiError>) -> Response {
    match e {
        Ok(r) => {
            record(&r);
            r.into_response()
        }
        Err(e) => e.into_response(),
    }
}

fn too_many_streams(cap: u32) -> Response {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_streams",
        "Too many open streams for this account. Close one and try again.",
    )
    .with_details(json!({ "scope": "streams", "limit": cap }))
    .into_response()
}

/// Keep `guard` alive for as long as the response body is.
fn hold_until_dropped(res: Response, guard: StreamGuard) -> Response {
    let (parts, body) = res.into_parts();
    let stream = body.into_data_stream().map(move |chunk| {
        let _held = &guard;
        chunk
    });
    Response::from_parts(parts, Body::from_stream(stream))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits::from_config(&ApiRateSection::default())
    }

    fn ch(key: &str, spec: Spec, scope: &'static str) -> Charge {
        Charge {
            key: key.into(),
            spec,
            scope,
            class: "x",
        }
    }

    #[test]
    fn classes() {
        let c = |m: Method, p: &str| classify(&m, p);
        assert_eq!(
            c(Method::POST, "/v1/conversations/1/messages"),
            Some(Class::Message)
        );
        assert_eq!(
            c(Method::POST, "/v1/turns/1/send-now"),
            Some(Class::Message)
        );
        assert_eq!(
            c(Method::POST, "/v1/conversations/1/attachments"),
            Some(Class::Upload)
        );
        assert_eq!(c(Method::PUT, "/v1/agents/1/avatar"), Some(Class::Upload));
        assert_eq!(
            c(Method::PUT, "/v1/skills/1/files/a/b.md"),
            Some(Class::Upload)
        );
        assert_eq!(c(Method::POST, "/v1/admin/plugins"), Some(Class::Upload));
        assert_eq!(
            c(Method::POST, "/v1/workspaces/1/policy/simulate"),
            Some(Class::Simulate)
        );
        assert_eq!(
            c(Method::GET, "/v1/agents/1/effective"),
            Some(Class::Simulate)
        );
        assert_eq!(
            c(Method::GET, "/v1/agents/1/memories/search"),
            Some(Class::Search)
        );
        assert_eq!(
            c(Method::GET, "/v1/conversations/1/export"),
            Some(Class::Export)
        );
        // (FIX-126) the Storage pages are plain reads; only the Refresh buttons are `export`.
        assert_eq!(c(Method::GET, "/v1/admin/storage"), None);
        assert_eq!(c(Method::GET, "/v1/orgs/1/storage"), None);
        assert_eq!(
            c(Method::POST, "/v1/admin/storage/refresh"),
            Some(Class::Export)
        );
        assert_eq!(
            c(Method::POST, "/v1/orgs/1/storage/refresh"),
            Some(Class::Export)
        );
        assert_eq!(c(Method::DELETE, "/v1/agents/1"), Some(Class::Write));
        assert_eq!(c(Method::PATCH, "/v1/agents/1"), Some(Class::Write));
        assert_eq!(c(Method::GET, "/v1/agents/1"), None);
        // a GET on an upload route is just a read
        assert_eq!(c(Method::GET, "/v1/conversations/1/messages"), None);
    }

    #[test]
    fn exemptions_and_streams() {
        for p in [
            "/healthz",
            "/readyz",
            "/metrics",
            "/internal/x",
            "/v1/runner/work",
            "/v1/connector/ops/claim",
            "/v1/hooks/tok",
        ] {
            assert!(exempt(p), "{p}");
        }
        assert!(!exempt("/v1/agents"));
        assert!(!exempt("/v1/hookshot"));
        // (P4-PRV-2) the relay plane is not exempt but has its own class.
        assert!(is_relay("/v1/relay/resolve"));
        assert!(is_relay("/v1/runner/preview-streams/open"));
        assert!(!is_relay("/v1/runner/work"));
        assert!(!is_relay("/v1/relayx"));
        let l = limits();
        assert_eq!(
            (l.preview.limit, l.preview.burst, l.preview.per_secs),
            (aurora_core::limits::PREVIEW_API_RATE, 200, 1)
        );
        assert!(is_stream(&Method::GET, "/v1/conversations/x/stream", None));
        assert!(is_stream(&Method::GET, "/v1/approvals/stream", None));
        assert!(is_stream(
            &Method::GET,
            "/v1/workspaces/x/approvals",
            Some("wait=20")
        ));
        assert!(!is_stream(
            &Method::GET,
            "/v1/workspaces/x/approvals",
            Some("wait=0")
        ));
        assert!(!is_stream(&Method::GET, "/v1/workspaces/x/approvals", None));
        assert!(!is_stream(
            &Method::POST,
            "/v1/conversations/x/stream",
            None
        ));
    }

    #[test]
    fn defaults_match_spec_and_overrides_apply() {
        let l = limits();
        assert_eq!((l.general.limit, l.general.burst), (600, 120));
        assert_eq!((l.message.limit, l.message.burst), (30, 10));
        assert_eq!(
            (l.export.limit, l.export.per_secs, l.export.burst),
            (6, 3600, 2)
        );
        assert_eq!((l.org.limit, l.org.burst), (3000, 600));
        let mut c = ApiRateSection {
            multiplier: 2.0,
            org_per_minute: 100,
            ..Default::default()
        };
        c.classes.insert(
            "write".into(),
            aurora_config::RateClassOverride {
                per_hour: Some(10),
                burst: Some(3),
                ..Default::default()
            },
        );
        let l = Limits::from_config(&c);
        assert_eq!(
            (l.write.limit, l.write.burst, l.write.per_secs),
            (20, 6, 3600)
        );
        assert_eq!(l.general.limit, 1200);
        assert_eq!((l.org.limit, l.org.burst), (200, 40));
    }

    #[test]
    fn burst_then_refill_and_retry_after() {
        let rl = Limiter::new(limits());
        let spec = Spec::per_min(60, 3); // 1/s
        let t0 = Instant::now();
        for _ in 0..3 {
            rl.take_at(t0, &[ch("a", spec, "class")]).unwrap();
        }
        let r = rl.take_at(t0, &[ch("a", spec, "class")]).unwrap_err();
        assert_eq!(r.retry_after_secs, 1);
        assert_eq!(r.scope, "class");
        // half a second later still blocked, a second later one token
        assert!(
            rl.take_at(t0 + Duration::from_millis(500), &[ch("a", spec, "class")])
                .is_err()
        );
        rl.take_at(t0 + Duration::from_secs(1), &[ch("a", spec, "class")])
            .unwrap();
        // empty again; slow bucket reports a long wait
        let slow = Spec::per_hour(6, 1); // 1 per 600 s
        rl.take_at(t0, &[ch("s", slow, "class")]).unwrap();
        let r = rl.take_at(t0, &[ch("s", slow, "class")]).unwrap_err();
        assert_eq!(r.retry_after_secs, 600);
    }

    #[test]
    fn refused_request_consumes_nothing_and_blocker_reported() {
        let rl = Limiter::new(limits());
        let big = Spec::per_min(600, 100);
        let small = Spec::per_min(60, 1);
        let t0 = Instant::now();
        rl.take_at(t0, &[ch("g", big, "general"), ch("c", small, "class")])
            .unwrap();
        let r = rl
            .take_at(t0, &[ch("g", big, "general"), ch("c", small, "class")])
            .unwrap_err();
        assert_eq!(r.scope, "class");
        // the general bucket was not charged by the refused attempt: 100 - 1 left → 99
        for _ in 0..99 {
            rl.take_at(t0, &[ch("g", big, "general")]).unwrap();
        }
        assert!(rl.take_at(t0, &[ch("g", big, "general")]).is_err());
    }

    #[test]
    fn principals_are_isolated_and_org_is_shared() {
        let rl = Limiter::new(limits());
        let spec = Spec::per_min(60, 1);
        let org = Spec::per_min(60, 2);
        let t0 = Instant::now();
        rl.take_at(
            t0,
            &[
                ch("key:a|general", spec, "general"),
                ch("org:o", org, "org"),
            ],
        )
        .unwrap();
        // same principal blocked, different principal unaffected
        assert!(
            rl.take_at(t0, &[ch("key:a|general", spec, "general")])
                .is_err()
        );
        rl.take_at(
            t0,
            &[
                ch("key:b|general", spec, "general"),
                ch("org:o", org, "org"),
            ],
        )
        .unwrap();
        // org exhausted by the two principals together
        let r = rl
            .take_at(
                t0,
                &[
                    ch("key:c|general", spec, "general"),
                    ch("org:o", org, "org"),
                ],
            )
            .unwrap_err();
        assert_eq!(r.scope, "org");
    }

    #[test]
    fn idle_eviction_and_cap() {
        let rl = Limiter::new(limits());
        let spec = Spec::per_min(60, 5);
        let t0 = Instant::now();
        rl.take_at(t0, &[ch("x", spec, "general")]).unwrap();
        rl.take_at(t0 + Duration::from_secs(300), &[ch("y", spec, "general")])
            .unwrap();
        assert_eq!(rl.tracked(), 2);
        rl.evict_idle(t0 + IDLE_EVICT + Duration::from_secs(1));
        assert_eq!(rl.tracked(), 1);
    }

    #[test]
    fn stream_cap_and_release() {
        let rl = Arc::new(Limiter::new(limits()));
        let guards: Vec<_> = (0..STREAMS_PER_PRINCIPAL)
            .map(|_| rl.acquire_stream("user:1").unwrap())
            .collect();
        assert!(rl.acquire_stream("user:1").is_none());
        assert!(rl.acquire_stream("user:2").is_some());
        drop(guards);
        assert_eq!(rl.open_streams("user:1"), 0);
        assert!(rl.acquire_stream("user:1").is_some());
    }

    #[test]
    fn path_targets() {
        let id = Uuid::nil();
        assert_eq!(path_target(&format!("/v1/orgs/{id}/x")), Some(("org", id)));
        assert_eq!(
            path_target(&format!("/v1/workspaces/{id}")),
            Some(("workspace", id))
        );
        assert_eq!(path_target("/v1/agents/abc"), None);
    }
}
