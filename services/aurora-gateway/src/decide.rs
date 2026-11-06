//! The MITM-lane `RequestHandler`: resolution, credential selection, policy
//! decision, injection, guidance and request log (docs/06 steps 4, 6, 7, 11, 14, 15).
//!
//! Composition: [`PolicyHandler`] = [`Resolver`] (cached policy view and
//! candidates), [`PlaintextCache`] (lazy plaintext, after the decision),
//! `MemoryCounter` (rate limits) and [`RequestLog`].

use crate::auth::{CredentialKind, RequestIdentity as Ident};
use crate::inject::{self, Target};
use crate::pipeline::{
    OpenLaneDecision, Outcome, PipelineRequest, PostHook, PreparedRequest, RequestHandler,
    TunnelReport,
};
use crate::plaintext::{PlaintextCache, PlaintextError};
use crate::reqlog::{RequestLog, Row};
use crate::resolve::{ResolvedHost, Resolver, SecretCand};
use crate::{Body, BoxError, guidance};
use aurora_core::ApiError;
use aurora_core::limits::{CONDITION_BODY_BUFFER, CONNECTION_CHOICE_MAX};
use aurora_policy::eval::{BlockReason, Decision, MemoryCounter, RateCounter, Verdict};
use aurora_policy::facts::Level;
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
    /// Churn column updates; `None` disables them (tests).
    touch_pool: Option<aurora_db::PgPool>,
    touched: Mutex<HashMap<Uuid, Instant>>,
}

pub struct PolicyHandlerParts {
    pub resolver: Arc<Resolver>,
    pub plaintext: Arc<PlaintextCache>,
    pub log: Arc<RequestLog>,
    pub external_url: String,
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
            app_id: None,
            tool_id: None,
            connection_id: None,
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
    ungranted: Option<(String, String)>,
    create_secret_url: String,
    manage_url: String,
}

impl PostHook for Hook {
    fn on_response(
        self: Box<Self>,
        status: StatusCode,
        _headers: &HeaderMap,
    ) -> Option<Response<Body>> {
        let mut ctx = self.ctx;
        let mut replacement = None;
        let mut code = None;
        if ctx.injected == 0 && matches!(status.as_u16(), 401 | 403) {
            let err = match &self.ungranted {
                Some((kind, provider)) => ApiError::new(
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
                None => ApiError::new(
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

fn facts_identity(i: &Ident) -> RequestIdentity {
    RequestIdentity {
        agent_id: i.agent_id,
        principals: i.principals.clone(),
    }
}

enum Pick {
    None,
    One(Arc<SecretCand>),
    Refuse(ApiError),
}

fn choices(c: &[Arc<SecretCand>]) -> serde_json::Value {
    c.iter()
        .take(CONNECTION_CHOICE_MAX as usize)
        .map(|s| {
            json!({
                "id": s.id, "app_id": null, "label": s.name,
                "scope": if s.scope == Level::Workspace { "workspace" } else { "organization" },
            })
        })
        .collect()
}

/// Step 6: choose among the selected candidates (ids only).
fn pick(mut sel: Vec<Arc<SecretCand>>, named: Option<&str>) -> Pick {
    if let Some(n) = named {
        let found = Uuid::parse_str(n)
            .ok()
            .and_then(|id| sel.iter().find(|c| c.id == id).cloned());
        return match found {
            Some(c) => Pick::One(c),
            None => Pick::Refuse(
                ApiError::not_found("No granted credential with that id is available here.")
                    .with_code_override("connection_not_found"),
            ),
        };
    }
    // Path-limited over whole-host, then workspace over organization.
    if sel.iter().any(|c| c.path.is_some()) {
        sel.retain(|c| c.path.is_some());
    }
    if sel.iter().any(|c| c.scope == Level::Workspace) {
        sel.retain(|c| c.scope == Level::Workspace);
    }
    match sel.len() {
        0 => Pick::None,
        1 => Pick::One(sel.remove(0)),
        _ => Pick::Refuse(
            ApiError::new(
                StatusCode::CONFLICT,
                "multiple_providers",
                "Several granted credentials match this request. Choose one by sending the x-aurora-connection-id header with the id of one of the choices.",
            )
            .with_details(json!({ "choices": choices(&sel), "header": "x-aurora-connection-id" })),
        ),
    }
}

trait CodeOverride {
    fn with_code_override(self, code: &str) -> Self;
}
impl CodeOverride for ApiError {
    fn with_code_override(self, code: &str) -> Self {
        let mut e = ApiError::new(self.status(), code, self.message().to_owned());
        if let Some(d) = self.details() {
            e = e.with_details(d.clone());
        }
        e
    }
}

/// Reads up to `limit` bytes of the body; returns the prefix, `truncated`, and a
/// body that replays the prefix followed by the rest.
async fn buffer_prefix(mut body: Body, limit: usize) -> (Bytes, bool, Body) {
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
            touch_pool: p.touch_pool,
            touched: Mutex::new(HashMap::new()),
        }
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
                // TODO(GW-004): create an approval hold instead of failing closed.
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
        // Candidates serving this path; selection by allow rules (07 §Injection selection).
        let cands: Vec<Arc<SecretCand>> = rv
            .secrets
            .iter()
            .filter(|c| c.path.as_ref().is_none_or(|p| p.matches(&path)))
            .cloned()
            .collect();
        let creds: Vec<Credential> = cands.iter().map(|c| c.credential()).collect();
        let selected: HashSet<_> = selected_candidates(rv.all_rules(), &fi, &creds);
        let (granted, ungranted): (Vec<_>, Vec<_>) = cands
            .into_iter()
            .partition(|c| selected.contains(&c.credential().key()));
        let winner = match pick(granted, aurora_headers.connection_id.as_deref()) {
            Pick::None => None,
            Pick::One(c) => Some(c),
            Pick::Refuse(e) => return Outcome::Respond(self.refuse(ctx, &e, "error")),
        };
        // 4. body prefix, only when a rule inspects the body.
        let (body_prefix, truncated, body) = if rv.needs_body() {
            let (p, t, b) = buffer_prefix(body, CONDITION_BODY_BUFFER).await;
            (p.to_vec(), t, b)
        } else {
            (Vec::new(), false, body)
        };
        // 7. decision (nothing decrypted yet).
        let facts = RequestFacts {
            host: host.clone(),
            port,
            path: path.clone(),
            query: query.clone(),
            method: method.to_string(),
            body_prefix,
            truncated,
            app_id: None,
            tool_id: None,
            tool_tags: Default::default(),
            winning_credential: winner.as_ref().map(|c| c.credential()),
            is_llm_host: rv.is_llm_host,
            identity: fi,
        };
        let d = evaluate(
            &rv.org_policy,
            &rv.ws_policy,
            &facts,
            self.counters.as_ref(),
        );
        ctx.secret_id = winner.as_ref().map(|c| c.id);
        ctx.rule = d.deciding_rule().map(|r| (r.logical_id, r.name.clone()));
        if let Some((err, decision)) = self.verdict_error(&rv, &d, &ctx.method, &path, &host) {
            return Outcome::Respond(self.refuse(ctx, &err, decision));
        }
        self.apply_hits(&d);
        // 11. injection (plaintext fetched only now).
        if let Some(c) = &winner {
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
                    let err = credential_unavailable();
                    return Outcome::Respond(self.refuse(ctx, &err, "error"));
                }
            };
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
                return Outcome::Respond(self.refuse(ctx, &credential_unavailable(), "error"));
            }
            ctx.injected = 1;
            self.touch(c.id);
            let hook = self.hook(&rv, ctx, &ungranted);
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
            });
        }
        let hook = self.hook(&rv, ctx, &ungranted);
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
        };
        let mut row = ctx.row(200, None, None);
        row.bytes_up = i64::try_from(r.bytes_up).unwrap_or(i64::MAX);
        row.bytes_down = i64::try_from(r.bytes_down).unwrap_or(i64::MAX);
        self.log.record(row);
    }
}

impl PolicyHandler {
    fn hook(
        &self,
        rv: &ResolvedHost,
        ctx: Ctx,
        ungranted: &[Arc<SecretCand>],
    ) -> Box<dyn PostHook> {
        let ungranted = ungranted.first().map(|c| {
            (
                "secret".to_owned(),
                c.provider.clone().unwrap_or_else(|| c.name.clone()),
            )
        });
        Box::new(Hook {
            ctx,
            log: self.log.clone(),
            ungranted,
            create_secret_url: self.url(rv, "secrets"),
            manage_url: self.url(rv, "policy"),
        })
    }
}

fn credential_unavailable() -> ApiError {
    ApiError::bad_gateway(
        "credential_unavailable",
        "A granted credential could not be produced. Retry later or contact a workspace admin.",
    )
    .with_details(json!({ "kind": "secret" }))
}
