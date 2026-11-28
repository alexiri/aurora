//! (phase 5, P5-WEBHOOK-2, D-320, C-910, SEC-127) Admission of a send on an outgoing-webhook
//! connection. The order is fixed: admission (grant, status, Read only and the policy verdict, in
//! the caller), the org switch, the configured method, the content type and size, the slot
//! values (before any approval exists), the dry-run branch, the approval, the rate take, the
//! render (the only point where the sealed URL and secrets are opened) and the post
//! (`webhook_send`). Nothing here puts the URL, a secret value, a slot value or a header value in a
//! log row, an approval beyond the slot values, a span or an error.

use super::super::*;
use super::LinkBody;
use crate::seclink::{Hops, LinkFetch, Report};
use crate::webhook_send::SendPlan;
use aurora_core::limits::WEBHOOK_VALUES_MAX;
use aurora_webhook::{SecretValues, WebhookConfig};
use http::Method;

/// The row of a send, written by the lane once it has answered.
struct SendLog {
    ctx: Ctx,
    log: Arc<RequestLog>,
}

impl Report for SendLog {
    fn finish(self: Box<Self>, status: u16, upstream: Option<u16>, code: Option<&str>) {
        let mut ctx = self.ctx;
        if status >= 400 {
            ctx.decision = "error";
        }
        self.log.record(ctx.row(status, upstream, code));
    }
}

/// The org's and workspace's egress policy on every real host of a send (as for a fetch).
struct SendHops {
    rv: Arc<ResolvedHost>,
    identity: RequestIdentity,
}

impl Hops for SendHops {
    fn check(&self, host: &str, port: u16) -> Result<(), ApiError> {
        super::LinkHops {
            rv: self.rv.clone(),
            identity: self.identity.clone(),
        }
        .check(host, port)
    }
}

/// The whole body, at most `max` bytes: `Err(true)` when it is longer, `Err(false)` when it cannot
/// be read.
async fn read_limited(mut b: crate::Body, max: usize) -> Result<bytes::Bytes, bool> {
    use http_body_util::BodyExt;
    let mut buf = bytes::BytesMut::new();
    while let Some(frame) = b.frame().await {
        let Ok(frame) = frame else { return Err(false) };
        if let Some(d) = frame.data_ref() {
            if buf.len() + d.len() > max {
                return Err(true);
            }
            buf.extend_from_slice(d);
        }
    }
    Ok(buf.freeze())
}

fn invalid(slot: &str, rule: &str) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_slot",
        format!("The value of `{slot}` is not accepted ({rule})."),
    )
    .with_details(json!({ "slot": slot, "rule": rule }))
}

fn summarize_slots(
    name: &str,
    host: &str,
    method: &str,
    checked: &aurora_webhook::CheckedValues,
) -> aurora_summary::ApprovalSummary {
    aurora_summary::slots::summary(name, host, method, checked.iter())
}

fn org_limit(org: Option<&serde_json::Value>, key: &str) -> Option<u32> {
    org.and_then(|o| o.get(key))
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
}

impl PolicyHandler {
    /// A refusal that carries `X-Aurora-Error` (C-910): the agent tells it from the receiver's own.
    fn wh_refuse(&self, ctx: Ctx, err: &ApiError, decision: &'static str) -> Outcome {
        let mut r = self.refuse(ctx, err, decision);
        if let Ok(v) = http::HeaderValue::from_str(err.code()) {
            r.headers_mut().insert("x-aurora-error", v);
        }
        Outcome::Respond(r)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn webhook_request(
        &self,
        mut ctx: Ctx,
        identity: &Arc<Ident>,
        dbc: &crate::resolve::DbConnection,
        app: Option<&aurora_registry::snapshot::AppEntry>,
        rv: &Arc<ResolvedHost>,
        cred: Credential,
        fi: RequestIdentity,
        method: &Method,
        headers: &HeaderMap,
        released: Option<Released>,
        body: LinkBody,
        dry: bool,
        release: &mut Option<(Released, bytes::Bytes, Ctx)>,
    ) -> Outcome {
        let host = ctx.host.clone();
        let conn_id = dbc.conn.id;
        // The stored configuration and the org's switch and ceilings, read at every send.
        let (config_json, org_settings) =
            match self.resolver.webhook_config(identity.org_id, conn_id).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "webhook configuration resolution failed");
                    let err = policy_unavailable(&e);
                    return Outcome::Respond(self.refuse(ctx, &err, "error"));
                }
            };
        let org_on = org_settings
            .as_ref()
            .and_then(|s| s.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        if !org_on {
            let err = ApiError::new(
                StatusCode::FORBIDDEN,
                "outgoing_webhooks_disabled",
                "Outgoing webhooks are turned off for this organization.",
            );
            return self.wh_refuse(ctx, &err, "blocked");
        }
        let Some(config) =
            config_json.and_then(|v| serde_json::from_value::<WebhookConfig>(v).ok())
        else {
            return Outcome::Respond(self.refuse(
                ctx,
                &credential_unavailable("connection"),
                "error",
            ));
        };
        // Only the configured method.
        if method.as_str() != config.method.as_str() {
            let err = ApiError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                format!("This webhook is sent with {}.", config.method.as_str()),
            );
            return self.wh_refuse(ctx, &err, "blocked");
        }
        // The agent's body: the slot values, JSON, bounded.
        let json_ct = headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
            });
        if !json_ct {
            let err = ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "webhook_content_type",
                "Send the slot values as a JSON object with Content-Type: application/json.",
            );
            return self.wh_refuse(ctx, &err, "blocked");
        }
        let bytes = match body {
            LinkBody::Collected(b) => b,
            LinkBody::Stream(b) => match read_limited(b, WEBHOOK_VALUES_MAX).await {
                Ok(c) => c,
                Err(too_big) => {
                    let err = if too_big {
                        ApiError::new(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "values_too_large",
                            format!("The slot values are larger than {WEBHOOK_VALUES_MAX} bytes."),
                        )
                    } else {
                        ApiError::new(
                            StatusCode::BAD_REQUEST,
                            "values_unreadable",
                            "The request body could not be read.",
                        )
                    };
                    return self.wh_refuse(ctx, &err, "blocked");
                }
            },
        };
        // Validated before any approval is created: a person is never asked about an invalid send.
        let used = aurora_webhook::secret_names_used(&config);
        let Ok(compiled) = aurora_webhook::validate(&config, &used) else {
            return Outcome::Respond(self.refuse(
                ctx,
                &credential_unavailable("connection"),
                "error",
            ));
        };
        // The rules read these bytes and the message is rendered from the parsed values: a
        // body with more than one reading is refused.
        let values = match aurora_policy::json::read_unrepeated(&bytes) {
            Ok(v) => v,
            Err(_) => return self.wh_refuse(ctx, &invalid("", "type"), "blocked"),
        };
        let checked = match aurora_webhook::check_values(&compiled, &values) {
            Ok(c) => c,
            Err(e) => return self.wh_refuse(ctx, &invalid(&e.slot, e.rule.as_str()), "blocked"),
        };
        let slot_names = checked.given().join(",");
        // The policy verdict for the synthetic request `<method> /`.
        let tool = app.and_then(|a| {
            apps::identify_tool_in(
                a,
                false,
                &host,
                443,
                "/",
                None,
                method.as_str(),
                &[],
                false,
                &[],
                dbc.conn.connect_method.as_deref(),
            )
        });
        ctx.tool_id = tool.map(|t| t.id.clone());
        // This lane relays the receiver's answer without any response projection (see the LDAP
        // check in `handle_with`): refused before the dry run, the approval and the send.
        if tool.is_some_and(|t| t.project.is_some()) {
            return self.wh_refuse(ctx, &lane_projection_refusal(), "blocked");
        }
        let facts = RequestFacts {
            host: host.clone(),
            port: 443,
            path: "/".to_owned(),
            query: None,
            method: method.as_str().to_owned(),
            body_prefix: bytes.to_vec(),
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
        let counted = released.is_none() && !dry;
        let pk = if counted {
            self.rate.peek(rv, identity.agent_id).await
        } else {
            Peeked::Local { fallback: false }
        };
        let d = evaluate(
            &rv.org_policy,
            &rv.ws_policy,
            &facts,
            if counted {
                pk.counter(&self.counters)
            } else {
                &NoCount
            },
        );
        ctx.rule = d.deciding_rule().map(|r| (r.logical_id, r.name.clone()));
        let real_host = dbc
            .conn
            .metadata
            .get("host")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        if dry {
            // Nothing leaves, so an `ask` verdict does not hold it; a deny refuses it.
            if d.verdict != Verdict::NeedsApproval
                && let Some((err, decision)) =
                    self.verdict_error(rv, &d, method.as_str(), "/", &host, None)
            {
                return self.wh_refuse(ctx, &err, decision);
            }
            let Some(rendered) = self.render(&compiled, &checked, None) else {
                let err = ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "rendered_too_large",
                    "The rendered message is larger than the gateway sends.",
                );
                return self.wh_refuse(ctx, &err, "blocked");
            };
            ctx.path = log_path(None, &slot_names, rendered.body.len(), true);
            ctx.decision = "allow";
            self.log.record(ctx.row(200, None, None));
            let out = json!({
                "dry_run": true,
                "method": config.method.as_str(),
                "content_type": rendered.content_type,
                "headers": rendered.headers.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
                "body": String::from_utf8_lossy(&rendered.body),
            });
            let mut r = http::Response::new(crate::full_body(out.to_string()));
            r.headers_mut().insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            );
            return Outcome::Respond(r);
        }
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
            // The approval shows the slot values by name, never a fixed template part, a header,
            // a secret value or the URL (SEC-127).
            let summary = summarize_slots(
                &dbc.conn.label,
                &real_host,
                config.method.as_str(),
                &checked,
            );
            let (recorded, outcome) = holds
                .hold_with_id(HoldRequest {
                    identity,
                    method: method.as_str(),
                    host: &host,
                    path: "/",
                    rule: ctx.rule.as_ref().map(|(i, n)| (*i, n.as_str())),
                    package_ask: d.package_ask,
                    connection_id: Some(conn_id),
                    app_id: ctx.app_id.as_deref(),
                    tool_id: ctx.tool_id.as_deref(),
                    batch: &crate::hygiene::AuroraHeaders::default(),
                    summary: &summary,
                    rule_timeout_secs: d.approval_timeout_secs,
                })
                .await;
            ctx.approval_id = recorded;
            return match outcome {
                // The grant and the policy are read again before the message is sent; the values
                // the person approved are the ones already read.
                HoldOutcome::Approved => {
                    let again = Released {
                        started: ctx.started,
                        bodyless: false,
                        had_credential: true,
                        approval_id: recorded,
                    };
                    let mut c2 = ctx;
                    c2.decision = "allow";
                    // The caller runs the second pass (`link_request`), so this function never
                    // recurses into it.
                    *release = Some((again, bytes, c2));
                    Outcome::Respond(http::Response::new(crate::empty_body()))
                }
                HoldOutcome::Denied(id) => {
                    let err = ApiError::new(
                        StatusCode::FORBIDDEN,
                        "manual_approval_denied",
                        "A reviewer denied this request. Do not retry it unchanged.",
                    )
                    .with_details(json!({ "approval_id": id }));
                    self.wh_refuse(ctx, &err, "approval_denied")
                }
                HoldOutcome::Expired(id) => {
                    let err = ApiError::new(
                        StatusCode::FORBIDDEN,
                        "manual_approval_expired",
                        "No reviewer decided in time, so nothing was sent.",
                    )
                    .with_details(json!({ "approval_id": id }));
                    self.wh_refuse(ctx, &err, "approval_expired")
                }
                HoldOutcome::Restarting => {
                    ctx.decision = "error";
                    self.log
                        .record(ctx.row(503, None, Some("gateway_restarting")));
                    Outcome::Respond(guidance::response(&ApiError::unavailable(
                        "gateway_restarting",
                        "The gateway is restarting while this request awaited approval. Retry shortly.",
                    )))
                }
                HoldOutcome::StoreUnavailable => {
                    let err = ApiError::unavailable(
                        "approval_store_unavailable",
                        "The approval store is unavailable. Retry shortly.",
                    );
                    Outcome::Respond(self.refuse(ctx, &err, "error"))
                }
            };
        } else if released.is_some() && matches!(d.verdict, Verdict::Allow | Verdict::NeedsApproval)
        {
            ctx.decision = "approved";
        } else if let Some((err, decision)) =
            self.verdict_error(rv, &d, method.as_str(), "/", &host, None)
        {
            return self.wh_refuse(ctx, &err, decision);
        }
        self.apply_hits(&d, &pk);
        // The rate take: after admission and approval, just before the request leaves. The
        // effective limit is the lowest of the instance, the org and the connection (C-912).
        let ceiling = **self.webhook_ceiling.load();
        let eff = aurora_webhook::effective_rate(
            ceiling,
            (
                org_limit(org_settings.as_ref(), "per_minute_max"),
                org_limit(org_settings.as_ref(), "per_day_max"),
            ),
            &config.rate,
        );
        if let Err(err) = self
            .rate
            .take_webhook(conn_id, eff.per_minute, eff.per_day)
            .await
        {
            let decision = if err.status() == StatusCode::TOO_MANY_REQUESTS {
                "rate_limited"
            } else {
                "error"
            };
            return self.wh_refuse(ctx, &err, decision);
        }
        // Admitted. Only now are the URL and the secrets opened.
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
        let creds: serde_json::Value = serde_json::from_slice(raw.expose()).unwrap_or_default();
        let url_key = app
            .and_then(|a| a.secret_link.as_ref())
            .map(|s| s.credential.as_str())
            .unwrap_or_default();
        let url = creds
            .get(url_key)
            .and_then(|u| u.as_str())
            .map(str::to_owned);
        let secrets: std::collections::BTreeMap<String, String> = creds
            .get("secret_values")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        let signing = config
            .signing
            .then(|| {
                creds
                    .get("signing_secret")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            })
            .flatten();
        let Some(url) = url else {
            return Outcome::Respond(self.refuse(
                ctx,
                &credential_unavailable("connection"),
                "error",
            ));
        };
        let secret_values: Vec<String> = secrets.values().cloned().collect();
        let rendered = match aurora_webhook::render(&compiled, &checked, &SecretValues(secrets)) {
            Ok(r) => r,
            Err(aurora_webhook::RenderError::TooLarge) => {
                let err = ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "rendered_too_large",
                    "The rendered message is larger than the gateway sends.",
                );
                return self.wh_refuse(ctx, &err, "blocked");
            }
            Err(_) => {
                return Outcome::Respond(self.refuse(
                    ctx,
                    &credential_unavailable("connection"),
                    "error",
                ));
            }
        };
        self.touch(conn_id);
        let delivery_id = Uuid::now_v7();
        ctx.path = log_path(Some(delivery_id), &slot_names, rendered.body.len(), false);
        let plan = SendPlan {
            method: method.clone(),
            content_type: rendered.content_type.clone(),
            body: bytes::Bytes::from(rendered.body),
            headers: rendered.headers,
            signing_secret: signing,
            secret_values,
            delivery_id,
        };
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
                connection_id: conn_id,
                version: dbc.conn.version,
                org: dbc.conn.org_id,
                workspace: dbc.conn.workspace_id,
                accept: None,
                if_none_match: None,
                head: false,
                cache: self.link_cache.clone(),
                hops: Box::new(SendHops {
                    rv: rv.clone(),
                    identity: fi,
                }),
                report: Box::new(SendLog { ctx, log }),
                send: Some(plan),
            }),
        })
    }

    /// The masked render of a dry run (secrets as `«secret:name»`).
    fn render(
        &self,
        compiled: &aurora_webhook::CompiledConfig,
        checked: &aurora_webhook::CheckedValues,
        _secrets: Option<&SecretValues>,
    ) -> Option<aurora_webhook::Rendered> {
        aurora_webhook::render_masked(compiled, checked).ok()
    }
}

/// What the request log records of a send in the `path` column: the slot **names**, the rendered
/// size, the delivery id and whether it was a dry run. Never a value, a header or the URL.
fn log_path(delivery: Option<Uuid>, slots: &str, bytes: usize, dry: bool) -> String {
    let d = delivery.map_or_else(String::new, |d| format!("delivery_id={d}&"));
    format!("/?{d}slots={slots}&bytes={bytes}&dry_run={}", u8::from(dry))
}
