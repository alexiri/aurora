//! Evaluator (07 §Evaluation).
use crate::facts::RequestFacts;
use crate::host::HostPattern;
use crate::path::PathPattern;
use crate::rule::{Action, RateLimit, Rule};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultAction {
    #[default]
    None,
    Allow,
    Block,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncredentialedDefault {
    #[default]
    None,
    Block,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowEntry {
    pub host: HostPattern,
    #[serde(default)]
    pub path: Option<PathPattern>,
}

/// One scope's ordered rules and defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScopePolicy {
    /// Hand rules by ascending priority, then derived rules in their order.
    #[serde(default)]
    pub ordered_rules: Vec<Rule>,
    #[serde(default)]
    pub default_action: DefaultAction,
    #[serde(default)]
    pub uncredentialed_default: UncredentialedDefault,
    #[serde(default)]
    pub uncredentialed_allowlist: Vec<AllowEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    Rule,
    DefaultDenied,
    EgressDenied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Allow,
    NeedsApproval,
    RateLimited,
    Block { reason: BlockReason },
}

impl Verdict {
    fn strictness(self) -> u8 {
        match self {
            Self::Allow => 1,
            Self::NeedsApproval => 2,
            Self::RateLimited => 3,
            Self::Block { .. } => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Organization,
    Workspace,
}

pub trait RateCounter {
    fn is_exhausted(&self, rule_key: Uuid, agent: Uuid, window: &RateLimit) -> bool;
    fn record(&self, rule_key: Uuid, agent: Uuid, window: &RateLimit);
}

/// In-memory sliding-window counter (D-05).
#[derive(Debug, Default)]
pub struct MemoryCounter {
    hits: Mutex<HashMap<(Uuid, Uuid), Vec<Instant>>>,
}

impl MemoryCounter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RateCounter for MemoryCounter {
    fn is_exhausted(&self, rule_key: Uuid, agent: Uuid, window: &RateLimit) -> bool {
        let mut g = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        let span = Duration::from_secs(window.per.seconds());
        let now = Instant::now();
        let Some(v) = g.get_mut(&(rule_key, agent)) else {
            return window.count == 0;
        };
        v.retain(|t| now.saturating_duration_since(*t) < span);
        v.len() >= window.count as usize
    }

    fn record(&self, rule_key: Uuid, agent: Uuid, _window: &RateLimit) {
        let mut g = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        g.entry((rule_key, agent)).or_default().push(Instant::now());
    }
}

/// A counter increment the caller must apply after an allow/needs_approval verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RateLimitHit {
    pub rule_key: Uuid,
    pub agent: Uuid,
    pub window: RateLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MatchedRule {
    pub logical_id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleResult {
    Disabled,
    IdentityMismatch,
    TargetMismatch,
    ConditionsNotMet,
    /// Matched; `unknown_conditions` says doubt contributed to the match.
    Matched,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleTrace {
    pub logical_id: Uuid,
    pub name: String,
    pub result: RuleResult,
    pub unknown_conditions: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    Rule,
    Default,
    Uncredentialed,
    UncredentialedAllowlist,
    Nothing,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScopeTrace {
    pub considered: Vec<RuleTrace>,
    pub matched: Option<MatchedRule>,
    pub via: Via,
    pub verdict: Option<Verdict>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Trace {
    pub organization: ScopeTrace,
    pub workspace: ScopeTrace,
    pub decided_by: Option<ScopeKind>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub verdict: Verdict,
    /// (RFE-011a) Minimum of the set `approval_timeout_secs` of the matching requiring rules of
    /// both scopes; `None` = no preference (the caller applies `approvals.timeout_secs`). Always
    /// `None` unless the verdict is `NeedsApproval`.
    pub approval_timeout_secs: Option<u32>,
    pub org_matched: Option<MatchedRule>,
    pub workspace_matched: Option<MatchedRule>,
    pub trace: Trace,
    hits: Vec<RateLimitHit>,
}

impl Decision {
    /// Counter increments to apply; empty unless the verdict is allow/needs_approval.
    pub fn rate_limit_hits(&self) -> &[RateLimitHit] {
        &self.hits
    }

    /// The rule behind the deciding verdict, if a rule decided.
    pub fn deciding_rule(&self) -> Option<&MatchedRule> {
        match self.trace.decided_by? {
            ScopeKind::Organization if self.trace.organization.via == Via::Rule => {
                self.org_matched.as_ref()
            }
            ScopeKind::Workspace if self.trace.workspace.via == Via::Rule => {
                self.workspace_matched.as_ref()
            }
            _ => None,
        }
    }
}

struct ScopeResult {
    trace: ScopeTrace,
    hit: Option<RateLimitHit>,
    /// The matched requiring rule's `approval_timeout_secs` (only when it yielded `NeedsApproval`).
    approval_timeout: Option<u32>,
}

fn allowlisted(p: &ScopePolicy, f: &RequestFacts) -> bool {
    p.uncredentialed_allowlist.iter().any(|e| {
        e.host.matches(&f.host, f.port)
            && if f.connect_time {
                // A path-scoped entry must not admit a tunnel (RFE-003).
                e.path.is_none()
            } else {
                e.path.as_ref().is_none_or(|pp| pp.matches(&f.path))
            }
    })
}

fn eval_scope(p: &ScopePolicy, f: &RequestFacts, counters: &dyn RateCounter) -> ScopeResult {
    let mut considered = Vec::with_capacity(p.ordered_rules.len());
    let finish = |considered, matched, via, verdict, hit| ScopeResult {
        trace: ScopeTrace {
            considered,
            matched,
            via,
            verdict,
        },
        hit,
        approval_timeout: None,
    };
    for r in &p.ordered_rules {
        let mut t = RuleTrace {
            logical_id: r.logical_id,
            name: r.name.clone(),
            result: RuleResult::Disabled,
            unknown_conditions: false,
        };
        if !r.enabled {
            considered.push(t);
            continue;
        }
        if !r.identity_matches(&f.identity) {
            t.result = RuleResult::IdentityMismatch;
            considered.push(t);
            continue;
        }
        let (targets_ok, ok, unknown) = if f.connect_time {
            // Doubt only tightens: the whole verdict is `connect_matches`.
            let m = r.connect_matches(f);
            (m, m, r.is_restrictive() && !r.conditions.is_empty())
        } else {
            let tm = r.targets_match(f);
            let (ok, unknown) = if tm {
                r.conditions_match(f)
            } else {
                (false, false)
            };
            (tm, ok, unknown)
        };
        if !targets_ok {
            t.result = RuleResult::TargetMismatch;
            considered.push(t);
            continue;
        }
        t.unknown_conditions = unknown;
        if !ok {
            t.result = RuleResult::ConditionsNotMet;
            considered.push(t);
            continue;
        }
        t.result = RuleResult::Matched;
        considered.push(t);
        let matched = Some(MatchedRule {
            logical_id: r.logical_id,
            name: r.name.clone(),
        });
        let agent = f.identity.agent_id;
        let (verdict, hit) = match r.action {
            Action::Block => (
                Verdict::Block {
                    reason: BlockReason::Rule,
                },
                None,
            ),
            Action::Allow => match &r.rate_limit {
                Some(rl) if counters.is_exhausted(r.logical_id, agent, rl) => {
                    (Verdict::RateLimited, None)
                }
                rl => {
                    let v = if r.require_approval {
                        Verdict::NeedsApproval
                    } else {
                        Verdict::Allow
                    };
                    (
                        v,
                        rl.map(|window| RateLimitHit {
                            rule_key: r.logical_id,
                            agent,
                            window,
                        }),
                    )
                }
            },
        };
        let mut res = finish(considered, matched, Via::Rule, Some(verdict), hit);
        if verdict == Verdict::NeedsApproval {
            res.approval_timeout = r.approval_timeout_secs;
        }
        return res;
    }
    if !f.injected() && p.uncredentialed_default == UncredentialedDefault::Block {
        return if allowlisted(p, f) {
            finish(considered, None, Via::UncredentialedAllowlist, None, None)
        } else {
            finish(
                considered,
                None,
                Via::Uncredentialed,
                Some(Verdict::Block {
                    reason: BlockReason::EgressDenied,
                }),
                None,
            )
        };
    }
    match p.default_action {
        DefaultAction::None => finish(considered, None, Via::Nothing, None, None),
        DefaultAction::Allow => finish(considered, None, Via::Default, Some(Verdict::Allow), None),
        DefaultAction::Block if f.injected() && !f.is_llm_host => finish(
            considered,
            None,
            Via::Default,
            Some(Verdict::Block {
                reason: BlockReason::DefaultDenied,
            }),
            None,
        ),
        DefaultAction::Block => finish(considered, None, Via::Nothing, None, None),
    }
}

/// (RFE-003) Whether an explicit allow rule of this scope is fully knowable at `CONNECT`
/// time and matches (identity, host, port). Used by the open-lane `allowlist_only` check;
/// the full evaluation still applies afterwards.
pub fn connect_allow_admits(p: &ScopePolicy, f: &RequestFacts) -> bool {
    p.ordered_rules.iter().any(|r| {
        r.enabled
            && r.action == Action::Allow
            && !r.is_restrictive()
            && r.identity_matches(&f.identity)
            && r.connect_matches(f)
    })
}

/// Evaluate both scopes and combine by strictness (ties: organization first).
pub fn evaluate(
    org: &ScopePolicy,
    ws: &ScopePolicy,
    facts: &RequestFacts,
    counters: &dyn RateCounter,
) -> Decision {
    let o = eval_scope(org, facts, counters);
    let w = eval_scope(ws, facts, counters);
    let strictness = |v: Option<Verdict>| v.map_or(0, Verdict::strictness);
    let (verdict, decided_by) = match (o.trace.verdict, w.trace.verdict) {
        (None, None) => (Verdict::Allow, None),
        (ov, wv) if strictness(ov) >= strictness(wv) => {
            (ov.unwrap_or(Verdict::Allow), Some(ScopeKind::Organization))
        }
        (_, wv) => (wv.unwrap_or(Verdict::Allow), Some(ScopeKind::Workspace)),
    };
    let hits = if matches!(verdict, Verdict::Allow | Verdict::NeedsApproval) {
        o.hit.into_iter().chain(w.hit).collect()
    } else {
        Vec::new()
    };
    // (RFE-011a) The effective hold timeout is the minimum of the timeouts *set* on the matching
    // requiring rules of both scopes; a rule without one expresses no preference.
    let approval_timeout_secs = if verdict == Verdict::NeedsApproval {
        [o.approval_timeout, w.approval_timeout]
            .into_iter()
            .flatten()
            .min()
    } else {
        None
    };
    Decision {
        verdict,
        approval_timeout_secs,
        org_matched: o.trace.matched.clone(),
        workspace_matched: w.trace.matched.clone(),
        trace: Trace {
            organization: o.trace,
            workspace: w.trace,
            decided_by,
        },
        hits,
    }
}
