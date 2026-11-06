//! The MITM-lane `RequestHandler`: resolution, credential selection, policy
//! decision, injection, guidance and request log (docs/06 steps 4, 6, 7, 11, 14, 15).
//!
//! Composition: [`PolicyHandler`] = [`Resolver`] (cached policy view and
//! candidates), [`PlaintextCache`] (lazy plaintext, after the decision),
//! `MemoryCounter` (rate limits) and [`RequestLog`].

use crate::apps::{self, Cand, Exec, Pick};
use crate::auth::{CredentialKind, RequestIdentity as Ident};
use crate::governance::{BackstopInfo, Governance};
use crate::hold::{HoldOutcome, HoldRequest, Holds};
use crate::inject::{self, Target};
use crate::mechanisms;
use crate::pipeline::{
    OpenLaneDecision, Outcome, PipelineRequest, PostHook, PreparedRequest, RequestHandler,
    TunnelReport,
};
use crate::plaintext::{PlaintextCache, PlaintextError};
use crate::reqlog::{RequestLog, Row};
use crate::resolve::{ResolvedHost, Resolver, SecretCand};
use crate::{Body, BoxError, guidance};
use aurora_core::ApiError;
use aurora_core::limits::{APPROVAL_BODY_PEEK, CONDITION_BODY_BUFFER};
use aurora_policy::eval::{BlockReason, Decision, MemoryCounter, RateCounter, Verdict};
use aurora_policy::{Credential, RequestFacts, RequestIdentity, evaluate, selected_candidates};
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use http::{HeaderMap, Response, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

const TOUCH_INTERVAL: Duration = Duration::from_secs(60);

pub struct PolicyHandler {
    resolver: Arc<Resolver>,
    plaintext: Arc<PlaintextCache>,
    counters: Arc<MemoryCounter>,
    log: Arc<RequestLog>,
    external_url: String,
    holds: Option<Arc<Holds>>,
    /// Churn column updates; `None` disables them (tests).
    touch_pool: Option<aurora_db::PgPool>,
    touched: Mutex<HashMap<Uuid, Instant>>,
    governance: Option<Arc<Governance>>,
}

pub struct PolicyHandlerParts {
    pub resolver: Arc<Resolver>,
    pub plaintext: Arc<PlaintextCache>,
    pub log: Arc<RequestLog>,
    pub external_url: String,
    /// Approval holds; `None` fails needs-approval requests closed (503).
    pub holds: Option<Arc<Holds>>,
    pub touch_pool: Option<aurora_db::PgPool>,
}

fn cred_kind(k: CredentialKind) -> &'static str {
    match k {
        CredentialKind::Agent => "agent",
        CredentialKind::Boot => "boot",
        CredentialKind::Context => "context",
    }
}

/// Per-request facts shared by the log row and the post hook.
struct Ctx {
    identity: Arc<Ident>,
    lane: &'static str,
    method: String,
    host: String,
    path: String,
    started: Instant,
    secret_id: Option<Uuid>,
    decision: &'static str,
    rule: Option<(Uuid, String)>,
    injected: i16,
    app_id: Option<String>,
    tool_id: Option<String>,
    connection_id: Option<Uuid>,
}

impl Ctx {
    fn row(&self, status: u16, upstream: Option<u16>, error_code: Option<&str>) -> Row {
        let i = &self.identity;
        Row {
            id: Uuid::now_v7(),
            ts: chrono::Utc::now(),
            org_id: i.org_id,
            workspace_id: i.workspace_id,
            agent_id: i.agent_id,
            conversation_id: i.conversation_id,
            turn_id: i.turn_id,
            credential_kind: cred_kind(i.kind),
            lane: self.lane,
            method: self.method.clone(),
            host: self.host.clone(),
            path: self.path.clone(),
            app_id: self.app_id.clone(),
            tool_id: self.tool_id.clone(),
            connection_id: self.connection_id,
            secret_id: self.secret_id,
            decision: self.decision,
            rule_logical_id: self.rule.as_ref().map(|r| r.0),
            rule_name: self.rule.as_ref().map(|r| r.1.clone()),
            upstream_status: upstream.map(i32::from),
            status: i32::from(status),
            error_code: error_code.map(str::to_owned),
            injected: self.injected,
            latency_ms: i32::try_from(self.started.elapsed().as_millis()).unwrap_or(i32::MAX),
            bytes_up: 0,
            bytes_down: 0,
        }
    }
}

struct Hook {
    ctx: Ctx,
    log: Arc<RequestLog>,
    /// Guidance for an upstream 401/403 when nothing was injected.
    fallback: Fallback,
    /// `registry_token_challenge`: let anonymous registry token flows proceed.
    passthrough_challenge: bool,
    create_secret_url: String,
    manage_url: String,
    connect_url: String,
    /// Step 14 origin backstop for an injected LLM key.
    backstop: Option<(Arc<Governance>, BackstopInfo)>,
}

impl PostHook for Hook {
    fn on_response(
        self: Box<Self>,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Option<Response<Body>> {
        let mut ctx = self.ctx;
        let mut replacement = None;
        let mut code = None;
        if let Some((g, b)) = &self.backstop
            && ctx.injected > 0
            && let Some(err) = g.backstop(b, headers)
        {
            ctx.decision = "governance_refused";
            code = Some(err.code().to_owned());
            let r = guidance::response(&err);
            self.log
                .record(ctx.row(r.status().as_u16(), Some(status.as_u16()), code.as_deref()));
            return Some(r);
        }
        let challenge = self.passthrough_challenge
            && status.as_u16() == 401
            && headers
                .get(http::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("bearer"));
        if ctx.injected == 0 && matches!(status.as_u16(), 401 | 403) && !challenge {
            let err = match &self.fallback {
                Fallback::Ungranted(kind, provider) => ApiError::new(
                    status,
                    "access_restricted",
                    format!(
                        "The upstream rejected the request. This workspace holds a credential for {} that this agent has not been granted. Ask a workspace admin to grant it: {}",
                        ctx.host, self.manage_url
                    ),
                )
                .with_details(json!({
                    "kind": kind, "provider": provider, "manage_url": self.manage_url
                })),
                Fallback::Disabled(app_id) => ApiError::new(
                    status,
                    "access_restricted",
                    format!(
                        "The upstream rejected the request. The connection for {app_id} is disabled. Ask a workspace admin to enable it: {}",
                        self.manage_url
                    ),
                )
                .with_details(json!({
                    "kind": "connection", "provider": app_id, "reason": "disabled",
                    "manage_url": self.manage_url
                })),
                Fallback::NotConnected(app_id) => ApiError::new(
                    status,
                    "app_not_connected",
                    format!(
                        "{app_id} is not connected in this workspace. Connect it at {}",
                        self.connect_url
                    ),
                )
                .with_details(json!({ "app_id": app_id, "connect_url": self.connect_url })),
                Fallback::PathUnserved => ApiError::new(
                    status,
                    "app_path_unserved",
                    format!(
                        "No installed app serves this path on {}. Check the request path.",
                        ctx.host
                    ),
                )
                .with_details(json!({ "host": ctx.host })),
                Fallback::NotFound => ApiError::new(
                    status,
                    "credential_not_found",
                    format!(
                        "The upstream rejected the request and Aurora holds no credential for {}. Store one at {}",
                        ctx.host, self.create_secret_url
                    ),
                )
                .with_details(json!({ "create_secret_url": self.create_secret_url })),
            };
            code = Some(err.code().to_owned());
            replacement = Some(guidance::response(&err));
        }
        if ctx.decision == "allow" && ctx.injected == 0 && ctx.rule.is_none() {
            ctx.decision = "allow";
        }
        self.log
            .record(ctx.row(status.as_u16(), Some(status.as_u16()), code.as_deref()));
        replacement
    }

    fn on_error(self: Box<Self>, err: &ApiError) {
        let mut ctx = self.ctx;
        ctx.decision = "error";
        self.log
            .record(ctx.row(err.status().as_u16(), None, Some(err.code())));
    }
}

/// What to tell the agent when the upstream rejects an uncredentialed request.
enum Fallback {
    Ungranted(String, String),
    Disabled(String),
    NotConnected(String),
    PathUnserved,
    NotFound,
}

fn facts_identity(i: &Ident) -> RequestIdentity {
    RequestIdentity {
        agent_id: i.agent_id,
        principals: i.principals.clone(),
    }
}

/// Reads up to `limit` bytes of the body; returns the prefix, `truncated`, and a
/// body that replays the prefix followed by the rest.
pub(crate) async fn buffer_prefix(mut body: Body, limit: usize) -> (Bytes, bool, Body) {
    let mut buf = BytesMut::new();
    let mut ended = false;
    let mut failed = None;
    while buf.len() < limit {
        match body.frame().await {
            None => {
                ended = true;
                break;
            }
            Some(Ok(f)) => {
                if let Ok(d) = f.into_data() {
                    buf.extend_from_slice(&d);
                }
            }
            Some(Err(e)) => {
                failed = Some(e);
                break;
            }
        }
    }
    let all = buf.freeze();
    let prefix = all.slice(..all.len().min(limit));
    let head = futures::stream::once({
        let all = all.clone();
        async move { Ok::<_, BoxError>(Frame::data(all)) }
    });
    let tail: futures::stream::BoxStream<'static, Result<Frame<Bytes>, BoxError>> = if ended {
        futures::stream::empty().boxed()
    } else if let Some(e) = failed {
        futures::stream::once(async move { Err(e) }).boxed()
    } else {
        futures::stream::unfold(body, |mut b| async move { b.frame().await.map(|f| (f, b)) })
            .boxed()
    };
    let replay = StreamBody::new(head.chain(tail)).boxed_unsync();
    let truncated = !ended || all.len() > limit;
    (prefix, truncated, replay)
}

impl PolicyHandler {
    pub fn new(p: PolicyHandlerParts) -> Self {
        Self {
            resolver: p.resolver,
            plaintext: p.plaintext,
            counters: Arc::new(MemoryCounter::new()),
            log: p.log,
            external_url: p.external_url.trim_end_matches('/').to_owned(),
            holds: p.holds,
            touch_pool: p.touch_pool,
            touched: Mutex::new(HashMap::new()),
            governance: None,
        }
    }

    /// Enable origin preflight/backstop (the resolver carries the effective governance).
    pub fn with_governance(mut self, g: Arc<Governance>) -> Self {
        self.governance = Some(g);
        self
    }

    fn url(&self, rv: &ResolvedHost, tail: &str) -> String {
        format!(
            "{}/o/{}/w/{}/{tail}",
            self.external_url, rv.org_slug, rv.ws_slug
        )
    }

    fn touch(&self, secret_id: Uuid) {
        let Some(pool) = self.touch_pool.clone() else {
            return;
        };
        {
            let Ok(mut t) = self.touched.lock() else {
                return;
            };
            let now = Instant::now();
            if t.get(&secret_id)
                .is_some_and(|l| now.duration_since(*l) < TOUCH_INTERVAL)
            {
                return;
            }
            t.insert(secret_id, now);
        }
        tokio::spawn(async move {
            let _ = aurora_db::repo::gateway::touch_secret_last_used(&pool, secret_id).await;
        });
    }

    fn refuse(&self, mut ctx: Ctx, err: &ApiError, decision: &'static str) -> Response<Body> {
        ctx.decision = decision;
        self.log
            .record(ctx.row(err.status().as_u16(), None, Some(err.code())));
        guidance::response(err)
    }

    /// Maps a non-allow verdict to its guidance error and log decision.
    fn verdict_error(
        &self,
        rv: &ResolvedHost,
        d: &Decision,
        method: &str,
        path: &str,
        host: &str,
    ) -> Option<(ApiError, &'static str)> {
        let policy_url = self.url(rv, "policy");
        match d.verdict {
            Verdict::Allow => None,
            Verdict::Block {
                reason: BlockReason::Rule,
            } => {
                let name = d.deciding_rule().map(|r| r.name.clone()).unwrap_or_default();
                Some((
                    ApiError::new(
                        StatusCode::FORBIDDEN,
                        "blocked_by_policy",
                        format!("The rule `{name}` blocks this request. Review policy at {policy_url}"),
                    )
                    .with_details(json!({
                        "rule_name": name, "method": method, "path": path, "manage_url": policy_url
                    })),
                    "blocked",
                ))
            }
            Verdict::Block {
                reason: BlockReason::DefaultDenied,
            } => Some((
                ApiError::new(
                    StatusCode::FORBIDDEN,
                    "blocked_by_default_policy",
                    format!("No rule allows this credentialed request (default policy is block). Add an allow rule at {policy_url}"),
                )
                .with_details(json!({ "manage_url": policy_url })),
                "default_denied",
            )),
            Verdict::Block {
                reason: BlockReason::EgressDenied,
            } => {
                let url = format!("{}/o/{}/policy", self.external_url, rv.org_slug);
                Some((
                    ApiError::new(
                        StatusCode::FORBIDDEN,
                        "blocked_by_egress_policy",
                        format!("Uncredentialed requests to {host} are not allow-listed. Add the host to the egress allow-list at {url}"),
                    )
                    .with_details(json!({ "host": host, "manage_url": url })),
                    "egress_denied",
                ))
            }
            Verdict::RateLimited => {
                let rl = d.deciding_rule().and_then(|m| {
                    rv.all_rules()
                        .find(|r| r.logical_id == m.logical_id)
                        .and_then(|r| r.rate_limit)
                });
                let (limit, per, secs) = match rl {
                    Some(r) => (
                        r.count,
                        format!("{:?}", r.per).to_lowercase(),
                        r.per.seconds().min(60),
                    ),
                    None => (0, "minute".to_owned(), 60),
                };
                let mut e = ApiError::rate_limited(
                    secs,
                    format!("Rate limit of {limit} per {per} reached. Retry after {secs} seconds."),
                );
                e = e.with_details(json!({ "limit": limit, "per": per }));
                Some((e, "rate_limited"))
            }
            Verdict::NeedsApproval => Some((
                // Reached only without a hold store (and on the open lane, which refuses).
                ApiError::unavailable(
                    "approval_store_unavailable",
                    "This request needs manual approval, which is not available yet.",
                ),
                "error",
            )),
        }
    }

    fn apply_hits(&self, d: &Decision) {
        for h in d.rate_limit_hits() {
            self.counters.record(h.rule_key, h.agent, &h.window);
        }
    }
}

#[async_trait::async_trait]
impl RequestHandler for PolicyHandler {
    async fn handle(&self, req: PipelineRequest) -> Outcome {
        let PipelineRequest {
            identity,
            lane: _,
            scheme,
            host,
            port,
            method,
            path,
            mut query,
            mut headers,
            aurora_headers,
            upgrade,
            body,
        } = req;
        let mut ctx = Ctx {
            identity: identity.clone(),
            lane: "mitm",
            method: method.to_string(),
            host: host.clone(),
            path: path.clone(),
            started: Instant::now(),
            secret_id: None,
            decision: "allow",
            rule: None,
            injected: 0,
            app_id: None,
            tool_id: None,
            connection_id: None,
        };
        let rv = match self.resolver.resolve(&identity, &host, port).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "policy resolution failed");
                let err = ApiError::unavailable(
                    "policy_unavailable",
                    "Policy cannot be evaluated right now. Retry shortly.",
                );
                return Outcome::Respond(self.refuse(ctx, &err, "error"));
            }
        };
        let fi = facts_identity(&identity);
        // 5. app identification (host/path); the tool follows once the body is known.
        let registry = rv.registry.clone();
        let app_match = registry
            .as_ref()
            .and_then(|l| apps::identify_app(l, &host, port, &path));
        // Candidates serving this path; selection by allow rules (07 §Injection selection).
        let mut cands: Vec<Cand> = rv
            .secrets
            .iter()
            .filter(|c| c.path.as_ref().is_none_or(|p| p.matches(&path)))
            .cloned()
            .map(Cand::Secret)
            .collect();
        let app_unavailable = app_match
            .as_ref()
            .is_some_and(|m| rv.unavailable_apps.contains(&m.entry.app_id));
        if let Some(m) = &app_match {
            let same: Vec<_> = rv
                .conns
                .iter()
                .filter(|c| c.app_id == m.entry.app_id)
                .cloned()
                .collect();
            // Tenant-bound entries only offer connections stored for this host; when none
            // does, the mismatching ones stay so that the guard answers 421.
            let bound: Vec<_> = if m.entry.tenant.is_some() {
                let ok: Vec<_> = same
                    .iter()
                    .filter(|c| {
                        c.tenant_host
                            .as_deref()
                            .is_some_and(|t| t.eq_ignore_ascii_case(&host))
                    })
                    .cloned()
                    .collect();
                if ok.is_empty() { same } else { ok }
            } else {
                same
            };
            cands.extend(bound.into_iter().map(Cand::App));
        }
        let creds: Vec<Credential> = cands.iter().map(Cand::credential).collect();
        let selected: HashSet<_> = selected_candidates(rv.all_rules(), &fi, &creds);
        let (granted, ungranted): (Vec<_>, Vec<_>) = cands
            .into_iter()
            .partition(|c| selected.contains(&c.credential().key()));
        let mut response_headers = HeaderMap::new();
        if granted.len() > 1
            && let Ok(v) = http::HeaderValue::from_str(&apps::choices(&granted).to_string())
        {
            response_headers.insert("x-aurora-connections", v);
        }
        let winner = match apps::pick(granted, aurora_headers.connection_id.as_deref()) {
            Pick::None => None,
            Pick::One(c) => Some(c),
            Pick::Refuse(e) => return Outcome::Respond(self.refuse(ctx, &e, "error")),
        };
        let wsec: Option<&SecretCand> = winner.as_ref().and_then(Cand::secret).map(|a| &**a);
        if let Some(Cand::App(c)) = &winner {
            ctx.connection_id = Some(c.id);
        }
        ctx.app_id = app_match.as_ref().map(|m| m.entry.app_id.clone());
        if winner.is_none() && app_unavailable {
            let err = ApiError::new(
                StatusCode::FORBIDDEN,
                "app_unavailable",
                "This app is not available to this workspace under the organization's app policy.",
            )
            .with_details(json!({ "app_id": ctx.app_id }));
            return Outcome::Respond(self.refuse(ctx, &err, "blocked"));
        }
        // 4. body prefix, only when a rule inspects the body.
        let content_type = headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let may_hold = self.holds.is_some() && rv.all_rules().any(|r| r.require_approval);
        let host_port = format!("{host}:{port}");
        let llm_body = rv
            .governance
            .as_ref()
            .is_some_and(|g| Governance::needs_body(g, &host_port, wsec))
            || app_match
                .as_ref()
                .is_some_and(|m| apps::needs_body(m.app, &host, port, &path, method.as_str()));
        let (body_prefix, truncated, mut body) = if rv.needs_body() || may_hold || llm_body {
            let limit = if rv.needs_body() || llm_body {
                CONDITION_BODY_BUFFER
            } else {
                APPROVAL_BODY_PEEK
            };
            let (p, t, b) = buffer_prefix(body, limit).await;
            (p.to_vec(), t, b)
        } else {
            (Vec::new(), false, body)
        };
        let tool = app_match.as_ref().and_then(|m| {
            apps::identify_tool(
                m.app,
                &host,
                port,
                &path,
                query.as_deref(),
                method.as_str(),
                &body_prefix,
                truncated,
            )
        });
        ctx.tool_id = tool.map(|t| t.id.clone());
        // 6a. LLM governance (cached effective governance; nothing decrypted).
        if let (Some(gov), Some(g)) = (&self.governance, &rv.governance)
            && rv.is_llm_host
            && let Err(err) = gov.check_6a(
                g,
                &host_port,
                &method,
                &path,
                &body_prefix,
                truncated,
                wsec,
                &self.url(&rv, "secrets"),
            )
        {
            ctx.secret_id = wsec.map(|c| c.id);
            return Outcome::Respond(self.refuse(ctx, &err, "governance_refused"));
        }
        // 7. decision (nothing decrypted yet).
        let peek = body_prefix[..body_prefix.len().min(APPROVAL_BODY_PEEK)].to_vec();
        let peek_truncated = truncated || body_prefix.len() > APPROVAL_BODY_PEEK;
        let facts = RequestFacts {
            host: host.clone(),
            port,
            path: path.clone(),
            query: query.clone(),
            method: method.to_string(),
            body_prefix,
            truncated,
            app_id: ctx.app_id.clone(),
            tool_id: ctx.tool_id.clone(),
            tool_tags: tool.map(apps::tool_tags).unwrap_or_default(),
            winning_credential: winner.as_ref().map(Cand::credential),
            is_llm_host: rv.is_llm_host,
            identity: fi,
        };
        let d = evaluate(
            &rv.org_policy,
            &rv.ws_policy,
            &facts,
            self.counters.as_ref(),
        );
        ctx.secret_id = wsec.map(|c| c.id);
        ctx.rule = d.deciding_rule().map(|r| (r.logical_id, r.name.clone()));
        if d.verdict == Verdict::NeedsApproval
            && let Some(holds) = &self.holds
        {
            // 8. approval hold (nothing decrypted yet).
            let red_path = crate::reqlog::redact_path(&path);
            let summary = aurora_summary::generic(
                &ctx.method,
                &host,
                &red_path,
                query.as_deref(),
                content_type.as_deref(),
                &peek,
                peek_truncated,
            );
            let outcome = holds
                .hold(HoldRequest {
                    identity: &identity,
                    method: &ctx.method,
                    host: &host,
                    path: &red_path,
                    rule: ctx.rule.as_ref().map(|(i, n)| (*i, n.as_str())),
                    batch: &aurora_headers,
                    summary: &summary,
                })
                .await;
            match outcome {
                HoldOutcome::Approved => ctx.decision = "approved",
                HoldOutcome::Denied(id) => {
                    let err = ApiError::new(
                        StatusCode::FORBIDDEN,
                        "manual_approval_denied",
                        "A reviewer denied this request. Do not retry it unchanged.",
                    )
                    .with_details(json!({ "approval_id": id }));
                    return Outcome::Respond(self.refuse(ctx, &err, "approval_denied"));
                }
                HoldOutcome::Expired(id) => {
                    let err = ApiError::new(
                        StatusCode::FORBIDDEN,
                        "manual_approval_expired",
                        "No reviewer decided in time, so this request was not sent.",
                    )
                    .with_details(json!({ "approval_id": id }));
                    return Outcome::Respond(self.refuse(ctx, &err, "approval_expired"));
                }
                HoldOutcome::Restarting => {
                    ctx.decision = "error";
                    self.log
                        .record(ctx.row(503, None, Some("gateway_restarting")));
                    return Outcome::Respond(guidance::restarting());
                }
                HoldOutcome::StoreUnavailable => {
                    let err = ApiError::unavailable(
                        "approval_store_unavailable",
                        "The approval store is unavailable. Retry shortly.",
                    );
                    return Outcome::Respond(self.refuse(ctx, &err, "error"));
                }
            }
        } else if let Some((err, decision)) = self.verdict_error(&rv, &d, &ctx.method, &path, &host)
        {
            return Outcome::Respond(self.refuse(ctx, &err, decision));
        }
        self.apply_hits(&d);
        // 11. injection (plaintext fetched only now).
        if let Some(Cand::App(conn)) = &winner {
            let Some(m) = &app_match else {
                return Outcome::Respond(self.refuse(
                    ctx,
                    &credential_unavailable("connection"),
                    "error",
                ));
            };
            let unavailable = |s: &Self, ctx: Ctx| {
                Outcome::Respond(s.refuse(ctx, &credential_unavailable("connection"), "error"))
            };
            if let Some(t) = &m.entry.tenant {
                match mechanisms::route::tenant_guard(t, conn.tenant_host.as_deref(), &host) {
                    Ok(()) => {}
                    Err(mechanisms::route::GuardError::Mismatch { expected }) => {
                        let err = ApiError::new(
                            StatusCode::MISDIRECTED_REQUEST,
                            "host_mismatch",
                            format!("This connection is bound to {expected}; send the request to that host."),
                        )
                        .with_details(json!({ "expected_host": expected }));
                        return Outcome::Respond(self.refuse(ctx, &err, "guard_refused"));
                    }
                    Err(mechanisms::route::GuardError::Unavailable) => {
                        return unavailable(self, ctx);
                    }
                }
            }
            let raw = match self.plaintext.get_connection(conn).await {
                Ok(v) => v,
                Err(PlaintextError::StoreUnavailable(store_id)) => {
                    let err = ApiError::unavailable(
                        "secret_store_unavailable",
                        "The secret store is unreachable. Retry shortly.",
                    )
                    .with_details(json!({ "store_id": store_id }));
                    return Outcome::Respond(self.refuse(ctx, &err, "error"));
                }
                Err(PlaintextError::Unavailable) => return unavailable(self, ctx),
            };
            let pkg_ctx: &[String] = registry
                .as_ref()
                .and_then(|l| {
                    l.snapshot
                        .packages
                        .iter()
                        .find(|p| p.package_id == m.app.package_id)
                })
                .map_or(&[], |p| p.context.as_slice());
            let cmap = HashMap::from([
                ("org_id".to_owned(), identity.org_id.to_string()),
                ("workspace_id".to_owned(), identity.workspace_id.to_string()),
                ("instance_url".to_owned(), self.external_url.clone()),
            ]);
            let Some(exec) = Exec::new(m.app, pkg_ctx, conn, raw.expose(), cmap) else {
                return unavailable(self, ctx);
            };
            // 9. token interception: answered locally, never forwarded.
            if let Some(i) = &m.entry.intercept
                && mechanisms::intercept::matches(i, &method, &path)
            {
                let Some(tok) = exec.credential("access_token") else {
                    return unavailable(self, ctx);
                };
                let expires_in = conn
                    .expires_at
                    .map_or(3600, |e| (e - chrono::Utc::now()).num_seconds());
                ctx.injected = 1;
                self.touch(conn.id);
                self.log.record(ctx.row(200, None, None));
                return Outcome::Respond(mechanisms::intercept::token_response(tok, expires_in));
            }
            // 11-12. inject, route, transform, finalize.
            let vars = exec.vars();
            let mut new_path = path.clone();
            let mut up_host = host.clone();
            let mut ok = mechanisms::inject::apply_all(
                &m.entry.inject,
                &vars,
                &mut Target {
                    headers: &mut headers,
                    path: &mut new_path,
                    query: &mut query,
                },
            )
            .is_ok();
            if ok && let Some(r) = &m.entry.rewrite_host {
                match mechanisms::route::rewrite_host(r, &vars) {
                    Ok(h) => up_host = h,
                    Err(_) => ok = false,
                }
            }
            if ok && let Some(mechanisms::Transform::GitCommitTrailer(t)) = &m.entry.transform {
                match mechanisms::transform::git_commit_trailer(
                    t,
                    &vars,
                    &method,
                    &mut headers,
                    body,
                )
                .await
                {
                    Ok(b) => body = b,
                    Err(_) => return unavailable(self, ctx),
                }
            }
            if ok && let Some(mechanisms::Finalize::AwsSigv4(f)) = &m.entry.finalize {
                let host_hdr = if port == scheme.default_port() {
                    up_host.clone()
                } else {
                    format!("{up_host}:{port}")
                };
                ok = mechanisms::finalize::finalize(
                    f,
                    &vars,
                    &method,
                    &host_hdr,
                    &new_path,
                    query.as_deref(),
                    &mut headers,
                    chrono::Utc::now(),
                )
                .is_ok();
            }
            if !ok {
                return unavailable(self, ctx);
            }
            ctx.injected = 1;
            self.touch(conn.id);
            let fb = self.fallback(&rv, app_match.as_ref(), &ungranted, &host);
            let passthrough = pass_challenge(app_match.as_ref());
            let hook = self.hook(&rv, ctx, fb, passthrough, None);
            return Outcome::Forward(PreparedRequest {
                scheme,
                host: up_host,
                port,
                method,
                path: new_path,
                query,
                headers,
                upgrade,
                body,
                hook: Some(hook),
                response_headers,
            });
        }
        if let Some(Cand::Secret(c)) = &winner {
            let value = match self.plaintext.get(c).await {
                Ok(v) => v,
                Err(PlaintextError::StoreUnavailable(store_id)) => {
                    let err = ApiError::unavailable(
                        "secret_store_unavailable",
                        "The secret store is unreachable. Retry shortly.",
                    )
                    .with_details(json!({ "store_id": store_id }));
                    return Outcome::Respond(self.refuse(ctx, &err, "error"));
                }
                Err(PlaintextError::Unavailable) => {
                    let err = credential_unavailable("secret");
                    return Outcome::Respond(self.refuse(ctx, &err, "error"));
                }
            };
            let mut backstop = None;
            if let (Some(gov), Some(g)) = (&self.governance, &rv.governance)
                && let Some(info) =
                    Governance::backstop_info(g, &host_port, c, &self.url(&rv, "secrets"))
            {
                // Origin preflight: verify unseen key values before injecting.
                if let Err(err) = gov.preflight(&info, value.expose()).await {
                    return Outcome::Respond(self.refuse(ctx, &err, "governance_refused"));
                }
                backstop = Some((gov.clone(), info));
            }
            let mut new_path = path.clone();
            let r = inject::apply(
                c,
                value.expose(),
                Target {
                    headers: &mut headers,
                    path: &mut new_path,
                    query: &mut query,
                },
            );
            if r.is_err() {
                return Outcome::Respond(self.refuse(
                    ctx,
                    &credential_unavailable("secret"),
                    "error",
                ));
            }
            ctx.injected = 1;
            self.touch(c.id);
            let fb = self.fallback(&rv, app_match.as_ref(), &ungranted, &host);
            let hook = self.hook(&rv, ctx, fb, false, backstop);
            return Outcome::Forward(PreparedRequest {
                scheme,
                host,
                port,
                method,
                path: new_path,
                query,
                headers,
                upgrade,
                body,
                hook: Some(hook),
                response_headers,
            });
        }
        let fb = self.fallback(&rv, app_match.as_ref(), &ungranted, &host);
        let passthrough = pass_challenge(app_match.as_ref());
        let hook = self.hook(&rv, ctx, fb, passthrough, None);
        Outcome::Forward(PreparedRequest {
            scheme,
            host,
            port,
            method,
            path,
            query,
            headers,
            upgrade,
            body,
            hook: Some(hook),
            response_headers,
        })
    }

    async fn open_lane_check(&self, identity: &Ident, host: &str, port: u16) -> OpenLaneDecision {
        let ctx = Ctx {
            identity: Arc::new(identity.clone()),
            lane: "open",
            method: "CONNECT".into(),
            host: host.to_owned(),
            path: "/".into(),
            started: Instant::now(),
            secret_id: None,
            decision: "allow",
            rule: None,
            injected: 0,
            app_id: None,
            tool_id: None,
            connection_id: None,
        };
        let rv = match self.resolver.resolve(identity, host, port).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "policy resolution failed");
                let err = ApiError::unavailable(
                    "policy_unavailable",
                    "Policy cannot be evaluated right now. Retry shortly.",
                );
                self.refuse(ctx, &err, "error");
                return OpenLaneDecision::Deny(err);
            }
        };
        if let Some(g) = &rv.governance
            && rv.is_llm_host
            && let Err(err) = Governance::check_open_lane(g, &format!("{host}:{port}"))
        {
            self.refuse(ctx, &err, "governance_refused");
            return OpenLaneDecision::Deny(err);
        }
        let facts = RequestFacts {
            host: host.to_owned(),
            port,
            path: "/".into(),
            query: None,
            method: "CONNECT".into(),
            body_prefix: Vec::new(),
            truncated: false,
            app_id: None,
            tool_id: None,
            tool_tags: Default::default(),
            winning_credential: None,
            is_llm_host: rv.is_llm_host,
            identity: facts_identity(identity),
        };
        let d = evaluate(
            &rv.org_policy,
            &rv.ws_policy,
            &facts,
            self.counters.as_ref(),
        );
        let mut ctx = ctx;
        ctx.rule = d.deciding_rule().map(|r| (r.logical_id, r.name.clone()));
        match self.verdict_error(&rv, &d, "CONNECT", "/", host) {
            None => {
                self.apply_hits(&d);
                OpenLaneDecision::Allow
            }
            Some((err, decision)) => {
                // A needs-approval verdict refuses the CONNECT (06 §Lanes).
                let (err, decision) = if d.verdict == Verdict::NeedsApproval {
                    (
                        ApiError::new(
                            StatusCode::FORBIDDEN,
                            "manual_approval_denied",
                            "This destination needs manual approval, which the open lane cannot hold. Use the default proxy lane.",
                        ),
                        "approval_denied",
                    )
                } else {
                    (err, decision)
                };
                self.refuse(ctx, &err, decision);
                OpenLaneDecision::Deny(err)
            }
        }
    }

    fn tunnel_closed(&self, r: TunnelReport) {
        let ctx = Ctx {
            identity: r.identity,
            lane: "open",
            method: "CONNECT".into(),
            host: r.host,
            path: "/".into(),
            started: Instant::now() - r.duration,
            secret_id: None,
            decision: "allow",
            rule: None,
            injected: 0,
            app_id: None,
            tool_id: None,
            connection_id: None,
        };
        let mut row = ctx.row(200, None, None);
        row.bytes_up = i64::try_from(r.bytes_up).unwrap_or(i64::MAX);
        row.bytes_down = i64::try_from(r.bytes_down).unwrap_or(i64::MAX);
        self.log.record(row);
    }
}

impl PolicyHandler {
    fn fallback(
        &self,
        rv: &ResolvedHost,
        app: Option<&apps::AppMatch<'_>>,
        ungranted: &[Cand],
        _host: &str,
    ) -> Fallback {
        if let Some(c) = ungranted.first() {
            return match c {
                Cand::Secret(c) => Fallback::Ungranted(
                    "secret".to_owned(),
                    c.provider.clone().unwrap_or_else(|| c.name.clone()),
                ),
                Cand::App(c) => Fallback::Ungranted("connection".to_owned(), c.app_id.clone()),
            };
        }
        match app {
            Some(m) if rv.disabled_apps.contains(&m.entry.app_id) => {
                Fallback::Disabled(m.entry.app_id.clone())
            }
            Some(m) => Fallback::NotConnected(m.entry.app_id.clone()),
            None if rv.known_app_host => Fallback::PathUnserved,
            None => Fallback::NotFound,
        }
    }

    fn hook(
        &self,
        rv: &ResolvedHost,
        ctx: Ctx,
        fallback: Fallback,
        passthrough_challenge: bool,
        backstop: Option<(Arc<Governance>, BackstopInfo)>,
    ) -> Box<dyn PostHook> {
        let connect_url = match &ctx.app_id {
            Some(a) => self.url(rv, &format!("apps/{a}")),
            None => self.url(rv, "apps"),
        };
        Box::new(Hook {
            ctx,
            log: self.log.clone(),
            fallback,
            passthrough_challenge,
            create_secret_url: self.url(rv, "secrets"),
            manage_url: self.url(rv, "policy"),
            connect_url,
            backstop,
        })
    }
}

fn pass_challenge(m: Option<&apps::AppMatch<'_>>) -> bool {
    m.is_some_and(|m| {
        m.entry
            .passthrough
            .contains(&aurora_registry::manifest::Passthrough::RegistryTokenChallenge)
    })
}

fn credential_unavailable(kind: &str) -> ApiError {
    ApiError::bad_gateway(
        "credential_unavailable",
        "A granted credential could not be produced. Retry later or contact a workspace admin.",
    )
    .with_details(json!({ "kind": kind }))
}
