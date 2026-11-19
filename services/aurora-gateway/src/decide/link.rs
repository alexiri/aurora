//! (phase 5, P5-ICS, D-273, C-843, SEC-107) Admission of a request on a secret-link connection's
//! synthetic host `<connection_id>.link.aurora.internal`. The host is no app's `hosts[]` entry, so
//! this is its own decision path (the database lane's pattern, D-80): the connection must be the
//! agent's to use (grant), the request is the synthetic `GET https://<id>.link.aurora.internal/`
//! decided by the same engine, approvals, rate limits, Read only and request log as any tool
//! request, and only then is the stored URL decrypted and handed to the lane (`seclink`).
//! Nothing here puts the URL in a log row, an approval, an error or a span.

use super::*;
use crate::seclink::{Hops, LinkFetch, Report};
use http::Method;

/// The request-log row of one link request, written by the lane when it has answered.
struct LinkLog {
    ctx: Ctx,
    log: Arc<RequestLog>,
}

impl Report for LinkLog {
    fn finish(self: Box<Self>, status: u16, upstream: Option<u16>, code: Option<&str>) {
        let mut ctx = self.ctx;
        if status >= 400 {
            ctx.decision = "error";
        }
        self.log.record(ctx.row(status, upstream, code));
    }
}

/// The org (and workspace) egress policy on every real host of a fetch: the same engine and rules
/// as an uncredentialed request to that host. A verdict other than allow refuses the hop (a hold is
/// impossible in the middle of a fetch).
struct LinkHops {
    rv: Arc<ResolvedHost>,
    identity: RequestIdentity,
}

impl Hops for LinkHops {
    fn check(&self, host: &str, port: u16) -> Result<(), ApiError> {
        let facts = RequestFacts {
            host: host.to_ascii_lowercase(),
            port,
            path: "/".to_owned(),
            query: None,
            method: "GET".to_owned(),
            body_prefix: Vec::new(),
            truncated: false,
            app_id: None,
            tool_id: None,
            tool_tags: Default::default(),
            winning_credential: None,
            is_llm_host: false,
            identity: self.identity.clone(),
            connect_time: false,
            host_apps: Vec::new(),
        };
        let d = evaluate(&self.rv.org_policy, &self.rv.ws_policy, &facts, &NoCount);
        if d.verdict == Verdict::Allow {
            Ok(())
        } else {
            Err(crate::seclink::refused(host))
        }
    }
}

impl PolicyHandler {
    /// `conn_id` is the parsed label of the synthetic host (`None` = not a connection id).
    /// `released` is the second pass of an approved hold: the grant and the policy are read again.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn link_request(
        &self,
        mut ctx: Ctx,
        identity: &Arc<Ident>,
        conn_id: Option<Uuid>,
        method: &Method,
        headers: &HeaderMap,
        released: Option<Released>,
        body: LinkBody,
        dry_run: bool,
    ) -> Outcome {
        // One answer for "no such connection", "not a link" and "not granted": no enumeration.
        let not_granted = || {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "access_restricted",
                "This agent has not been granted a calendar link with this address. Ask a workspace admin to grant it.",
            )
        };
        if crate::byo::needs_context_credential(identity) {
            let err = crate::byo::context_required("/v1/agent/context-credentials");
            return Outcome::Respond(self.refuse(ctx, &err, "blocked"));
        }
        let Some(conn_id) = conn_id else {
            return Outcome::Respond(self.refuse(ctx, &not_granted(), "blocked"));
        };
        let dbc = match self.resolver.db_connection(identity, conn_id).await {
            Ok(Some(c)) => c,
            Ok(None) => return Outcome::Respond(self.refuse(ctx, &not_granted(), "blocked")),
            Err(e) => {
                tracing::warn!(error = %e, "link connection resolution failed");
                let err = policy_unavailable(&e);
                return Outcome::Respond(self.refuse(ctx, &err, "error"));
            }
        };
        let host = ctx.host.clone();
        let rv = match self.resolver.resolve(identity, &host, 443).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "policy resolution failed");
                let err = policy_unavailable(&e);
                return Outcome::Respond(self.refuse(ctx, &err, "error"));
            }
        };
        let fi = facts_identity(identity);
        let cred = dbc.conn.credential();
        let selected: HashSet<_> =
            selected_candidates(rv.all_rules(), &fi, std::slice::from_ref(&cred));
        if !selected.contains(&cred.key()) {
            return Outcome::Respond(self.refuse(ctx, &not_granted(), "blocked"));
        }
        ctx.app_id = Some(dbc.conn.app_id.clone());
        ctx.connection_id = Some(dbc.conn.id);
        let app = dbc
            .registry
            .as_ref()
            .and_then(|l| l.snapshot.app(&dbc.conn.app_id));
        if app.and_then(|a| a.secret_link.as_ref()).is_none() {
            return Outcome::Respond(self.refuse(ctx, &not_granted(), "blocked"));
        }
        if dbc.unavailable {
            let err = ApiError::new(
                StatusCode::FORBIDDEN,
                "app_unavailable",
                "This app is not available to this workspace under the organization's app policy.",
            );
            return Outcome::Respond(self.refuse(ctx, &err, "blocked"));
        }
        match dbc.status.as_str() {
            "active" => {}
            "disabled" => {
                let err = ApiError::new(
                    StatusCode::FORBIDDEN,
                    "connection_disabled",
                    "This calendar link is disabled. Ask a workspace admin to enable it.",
                );
                return Outcome::Respond(self.refuse(ctx, &err, "blocked"));
            }
            _ => {
                let err = ApiError::new(
                    StatusCode::FORBIDDEN,
                    "reauth_required",
                    "This calendar link must be reconnected with a new link by a workspace admin.",
                );
                return Outcome::Respond(self.refuse(ctx, &err, "blocked"));
            }
        }
        // (P5-WEBHOOK-2, C-910) A send-mode connection (an outgoing webhook) has its own path.
        if app
            .and_then(|a| a.secret_link.as_ref())
            .is_some_and(|s| s.mode() == aurora_registry::manifest::SecretLinkMode::Send)
        {
            let mut release = None;
            let out = self
                .webhook_request(
                    ctx,
                    identity,
                    &dbc,
                    app,
                    &rv,
                    cred,
                    fi,
                    method,
                    headers,
                    released,
                    body,
                    dry_run,
                    &mut release,
                )
                .await;
            // An approved send: the second pass reads the grant and the policy again.
            return match release {
                Some((again, bytes, c2)) => {
                    Box::pin(self.link_request(
                        c2,
                        identity,
                        Some(conn_id),
                        method,
                        headers,
                        Some(again),
                        LinkBody::Collected(bytes),
                        false,
                    ))
                    .await
                }
                None => out,
            };
        }
        let head = *method == Method::HEAD;
        if *method != Method::GET && !head {
            let err = ApiError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "A calendar link can only be read with GET or HEAD.",
            );
            return Outcome::Respond(self.refuse(ctx, &err, "blocked"));
        }
        // A HEAD is decided as the GET it stands for.
        let tool = app.and_then(|a| {
            apps::identify_tool_in(
                a,
                false,
                &host,
                443,
                "/",
                None,
                "GET",
                &[],
                false,
                &[],
                dbc.conn.connect_method.as_deref(),
            )
        });
        ctx.tool_id = tool.map(|t| t.id.clone());
        let facts = RequestFacts {
            host: host.clone(),
            port: 443,
            path: "/".to_owned(),
            query: None,
            method: "GET".to_owned(),
            body_prefix: Vec::new(),
            truncated: false,
            app_id: ctx.app_id.clone(),
            tool_id: ctx.tool_id.clone(),
            tool_tags: tool
                .zip(app)
                .map(|(t, a)| apps::tool_tags(a, t, None))
                .unwrap_or_default(),
            winning_credential: Some(cred),
            is_llm_host: false,
            identity: fi.clone(),
            connect_time: false,
            host_apps: Vec::new(),
        };
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
        ctx.rule = d.deciding_rule().map(|r| (r.logical_id, r.name.clone()));
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
            // The approval names the connection and the real host, never the URL (SEC-107).
            let real_host = dbc
                .conn
                .metadata
                .get("host")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            let summary = aurora_summary::ApprovalSummary {
                title: format!("Read calendar link {}", dbc.conn.label),
                fields: vec![
                    aurora_summary::SummaryField {
                        label: "Connection".into(),
                        value: dbc.conn.label.clone(),
                    },
                    aurora_summary::SummaryField {
                        label: "Host".into(),
                        value: real_host,
                    },
                ],
                text: None,
                link: None,
                group: None,
                source: aurora_summary::SummarySource::Generic,
            }
            .clamp();
            let (recorded, outcome) = holds
                .hold_with_id(HoldRequest {
                    identity,
                    method: "GET",
                    host: &host,
                    path: "/",
                    rule: ctx.rule.as_ref().map(|(i, n)| (*i, n.as_str())),
                    package_ask: d.package_ask,
                    connection_id: Some(dbc.conn.id),
                    app_id: ctx.app_id.as_deref(),
                    tool_id: ctx.tool_id.as_deref(),
                    batch: &crate::hygiene::AuroraHeaders::default(),
                    summary: &summary,
                    rule_timeout_secs: d.approval_timeout_secs,
                })
                .await;
            ctx.approval_id = recorded;
            match outcome {
                // Re-read the grant and the policy before the link is read.
                HoldOutcome::Approved => {
                    let again = Released {
                        started: ctx.started,
                        bodyless: true,
                        had_credential: true,
                        approval_id: recorded,
                    };
                    let mut c2 = ctx;
                    c2.decision = "allow";
                    return Box::pin(self.link_request(
                        c2,
                        identity,
                        Some(conn_id),
                        method,
                        headers,
                        Some(again),
                        LinkBody::Stream(crate::empty_body()),
                        false,
                    ))
                    .await;
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
                        "No reviewer decided in time, so the link was not read.",
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
                    let err = self.approval_store_unavailable();
                    return Outcome::Respond(self.refuse(ctx, &err, "error"));
                }
            }
        } else if released.is_some() && matches!(d.verdict, Verdict::Allow | Verdict::NeedsApproval)
        {
            ctx.decision = "approved";
        } else if let Some((err, decision)) = self.verdict_error(&rv, &d, "GET", "/", &host, None) {
            return Outcome::Respond(self.refuse(ctx, &err, decision));
        }
        self.apply_hits(&d, &pk);
        // Admitted. Only now is the URL decrypted.
        let raw = match self.plaintext.get_connection(&dbc.conn).await {
            Ok(v) => v,
            Err(PlaintextError::StoreUnavailable(_)) => {
                let err = ApiError::unavailable(
                    "secret_store_unavailable",
                    "The secret store is unreachable. Retry shortly.",
                );
                return Outcome::Respond(self.refuse(ctx, &err, "error"));
            }
            Err(PlaintextError::DatabaseUnavailable) => {
                let err = guidance::gateway_degraded("credential_store_unavailable");
                return Outcome::Respond(self.refuse(ctx, &err, "error"));
            }
            Err(PlaintextError::Unavailable) => {
                return Outcome::Respond(self.refuse(
                    ctx,
                    &credential_unavailable("connection"),
                    "error",
                ));
            }
        };
        // The stored credentials are the JSON object of the connect form's secret fields; the URL
        // is the one the manifest names.
        let name = app
            .and_then(|a| a.secret_link.as_ref())
            .map(|s| s.credential.as_str())
            .unwrap_or_default();
        let url = serde_json::from_slice::<serde_json::Value>(raw.expose())
            .ok()
            .and_then(|v| v.get(name).and_then(|u| u.as_str()).map(str::to_owned));
        let Some(url) = url else {
            return Outcome::Respond(self.refuse(
                ctx,
                &credential_unavailable("connection"),
                "error",
            ));
        };
        self.touch(dbc.conn.id);
        let accept = headers
            .get(http::header::ACCEPT)
            .filter(|v| v.len() <= 512)
            .cloned();
        let if_none_match = headers
            .get(http::header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let log = Arc::clone(&self.log);
        Outcome::Forward(PreparedRequest {
            scheme: crate::pipeline::Scheme::Https,
            host: host.clone(),
            port: 443,
            method: method.clone(),
            path: "/".to_owned(),
            query: None,
            headers: HeaderMap::new(),
            upgrade: None,
            body: crate::empty_body(),
            hook: None,
            response_headers: HeaderMap::new(),
            retry: None,
            ldap: None,
            link: Some(LinkFetch {
                url,
                connection_id: dbc.conn.id,
                version: dbc.conn.version,
                org: dbc.conn.org_id,
                workspace: dbc.conn.workspace_id,
                accept,
                if_none_match,
                head,
                cache: self.link_cache.clone(),
                hops: Box::new(LinkHops {
                    rv: rv.clone(),
                    identity: fi,
                }),
                report: Box::new(LinkLog { ctx, log }),
                send: None,
            }),
        })
    }
}

/// The agent's request body: the live stream, or what the first pass of a send already read.
pub(super) enum LinkBody {
    Stream(crate::Body),
    Collected(bytes::Bytes),
}

mod webhook;
