//! Shared rule quotas of the gateway (phase 4, P4-CNT, D-119, SEC-62; docs/06 §Rate limiting
//! *Shared counters*).
//!
//! With `ratelimit.counters.backend = postgres|redis` the `rate_limit` modifier of policy rules
//! counts in the shared store: **peek** (one batched read, only when the host has a rate-limited
//! rule) hands the counts to the pure evaluator; after an allow / needs-approval verdict one
//! all-or-nothing **consume** is the authority (a lost race answers `rate_limited` like the
//! evaluator would). An unreachable store applies the `gateway` class policy: `local` counts in
//! the replica's memory ([`MemoryCounter`], today's behaviour), `closed` refuses with 503
//! `counter_unavailable`. A store failure is never read as "under the limit".

use crate::resolve::ResolvedHost;
use aurora_core::ApiError;
use aurora_counters::{ClassCounters, Fallback, Period, Verdict, WindowProbe, WindowUnit};
use aurora_policy::eval::{Decision, MemoryCounter, RateCounter, Verdict as PolicyVerdict};
use aurora_policy::rule::{Per, RateLimit};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use uuid::Uuid;

/// Counts passed to the evaluator (peek); `record` is a no-op: consumption is a separate
/// atomic store call.
pub struct PeekCounter {
    counts: HashMap<(Uuid, Uuid, Per), u64>,
    /// `closed` and the peek failed: nothing is "exhausted", but a request that would consume is
    /// refused afterwards (503), so no request is allowed without a count.
    blind: bool,
}

impl RateCounter for PeekCounter {
    fn is_exhausted(&self, rule_key: Uuid, agent: Uuid, window: &RateLimit) -> bool {
        if self.blind {
            return false;
        }
        let n = self
            .counts
            .get(&(rule_key, agent, window.per))
            .copied()
            .unwrap_or(0);
        n >= u64::from(window.count)
    }
    fn record(&self, _rule_key: Uuid, _agent: Uuid, _window: &RateLimit) {}
}

/// What the peek decided for one request.
pub enum Peeked {
    /// The evaluator reads the replica's [`MemoryCounter`] and hits are recorded there (the
    /// `memory` backend, no rate-limited rule on the host, or the fallback of class policy
    /// `local`; `fallback` marks the last case for the request log).
    Local {
        fallback: bool,
    },
    Shared(PeekCounter),
}

impl Peeked {
    /// The counter the evaluator is given.
    pub fn counter<'a>(&'a self, local: &'a MemoryCounter) -> &'a dyn RateCounter {
        match self {
            Self::Local { .. } => local,
            Self::Shared(p) => p,
        }
    }

    /// Whether `apply_hits` must record in the replica's memory (everything but the shared path).
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    pub fn fallback(&self) -> bool {
        matches!(self, Self::Local { fallback: true })
    }
}

/// The result of consuming after the verdict.
pub enum Settled {
    /// Go on. `fallback`: counted locally because the store was unreachable.
    Proceed { fallback: bool },
    /// Refuse with this error; `decision` is the request-log decision.
    Refuse(ApiError, &'static str),
}

#[derive(Default)]
pub struct RuleCounters {
    class: Option<ClassCounters>,
}

fn period(p: Per) -> Period {
    match p {
        Per::Minute => Period::Minute,
        Per::Hour => Period::Hour,
        Per::Day => Period::Day,
    }
}

fn key(rule: Uuid, agent: Uuid, per: Per) -> String {
    format!("gw:rl:{rule}:{agent}:{}", period(per).as_str())
}

/// `rate_limited` for a rule whose window is full (same shape as the evaluator's refusal).
pub fn rate_limited(limit: u32, per: Per, retry_after: Duration) -> ApiError {
    let secs = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
    let secs = secs.max(1);
    let per_s = period(per).as_str();
    ApiError::rate_limited(
        secs,
        format!("Rate limit of {limit} per {per_s} reached. Retry after {secs} seconds."),
    )
    .with_details(json!({ "limit": limit, "per": per_s }))
}

fn unavailable() -> ApiError {
    ApiError::counter_unavailable("gateway_rule")
}

impl RuleCounters {
    /// The default: rule quotas per replica in memory.
    pub fn local() -> Self {
        Self::default()
    }

    pub fn shared(class: ClassCounters) -> Self {
        Self { class: Some(class) }
    }

    pub fn class(&self) -> Option<&ClassCounters> {
        self.class.as_ref()
    }

    /// Read the counts of the rate-limited rules matching this host for `agent` (one batched
    /// `fixed_window_peek`; nothing at all when the host has no such rule).
    pub async fn peek(&self, rv: &ResolvedHost, agent: Uuid) -> Peeked {
        let Some(class) = &self.class else {
            return Peeked::Local { fallback: false };
        };
        let mut seen = HashSet::new();
        let mut order: Vec<(Uuid, Per)> = Vec::new();
        for r in rv.all_rules() {
            if let Some(rl) = r.rate_limit
                && seen.insert((r.logical_id, rl.per))
            {
                order.push((r.logical_id, rl.per));
            }
        }
        if order.is_empty() {
            return Peeked::Local { fallback: false };
        }
        let probes: Vec<WindowProbe> = order
            .iter()
            .map(|(rule, per)| WindowProbe {
                key: key(*rule, agent, *per),
                period: period(*per),
            })
            .collect();
        match class.peek(&probes).await {
            Ok(counts) => Peeked::Shared(PeekCounter {
                counts: order
                    .iter()
                    .zip(counts)
                    .map(|((rule, per), n)| ((*rule, agent, *per), n))
                    .collect(),
                blind: false,
            }),
            Err(Fallback::Local) => Peeked::Local { fallback: true },
            Err(Fallback::Closed) => Peeked::Shared(PeekCounter {
                counts: HashMap::new(),
                blind: true,
            }),
        }
    }

    /// After the final verdict: consume one unit of each matched rule, all-or-nothing. Only an
    /// `allow` or `needs_approval` verdict consumes (a blocked or already rate-limited request
    /// consumes nothing). Under the shared path this is the authority.
    pub async fn settle(&self, peeked: &Peeked, d: &Decision, local: &MemoryCounter) -> Settled {
        let hits = d.rate_limit_hits();
        if hits.is_empty()
            || !matches!(
                d.verdict,
                PolicyVerdict::Allow | PolicyVerdict::NeedsApproval
            )
        {
            return Settled::Proceed {
                fallback: peeked.fallback(),
            };
        }
        let Peeked::Shared(pc) = peeked else {
            // Counted in this replica's memory (`apply_hits`).
            return Settled::Proceed {
                fallback: peeked.fallback(),
            };
        };
        let Some(class) = &self.class else {
            return Settled::Proceed { fallback: false };
        };
        if pc.blind {
            // `closed` and the peek failed: this request needs a count nobody can give.
            return Settled::Refuse(unavailable(), "error");
        }
        let units: Vec<WindowUnit> = hits
            .iter()
            .map(|h| WindowUnit {
                key: key(h.rule_key, h.agent, h.window.per),
                period: period(h.window.per),
                limit: u64::from(h.window.count),
                cost: 1,
            })
            .collect();
        match class.consume(&units).await {
            Ok(Verdict::Allowed) => Settled::Proceed { fallback: false },
            Ok(Verdict::Refused {
                key: k,
                retry_after,
            }) => {
                // Another replica spent the last unit between peek and consume.
                let h = hits
                    .iter()
                    .find(|h| key(h.rule_key, h.agent, h.window.per) == k)
                    .or_else(|| hits.first());
                let (limit, per) = h.map_or((0, Per::Minute), |h| (h.window.count, h.window.per));
                Settled::Refuse(rate_limited(limit, per, retry_after), "rate_limited")
            }
            Err(Fallback::Local) => {
                // The store failed between peek and consume: count in this replica's memory.
                if let Some(h) = hits
                    .iter()
                    .find(|h| local.is_exhausted(h.rule_key, h.agent, &h.window))
                {
                    return Settled::Refuse(
                        rate_limited(h.window.count, h.window.per, Duration::from_secs(1)),
                        "rate_limited",
                    );
                }
                for h in hits {
                    local.record(h.rule_key, h.agent, &h.window);
                }
                Settled::Proceed { fallback: true }
            }
            Err(Fallback::Closed) => Settled::Refuse(unavailable(), "error"),
        }
    }
}
