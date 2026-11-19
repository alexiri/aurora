//! Simulation: builds request facts the way the gateway resolves them and runs the shared
//! `aurora_policy::evaluate`. For `lane: request` that is (FIX-159, C-930): secret candidates,
//! the app the request is for (Active host claims, registered instances), that app's connections
//! as candidates, injection selection, the winner rule, the tool (`params`, `connect_methods`),
//! the tool tags and the connection's `read_only`; identification, binding and the winner rule are
//! `aurora_registry::identify`, the gateway's own code.
//! Not simulated: LLM governance step 6a, live rate-limit counters (always fresh), approval holds,
//! request headers (a body is assumed to be JSON or a urlencoded form; a tool whose `params` read
//! another header never matches), the LDAP lane's synthetic
//! hosts, `reauth_required` guidance and the test-only loopback injection switch.
use super::dto::*;
use crate::app::AppState;
use crate::domains::secrets::gov;
use aurora_core::ApiError;
use aurora_db::repo::gateway::{self, ScopePolicyRows};
use aurora_db::repo::policy as repo;
use aurora_llm::governance::{EffectiveGovernance, OpenLaneAdmission, OpenLanePosture};
use aurora_policy::eval::{
    AllowEntry, BlockReason, RuleResult, ScopePolicy, UncredentialedDefault as Unc, Verdict, Via,
};
use aurora_policy::facts::{Level, SecretKind};
use aurora_policy::{
    Credential, HostPattern, MemoryCounter, PathPattern, RequestFacts, RequestIdentity, Rule,
    evaluate, selected_candidates,
};
use aurora_registry::identify;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use uuid::Uuid;

/// (D-275) Whether the connection's metadata `hosts_field` array of its `own_server` app holds
/// `origin` (case-insensitively), the gateway resolver's match.
fn pins_origin(
    snap: &aurora_registry::snapshot::Snapshot,
    c: &gateway::AppConnectionRow,
    origin: &str,
) -> bool {
    let Some(o) = snap.app(&c.app_id).and_then(|a| a.own_server.as_ref()) else {
        return false;
    };
    c.metadata
        .get(&o.hosts_field)
        .and_then(|v| v.as_array())
        .is_some_and(|a| {
            a.iter()
                .filter_map(|h| h.as_str())
                .any(|h| h.eq_ignore_ascii_case(origin))
        })
}

/// Same assembly as the gateway resolver: hand rules by priority, then derived rules.
pub fn to_scope_policy(rows: ScopePolicyRows) -> Result<ScopePolicy, ApiError> {
    let bad = |what: &str, e: serde_json::Error| {
        tracing::error!(error = %e, what, "stored policy is malformed");
        ApiError::internal("Internal error.")
    };
    let mut rules = Vec::with_capacity(rows.rules.len() + rows.derived.len());
    for r in rows.rules {
        let v = json!({
            "logical_id": r.logical_id, "name": r.name, "enabled": r.enabled,
            "identities": r.identities, "targets": r.targets, "action": r.action,
            "require_approval": r.require_approval,
            "approval_timeout_secs": r.approval_timeout_secs, "rate_limit": r.rate_limit,
            "conditions": r.conditions,
        });
        rules.push(serde_json::from_value::<Rule>(v).map_err(|e| bad("rule", e))?);
    }
    for (logical_id, mut v) in rows.derived {
        if let Some(o) = v.as_object_mut() {
            o.insert("logical_id".into(), json!(logical_id));
        }
        rules.push(serde_json::from_value::<Rule>(v).map_err(|e| bad("derived rule", e))?);
    }
    let v = json!({
        "default_action": rows.default_action,
        "uncredentialed_default": rows.uncredentialed_default,
        "uncredentialed_allowlist": rows.uncredentialed_allowlist,
    });
    let mut p: ScopePolicy = serde_json::from_value(v).map_err(|e| bad("scope", e))?;
    p.ordered_rules = rules;
    Ok(p)
}

/// (RFE-011a) The hold timeout a `needs_approval` verdict would get; same function as the gateway.
pub fn effective_timeout_secs(a: &aurora_config::ApprovalsSection, rule: Option<u32>) -> u64 {
    aurora_core::limits::effective_approval_timeout_secs(
        rule,
        a.timeout_secs,
        aurora_core::limits::APPROVAL_TIMEOUT_MIN.as_secs(),
        timeout_ceiling(a),
    )
}

/// (P4-011B-2, C-829) The longest timeout a question may be given now: the parking maximum while
/// parking is on, else the hold maximum.
fn timeout_ceiling(a: &aurora_config::ApprovalsSection) -> u64 {
    if a.park.enabled {
        a.park.max_timeout_secs
    } else {
        a.max_timeout_secs
    }
}

/// `(approval_parks, approval_timeout_clamped)` of a `needs_approval` verdict (C-829): whether its
/// effective timeout is above the parking threshold with parking on (the request parks, if it is
/// otherwise eligible), and whether the wanted timeout was cut to the ceiling.
pub fn timeout_flags(
    a: &aurora_config::ApprovalsSection,
    rule: Option<u32>,
    needs_approval: bool,
) -> (bool, bool) {
    if !needs_approval {
        return (false, false);
    }
    let secs = effective_timeout_secs(a, rule);
    let wanted = rule.map_or(a.timeout_secs, u64::from);
    (
        a.park.enabled && secs > a.park.threshold_secs,
        wanted > timeout_ceiling(a),
    )
}

fn field(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_field("Invalid request.", path, msg)
}

fn sim_verdict(v: Verdict) -> SimVerdict {
    match v {
        Verdict::Allow => SimVerdict::Allow,
        Verdict::NeedsApproval => SimVerdict::NeedsApproval,
        Verdict::RateLimited => SimVerdict::RateLimited,
        Verdict::Block { .. } => SimVerdict::Block,
    }
}

fn reason_str(r: RuleResult) -> &'static str {
    match r {
        RuleResult::Disabled => "disabled",
        RuleResult::IdentityMismatch => "identity_mismatch",
        RuleResult::TargetMismatch => "target_mismatch",
        RuleResult::ConditionsNotMet => "conditions_not_met",
        RuleResult::Matched => "matched",
    }
}

fn scope_trace(t: &aurora_policy::eval::ScopeTrace, derived: &HashSet<Uuid>) -> ScopeTrace {
    let mut considered: Vec<TraceEntry> = t
        .considered
        .iter()
        .map(|c| TraceEntry {
            rule_logical_id: c.logical_id,
            rule_name: c.name.clone(),
            source: if derived.contains(&c.logical_id) {
                TraceSource::Derived
            } else {
                TraceSource::Hand
            },
            matched: c.result == RuleResult::Matched,
            reason: reason_str(c.result).to_owned(),
        })
        .collect();
    // (matched, reason): the skipped Default Rule did not apply, so it is `matched: false` (C-350).
    let default = match t.via {
        Via::Default => Some(("Default rule", "default_action", true)),
        Via::DefaultSkippedLlmHost => {
            Some(("Default rule", "default_rule_skipped_llm_host", false))
        }
        Via::Uncredentialed => Some(("Uncredentialed default", "uncredentialed_default", true)),
        Via::UncredentialedAllowlist => {
            Some(("Uncredentialed allowlist", "uncredentialed_allowlist", true))
        }
        // (FIX-159, C-930) The checks a connection winner runs before any rule: read-only,
        // `block_untooled` and a missing scope. No rule explains them, so they get their own
        // entry (`pre_rule_entry`).
        v @ (Via::AppUntooledBlocked | Via::ReadOnlyBlocked | Via::ToolScopeBlocked) => {
            pre_rule_entry(v).map(|(name, reason)| (name, reason, true))
        }
        Via::Rule | Via::Nothing => None,
    };
    if let Some((name, reason, matched)) = default {
        considered.push(TraceEntry {
            rule_logical_id: Uuid::nil(),
            rule_name: name.to_owned(),
            source: TraceSource::Default,
            matched,
            reason: reason.to_owned(),
        });
    }
    ScopeTrace {
        verdict: t.verdict.map(sim_verdict),
        considered,
    }
}

/// A credential candidate: a generic secret or an app connection (FIX-159).
struct Cand {
    id: Uuid,
    name: String,
    scope: Level,
    has_path: bool,
    cred: Credential,
    /// An app connection (not a secret).
    app: bool,
    connect_method: Option<String>,
    scopes: Vec<String>,
    app_id: Option<String>,
}

impl Cand {
    fn credential(&self) -> Credential {
        self.cred.clone()
    }
}

/// The pre-rule checks the evaluator ran that no rule explains (C-930): shown in the trace.
fn pre_rule_entry(via: Via) -> Option<(&'static str, &'static str)> {
    match via {
        Via::AppUntooledBlocked => Some(("Request not in the app's catalogue", "block_untooled")),
        Via::ReadOnlyBlocked => Some(("Read-only connection", "read_only_connection")),
        Via::ToolScopeBlocked => {
            Some(("Connection lacks a scope the action needs", "scope_missing"))
        }
        _ => None,
    }
}

pub async fn run(
    state: &AppState,
    org: Uuid,
    ws: Uuid,
    agent_id: Uuid,
    req: SimulateRequest,
) -> Result<Simulation, ApiError> {
    let method = req.method.trim().to_ascii_uppercase();
    if method.is_empty() || method.len() > 16 || !method.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(field("/method", "invalid HTTP method"));
    }
    let url = reqwest::Url::parse(&req.url).map_err(|_| field("/url", "invalid URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(field("/url", "scheme must be http or https"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| field("/url", "URL has no host"))?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let port = url.port_or_known_default().unwrap_or(443);
    let path = url.path().to_owned();
    if req.lane == Some(SimLane::Open)
        && (url.scheme() != "https" || path != "/" || url.query().is_some())
    {
        return Err(field("/url", "the open lane takes https://host[:port]"));
    }
    let use_draft = req.use_draft.unwrap_or(false);
    // RFE-005: a speaker who does not hold `can_use` never binds in the gateway, so refuse it
    // rather than simulate a situation that cannot occur.
    if let Some(u) = req.speaker_user_id
        && !gateway::can_use(&state.pool, ws, u).await?
    {
        return Err(field(
            "/speaker_user_id",
            "not a member with access to this workspace",
        ));
    }
    let speaker = req.speaker_user_id;

    let org_rows = repo::load_scope_rows(&state.pool, org, None, use_draft).await?;
    let ws_rows = repo::load_scope_rows(&state.pool, org, Some(ws), use_draft).await?;
    let derived: HashSet<Uuid> = org_rows
        .derived
        .iter()
        .chain(&ws_rows.derived)
        .map(|(id, _)| *id)
        .collect();
    // An unparsable stored posture fails closed.
    let ws_posture =
        OpenLanePosture::parse(&ws_rows.open_lane_posture).unwrap_or(OpenLanePosture::Disabled);
    let mut org_policy = to_scope_policy(org_rows)?;
    let ws_policy = to_scope_policy(ws_rows)?;
    let gov = gov::effective_for(state, org).await?;
    if gov.egress_posture == aurora_llm::governance::EgressPosture::CredentialedOnly {
        org_policy.uncredentialed_default = Unc::Block;
        org_policy.uncredentialed_allowlist = gov
            .egress_allowlist
            .iter()
            .map(|a| AllowEntry {
                host: a.host.clone(),
                path: a.path.clone(),
            })
            .collect();
    }
    let host_port = if port == 443 || port == 80 {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    let is_llm_host = state.llm().is_endpoint_host(&host_port) || gov.is_known_llm_host(&host_port);
    if req.lane == Some(SimLane::Open) {
        return open_lane(
            state,
            OpenInput {
                org: org_policy,
                ws: ws_policy,
                gov: &gov,
                ws_posture,
                derived: &derived,
                host,
                port,
                is_llm_host,
                // Tunnels (open lane) carry no speaker (RFE-005).
                identity: RequestIdentity {
                    agent_id,
                    speaker_user_id: None,
                    speaker_group_ids: Vec::new(),
                },
            },
        )
        .await;
    }

    // Candidates: active secrets whose host/path serve this request (08 §6: generic secrets
    // are never candidates on governed LLM hosts).
    let mut cands = Vec::new();
    for r in gateway::active_secrets(&state.pool, org, ws).await? {
        if is_llm_host && r.kind != "llm" {
            continue;
        }
        if !HostPattern::lenient(&r.host_pattern).matches(&host, port) {
            continue;
        }
        if r.path_pattern
            .as_deref()
            .is_some_and(|p| !PathPattern::lenient(p).matches(&path))
        {
            continue;
        }
        let scope = if r.workspace_id.is_some() {
            Level::Workspace
        } else {
            Level::Organization
        };
        cands.push(Cand {
            id: r.id,
            name: r.name,
            scope,
            has_path: r.path_pattern.is_some(),
            cred: Credential::Secret {
                id: r.id,
                kind: if r.kind == "llm" {
                    SecretKind::Llm
                } else {
                    SecretKind::Generic
                },
                scope,
            },
            app: false,
            connect_method: None,
            scopes: Vec::new(),
            app_id: None,
        });
    }

    // (FIX-159, C-930) The app the request is for, and its connections as candidates: the same
    // identification, binding and winner rule the gateway runs (`aurora_registry::identify`).
    // Governed LLM hosts and reserved hosts never serve apps (SEC-37).
    let snap = if is_llm_host {
        None
    } else {
        crate::domains::apps::service::snapshot(state).await?
    }
    .filter(|s| {
        !s.reserved_hosts
            .iter()
            .any(|p| HostPattern::lenient(p).matches(&host, port))
    });
    let query = url.query().map(str::to_owned);
    let https = url.scheme() == "https";
    let mut app_unavailable = false;
    let mut req_app = None;
    if let Some(snap) = snap.as_ref() {
        let entries = identify::entries_for_host(snap, &host, port);
        let visible = gateway::visible_instances(&state.pool, org).await?;
        let inst_hosts: HashMap<Uuid, (String, Option<String>)> = visible
            .iter()
            .map(|i| (i.id, (i.host.clone(), i.base_path.clone())))
            .collect();
        let instances = identify::host_instances(
            snap,
            visible,
            &host,
            &entries,
            |i| i.host.as_str(),
            |i| i.app_id.as_str(),
        );
        let reg_read: identify::RegRead<gateway::InstanceHostRow> =
            |i| (i.app_id.as_str(), i.base_path.as_deref());
        req_app = identify::identify_request(snap, &instances, reg_read, &host, port, &path);
        // (D-275, SEC-109) As the gateway's resolver: no static claim or instance covers the
        // origin, so an `own_server` app whose connection pinned exactly this `host:port` is the
        // app; two apps pinning one origin serve nothing (fail closed).
        let origin = identify::origin_of(&host, port);
        if req_app.is_none() && entries.is_empty() && instances.is_empty() {
            let own_ids: Vec<String> = snap
                .apps
                .iter()
                .filter(|a| a.own_server.is_some())
                .map(|a| a.app_id.clone())
                .collect();
            if !own_ids.is_empty() {
                let pinning: HashSet<String> =
                    gateway::app_connections(&state.pool, org, ws, &own_ids)
                        .await?
                        .into_iter()
                        .filter(|c| c.status != "disabled" && pins_origin(snap, c, &origin))
                        .map(|c| c.app_id)
                        .collect();
                if pinning.len() == 1
                    && let Some(matched) = pinning
                        .iter()
                        .next()
                        .and_then(|a| identify::identify_own_server_app(snap, a))
                {
                    req_app = Some(identify::RequestApp {
                        matched,
                        instance: None,
                        selected: None,
                        app_path: path.clone(),
                    });
                }
            }
        }
        let own_pinned = req_app
            .as_ref()
            .is_some_and(|r| r.matched.app.own_server.is_some());
        if let Some(r) = &req_app {
            let m = &r.matched;
            let mut app_ids: Vec<String> = entries.iter().map(|e| e.app_id.clone()).collect();
            app_ids.extend(instances.iter().map(|i| i.app_id.clone()));
            if own_pinned {
                app_ids.push(m.entry.app_id.clone());
            }
            app_ids.sort();
            app_ids.dedup();
            let (restricted, available) = gateway::app_availability(&state.pool, org, ws).await?;
            app_unavailable = restricted && !available.contains(&m.entry.app_id);
            let mut same = Vec::new();
            for row in gateway::app_connections(&state.pool, org, ws, &app_ids).await? {
                if row.app_id != m.entry.app_id
                    || (restricted && !available.contains(&row.app_id))
                    || row.status == "disabled"
                    || (own_pinned && !pins_origin(snap, &row, &origin))
                {
                    continue;
                }
                // A connection of a registered instance: only while that instance is active and
                // visible.
                if row
                    .instance_id
                    .is_some_and(|id| !inst_hosts.contains_key(&id))
                {
                    continue;
                }
                same.push(row);
            }
            let on_instance = r.instance.map(|ix| instances[ix].id);
            let bound = identify::bind_connections(
                m,
                on_instance,
                &host,
                same,
                |c: &gateway::AppConnectionRow| (c.instance_id, c.tenant_host.as_deref()),
            );
            // (FIX-104, D-180, SEC-04) A host claim is injected only over TLS to the claimed port.
            // A person-pinned origin is injected over TLS to its own port only (D-275, SEC-04).
            let claim_port = if own_pinned {
                Some(port)
            } else {
                identify::claim_port(&m.entry.host)
            };
            if identify::claim_transport_ok(claim_port, https, port) {
                // (FIX-084, SEC-67) An app absent from the snapshot is `custom_only` and
                // `block_untooled`.
                let restrictions = m.app.grants;
                for c in bound {
                    let scope = if c.workspace_id.is_some() {
                        Level::Workspace
                    } else {
                        Level::Organization
                    };
                    cands.push(Cand {
                        id: c.id,
                        name: c.label,
                        scope,
                        has_path: false,
                        cred: Credential::Connection {
                            id: c.id,
                            app_id: c.app_id.clone(),
                            scope,
                            custom_only: restrictions.custom_only,
                            block_untooled: restrictions.block_untooled,
                            // Fail closed: anything but the known `write` reads as read-only.
                            read_only: c.access != "write",
                        },
                        app: true,
                        connect_method: c.connect_method,
                        scopes: c.scopes,
                        app_id: Some(c.app_id),
                    });
                }
            }
        }
    }

    let speaker_group_ids = match speaker {
        Some(u) => gateway::speaker_group_ids(&state.pool, org, u).await?,
        None => Vec::new(),
    };
    let identity = RequestIdentity {
        agent_id,
        speaker_user_id: speaker,
        speaker_group_ids,
    };
    let creds: Vec<Credential> = cands.iter().map(Cand::credential).collect();
    let selected = selected_candidates(
        org_policy
            .ordered_rules
            .iter()
            .chain(&ws_policy.ordered_rules),
        &identity,
        &creds,
    );
    let is_sel = |c: &Cand| selected.contains(&c.credential().key());
    // Winner precedence (06 step 6, the gateway's rule): app connection over generic secret,
    // path-limited over whole-host, workspace over org.
    let granted: Vec<&Cand> = cands.iter().filter(|c| is_sel(c)).collect();
    let ranks: Vec<identify::Rank> = granted
        .iter()
        .map(|c| identify::Rank {
            is_app: c.app,
            path_limited: c.has_path,
            workspace: c.scope == Level::Workspace,
        })
        .collect();
    let kept: Vec<&Cand> = identify::narrow(&ranks)
        .into_iter()
        .map(|i| granted[i])
        .collect();
    let winner = if kept.len() == 1 { Some(kept[0]) } else { None };
    let kind_of = |c: &Cand| {
        if c.app {
            CredentialKind::Connection
        } else {
            CredentialKind::Secret
        }
    };
    let candidates: Vec<SimCandidate> = cands
        .iter()
        .map(|c| SimCandidate {
            kind: kind_of(c),
            id: c.id,
            name: c.name.clone(),
            selected: is_sel(c),
        })
        .collect();
    let winning_credential = winner.map(|c| WinningCredential {
        kind: kind_of(c),
        id: c.id,
        name: c.name.clone(),
    });

    let body = req.body.unwrap_or_default().into_bytes();
    let limit = aurora_core::limits::CONDITION_BODY_BUFFER;
    let truncated = body.len() > limit;
    let body_prefix: Vec<u8> = body.into_iter().take(limit).collect();
    // The simulation carries no headers; a body is assumed to be what a client sends with it: JSON
    // when it looks like JSON, a form otherwise (a `params` form condition reads the content type).
    let content_type = if body_prefix.is_empty() {
        None
    } else if matches!(body_prefix.first(), Some(b'{' | b'[')) {
        Some("application/json")
    } else {
        Some("application/x-www-form-urlencoded")
    };
    let param_headers: Vec<(&str, &str)> = content_type
        .map(|ct| vec![("content-type", ct)])
        .unwrap_or_default();
    // The tool follows the winner: a tool restricted to other connect methods is not this
    // connection's (FIX-123).
    let tool = req_app.as_ref().and_then(|r| {
        identify::identify_tool(
            &r.matched,
            &host,
            port,
            &r.app_path,
            query.as_deref(),
            &method,
            &body_prefix,
            truncated,
            &param_headers,
            winner.and_then(|c| c.connect_method.as_deref()),
        )
    });
    let app_dto = req_app.as_ref().map(|r| SimApp {
        app_id: r.matched.entry.app_id.clone(),
        tool_id: tool.map(|t| t.id.clone()),
    });
    let tool_tags = tool
        .zip(req_app.as_ref())
        .map(|(t, r)| {
            identify::tool_tags(
                r.matched.app,
                t,
                winner.filter(|c| c.app).map(|c| c.scopes.as_slice()),
            )
        })
        .unwrap_or_default();
    let facts = RequestFacts {
        host,
        port,
        path,
        query,
        method,
        body_prefix,
        truncated,
        app_id: app_dto.as_ref().map(|a| a.app_id.clone()),
        tool_id: tool.map(|t| t.id.clone()),
        tool_tags,
        winning_credential: winner.map(Cand::credential),
        is_llm_host,
        identity,
        connect_time: false,
        host_apps: Vec::new(),
    };
    let d = evaluate(&org_policy, &ws_policy, &facts, &MemoryCounter::new());
    let mut trace = SimTrace {
        organization: scope_trace(&d.trace.organization, &derived),
        workspace: scope_trace(&d.trace.workspace, &derived),
    };
    // (P5-TOOLASK, C-1043) The app's own default decided: no rule did. Shown as the last entry of
    // the workspace trace, in the shape the synthetic default entries use.
    if d.package_ask {
        trace.workspace.considered.push(TraceEntry {
            rule_logical_id: Uuid::nil(),
            rule_name: "App default: asks first".to_owned(),
            source: TraceSource::Default,
            matched: true,
            reason: "package_default_ask".to_owned(),
        });
    }

    if kept.len() > 1 {
        let same_app = kept[0].app_id.is_some() && kept.iter().all(|c| c.app_id == kept[0].app_id);
        return Ok(Simulation {
            approval_timeout_secs: None,
            approval_parks: false,
            approval_timeout_clamped: false,
            verdict: SimVerdict::Block,
            reason: None,
            guidance_code: Some(
                if same_app {
                    "multiple_connections"
                } else {
                    "multiple_providers"
                }
                .into(),
            ),
            winning_credential: None,
            candidates,
            app: app_dto,
            trace,
        });
    }
    // The gateway refuses a request for an app the organization's app policy withholds from this
    // workspace when no credential won (`app_unavailable`).
    if winner.is_none() && app_unavailable {
        return Ok(Simulation {
            approval_timeout_secs: None,
            approval_parks: false,
            approval_timeout_clamped: false,
            verdict: SimVerdict::Block,
            reason: None,
            guidance_code: Some("app_unavailable".into()),
            winning_credential: None,
            candidates,
            app: app_dto,
            trace,
        });
    }
    let (reason, guidance) = match d.verdict {
        Verdict::Allow => (None, None),
        Verdict::NeedsApproval | Verdict::RateLimited => (Some(SimReason::Rule), None),
        Verdict::Block {
            reason:
                BlockReason::Rule
                | BlockReason::AppUntooledBlocked
                | BlockReason::ReadOnlyConnection
                | BlockReason::ToolScopeMissing,
        } => (Some(SimReason::Rule), Some("blocked_by_policy")),
        Verdict::Block {
            reason: BlockReason::DefaultDenied,
        } => (
            Some(SimReason::DefaultDenied),
            Some("blocked_by_default_policy"),
        ),
        Verdict::Block {
            reason: BlockReason::EgressDenied,
        } => (
            Some(SimReason::EgressDenied),
            Some("blocked_by_egress_policy"),
        ),
    };
    let needs = d.verdict == Verdict::NeedsApproval;
    let approval_timeout_secs =
        needs.then(|| effective_timeout_secs(&state.ops().approvals, d.approval_timeout_secs));
    let (approval_parks, approval_timeout_clamped) =
        timeout_flags(&state.ops().approvals, d.approval_timeout_secs, needs);
    Ok(Simulation {
        approval_timeout_secs,
        approval_parks,
        approval_timeout_clamped,
        verdict: sim_verdict(d.verdict),
        reason,
        guidance_code: guidance.map(str::to_owned),
        winning_credential,
        candidates,
        app: app_dto,
        trace,
    })
}

struct OpenInput<'a> {
    org: ScopePolicy,
    ws: ScopePolicy,
    gov: &'a EffectiveGovernance,
    ws_posture: OpenLanePosture,
    derived: &'a HashSet<Uuid>,
    host: String,
    port: u16,
    is_llm_host: bool,
    identity: RequestIdentity,
}

/// `lane: "open"` (RFE-003): the open-lane posture, then CONNECT-time rule semantics. The
/// credential capability is a property of the sandbox boot and is not simulated; governed LLM
/// hosts under `sanctioned_only` are refused by the gateway and are not simulated either.
async fn open_lane(state: &AppState, i: OpenInput<'_>) -> Result<Simulation, ApiError> {
    let host_apps: Vec<String> = if i.is_llm_host {
        vec![]
    } else {
        match crate::domains::apps::service::snapshot(state).await? {
            Some(snap) => {
                let reserved = snap
                    .reserved_hosts
                    .iter()
                    .any(|p| HostPattern::lenient(p).matches(&i.host, i.port));
                let mut ids: Vec<String> = if reserved {
                    vec![]
                } else {
                    snap.hosts
                        .iter()
                        .filter(|h| {
                            h.claim_status == aurora_registry::snapshot::ClaimStatus::Active
                        })
                        .filter(|h| HostPattern::lenient(&h.host).matches(&i.host, i.port))
                        .map(|h| h.app_id.clone())
                        .collect()
                };
                ids.sort();
                ids.dedup();
                ids
            }
            None => vec![],
        }
    };
    let facts = RequestFacts {
        host: i.host,
        port: i.port,
        path: "/".into(),
        query: None,
        method: "CONNECT".into(),
        body_prefix: Vec::new(),
        truncated: false,
        app_id: None,
        tool_id: None,
        tool_tags: Default::default(),
        winning_credential: None,
        is_llm_host: i.is_llm_host,
        identity: i.identity,
        connect_time: true,
        host_apps,
    };
    let d = evaluate(&i.org, &i.ws, &facts, &MemoryCounter::new());
    let trace = SimTrace {
        organization: scope_trace(&d.trace.organization, i.derived),
        workspace: scope_trace(&d.trace.workspace, i.derived),
    };
    let refused = |reason, code: &str| Simulation {
        approval_timeout_secs: None,
        approval_parks: false,
        approval_timeout_clamped: false,
        verdict: SimVerdict::Block,
        reason: Some(reason),
        guidance_code: Some(code.to_owned()),
        winning_credential: None,
        candidates: vec![],
        app: None,
        trace: trace.clone(),
    };
    match i
        .gov
        .open_lane_admission(i.ws_posture, &i.org, &i.ws, &facts)
    {
        OpenLaneAdmission::Disabled => {
            return Ok(refused(SimReason::OpenLane, "open_lane_disabled"));
        }
        OpenLaneAdmission::NotAllowlisted => {
            return Ok(refused(SimReason::EgressDenied, "blocked_by_egress_policy"));
        }
        OpenLaneAdmission::Admitted => {}
    }
    let (verdict, reason, guidance) = match d.verdict {
        Verdict::Allow => (SimVerdict::Allow, None, None),
        // A tunnel is never held: the gateway refuses the CONNECT.
        Verdict::NeedsApproval => (
            SimVerdict::Block,
            Some(SimReason::Rule),
            Some("blocked_by_policy"),
        ),
        Verdict::RateLimited => (SimVerdict::RateLimited, Some(SimReason::Rule), None),
        Verdict::Block {
            reason:
                BlockReason::Rule
                | BlockReason::AppUntooledBlocked
                | BlockReason::ReadOnlyConnection
                | BlockReason::ToolScopeMissing,
        } => (
            SimVerdict::Block,
            Some(SimReason::Rule),
            Some("blocked_by_policy"),
        ),
        Verdict::Block {
            reason: BlockReason::DefaultDenied,
        } => (
            SimVerdict::Block,
            Some(SimReason::DefaultDenied),
            Some("blocked_by_default_policy"),
        ),
        Verdict::Block {
            reason: BlockReason::EgressDenied,
        } => (
            SimVerdict::Block,
            Some(SimReason::EgressDenied),
            Some("blocked_by_egress_policy"),
        ),
    };
    Ok(Simulation {
        approval_timeout_secs: None,
        approval_parks: false,
        approval_timeout_clamped: false,
        verdict,
        reason,
        guidance_code: guidance.map(str::to_owned),
        winning_credential: None,
        candidates: vec![],
        app: None,
        trace,
    })
}

#[cfg(test)]
mod timeout_flag_tests {
    use super::*;

    fn cfg(enabled: bool) -> aurora_config::ApprovalsSection {
        let mut a = aurora_config::ApprovalsSection::default();
        a.park.enabled = enabled;
        a.max_timeout_secs = 3600;
        a
    }

    // C-829: the timeout bound follows the switch; `approval_parks` needs parking on and a
    // timeout above the threshold; `approval_timeout_clamped` says the wanted value was cut.
    #[test]
    fn the_bound_and_the_flags_follow_the_parking_switch() {
        let off = cfg(false);
        assert_eq!(effective_timeout_secs(&off, Some(7200)), 3600);
        assert_eq!(timeout_flags(&off, Some(7200), true), (false, true));
        assert_eq!(timeout_flags(&off, Some(600), true), (false, false));
        let on = cfg(true);
        assert_eq!(effective_timeout_secs(&on, Some(7200)), 7200);
        assert_eq!(timeout_flags(&on, Some(7200), true), (true, false));
        assert_eq!(timeout_flags(&on, Some(600), true), (false, false));
        assert_eq!(timeout_flags(&on, Some(200_000), true), (true, true));
        assert_eq!(timeout_flags(&on, Some(7200), false), (false, false));
    }
}
