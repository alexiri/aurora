//! The MITM-lane `RequestHandler`: resolution, credential selection, policy
//! decision, injection, guidance and request log (docs/06 steps 4, 6, 7, 11, 14, 15).
//!
//! Composition: [`PolicyHandler`] = [`Resolver`] (cached policy view and
//! candidates), [`PlaintextCache`] (lazy plaintext, after the decision),
//! `MemoryCounter` (rate limits) and [`RequestLog`].

use crate::apps::{self, Cand, Exec, Pick};
use crate::auth::{CredentialKind, RequestIdentity as Ident};
use crate::db::{DbAuthorizer, DbEndpoint, DbOpenError, DbOpened, DbVerdict};
use crate::governance::{BackstopInfo, Governance};
use crate::hold::{HoldOutcome, HoldRequest, Holds};
use crate::inject::{self, Target};
use crate::links::Links;
use crate::mechanisms;
use crate::pipeline::{
    OpenLaneDecision, Outcome, PipelineRequest, PostHook, PreparedRequest, RequestHandler, Retry,
    TunnelReport,
};
use crate::plaintext::{PlaintextCache, PlaintextError};
use crate::ratelimit::{Peeked, RuleCounters, Settled};
use crate::refresh::{RefreshError, TokenManager};
use crate::reqlog::{RequestLog, Row};
use crate::resolve::{ResolvedHost, Resolver, SecretCand};
use crate::{Body, BoxError, guidance};
use aurora_core::ApiError;
use aurora_core::limits::{APPROVAL_BODY_PEEK, CONDITION_BODY_BUFFER};
use aurora_db::repo::gateway as repo;
use aurora_llm::governance::{OpenLaneAdmission, OpenLanePosture};
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
    /// (P4-CNT, D-119) shared rule quotas; per replica (`counters` above) when not configured.
    rate: RuleCounters,
    log: Arc<RequestLog>,
    external_url: String,
    holds: Option<Arc<Holds>>,
    /// Churn column updates; `None` disables them (tests).
    touch_pool: Option<aurora_db::PgPool>,
    touched: Mutex<HashMap<Uuid, Instant>>,
    governance: Option<Arc<Governance>>,
    tokens: Option<Arc<TokenManager>>,
    summarizer: Option<Arc<dyn crate::summarize_client::Summarizer>>,
    /// Body prefix buffered for condition evaluation (`gateway.condition_body_bytes`); a live
    /// setting (SET-5), read once per request.
    condition_body_limit: std::sync::atomic::AtomicUsize,
    /// (RFE-014) Outage state: churn writes are skipped and refresh failures map to
    /// `gateway_degraded` while the database is unreachable.
    health: Option<Arc<crate::health::Health>>,
    /// (FIX-104, D-180) `gateway.insecure_loopback_injection`, tests only: package host claims
    /// are injected over any scheme and port when the destination is a loopback address.
    insecure_loopback_injection: bool,
    /// (P4-LLM-2, C-807) Mints the credential of the minted LLM modes; `None` refuses them (502
    /// `credential_unavailable`, `kind: llm_mint`).
    llm_mint: Option<Arc<crate::llm_mint::LlmMint>>,
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
    /// (P4-CNT, D-119) the rate-limit count of this request came from the local fallback because
    /// the shared counter backend was unreachable: the row carries `counter_fallback`.
    counter_fallback: bool,
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
            error_code: error_code
                .map(str::to_owned)
                .or_else(|| self.counter_fallback.then(|| "counter_fallback".to_owned())),
            injected: self.injected,
            latency_ms: i32::try_from(self.started.elapsed().as_millis()).unwrap_or(i32::MAX),
            bytes_up: 0,
            bytes_down: 0,
            context_label: i.context_label.clone(),
            speaker_user_id: i.speaker_user_id,
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
    /// (FIX-153, C-893) The requesting agent's Access tab.
    access_url: String,
    /// (FIX-153, C-892) The matched app's id and display name, for LDAP refusals.
    app: Option<AppRef>,
    /// Step 14 origin backstop for an injected LLM key.
    backstop: Option<(Arc<Governance>, BackstopInfo)>,
    /// Injected values placed in the URL (FIX-104).
    echo: Vec<Vec<u8>>,
    /// (P4-LLM-1, C-808) `(provider, endpoint id)` of an injected LLM key: the response is classified for the
    /// request log and `aurora_gateway_llm_upstream_total`.
    llm: Option<(String, String)>,
}

impl Hook {
    /// (FIX-154, C-897) The matched app `app_id` is credential-free.
    fn credential_free(&self, app_id: &str) -> bool {
        matches!(&self.app, Some((id, _, true)) if id == app_id)
    }

    /// The `app_not_connected` refusal (C-893, C-897): a credential-free app leads with the
    /// agent's Access tab, any other with the connect link.
    fn not_connected(
        status: StatusCode,
        app_id: &str,
        free: bool,
        connect_url: &str,
        access_url: &str,
    ) -> ApiError {
        let msg = if free {
            format!(
                "{app_id} is not connected in this workspace. A workspace owner can give this agent access on its Access tab: {access_url}"
            )
        } else {
            format!(
                "{app_id} is not connected in this workspace. Connect it at {connect_url}, then give this agent access"
            )
        };
        ApiError::new(status, "app_not_connected", msg).with_details(json!({
            "app_id": app_id, "connect_url": connect_url, "access_url": access_url
        }))
    }
}

impl PostHook for Hook {
    fn echo_needles(&self) -> Vec<Vec<u8>> {
        self.echo.clone()
    }

    fn ldap_refusal(&self) -> Option<ApiError> {
        let name = |id: &str| match &self.app {
            Some((aid, n, _)) if aid == id => n.clone(),
            _ => id.to_owned(),
        };
        let err = match &self.fallback {
            Fallback::Ungranted(kind, app_id) if kind == "connection" => {
                let n = name(app_id);
                ApiError::new(
                    StatusCode::FORBIDDEN,
                    "ldap_no_connection",
                    format!(
                        "This agent has not been given access to {n}. A workspace owner can give it on the agent's Access tab: {}",
                        self.access_url
                    ),
                )
                .with_details(json!({
                    "app_id": app_id, "app_name": n, "reason": "not_granted",
                    "access_url": self.access_url
                }))
            }
            Fallback::NotConnected(app_id) => {
                let n = name(app_id);
                // (FIX-154, C-897) Nothing to sign in to: the fix is the agent's Access tab.
                let msg = if self.credential_free(app_id) {
                    format!(
                        "{n} is not connected in this workspace. A workspace owner can give this agent access on its Access tab: {}",
                        self.access_url
                    )
                } else {
                    format!(
                        "{n} is not connected in this workspace. A workspace owner can connect it at {} and give this agent access.",
                        self.connect_url
                    )
                };
                ApiError::new(StatusCode::FORBIDDEN, "ldap_no_connection", msg).with_details(
                    json!({
                        "app_id": app_id, "app_name": n, "reason": "not_connected",
                        "connect_url": self.connect_url, "access_url": self.access_url
                    }),
                )
            }
            _ => ApiError::new(
                StatusCode::FORBIDDEN,
                "ldap_no_connection",
                "No installed directory app serves this host.",
            )
            .with_details(json!({ "reason": "no_app" })),
        };
        Some(err)
    }

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
        // (P4-LLM-1, C-808) Classify an injected LLM request's answer by status (and Bedrock's error-type header):
        // the request log's `error_code` and the upstream metric. Never reads a body, never alters the answer.
        if let Some((provider, endpoint)) = &self.llm
            && ctx.injected > 0
        {
            let class = if status.is_success() {
                "ok"
            } else {
                let error_type = (provider == "bedrock")
                    .then(|| {
                        headers
                            .get("x-amzn-errortype")
                            .and_then(|v| v.to_str().ok())
                    })
                    .flatten();
                let c = aurora_llm::classify(provider, status.as_u16(), error_type);
                code = c.error_code().map(str::to_owned);
                c.as_str()
            };
            metrics::counter!(
                "aurora_gateway_llm_upstream_total",
                "provider" => provider.clone(),
                "endpoint" => endpoint.clone(),
                "class" => class
            )
            .increment(1);
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
                        "The upstream rejected the request. This workspace holds a credential for {} that this agent has not been granted. Ask a workspace owner to give this agent access: {}",
                        ctx.host, self.access_url
                    ),
                )
                .with_details(json!({
                    "kind": kind, "provider": provider, "manage_url": self.manage_url,
                    "access_url": self.access_url
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
                Fallback::NotConnected(app_id) => Hook::not_connected(
                    status,
                    app_id,
                    matches!(&self.app, Some((id, _, true)) if id == app_id),
                    &self.connect_url,
                    &self.access_url,
                ),
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

/// The matched app (for refusals that name it): id, display name and whether it is
/// credential-free (FIX-154, C-895), so its refusals lead with the Access tab.
type AppRef = (String, String, bool);

fn app_label(m: Option<&apps::AppMatch<'_>>) -> Option<AppRef> {
    m.map(|m| {
        (
            m.app.app_id.clone(),
            m.app.about.name.clone(),
            aurora_registry::credential_free::entry_is_credential_free(m.app),
        )
    })
}

fn facts_identity(i: &Ident) -> RequestIdentity {
    RequestIdentity {
        agent_id: i.agent_id,
        speaker_user_id: i.speaker_user_id,
        speaker_group_ids: i.speaker_group_ids.clone(),
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
            rate: RuleCounters::local(),
            log: p.log,
            external_url: p.external_url.trim_end_matches('/').to_owned(),
            holds: p.holds,
            touch_pool: p.touch_pool,
            touched: Mutex::new(HashMap::new()),
            governance: None,
            tokens: None,
            summarizer: None,
            condition_body_limit: std::sync::atomic::AtomicUsize::new(CONDITION_BODY_BUFFER),
            health: None,
            insecure_loopback_injection: false,
            llm_mint: None,
        }
    }

    /// (P4-LLM-2) The LLM credential mint service.
    #[must_use]
    pub fn with_llm_mint(mut self, m: Arc<crate::llm_mint::LlmMint>) -> Self {
        self.llm_mint = Some(m);
        self
    }

    /// (P4-LLM-2, C-806, C-807) The minted credential a secret's recipe needs (`None` for plain keys): produced by the
    /// mint service from the secret's own value, never taken from the request.
    async fn llm_token(
        &self,
        c: &SecretCand,
        value: &[u8],
    ) -> Result<Option<Arc<aurora_core::Secret<String>>>, aurora_llm::mint::MintError> {
        use aurora_llm::{AuthMode, InjectionHeader};
        let crate::resolve::Injection::Llm(l) = &c.injection else {
            return Ok(None);
        };
        let mode = match l.header {
            InjectionHeader::GcpToken => AuthMode::GcpServiceAccount,
            InjectionHeader::EntraToken => AuthMode::AzureEntra,
            _ => return Ok(None),
        };
        let unavailable = aurora_llm::mint::MintError::unavailable;
        let mint = self.llm_mint.as_ref().ok_or_else(unavailable)?;
        let llm = self.resolver.llm();
        let ep = c
            .llm_endpoint
            .as_deref()
            .and_then(|id| llm.endpoint(id))
            .ok_or_else(unavailable)?;
        mint.token(&crate::llm_mint::MintRequest {
            secret_id: c.id,
            version: c.version,
            org_id: c.org_id,
            workspace_id: c.workspace_id,
            endpoint: ep,
            mode,
            value,
        })
        .await
        .map(Some)
    }

    /// (FIX-104, D-180) `gateway.insecure_loopback_injection`: tests and development only.
    #[must_use]
    pub fn with_insecure_loopback_injection(mut self, on: bool) -> Self {
        self.insecure_loopback_injection = on;
        self
    }

    /// (P4-CNT, D-119) Count rule quotas in the shared store of the `gateway` class.
    #[must_use]
    pub fn with_rule_counters(mut self, r: RuleCounters) -> Self {
        self.rate = r;
        self
    }

    /// (RFE-014) Attach the outage state.
    #[must_use]
    pub fn with_health(mut self, h: Arc<crate::health::Health>) -> Self {
        self.health = Some(h);
        self
    }

    fn db_down(&self) -> bool {
        self.health.as_ref().is_some_and(|h| h.db_unreachable())
    }

    /// Set the condition body buffer (`gateway.condition_body_bytes`). Never below
    /// [`APPROVAL_BODY_PEEK`].
    pub fn with_condition_body_limit(self, bytes: usize) -> Self {
        self.set_condition_body_limit(bytes);
        self
    }

    /// Replace the condition body buffer live (SET-5); applies to the next request.
    pub fn set_condition_body_limit(&self, bytes: usize) {
        self.condition_body_limit.store(
            bytes.max(APPROVAL_BODY_PEEK),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Enable the `approval.summarize` hook for held requests.
    pub fn with_summarizer(mut self, s: Arc<dyn crate::summarize_client::Summarizer>) -> Self {
        self.summarizer = Some(s);
        self
    }

    /// Enable token refresh / mint and stale-token retry for app connections.
    pub fn with_tokens(mut self, t: Arc<TokenManager>) -> Self {
        self.tokens = Some(t);
        self
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

    /// (FIX-153, C-891) The dashboard link builder for a resolved workspace.
    fn links<'a>(&'a self, rv: &'a ResolvedHost) -> Links<'a> {
        Links::new(&self.external_url, &rv.org_slug, &rv.ws_slug)
    }

    fn touch(&self, secret_id: Uuid) {
        // (RFE-014) `last_used_at` writes are skipped while the database is unreachable.
        if self.db_down() {
            return;
        }
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
        tool: Option<(&str, &[String])>,
    ) -> Option<(ApiError, &'static str)> {
        let policy_url = self.url(rv, "policy");
        match d.verdict {
            Verdict::Allow => None,
            // (FIX-136, D-241) The token cannot serve the tool: refused before injection, so the
            // agent gets a clear answer instead of an upstream 403 and the token is not sent.
            Verdict::Block {
                reason: BlockReason::ToolScopeMissing,
            } => {
                let (tool_id, needs) = tool.unwrap_or(("", &[]));
                Some((
                    ApiError::new(
                        StatusCode::FORBIDDEN,
                        "tool_scope_missing",
                        format!(
                            "This connection's token does not have the scope this tool needs ({}). Reconnect with it, or edit the connection's scopes if the token has it.",
                            needs.join(" or ")
                        ),
                    )
                    .with_details(json!({
                        "tool_id": tool_id, "needs": needs, "method": method, "path": path,
                        "manage_url": policy_url
                    })),
                    "tool_scope_missing",
                ))
            }
            Verdict::Block {
                reason: BlockReason::Rule,
            } => {
                // (D-178, Q-R5-14) Workspace rules are named, with the workspace policy URL; an
                // organization-level rule is answered generically: no rule name, no policy URL.
                if d.trace.decided_by == Some(aurora_policy::eval::ScopeKind::Organization) {
                    return Some((
                        ApiError::new(
                            StatusCode::FORBIDDEN,
                            "blocked_by_policy",
                            "An organization policy blocks this request. Ask an organization admin if you need it allowed.".to_owned(),
                        )
                        .with_details(json!({
                            "scope": "organization", "method": method, "path": path
                        })),
                        "blocked",
                    ));
                }
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
            // (P4-GRANTS, D-123, SEC-67) Decided before any rule: the app's manifest says only
            // catalogued tools may use its credential.
            Verdict::Block {
                reason: BlockReason::AppUntooledBlocked,
            } => Some((
                ApiError::new(
                    StatusCode::FORBIDDEN,
                    "blocked_by_policy",
                    "This app only allows the operations in its tool list, and this request matches none of them. No rule can allow it.".to_owned(),
                )
                .with_details(json!({
                    "reason": "app_untooled_blocked", "method": method, "path": path,
                    "manage_url": policy_url
                })),
                "app_untooled_blocked",
            )),
            // (FIX-133, D-240) Decided before any rule: the connection is read-only.
            Verdict::Block {
                reason: BlockReason::ReadOnlyConnection,
            } => Some((
                ApiError::new(
                    StatusCode::FORBIDDEN,
                    "blocked_by_policy",
                    "This connection is read-only: it allows only the app's read tools, and this request is not one of them. No rule can allow it.".to_owned(),
                )
                .with_details(json!({
                    "reason": "read_only_connection", "method": method, "path": path,
                    "manage_url": policy_url
                })),
                "read_only_connection",
            )),
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

    /// Record the hits in this replica's memory: the `memory` backend and the `local` fallback.
    /// Under a shared backend the consume already happened ([`RuleCounters::settle`]).
    fn apply_hits(&self, d: &Decision, pk: &Peeked) {
        if !pk.is_local() {
            return;
        }
        for h in d.rate_limit_hits() {
            self.counters.record(h.rule_key, h.agent, &h.window);
        }
    }
}

/// (FIX-084) The state a released hold hands to the second pass of [`PolicyHandler::handle_with`].
#[derive(Clone, Copy)]
struct Released {
    /// The request's arrival, so the log row's latency includes the hold.
    started: Instant,
    /// Whether the client's body was empty (the replay stream no longer says).
    bodyless: bool,
    /// The held request had a credential to inject. One that no longer has any is refused
    /// (`credential_unavailable`), not forwarded without it.
    had_credential: bool,
}

/// The counter of a released request: its rule quotas were consumed once when the hold was
/// created, so the second evaluation must neither be refused by that very count nor charge again.
struct NoCount;

impl RateCounter for NoCount {
    fn is_exhausted(
        &self,
        _rule_key: Uuid,
        _agent: Uuid,
        _window: &aurora_policy::rule::RateLimit,
    ) -> bool {
        false
    }
    fn record(&self, _rule_key: Uuid, _agent: Uuid, _window: &aurora_policy::rule::RateLimit) {}
}

impl PolicyHandler {
    /// The MITM-lane decision. `released` is `Some` on the second pass that follows an approved
    /// hold: an approval authorizes the held request only under the policy in force when it is
    /// released (docs/07 §Lifecycle), so the credential is resolved and the policy evaluated
    /// again; a request that would now be blocked is refused as on arrival, one that would still
    /// need approval is released (the approval stands, no second hold, no second charge).
    async fn handle_with(&self, req: PipelineRequest, released: Option<Released>) -> Outcome {
        let PipelineRequest {
            identity,
            lane,
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
        let bodyless =
            released.map_or_else(|| hyper::body::Body::is_end_stream(&body), |r| r.bodyless);
        let mut ctx = Ctx {
            identity: identity.clone(),
            lane: "mitm",
            method: method.to_string(),
            host: host.clone(),
            path: path.clone(),
            started: released.map_or_else(Instant::now, |r| r.started),
            secret_id: None,
            decision: "allow",
            rule: None,
            injected: 0,
            app_id: None,
            tool_id: None,
            connection_id: None,
            counter_fallback: false,
        };
        let rv = match self.resolver.resolve(&identity, &host, port).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "policy resolution failed");
                let err = policy_unavailable(&e);
                return Outcome::Respond(self.refuse(ctx, &err, "error"));
            }
        };
        let fi = facts_identity(&identity);
        // 5. app identification (host/path); the tool follows once the body is known.
        let registry = rv.registry.clone();
        // (D-74) A host that is exactly a registered app instance serves that app's `{instance}`
        // host entries; otherwise the static host index decides.
        // (D-231, SEC-59) Registrations may share the host under disjoint base paths: the one whose
        // base path covers the (already normalized) path serves the request, and every manifest
        // decision (host entry `path_prefix`, tool `path`/`params`, `{path.N}`) uses the **app
        // path**, the path below the base path. A path outside every base path matches no
        // instance: nothing is injected.
        let inst =
            aurora_registry::instance::select(&rv.instances, &path, |i| i.base_path.as_deref());
        let inst_id = inst.map(|i| i.id);
        let inst_app_path = inst
            .map(|i| aurora_registry::instance::app_path(i.base_path.as_deref(), &path).to_owned());
        let app_match = registry.as_ref().and_then(|l| {
            inst.zip(inst_app_path.as_deref())
                .and_then(|(i, ap)| apps::identify_instance_app(l, &i.app_id, ap))
                .or_else(|| apps::identify_app(l, &host, port, &path))
        });
        let app_path: String = match (&app_match, inst_app_path) {
            (Some(m), Some(ap)) if m.instance => ap,
            _ => path.clone(),
        };
        // A named connection of a registration on this host, used outside every registered base
        // path: refused like a connection on the wrong host (421 with `expected_base_url`), when
        // the agent may use it at all.
        if inst.is_none()
            && !rv.instances.is_empty()
            && let Some(c) = aurora_headers
                .connection_id
                .as_deref()
                .and_then(|n| Uuid::parse_str(n).ok())
                .and_then(|n| rv.conns.iter().find(|c| c.id == n))
                .filter(|c| {
                    c.instance_id.is_some()
                        && c.instance_host
                            .as_deref()
                            .is_some_and(|h| h.eq_ignore_ascii_case(&host))
                })
            && !selected_candidates(rv.all_rules(), &fi, &[Cand::App(c.clone()).credential()])
                .is_empty()
        {
            let err = guidance::host_mismatch(&host, c.instance_base_path.as_deref());
            return Outcome::Respond(self.refuse(ctx, &err, "guard_refused"));
        }
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
            let on_instance = inst_id.filter(|_| m.instance);
            let bound: Vec<_> = if let Some(iid) = on_instance {
                // Instance host: only connections bound to this very instance; connections of
                // other instances stay so the guard answers 421, default-host ones never ride here.
                let ok: Vec<_> = same
                    .iter()
                    .filter(|c| c.instance_id == Some(iid))
                    .cloned()
                    .collect();
                if ok.is_empty() {
                    same.into_iter()
                        .filter(|c| c.instance_id.is_some())
                        .collect()
                } else {
                    ok
                }
            } else if m.app.instances.is_some() {
                // Static (default) host of an instance-bound app: connections without an
                // instance; instance-bound ones stay only so the guard answers 421.
                let ok: Vec<_> = same
                    .iter()
                    .filter(|c| c.instance_id.is_none())
                    .cloned()
                    .collect();
                if ok.is_empty() { same } else { ok }
            } else if m.entry.tenant.is_some() {
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
        // (FIX-104, D-180, SEC-04) Transport rule: a package's or secret's host claim is injected
        // only over TLS to the claimed port; an operator-configured LLM endpoint only to the exact
        // scheme, host and port the operator configured. Otherwise the request is an
        // uncredentialed one to that destination, decided by policy as such.
        let https = scheme == crate::pipeline::Scheme::Https;
        if !(self.insecure_loopback_injection && mechanisms::route::is_loopback_host(&host)) {
            let app_claim_port = app_match
                .as_ref()
                .and_then(|m| mechanisms::route::claim_port(&m.entry.host));
            let llm = self.resolver.llm();
            cands.retain(|c| match c {
                Cand::App(_) => mechanisms::route::claim_transport_ok(app_claim_port, https, port),
                Cand::Secret(s) if s.kind == aurora_policy::facts::SecretKind::Llm => llm
                    .accepts_injection(
                        s.llm_endpoint.as_deref(),
                        s.provider.as_deref(),
                        https,
                        &host,
                        port,
                    ),
                Cand::Secret(s) => mechanisms::route::claim_transport_ok(s.claim_port, https, port),
            });
        }
        let creds: Vec<Credential> = cands.iter().map(Cand::credential).collect();
        let selected: HashSet<_> = selected_candidates(rv.all_rules(), &fi, &creds);
        let (granted, ungranted): (Vec<_>, Vec<_>) = cands
            .into_iter()
            .partition(|c| selected.contains(&c.credential().key()));
        // (RFE-025) BYO admission check 3: an agent that requires context credentials never has a
        // credential injected for an `aug_`-authenticated request. Before any approval is created
        // and before any value is fetched.
        if !granted.is_empty() && crate::byo::needs_context_credential(&identity) {
            metrics::counter!(
                "aurora_gateway_byo_refused_total",
                "reason" => crate::byo::Refusal::ContextRequired.label()
            )
            .increment(1);
            let err = crate::byo::context_required("/v1/agent/context-credentials");
            return Outcome::Respond(self.refuse(ctx, &err, "blocked"));
        }
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
        // (FIX-084) The grant or connection the hold stood on is gone: refuse, never send the
        // approved request on without the credential it was approved for.
        if winner.is_none() && released.is_some_and(|r| r.had_credential) {
            let kind = if app_match.is_some() {
                "connection"
            } else {
                "secret"
            };
            return Outcome::Respond(self.refuse(ctx, &credential_unavailable(kind), "error"));
        }
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
                .is_some_and(|m| apps::needs_body(m, &host, port, &app_path, method.as_str()));
        let (body_prefix, truncated, mut body) = if rv.needs_body() || may_hold || llm_body {
            let limit = if rv.needs_body() || llm_body {
                self.condition_body_limit
                    .load(std::sync::atomic::Ordering::Relaxed)
            } else {
                APPROVAL_BODY_PEEK
            };
            let (p, t, b) = buffer_prefix(body, limit).await;
            (p.to_vec(), t, b)
        } else {
            (Vec::new(), false, body)
        };
        // (P4-GRANTS, Q8) Headers for `params` conditions; an unreadable value stays visible (as
        // a replacement character) so that doubt can never look like "absent" or "identity".
        let param_headers: Vec<(&str, &str)> = if app_match.is_some() {
            headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.to_str().unwrap_or("\u{fffd}")))
                .collect()
        } else {
            Vec::new()
        };
        let tool = app_match.as_ref().and_then(|m| {
            apps::identify_tool(
                m,
                &host,
                port,
                &app_path,
                query.as_deref(),
                method.as_str(),
                &body_prefix,
                truncated,
                &param_headers,
                winner.as_ref().and_then(Cand::connect_method),
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
                &self.links(&rv).secrets(),
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
            tool_tags: tool
                .zip(app_match.as_ref())
                .map(|(t, m)| {
                    apps::tool_tags(
                        m.app,
                        t,
                        match &winner {
                            Some(Cand::App(c)) => Some(c.as_ref()),
                            _ => None,
                        },
                    )
                })
                .unwrap_or_default(),
            winning_credential: winner.as_ref().map(Cand::credential),
            is_llm_host: rv.is_llm_host,
            identity: fi,
            connect_time: false,
            host_apps: Vec::new(),
        };
        // (P4-CNT) With a shared backend the counts are read first (peek) and passed in; the
        // evaluator stays pure.
        let pk = if released.is_some() {
            Peeked::Local { fallback: false }
        } else {
            self.rate.peek(&rv, identity.agent_id).await
        };
        let d = evaluate(
            &rv.org_policy,
            &rv.ws_policy,
            &facts,
            if released.is_some() {
                &NoCount
            } else {
                pk.counter(&self.counters)
            },
        );
        ctx.secret_id = wsec.map(|c| c.id);
        ctx.rule = d.deciding_rule().map(|r| (r.logical_id, r.name.clone()));
        // The consume is the authority (all-or-nothing, after an allow / needs-approval verdict;
        // a held request consumes now, once, and is not charged again on approval).
        if released.is_none() {
            match self.rate.settle(&pk, &d, &self.counters).await {
                Settled::Refuse(err, decision) => {
                    return Outcome::Respond(self.refuse(ctx, &err, decision));
                }
                Settled::Proceed { fallback } => ctx.counter_fallback = fallback,
            }
        }
        if d.verdict == Verdict::NeedsApproval
            && released.is_none()
            && let Some(holds) = &self.holds
        {
            // 8. approval hold (nothing decrypted yet).
            let red_path = crate::reqlog::redact_path(&path);
            let red_app_path = crate::reqlog::redact_path(&app_path);
            let hdr = |n: &str| {
                headers
                    .get(n)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let all_headers: Vec<(String, String)> = headers
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
                .collect();
            let summary = crate::hold::build_summary_hooked(
                self.summarizer.as_deref(),
                registry.as_deref(),
                app_match.as_ref(),
                ctx.tool_id.as_deref(),
                &crate::hold::SummaryRequest {
                    method: &ctx.method,
                    host: &host,
                    // `{path.N}` in the summary counts from the app path (D-231); the approval
                    // display below keeps the full path.
                    path: &red_app_path,
                    query: query.as_deref(),
                    content_type: content_type.as_deref(),
                    header: &hdr,
                    body_prefix: &peek,
                    truncated: peek_truncated,
                    headers: &all_headers,
                },
            )
            .await;
            let outcome = holds
                .hold(HoldRequest {
                    identity: &identity,
                    method: &ctx.method,
                    host: &host,
                    path: &red_path,
                    rule: ctx.rule.as_ref().map(|(i, n)| (*i, n.as_str())),
                    connection_id: ctx.connection_id,
                    batch: &aurora_headers,
                    summary: &summary,
                    rule_timeout_secs: d.approval_timeout_secs,
                })
                .await;
            match outcome {
                // (FIX-084) Not forwarded on the strength of the pre-hold decision: the second
                // pass resolves the credential and evaluates policy again, against the policy in
                // force now, with the request exactly as it was held (the body is the buffered one).
                HoldOutcome::Approved => {
                    let again = PipelineRequest {
                        identity,
                        lane,
                        scheme,
                        host,
                        port,
                        method,
                        path,
                        query,
                        headers,
                        aurora_headers,
                        upgrade,
                        body,
                    };
                    let released = Released {
                        started: ctx.started,
                        bodyless,
                        had_credential: winner.is_some(),
                    };
                    return Box::pin(self.handle_with(again, Some(released))).await;
                }
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
                    if self.db_down() {
                        crate::health::count_refused("approval_store_unavailable");
                    }
                    let err = ApiError::unavailable(
                        "approval_store_unavailable",
                        "The approval store is unavailable. Retry shortly.",
                    )
                    .with_details(json!({ "reason": "database_unavailable" }));
                    return Outcome::Respond(self.refuse(ctx, &err, "error"));
                }
            }
        } else if released.is_some() && matches!(d.verdict, Verdict::Allow | Verdict::NeedsApproval)
        {
            // The approval stands: released, whatever the rules now say about asking.
            ctx.decision = "approved";
        } else if let Some((err, decision)) = self.verdict_error(
            &rv,
            &d,
            &ctx.method,
            &path,
            &host,
            tool.map(|t| (t.id.as_str(), t.scopes.as_slice())),
        ) {
            return Outcome::Respond(self.refuse(ctx, &err, decision));
        }
        self.apply_hits(&d, &pk);
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
            // (D-74, SEC-04/SEC-59) An instance-bound connection is injected only on its
            // instance's exact host; one that is not bound never rides on an instance host.
            match mechanisms::route::instance_guard(
                conn.instance_id,
                conn.instance_host.as_deref(),
                conn.instance_base_path.as_deref(),
                inst_id.filter(|_| m.instance),
                &host,
                &path,
            ) {
                Ok(()) => {}
                Err(mechanisms::route::GuardError::Mismatch { expected }) => {
                    let err =
                        guidance::host_mismatch(&expected, conn.instance_base_path.as_deref());
                    return Outcome::Respond(self.refuse(ctx, &err, "guard_refused"));
                }
                Err(mechanisms::route::GuardError::Unavailable) => {
                    return unavailable(self, ctx);
                }
            }
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
            let pkg_ctx: &[String] = registry
                .as_ref()
                .and_then(|l| {
                    l.snapshot
                        .packages
                        .iter()
                        .find(|p| p.package_id == m.app.package_id)
                })
                .map_or(&[], |p| p.context.as_slice());
            if conn.reauth_required {
                return Outcome::Respond(self.refuse(
                    ctx,
                    &reauth_required(&conn.app_id, conn.id, &self.links(&rv).reconnect(conn.id)),
                    "error",
                ));
            }
            let cmap = self.context_map(&identity, conn, pkg_ctx).await;
            // 10. token freshness (after the decision): refresh / mint under the lock.
            let refreshed;
            let conn: &Arc<apps::ConnCand> = match (&self.tokens, &registry) {
                (Some(t), Some(l))
                    if TokenManager::applies(m.app, conn.connect_method.as_deref()) =>
                {
                    match t.ensure_fresh(l, conn, &cmap, false).await {
                        Ok(c) => {
                            refreshed = Arc::new(c);
                            &refreshed
                        }
                        Err(e) => {
                            return Outcome::Respond(self.refuse(
                                ctx,
                                &self.refresh_error(&rv, conn, e),
                                "error",
                            ));
                        }
                    }
                }
                _ => conn,
            };
            ctx.connection_id = Some(conn.id);
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
                Err(PlaintextError::DatabaseUnavailable) => {
                    let err = guidance::gateway_degraded("credential_store_unavailable");
                    return Outcome::Respond(self.refuse(ctx, &err, "error"));
                }
                Err(PlaintextError::Unavailable) => return unavailable(self, ctx),
            };
            let Some(exec) = Exec::new(m.app, pkg_ctx, conn, raw.expose(), cmap.clone()) else {
                return unavailable(self, ctx);
            };
            // 9. token interception: answered locally, never forwarded.
            if let Some(i) = &m.entry.intercept
                && mechanisms::intercept::matches(i, &method, &app_path)
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
            let pre = (self.stale_retry_eligible(
                m,
                conn.connect_method.as_deref(),
                bodyless,
                upgrade.is_some(),
            ))
            .then(|| (headers.clone(), query.clone()));
            let mut new_path = path.clone();
            let mut up_host = host.clone();
            let mut ok =
                match inject_and_route(m.entry, &vars, &mut headers, &mut new_path, &mut query) {
                    Ok(h) => {
                        if let Some(h) = h {
                            up_host = h;
                        }
                        true
                    }
                    Err(()) => false,
                };
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
            if ok {
                ok = finalize_request(
                    m.entry,
                    &vars,
                    &method,
                    &up_host,
                    port,
                    scheme,
                    &new_path,
                    query.as_deref(),
                    &mut headers,
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
            let echo = inject::url_needles(
                exec.string_values().map(str::as_bytes),
                &new_path,
                query.as_deref(),
            );
            let hook = self.hook(
                &rv,
                ctx,
                fb,
                app_label(app_match.as_ref()),
                passthrough,
                None,
                echo,
            );
            let retry = match (pre, &self.tokens, &registry) {
                (Some((h0, q0)), Some(t), Some(l)) => Some(Box::new(StaleRetry {
                    tokens: t.clone(),
                    plaintext: self.plaintext.clone(),
                    loaded: l.clone(),
                    conn: (**conn).clone(),
                    entry: m.entry.clone(),
                    codes: m
                        .app
                        .token_for(conn.connect_method.as_deref())
                        .map(|t| t.stale_codes.clone())
                        .unwrap_or_default(),
                    pkg_ctx: pkg_ctx.to_vec(),
                    context: cmap,
                    scheme,
                    up_host: up_host.clone(),
                    port,
                    method: method.clone(),
                    path: path.clone(),
                    query: q0,
                    headers: h0,
                }) as Box<dyn Retry>),
                _ => None,
            };
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
                retry,
                // (D-251) Only a request that won a granted connection of an LDAP app carries
                // the block: the lane is a capability the grant confers, not open egress.
                ldap: crate::ldap::is_ldap_host(&host)
                    .then(|| m.app.ldap.clone())
                    .flatten(),
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
                Err(PlaintextError::DatabaseUnavailable) => {
                    let err = guidance::gateway_degraded("credential_store_unavailable");
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
                    Governance::backstop_info(g, &host_port, c, &self.links(&rv).secrets())
            {
                // Origin preflight: verify unseen key values before injecting.
                if let Err(err) = gov.preflight(&info, value.expose()).await {
                    return Outcome::Respond(self.refuse(ctx, &err, "governance_refused"));
                }
                backstop = Some((gov.clone(), info));
            }
            let mut new_path = path.clone();
            // (P4-LLM-2, C-807) A minted mode injects the token the mint service produced from the value.
            let minted = match self.llm_token(c, value.expose()).await {
                Ok(t) => t,
                Err(e) => {
                    let err = credential_unavailable("llm_mint")
                        .with_details(json!({ "kind": "llm_mint", "reason": e.reason() }));
                    let code = if e.class == aurora_llm::mint::MintClass::Rejected {
                        "llm_auth"
                    } else {
                        "credential_unavailable"
                    };
                    ctx.decision = "error";
                    self.log.record(ctx.row(502, None, Some(code)));
                    return Outcome::Respond(guidance::response(&err));
                }
            };
            let target = Target {
                headers: &mut headers,
                path: &mut new_path,
                query: &mut query,
            };
            let r = match &minted {
                Some(t) => inject::apply_minted(c, t.expose().as_bytes(), target),
                None => inject::apply(c, value.expose(), target),
            };
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
            let echo =
                inject::url_needles([value.expose().as_slice()], &new_path, query.as_deref());
            let llm = (c.kind == aurora_policy::facts::SecretKind::Llm)
                .then(|| c.provider.clone().zip(c.llm_endpoint.clone()))
                .flatten();
            let mut hook = self.hook(
                &rv,
                ctx,
                fb,
                app_label(app_match.as_ref()),
                false,
                backstop,
                echo,
            );
            hook.llm = llm;
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
                retry: None,
                ldap: None,
            });
        }
        let fb = self.fallback(&rv, app_match.as_ref(), &ungranted, &host);
        let passthrough = pass_challenge(app_match.as_ref());
        let hook = self.hook(
            &rv,
            ctx,
            fb,
            app_label(app_match.as_ref()),
            passthrough,
            None,
            Vec::new(),
        );
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
            retry: None,
            ldap: None,
        })
    }
}

#[async_trait::async_trait]
impl RequestHandler for PolicyHandler {
    async fn handle(&self, req: PipelineRequest) -> Outcome {
        let outcome = self.handle_with(req, None).await;
        match outcome {
            // (D-251) A request on an LDAP app's synthetic host is answered by the LDAP lane
            // only when it won a granted connection of that app (then it carries the approved
            // `ldap` block). Anything else on that host — no connection, no grant, a host of no
            // LDAP app — is never forwarded and never reaches a directory.
            Outcome::Forward(mut p)
                if crate::ldap::is_ldap_host(&p.host)
                    && (p.ldap.is_none() || p.scheme == crate::pipeline::Scheme::Http) =>
            {
                // (FIX-153, C-892) Plain HTTP is its own refusal; the others say why and where
                // to fix it, from the fallback the decision already computed.
                let err = if p.scheme == crate::pipeline::Scheme::Http {
                    let url = format!("https://{}/search", p.host);
                    ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "ldap_https_required",
                        format!("This directory is only served over HTTPS. Use {url}"),
                    )
                    .with_details(json!({ "url": url }))
                } else {
                    p.hook
                        .as_ref()
                        .and_then(|h| h.ldap_refusal())
                        .unwrap_or_else(|| {
                            ApiError::new(
                                StatusCode::FORBIDDEN,
                                "ldap_no_connection",
                                "No installed directory app serves this host.",
                            )
                            .with_details(json!({ "reason": "no_app" }))
                        })
                };
                if let Some(h) = p.hook.take() {
                    h.on_error(&err);
                }
                Outcome::Respond(guidance::response(&err))
            }
            o => o,
        }
    }

    async fn db_open(
        self: Arc<Self>,
        identity: &Arc<Ident>,
        connection_id: Uuid,
    ) -> Result<DbOpened, DbOpenError> {
        let refuse = |code: &str, msg: &str| {
            DbOpenError::Refused(ApiError::new(StatusCode::FORBIDDEN, code, msg))
        };
        let unavailable = || {
            DbOpenError::Unavailable(ApiError::unavailable(
                "policy_unavailable",
                "Policy cannot be evaluated right now. Retry shortly.",
            ))
        };
        // One message for "no such connection" and "not granted": no enumeration of ids.
        let not_granted = || {
            refuse(
                "access_restricted",
                "This agent has not been granted a database connection with this id. Ask a workspace admin to grant it.",
            )
        };
        if crate::byo::needs_context_credential(identity) {
            return Err(DbOpenError::Refused(crate::byo::context_required(
                "/v1/agent/context-credentials",
            )));
        }
        let dbc = match self.resolver.db_connection(identity, connection_id).await {
            Ok(Some(c)) => c,
            Ok(None) => return Err(not_granted()),
            Err(e) => {
                tracing::warn!(error = %e, "database connection resolution failed");
                return Err(unavailable());
            }
        };
        let host = crate::db::synthetic_host(connection_id);
        let rv = match self.resolver.resolve(identity, &host, 443).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "policy resolution failed");
                return Err(unavailable());
            }
        };
        let cred = dbc.conn.credential();
        let selected: HashSet<_> = selected_candidates(
            rv.all_rules(),
            &facts_identity(identity),
            std::slice::from_ref(&cred),
        );
        if !selected.contains(&cred.key()) {
            return Err(not_granted());
        }
        if dbc.unavailable {
            return Err(refuse(
                "app_unavailable",
                "This app is not available to this workspace under the organization's app policy.",
            ));
        }
        match dbc.status.as_str() {
            "active" => {}
            "disabled" => {
                return Err(refuse(
                    "connection_disabled",
                    "This database connection is disabled. Ask a workspace admin to enable it.",
                ));
            }
            _ => {
                return Err(refuse(
                    "reauth_required",
                    "This database connection needs its credentials updated by a workspace admin.",
                ));
            }
        }
        let is_db = dbc
            .registry
            .as_ref()
            .and_then(|l| l.snapshot.app(&dbc.conn.app_id))
            .and_then(|a| a.database.as_ref())
            .is_some_and(|d| d.engine == "postgres");
        if !is_db {
            return Err(refuse(
                "not_a_database_connection",
                "This connection is not a PostgreSQL database connection.",
            ));
        }
        let raw = match self.plaintext.get_connection(&dbc.conn).await {
            Ok(v) => v,
            Err(PlaintextError::StoreUnavailable(_)) => {
                return Err(DbOpenError::Unavailable(ApiError::unavailable(
                    "secret_store_unavailable",
                    "The secret store is unreachable. Retry shortly.",
                )));
            }
            Err(PlaintextError::DatabaseUnavailable) => {
                return Err(DbOpenError::Unavailable(guidance::gateway_degraded(
                    "credential_store_unavailable",
                )));
            }
            Err(PlaintextError::Unavailable) => {
                return Err(DbOpenError::Unavailable(credential_unavailable(
                    "connection",
                )));
            }
        };
        let endpoint = DbEndpoint::from_credentials(raw.expose(), &dbc.conn.metadata).map_err(|e| {
            tracing::warn!(connection = %connection_id, error = %e, "unusable database credentials");
            DbOpenError::Unavailable(credential_unavailable("connection"))
        })?;
        self.touch(dbc.conn.id);
        Ok(DbOpened {
            endpoint,
            label: dbc.conn.label.clone(),
            authorizer: Arc::new(DbSession {
                handler: self.clone(),
                identity: identity.clone(),
                conn: dbc.conn,
                host,
            }),
        })
    }

    fn byo_refused(
        &self,
        identity: &Arc<Ident>,
        method: &str,
        host: &str,
        reason: crate::byo::Refusal,
        err: &ApiError,
    ) {
        let ctx = Ctx {
            identity: identity.clone(),
            lane: "mitm",
            method: method.to_owned(),
            host: host.to_owned(),
            path: "/".into(),
            started: Instant::now(),
            secret_id: None,
            decision: "blocked",
            rule: None,
            injected: 0,
            app_id: None,
            tool_id: None,
            connection_id: None,
            counter_fallback: false,
        };
        let _ = reason;
        self.log
            .record(ctx.row(err.status().as_u16(), None, Some(err.code())));
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
            counter_fallback: false,
        };
        let rv = match self.resolver.resolve(identity, host, port).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "policy resolution failed");
                let err = policy_unavailable(&e);
                self.refuse(ctx, &err, "error");
                return OpenLaneDecision::Deny(err);
            }
        };
        // (RFE-003 a) Open-lane posture, before any policy evaluation. Facts are the
        // CONNECT-time facts of `07 §CONNECT-time evaluation`: host/port/identity only.
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
            connect_time: true,
            host_apps: if rv.is_llm_host {
                Vec::new()
            } else {
                rv.host_apps.clone()
            },
        };
        let admission = match &rv.governance {
            Some(g) => {
                g.open_lane_admission(rv.ws_open_lane, &rv.org_policy, &rv.ws_policy, &facts)
            }
            // Governance not wired (tests): the workspace's own posture still applies.
            None => match rv.ws_open_lane {
                OpenLanePosture::Enabled => OpenLaneAdmission::Admitted,
                OpenLanePosture::Disabled => OpenLaneAdmission::Disabled,
                OpenLanePosture::AllowlistOnly => {
                    let listed = rv
                        .ws_policy
                        .uncredentialed_allowlist
                        .iter()
                        .any(|a| a.path.is_none() && a.host.matches(host, port));
                    if listed || aurora_policy::connect_allow_admits(&rv.ws_policy, &facts) {
                        OpenLaneAdmission::Admitted
                    } else {
                        OpenLaneAdmission::NotAllowlisted
                    }
                }
            },
        };
        match admission {
            OpenLaneAdmission::Admitted => {}
            OpenLaneAdmission::Disabled => {
                let url = if rv.ws_open_lane == OpenLanePosture::Disabled {
                    self.url(&rv, "policy")
                } else {
                    format!("{}/o/{}/governance", self.external_url, rv.org_slug)
                };
                let err = guidance::open_lane_disabled(&url);
                self.refuse(ctx, &err, "blocked");
                return OpenLaneDecision::Deny(err);
            }
            OpenLaneAdmission::NotAllowlisted => {
                let url = format!("{}/o/{}/governance", self.external_url, rv.org_slug);
                let err = ApiError::new(
                    StatusCode::FORBIDDEN,
                    "blocked_by_egress_policy",
                    format!("Browser tunnels are limited to allow-listed hosts and {host}:{port} is not on the allow-list. Review the egress allow-list at {url}"),
                )
                .with_details(json!({ "host": host, "port": port, "manage_url": url, "open_lane": true }));
                self.refuse(ctx, &err, "egress_denied");
                return OpenLaneDecision::Deny(err);
            }
        }
        if let Some(g) = &rv.governance
            && rv.is_llm_host
            && let Err(err) = Governance::check_open_lane(g, &format!("{host}:{port}"))
        {
            self.refuse(ctx, &err, "governance_refused");
            return OpenLaneDecision::Deny(err);
        }
        let pk = self.rate.peek(&rv, identity.agent_id).await;
        let d = evaluate(
            &rv.org_policy,
            &rv.ws_policy,
            &facts,
            pk.counter(&self.counters),
        );
        let mut ctx = ctx;
        ctx.rule = d.deciding_rule().map(|r| (r.logical_id, r.name.clone()));
        match self.verdict_error(&rv, &d, "CONNECT", "/", host, None) {
            None => {
                // A CONNECT counts as one request (consumed once, when it is allowed).
                if let Settled::Refuse(err, decision) =
                    self.rate.settle(&pk, &d, &self.counters).await
                {
                    self.refuse(ctx, &err, decision);
                    return OpenLaneDecision::Deny(err);
                }
                self.apply_hits(&d, &pk);
                OpenLaneDecision::Allow
            }
            Some((err, decision)) => {
                // (RFE-003) A needs-approval verdict refuses the CONNECT (a tunnel is never
                // held); a block by rule says it was applied to the whole host.
                let (err, decision) = if d.verdict == Verdict::NeedsApproval {
                    let name = d
                        .deciding_rule()
                        .map(|r| r.name.clone())
                        .unwrap_or_default();
                    let url = self.url(&rv, "policy");
                    (
                        ApiError::new(
                            StatusCode::FORBIDDEN,
                            "blocked_by_policy",
                            format!("The rule `{name}` requires approval, and a browser tunnel cannot be held for approval. Use the default proxy lane. Review policy at {url}"),
                        )
                        .with_details(json!({
                            "rule_name": name, "reason": "approval_required",
                            "connect_time": true, "manage_url": url
                        })),
                        "approval_denied",
                    )
                } else if matches!(
                    d.verdict,
                    Verdict::Block {
                        reason: BlockReason::Rule
                    }
                ) {
                    let details = err.details().cloned().unwrap_or_else(|| json!({}));
                    let mut details = details;
                    if let Some(o) = details.as_object_mut() {
                        o.insert("connect_time".into(), json!(true));
                    }
                    (err.with_details(details), decision)
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
            counter_fallback: false,
        };
        let mut row = ctx.row(200, None, None);
        row.bytes_up = i64::try_from(r.bytes_up).unwrap_or(i64::MAX);
        row.bytes_down = i64::try_from(r.bytes_down).unwrap_or(i64::MAX);
        self.log.record(row);
    }
}

impl PolicyHandler {
    /// Body-less, non-upgrade app requests of a refreshing app may be resent once.
    fn stale_retry_eligible(
        &self,
        m: &apps::AppMatch<'_>,
        connect_method: Option<&str>,
        bodyless: bool,
        upgrade: bool,
    ) -> bool {
        self.tokens.is_some()
            && bodyless
            && !upgrade
            && m.entry.transform.is_none()
            && m.entry.intercept.is_none()
            && TokenManager::applies(m.app, connect_method)
            && m.app
                .token_for(connect_method)
                .is_some_and(|t| !t.stale_codes.is_empty())
    }

    /// Context variables of docs/09 §3.4 the package declared in `permissions.context`.
    async fn context_map(
        &self,
        ident: &Ident,
        conn: &apps::ConnCand,
        allowed: &[String],
    ) -> HashMap<String, String> {
        let want = |k: &str| allowed.iter().any(|a| a == k);
        let mut m = HashMap::new();
        let mut put = |k: &str, v: String| {
            if want(k) {
                m.insert(k.to_owned(), v);
            }
        };
        put("org_id", ident.org_id.to_string());
        put(
            "workspace_id",
            conn.workspace_id.unwrap_or(ident.workspace_id).to_string(),
        );
        put("instance_url", self.external_url.clone());
        if ["org_external_id", "agent_name", "agent_handle"]
            .iter()
            .any(|k| want(k))
            && let Some(pool) = self
                .touch_pool
                .as_ref()
                .or_else(|| self.tokens.as_ref().map(|t| t.pool()))
            && let Ok(Some((ext, name, handle))) =
                repo::context_names(pool, ident.org_id, ident.agent_id).await
        {
            put("org_external_id", ext);
            put("agent_name", name);
            put("agent_handle", handle);
        }
        m
    }

    fn refresh_error(&self, rv: &ResolvedHost, c: &apps::ConnCand, e: RefreshError) -> ApiError {
        match e {
            RefreshError::Reauth => {
                reauth_required(&c.app_id, c.id, &self.links(rv).reconnect(c.id))
            }
            RefreshError::StoreUnavailable(store_id) => ApiError::unavailable(
                "secret_store_unavailable",
                "The secret store is unreachable. Retry shortly.",
            )
            .with_details(json!({ "store_id": store_id })),
            // (RFE-014) A refresh needs the database; while it is unreachable the request is
            // refused as degraded, not as a broken credential.
            RefreshError::Unavailable if self.db_down() => {
                guidance::gateway_degraded("credential_store_unavailable")
            }
            RefreshError::Unavailable => credential_unavailable("connection"),
        }
    }

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

    #[allow(clippy::too_many_arguments)]
    fn hook(
        &self,
        rv: &ResolvedHost,
        ctx: Ctx,
        fallback: Fallback,
        app: Option<AppRef>,
        passthrough_challenge: bool,
        backstop: Option<(Arc<Governance>, BackstopInfo)>,
        echo: Vec<Vec<u8>>,
    ) -> Box<Hook> {
        let links = self.links(rv);
        let connect_url = match &ctx.app_id {
            Some(a) => links.connect(a),
            None => links.connect_any(),
        };
        let access_url = links.access(ctx.identity.agent_id);
        Box::new(Hook {
            llm: None,
            ctx,
            log: self.log.clone(),
            fallback,
            passthrough_challenge,
            create_secret_url: links.secrets(),
            manage_url: links.policy(),
            connect_url,
            access_url,
            app,
            backstop,
            echo,
        })
    }
}

/// (phase 2, P2-DBL-1) The per-statement policy of one database session: each statement is the
/// synthetic request `POST https://<connection_id>.db.aurora.internal/<class>`, decided by the
/// same engine, approvals, rate limits and request log as HTTP.
struct DbSession {
    handler: Arc<PolicyHandler>,
    identity: Arc<Ident>,
    conn: Arc<apps::ConnCand>,
    host: String,
}

#[async_trait::async_trait]
impl DbAuthorizer for DbSession {
    async fn authorize(&self, class: aurora_sqlclass::Class, statement: &str) -> DbVerdict {
        self.authorize_with(class, statement, None).await
    }
}

impl DbSession {
    /// `released` is the arrival time on the second pass after an approved hold (FIX-084): the
    /// grant and the policy are read again, a statement that would now be refused is refused, and
    /// one that would still need approval is released without a second hold or charge.
    async fn authorize_with(
        &self,
        class: aurora_sqlclass::Class,
        statement: &str,
        released: Option<Instant>,
    ) -> DbVerdict {
        let h = &self.handler;
        let path = format!("/{}", class.as_str());
        let mut ctx = Ctx {
            identity: self.identity.clone(),
            lane: "db",
            method: "POST".to_owned(),
            host: self.host.clone(),
            path: path.clone(),
            started: released.unwrap_or_else(Instant::now),
            secret_id: None,
            decision: "allow",
            rule: None,
            injected: 0,
            app_id: Some(self.conn.app_id.clone()),
            tool_id: None,
            connection_id: Some(self.conn.id),
            counter_fallback: false,
        };
        let rv = match h.resolver.resolve(&self.identity, &self.host, 443).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "policy resolution failed");
                let err = policy_unavailable(&e);
                h.refuse(ctx, &err, "error");
                return DbVerdict::Unavailable(err);
            }
        };
        let fi = facts_identity(&self.identity);
        let cred = self.conn.credential();
        // The grant is re-checked per statement: a revoked grant ends access at once.
        let selected: HashSet<_> =
            selected_candidates(rv.all_rules(), &fi, std::slice::from_ref(&cred));
        if !selected.contains(&cred.key()) {
            let err = ApiError::new(
                StatusCode::FORBIDDEN,
                "access_restricted",
                "This agent is no longer granted this database connection.",
            );
            h.refuse(ctx, &err, "blocked");
            return DbVerdict::Deny(err);
        }
        let app = rv
            .registry
            .as_ref()
            .and_then(|l| l.snapshot.app(&self.conn.app_id));
        let limit = h
            .condition_body_limit
            .load(std::sync::atomic::Ordering::Relaxed);
        let bytes = statement.as_bytes();
        let body_prefix = bytes[..bytes.len().min(limit)].to_vec();
        let truncated = bytes.len() > limit;
        let tool = app.and_then(|a| {
            apps::identify_tool_in(
                a,
                false,
                &self.host,
                443,
                &path,
                None,
                "POST",
                &body_prefix,
                truncated,
                &[],
                self.conn.connect_method.as_deref(),
            )
        });
        ctx.tool_id = tool.map(|t| t.id.clone());
        let facts = RequestFacts {
            host: self.host.clone(),
            port: 443,
            path: path.clone(),
            query: None,
            method: "POST".to_owned(),
            body_prefix,
            truncated,
            app_id: ctx.app_id.clone(),
            tool_id: ctx.tool_id.clone(),
            tool_tags: tool
                .zip(app)
                .map(|(t, a)| apps::tool_tags(a, t, None))
                .unwrap_or_default(),
            winning_credential: Some(cred),
            is_llm_host: false,
            identity: fi,
            connect_time: false,
            host_apps: Vec::new(),
        };
        let pk = if released.is_some() {
            Peeked::Local { fallback: false }
        } else {
            h.rate.peek(&rv, self.identity.agent_id).await
        };
        let d = evaluate(
            &rv.org_policy,
            &rv.ws_policy,
            &facts,
            if released.is_some() {
                &NoCount
            } else {
                pk.counter(&h.counters)
            },
        );
        ctx.rule = d.deciding_rule().map(|r| (r.logical_id, r.name.clone()));
        if released.is_none() {
            match h.rate.settle(&pk, &d, &h.counters).await {
                Settled::Refuse(err, decision) => {
                    h.refuse(ctx, &err, decision);
                    return if err.status() == StatusCode::SERVICE_UNAVAILABLE {
                        DbVerdict::Unavailable(err)
                    } else {
                        DbVerdict::Deny(err)
                    };
                }
                Settled::Proceed { fallback } => ctx.counter_fallback = fallback,
            }
        }
        if d.verdict == Verdict::NeedsApproval
            && released.is_none()
            && let Some(holds) = &h.holds
        {
            let summary = aurora_summary::ApprovalSummary {
                title: format!("Database {} on {}", class.as_str(), self.conn.label),
                fields: vec![
                    aurora_summary::SummaryField {
                        label: "Connection".into(),
                        value: self.conn.label.clone(),
                    },
                    aurora_summary::SummaryField {
                        label: "Class".into(),
                        value: class.as_str().to_owned(),
                    },
                ],
                // Literals are redacted lexically; the text is clamped by `clamp`.
                text: Some(aurora_sqlclass::redact(statement, 2_000)),
                link: None,
                group: None,
                source: aurora_summary::SummarySource::Generic,
            }
            .clamp();
            let outcome = holds
                .hold(HoldRequest {
                    identity: &self.identity,
                    method: "POST",
                    host: &self.host,
                    path: &path,
                    rule: ctx.rule.as_ref().map(|(i, n)| (*i, n.as_str())),
                    connection_id: Some(self.conn.id),
                    batch: &crate::hygiene::AuroraHeaders::default(),
                    summary: &summary,
                    // RFE-011a (D-56): the rule's approval timeout applies to statements too.
                    rule_timeout_secs: d.approval_timeout_secs,
                })
                .await;
            match outcome {
                // (FIX-084) Re-read the grant and the policy before the statement is released.
                HoldOutcome::Approved => {
                    return Box::pin(self.authorize_with(class, statement, Some(ctx.started)))
                        .await;
                }
                HoldOutcome::Denied(id) => {
                    let err = ApiError::new(
                        StatusCode::FORBIDDEN,
                        "manual_approval_denied",
                        "A reviewer denied this statement. Do not retry it unchanged.",
                    )
                    .with_details(json!({ "approval_id": id }));
                    h.refuse(ctx, &err, "approval_denied");
                    return DbVerdict::Deny(err);
                }
                HoldOutcome::Expired(id) => {
                    let err = ApiError::new(
                        StatusCode::FORBIDDEN,
                        "manual_approval_expired",
                        "No reviewer decided in time, so this statement was not sent.",
                    )
                    .with_details(json!({ "approval_id": id }));
                    h.refuse(ctx, &err, "approval_expired");
                    return DbVerdict::Deny(err);
                }
                HoldOutcome::Restarting => {
                    ctx.decision = "error";
                    h.log.record(ctx.row(503, None, Some("gateway_restarting")));
                    return DbVerdict::Unavailable(ApiError::unavailable(
                        "gateway_restarting",
                        "The gateway is restarting while this statement awaited approval. Retry shortly.",
                    ));
                }
                HoldOutcome::StoreUnavailable => {
                    let err = ApiError::unavailable(
                        "approval_store_unavailable",
                        "The approval store is unavailable. Retry shortly.",
                    );
                    h.refuse(ctx, &err, "error");
                    return DbVerdict::Unavailable(err);
                }
            }
        } else if released.is_some() && matches!(d.verdict, Verdict::Allow | Verdict::NeedsApproval)
        {
            ctx.decision = "approved";
        } else if let Some((err, decision)) =
            h.verdict_error(&rv, &d, "POST", &path, &self.host, None)
        {
            h.refuse(ctx, &err, decision);
            return if err.status() == StatusCode::SERVICE_UNAVAILABLE {
                DbVerdict::Unavailable(err)
            } else {
                DbVerdict::Deny(err)
            };
        }
        h.apply_hits(&d, &pk);
        h.touch(self.conn.id);
        h.log.record(ctx.row(200, None, None));
        DbVerdict::Allow
    }
}

fn pass_challenge(m: Option<&apps::AppMatch<'_>>) -> bool {
    m.is_some_and(|m| {
        m.entry
            .passthrough
            .contains(&aurora_registry::manifest::Passthrough::RegistryTokenChallenge)
    })
}

/// The refusal of a failed policy resolution: 503 `gateway_degraded` when the database is
/// unreachable (RFE-014), else 503 `policy_unavailable`.
fn policy_unavailable(e: &crate::resolve::ResolveFailure) -> ApiError {
    if e.is_db_unavailable() {
        guidance::gateway_degraded("database_unavailable")
    } else {
        ApiError::unavailable(
            "policy_unavailable",
            "Policy cannot be evaluated right now. Retry shortly.",
        )
    }
}

fn credential_unavailable(kind: &str) -> ApiError {
    ApiError::bad_gateway(
        "credential_unavailable",
        "A granted credential could not be produced. Retry later or contact a workspace admin.",
    )
    .with_details(json!({ "kind": kind }))
}

fn reauth_required(app_id: &str, connection_id: Uuid, connect_url: &str) -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "connection_reauth_required",
        format!("The {app_id} connection must be reconnected by a user before it can be used. Reconnect it at {connect_url}"),
    )
    .with_details(json!({
        "app_id": app_id, "connection_id": connection_id, "connect_url": connect_url
    }))
}

/// Inject and `rewrite_host`; returns the rewritten upstream host, if any.
#[allow(clippy::result_unit_err)]
pub fn inject_and_route(
    entry: &aurora_registry::snapshot::HostEntry,
    vars: &mechanisms::template::Vars<'_>,
    headers: &mut HeaderMap,
    path: &mut String,
    query: &mut Option<String>,
) -> Result<Option<String>, ()> {
    mechanisms::inject::apply_all(
        &entry.inject,
        vars,
        &mut Target {
            headers,
            path,
            query,
        },
    )
    .map_err(|_| ())?;
    match &entry.rewrite_host {
        Some(r) => mechanisms::route::rewrite_host(r, vars)
            .map(Some)
            .map_err(|_| ()),
        None => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::result_unit_err)]
pub fn finalize_request(
    entry: &aurora_registry::snapshot::HostEntry,
    vars: &mechanisms::template::Vars<'_>,
    method: &http::Method,
    up_host: &str,
    port: u16,
    scheme: crate::pipeline::Scheme,
    path: &str,
    query: Option<&str>,
    headers: &mut HeaderMap,
) -> Result<(), ()> {
    let Some(mechanisms::Finalize::AwsSigv4(f)) = &entry.finalize else {
        return Ok(());
    };
    let host_hdr = if port == scheme.default_port() {
        up_host.to_owned()
    } else {
        format!("{up_host}:{port}")
    };
    mechanisms::finalize::finalize(
        f,
        vars,
        method,
        &host_hdr,
        path,
        query,
        headers,
        chrono::Utc::now(),
    )
    .map_err(|_| ())
}

/// Resend of a body-less request with freshly refreshed credentials (step 14).
struct StaleRetry {
    tokens: Arc<TokenManager>,
    plaintext: Arc<PlaintextCache>,
    loaded: Arc<crate::snapshot::Loaded>,
    conn: apps::ConnCand,
    entry: aurora_registry::snapshot::HostEntry,
    codes: Vec<aurora_registry::manifest::StaleCode>,
    pkg_ctx: Vec<String>,
    context: HashMap<String, String>,
    scheme: crate::pipeline::Scheme,
    up_host: String,
    port: u16,
    method: http::Method,
    path: String,
    query: Option<String>,
    headers: HeaderMap,
}

#[async_trait::async_trait]
impl Retry for StaleRetry {
    fn stale_codes(&self) -> &[aurora_registry::manifest::StaleCode] {
        &self.codes
    }

    async fn rebuild(self: Box<Self>) -> Option<PreparedRequest> {
        let s = *self;
        let conn = s
            .tokens
            .ensure_fresh(&s.loaded, &s.conn, &s.context, true)
            .await
            .ok()?;
        let app = s.loaded.snapshot.app(&conn.app_id)?;
        let raw = s.plaintext.get_connection(&conn).await.ok()?;
        let exec = Exec::new(app, &s.pkg_ctx, &conn, raw.expose(), s.context.clone())?;
        let vars = exec.vars();
        let (mut headers, mut query, mut path) = (s.headers, s.query, s.path);
        let rewritten =
            inject_and_route(&s.entry, &vars, &mut headers, &mut path, &mut query).ok()?;
        let up_host = rewritten.unwrap_or(s.up_host);
        finalize_request(
            &s.entry,
            &vars,
            &s.method,
            &up_host,
            s.port,
            s.scheme,
            &path,
            query.as_deref(),
            &mut headers,
        )
        .ok()?;
        Some(PreparedRequest {
            scheme: s.scheme,
            host: up_host,
            port: s.port,
            method: s.method,
            path,
            query,
            headers,
            upgrade: None,
            body: crate::empty_body(),
            hook: None,
            response_headers: HeaderMap::new(),
            retry: None,
            ldap: None,
        })
    }
}
