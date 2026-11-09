//! Simulation: builds request facts the way the gateway resolves them (secret candidates,
//! injection selection, LLM-host handling) and runs the shared `aurora_policy::evaluate`.
//! Not simulated: LLM governance step 6a, app/tool identification (M3), live rate-limit
//! counters (always fresh) and approval holds.
use super::dto::*;
use crate::app::AppState;
use crate::domains::secrets::gov;
use aurora_core::{ApiError, FieldError};
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
use serde_json::json;
use std::collections::HashSet;
use uuid::Uuid;

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
        a.max_timeout_secs,
    )
}

fn field(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid request.", vec![FieldError::new(path, msg)])
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

struct Cand {
    id: Uuid,
    name: String,
    kind: SecretKind,
    scope: Level,
    has_path: bool,
}

impl Cand {
    fn credential(&self) -> Credential {
        Credential::Secret {
            id: self.id,
            kind: self.kind,
            scope: self.scope,
        }
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
    let is_llm_host = state.llm.is_endpoint_host(&host_port) || gov.is_known_llm_host(&host_port);
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
        cands.push(Cand {
            id: r.id,
            name: r.name,
            kind: if r.kind == "llm" {
                SecretKind::Llm
            } else {
                SecretKind::Generic
            },
            scope: if r.workspace_id.is_some() {
                Level::Workspace
            } else {
                Level::Organization
            },
            has_path: r.path_pattern.is_some(),
        });
    }

    let identity = RequestIdentity {
        agent_id,
        speaker_user_id: speaker,
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
    // Winner precedence (06 step 6): path-limited over whole-host, workspace over org.
    let mut granted: Vec<&Cand> = cands.iter().filter(|c| is_sel(c)).collect();
    if granted.iter().any(|c| c.has_path) {
        granted.retain(|c| c.has_path);
    }
    if granted.iter().any(|c| c.scope == Level::Workspace) {
        granted.retain(|c| c.scope == Level::Workspace);
    }
    let winner = if granted.len() == 1 {
        Some(granted[0])
    } else {
        None
    };
    let candidates: Vec<SimCandidate> = cands
        .iter()
        .map(|c| SimCandidate {
            kind: CredentialKind::Secret,
            id: c.id,
            name: c.name.clone(),
            selected: is_sel(c),
        })
        .collect();
    let winning_credential = winner.map(|c| WinningCredential {
        kind: CredentialKind::Secret,
        id: c.id,
        name: c.name.clone(),
    });

    let body = req.body.unwrap_or_default().into_bytes();
    let limit = aurora_core::limits::CONDITION_BODY_BUFFER;
    let truncated = body.len() > limit;
    let facts = RequestFacts {
        host,
        port,
        path,
        query: url.query().map(str::to_owned),
        method,
        body_prefix: body.into_iter().take(limit).collect(),
        truncated,
        app_id: None,
        tool_id: None,
        tool_tags: Default::default(),
        winning_credential: winner.map(Cand::credential),
        is_llm_host,
        identity,
        connect_time: false,
        host_apps: Vec::new(),
    };
    let d = evaluate(&org_policy, &ws_policy, &facts, &MemoryCounter::new());
    let trace = SimTrace {
        organization: scope_trace(&d.trace.organization, &derived),
        workspace: scope_trace(&d.trace.workspace, &derived),
    };

    if granted.len() > 1 {
        return Ok(Simulation {
            approval_timeout_secs: None,
            verdict: SimVerdict::Block,
            reason: None,
            guidance_code: Some("multiple_providers".into()),
            winning_credential: None,
            candidates,
            trace,
        });
    }
    let (reason, guidance) = match d.verdict {
        Verdict::Allow => (None, None),
        Verdict::NeedsApproval | Verdict::RateLimited => (Some(SimReason::Rule), None),
        Verdict::Block {
            reason: BlockReason::Rule,
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
    let approval_timeout_secs = (d.verdict == Verdict::NeedsApproval)
        .then(|| effective_timeout_secs(&state.config.approvals, d.approval_timeout_secs));
    Ok(Simulation {
        approval_timeout_secs,
        verdict: sim_verdict(d.verdict),
        reason,
        guidance_code: guidance.map(str::to_owned),
        winning_credential,
        candidates,
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
        verdict: SimVerdict::Block,
        reason: Some(reason),
        guidance_code: Some(code.to_owned()),
        winning_credential: None,
        candidates: vec![],
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
            reason: BlockReason::Rule,
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
        verdict,
        reason,
        guidance_code: guidance.map(str::to_owned),
        winning_credential: None,
        candidates: vec![],
        trace,
    })
}
