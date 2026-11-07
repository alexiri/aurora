//! Effective governance = instance ∩ org (08 §11.1) and enforcement helpers.
use super::instance::{Instance, InstanceEndpoint};
use super::model::{
    EgressPosture, LlmPosture, OpenLanePosture, OrgEndpoint, OrgGovernance, OrgOrigin,
    OriginStatus, StoredOrigin,
};
use crate::{KeyOriginRule, ModelRef, OriginMethod, origin};
use aurora_policy::{HostPattern, PathPattern};
use std::collections::BTreeMap;

/// Outcome of the open-lane posture check (RFE-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenLaneAdmission {
    Admitted,
    /// Effective posture is `disabled` → 403 `open_lane_disabled`.
    Disabled,
    /// `allowlist_only` and a level's allow-list does not cover the target → 403
    /// `blocked_by_egress_policy` (`details.open_lane = true`).
    NotAllowlisted,
}

/// Which level determined a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Instance,
    Org,
}

#[derive(Debug, Clone)]
pub struct EffectiveEndpoint {
    pub id: String,
    pub permitted: bool,
    pub permitted_source: Source,
    pub key_scopes: Vec<String>,
    pub key_scopes_source: Source,
    /// Empty = any model.
    pub allowed_models: Vec<String>,
    pub allowed_models_source: Source,
    pub origin_rule: Option<KeyOriginRule>,
    pub origin_rule_source: Source,
}

#[derive(Debug, Clone)]
pub struct EgressAllow {
    pub host: HostPattern,
    pub path: Option<PathPattern>,
    pub source: Source,
}

#[derive(Debug, Clone)]
pub struct EffectiveGovernance {
    pub llm_posture: LlmPosture,
    pub llm_posture_source: Source,
    pub egress_posture: EgressPosture,
    pub egress_posture_source: Source,
    /// (RFE-003) effective open-lane posture = strictest of instance minimum and org.
    pub open_lane: OpenLanePosture,
    pub open_lane_source: Source,
    /// The instance's own open-lane minimum (decides whether the instance level
    /// requires the baseline allow-list under `allowlist_only`).
    pub open_lane_instance: OpenLanePosture,
    /// The org's own open-lane posture.
    pub open_lane_org: OpenLanePosture,
    /// Instance baseline allow-list only (`allowlist_only` at instance level).
    pub baseline_allowlist: Vec<EgressAllow>,
    pub endpoints: BTreeMap<String, EffectiveEndpoint>,
    /// Org entries first, then the instance baseline.
    pub egress_allowlist: Vec<EgressAllow>,
    /// Endpoint hosts ∪ known LLM hosts (pattern strings).
    pub governed_hosts: Vec<String>,
    endpoint_hosts: Vec<(String, Vec<HostPattern>)>,
    known_hosts: Vec<HostPattern>,
}

fn intersect(org: &[String], inst: &[String]) -> Option<Vec<String>> {
    match (org.is_empty(), inst.is_empty()) {
        (true, _) => Some(inst.to_vec()),
        (false, true) => Some(org.to_vec()),
        (false, false) => {
            let v: Vec<String> = org.iter().filter(|o| inst.contains(o)).cloned().collect();
            if v.is_empty() { None } else { Some(v) }
        }
    }
}

/// Narrow a dimension; `None` = unsatisfiable (fail closed).
fn dim(org: &[String], inst: &[String]) -> Option<Vec<String>> {
    intersect(org, inst)
}

fn narrow_rule(rule: &KeyOriginRule, o: &OrgOrigin) -> Option<KeyOriginRule> {
    if o.is_empty() {
        return Some(rule.clone());
    }
    if rule.method == OriginMethod::Endpoint {
        if o.allowed_team_ids.is_empty() {
            return Some(rule.clone());
        }
        return Some(KeyOriginRule {
            method: OriginMethod::LitellmTeam,
            allowed_organization_ids: vec![],
            allowed_workspace_ids: vec![],
            allowed_team_ids: o.allowed_team_ids.clone(),
        });
    }
    Some(KeyOriginRule {
        method: rule.method,
        allowed_organization_ids: dim(&o.allowed_organization_ids, &rule.allowed_organization_ids)?,
        allowed_workspace_ids: dim(&o.allowed_workspace_ids, &rule.allowed_workspace_ids)?,
        allowed_team_ids: dim(&o.allowed_team_ids, &rule.allowed_team_ids)?,
    })
}

fn eff_endpoint(
    inst: &Instance,
    ie: &InstanceEndpoint,
    org: Option<&OrgEndpoint>,
) -> EffectiveEndpoint {
    let mut e = EffectiveEndpoint {
        id: ie.id.clone(),
        permitted: true,
        permitted_source: Source::Instance,
        key_scopes: inst.allowed_key_scopes.clone(),
        key_scopes_source: Source::Instance,
        allowed_models: ie.allowed_models.clone(),
        allowed_models_source: Source::Instance,
        origin_rule: ie.key_origin.clone(),
        origin_rule_source: Source::Instance,
    };
    let Some(o) = org.filter(|o| o.allowed) else {
        e.permitted = false;
        e.permitted_source = Source::Org;
        return e;
    };
    if !o.key_scopes.is_empty() {
        let v: Vec<String> = o
            .key_scopes
            .iter()
            .filter(|s| inst.allowed_key_scopes.contains(s))
            .cloned()
            .collect();
        e.key_scopes = v;
        e.key_scopes_source = Source::Org;
    }
    if !o.allowed_models.is_empty() {
        match intersect(&o.allowed_models, &ie.allowed_models) {
            Some(v) => e.allowed_models = v,
            None => e.permitted = false,
        }
        e.allowed_models_source = Source::Org;
        if !e.permitted {
            e.permitted_source = Source::Org;
        }
    }
    if ie.key_origin.is_none()
        && !o.origin.is_empty()
        && let Some(method) = super::validate::natural_method(ie, &o.origin)
    {
        e.origin_rule = Some(KeyOriginRule {
            method,
            allowed_organization_ids: o.origin.allowed_organization_ids.clone(),
            allowed_workspace_ids: o.origin.allowed_workspace_ids.clone(),
            allowed_team_ids: o.origin.allowed_team_ids.clone(),
        });
        e.origin_rule_source = Source::Org;
    }
    if let (Some(rule), false) = (&ie.key_origin, o.origin.is_empty()) {
        match narrow_rule(rule, &o.origin) {
            Some(r) => e.origin_rule = Some(r),
            None => {
                e.permitted = false;
                e.permitted_source = Source::Org;
            }
        }
        e.origin_rule_source = Source::Org;
    }
    e
}

/// Compute the effective governance of an org under the current instance ceiling.
pub fn effective(inst: &Instance, org: &OrgGovernance) -> EffectiveGovernance {
    let endpoints = inst
        .endpoints
        .iter()
        .map(|ie| {
            (
                ie.id.clone(),
                eff_endpoint(inst, ie, org.llm.endpoints.get(&ie.id)),
            )
        })
        .collect();
    let lp = org.llm.posture.max(inst.llm_minimum_posture);
    let ep = org.egress.posture.max(inst.egress_minimum_posture);
    let mut allow: Vec<EgressAllow> = Vec::new();
    for (list, source) in [
        (&org.egress.allowlist, Source::Org),
        (&inst.baseline_allowlist, Source::Instance),
    ] {
        for a in list {
            let host = HostPattern::lenient(&a.host);
            let path = a.path.as_deref().map(PathPattern::lenient);
            if !allow.iter().any(|x| x.host == host && x.path == path) {
                allow.push(EgressAllow { host, path, source });
            }
        }
    }
    let olp = org.egress.open_lane.max(inst.open_lane_minimum);
    let baseline_allowlist: Vec<EgressAllow> = inst
        .baseline_allowlist
        .iter()
        .map(|a| EgressAllow {
            host: HostPattern::lenient(&a.host),
            path: a.path.as_deref().map(PathPattern::lenient),
            source: Source::Instance,
        })
        .collect();
    let mut governed_hosts: Vec<String> = Vec::new();
    for h in inst
        .endpoints
        .iter()
        .flat_map(|e| e.hosts.iter())
        .chain(inst.known_llm_hosts.iter())
    {
        if !governed_hosts.iter().any(|g| g.eq_ignore_ascii_case(h)) {
            governed_hosts.push(h.clone());
        }
    }
    EffectiveGovernance {
        llm_posture: lp,
        llm_posture_source: if lp > org.llm.posture
            || lp == inst.llm_minimum_posture && lp > LlmPosture::Open
        {
            Source::Instance
        } else {
            Source::Org
        },
        egress_posture: ep,
        egress_posture_source: if ep > org.egress.posture
            || ep == inst.egress_minimum_posture && ep > EgressPosture::Open
        {
            Source::Instance
        } else {
            Source::Org
        },
        open_lane: olp,
        open_lane_source: if olp > org.egress.open_lane
            || olp == inst.open_lane_minimum && olp > OpenLanePosture::Enabled
        {
            Source::Instance
        } else {
            Source::Org
        },
        open_lane_instance: inst.open_lane_minimum,
        open_lane_org: org.egress.open_lane,
        baseline_allowlist,
        endpoints,
        egress_allowlist: allow,
        governed_hosts,
        endpoint_hosts: inst
            .endpoints
            .iter()
            .map(|e| {
                (
                    e.id.clone(),
                    e.hosts.iter().map(|h| HostPattern::lenient(h)).collect(),
                )
            })
            .collect(),
        known_hosts: inst
            .known_llm_hosts
            .iter()
            .map(|h| HostPattern::lenient(h))
            .collect(),
    }
}

/// Split an optional `:port` (default 443 when absent).
fn split_host(host: &str) -> (&str, u16) {
    match host.rsplit_once(':') {
        Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) && !p.is_empty() => {
            (h, p.parse().unwrap_or(443))
        }
        _ => (host, 443),
    }
}

impl EffectiveGovernance {
    /// Endpoint id owning `host` (`host` or `host:port`; no port = 443).
    pub fn endpoint_for_host(&self, host: &str) -> Option<&str> {
        let (h, port) = split_host(host);
        self.endpoint_hosts
            .iter()
            .find(|(_, pats)| pats.iter().any(|p| p.matches(h, port)))
            .map(|(id, _)| id.as_str())
    }

    /// Host of a provider Aurora has no configured endpoint for.
    pub fn is_known_llm_host(&self, host: &str) -> bool {
        let (h, port) = split_host(host);
        self.known_hosts.iter().any(|p| p.matches(h, port))
    }

    /// Whether the endpoint is permitted and `scope` is an allowed key scope for it.
    pub fn key_permitted(&self, endpoint_id: &str, scope: &str) -> bool {
        self.endpoints
            .get(endpoint_id)
            .is_some_and(|e| e.permitted && e.key_scopes.iter().any(|s| s == scope))
    }

    /// 08 §11.3 compliance of a stored origin. No rule => compliant. Otherwise the origin
    /// must be `verified`, match `current_value_version` and satisfy the rule; method
    /// `endpoint` needs no id checks. Unknown/non-permitted endpoint or unparsable JSON => false.
    pub fn origin_complies(
        &self,
        endpoint_id: &str,
        stored_origin: &serde_json::Value,
        current_value_version: i64,
    ) -> bool {
        let Some(e) = self.endpoints.get(endpoint_id) else {
            return false;
        };
        if !e.permitted {
            return false;
        }
        let Some(rule) = &e.origin_rule else {
            return true;
        };
        let Ok(o) = serde_json::from_value::<StoredOrigin>(stored_origin.clone()) else {
            return false;
        };
        o.status == OriginStatus::Verified
            && o.value_version == current_value_version
            && origin::complies(&o.to_key_origin(), rule)
    }

    /// 08 §11.5. `Err(list)` carries the allowed list (empty when the endpoint is not permitted).
    pub fn model_allowed(&self, endpoint_id: &str, model: &ModelRef) -> Result<(), Vec<String>> {
        let Some(e) = self.endpoints.get(endpoint_id).filter(|e| e.permitted) else {
            return Err(vec![]);
        };
        if e.allowed_models.is_empty() {
            return Ok(());
        }
        match model {
            ModelRef::NoModel => Ok(()),
            ModelRef::Model(m) if e.allowed_models.contains(m) => Ok(()),
            _ => Err(e.allowed_models.clone()),
        }
    }

    /// (RFE-003) Open-lane posture admission (`06 §Open-lane admission and posture`,
    /// step 2), evaluated before any policy decision. `ws_posture`, `ws_policy` and
    /// `org_policy` are the workspace's published posture and the two scope policies;
    /// `facts` are the `CONNECT`-time facts. Strictest of instance, org and workspace wins;
    /// under `allowlist_only` every level that sets it must cover the target: the instance
    /// by its baseline, the org by its list ∪ baseline, the workspace by its
    /// `uncredentialed_allowlist`. Entries carrying a `path` never admit a tunnel, and an
    /// explicit allow rule that is fully knowable at `CONNECT` time admits too.
    pub fn open_lane_admission(
        &self,
        ws_posture: OpenLanePosture,
        org_policy: &aurora_policy::ScopePolicy,
        ws_policy: &aurora_policy::ScopePolicy,
        facts: &aurora_policy::RequestFacts,
    ) -> OpenLaneAdmission {
        let effective = self.open_lane.max(ws_posture);
        if effective == OpenLanePosture::Disabled {
            return OpenLaneAdmission::Disabled;
        }
        if effective == OpenLanePosture::Enabled {
            return OpenLaneAdmission::Admitted;
        }
        let covered = |list: &[EgressAllow]| {
            list.iter()
                .any(|a| a.path.is_none() && a.host.matches(&facts.host, facts.port))
        };
        let org_rule = aurora_policy::connect_allow_admits(org_policy, facts);
        let mut ok = true;
        if self.open_lane_instance == OpenLanePosture::AllowlistOnly {
            ok &= covered(&self.baseline_allowlist) || org_rule;
        }
        if self.open_lane_org == OpenLanePosture::AllowlistOnly {
            ok &= covered(&self.egress_allowlist) || org_rule;
        }
        if ws_posture == OpenLanePosture::AllowlistOnly {
            let list = &ws_policy.uncredentialed_allowlist;
            ok &= list
                .iter()
                .any(|a| a.path.is_none() && a.host.matches(&facts.host, facts.port))
                || aurora_policy::connect_allow_admits(ws_policy, facts);
        }
        if ok {
            OpenLaneAdmission::Admitted
        } else {
            OpenLaneAdmission::NotAllowlisted
        }
    }

    /// Whether `host` (`host` or `host:port`; no port = 443) and `path` are on the egress
    /// allow-list (org ∪ baseline). Entries without a path match every path.
    pub fn egress_allowlisted(&self, host: &str, path: &str) -> bool {
        let (h, port) = split_host(host);
        self.egress_allowlist
            .iter()
            .any(|a| a.host.matches(h, port) && a.path.as_ref().is_none_or(|p| p.matches(path)))
    }
}
