//! Validation and normalization of the org governance document (08 §11.2).
use super::byo::{ByoPosture, OrgByo};
use super::instance::{Instance, InstanceEndpoint};
use super::model::{
    AllowEntry, BELOW_MIN, EgressPosture, GovernanceError, INVALID, LlmPosture, OpenLanePosture,
    OrgAgentTokens, OrgEgress, OrgEndpoint, OrgGovernance, OrgLlm, OrgOrigin,
};
use crate::OriginMethod;
use aurora_core::cidr::Cidr;
use aurora_core::limits::{BYO_CIDR_MAX, EGRESS_ALLOWLIST_MAX};
use aurora_policy::{HostPattern, PathPattern};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDoc {
    llm: Option<RawLlm>,
    egress: Option<RawEgress>,
    byo: Option<RawByo>,
    agent_tokens: Option<RawAgentTokens>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAgentTokens {
    /// Kept as a JSON value so a non-integer is a field error, not a parse failure.
    max_age_days: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawByo {
    posture: Option<String>,
    source_cidrs: Option<Vec<String>>,
    require_context_credentials: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLlm {
    posture: Option<String>,
    endpoints: Option<BTreeMap<String, RawEndpoint>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEndpoint {
    allowed: Option<bool>,
    key_scopes: Option<Vec<String>>,
    allowed_models: Option<Vec<String>>,
    origin: Option<OrgOrigin>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEgress {
    posture: Option<String>,
    allowlist: Option<Vec<AllowEntry>>,
    open_lane: Option<String>,
}

fn dedup(v: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(v.len());
    for s in v {
        if !out.contains(s) {
            out.push(s.clone());
        }
    }
    out
}

/// Every instance endpoint allowed, instance scopes, no narrowing, minimum postures,
/// empty allow-list.
pub fn default_for_new_org(inst: &Instance) -> OrgGovernance {
    OrgGovernance {
        llm: OrgLlm {
            posture: inst.llm_minimum_posture,
            endpoints: inst
                .endpoints
                .iter()
                .map(|e| (e.id.clone(), OrgEndpoint::default()))
                .collect(),
        },
        egress: OrgEgress {
            posture: inst.egress_minimum_posture,
            allowlist: vec![],
            open_lane: inst.open_lane_minimum,
        },
        byo: OrgByo::default(),
        agent_tokens: OrgAgentTokens::default(),
    }
}

/// `agent_tokens.max_age_days`: an integer in `AGENT_TOKEN_MAX_AGE_MIN..=_MAX`, never above the
/// instance value when that is set.
fn agent_tokens(
    a: RawAgentTokens,
    inst: &Instance,
    errs: &mut Vec<GovernanceError>,
) -> OrgAgentTokens {
    const PATH: &str = "agent_tokens.max_age_days";
    let (min, max) = (
        aurora_core::limits::AGENT_TOKEN_MAX_AGE_MIN,
        aurora_core::limits::AGENT_TOKEN_MAX_AGE_MAX,
    );
    let Some(v) = a.max_age_days.filter(|v| !v.is_null()) else {
        return OrgAgentTokens::default();
    };
    match v.as_i64() {
        Some(d) if (min..=max).contains(&d) => {
            let d = u32::try_from(d).unwrap_or(u32::MAX);
            if inst.agent_token_max_age_days.is_some_and(|i| d > i) {
                errs.push(GovernanceError::new(
                    BELOW_MIN,
                    PATH,
                    "longer than the instance maximum age",
                ));
                return OrgAgentTokens::default();
            }
            OrgAgentTokens {
                max_age_days: Some(d),
            }
        }
        _ => {
            errs.push(GovernanceError::new(
                INVALID,
                PATH,
                format!("must be an integer from {min} to {max}"),
            ));
            OrgAgentTokens::default()
        }
    }
}

/// Interpret a **stored** `organizations.governance` document (`03`: `{}` =
/// defaults derived from the instance). A missing `llm` or `egress` section takes
/// the instance defaults (`default_for_new_org`); inside a present `llm` section,
/// endpoints absent from `endpoints` stay not permitted (`08 §11.2`).
pub fn from_stored(
    raw: &serde_json::Value,
    inst: &Instance,
) -> Result<OrgGovernance, serde_json::Error> {
    let mut doc = default_for_new_org(inst);
    if let Some(llm) = raw.get("llm") {
        doc.llm = serde_json::from_value(llm.clone())?;
    }
    if let Some(egress) = raw.get("egress") {
        doc.egress = serde_json::from_value(egress.clone())?;
    }
    // A malformed `byo` never errors the whole document: it fails closed (08 §11.9).
    doc.byo = OrgByo::from_stored(raw.get("byo"));
    // An unparsable `agent_tokens` is ignored (no org limit); the instance value still applies.
    if let Some(a) = raw.get("agent_tokens") {
        doc.agent_tokens = serde_json::from_value(a.clone()).unwrap_or_default();
    }
    Ok(doc)
}

/// Validate a submitted document and return it normalized (defaults filled in, lists
/// deduplicated, hosts lowercased) or every error found. `{}` yields the defaults.
pub fn validate(
    doc: &serde_json::Value,
    inst: &Instance,
) -> Result<OrgGovernance, Vec<GovernanceError>> {
    let raw: RawDoc = serde_json::from_value(doc.clone())
        .map_err(|e| vec![GovernanceError::new(INVALID, "", e.to_string())])?;
    let mut errs = Vec::new();
    let mut out = default_for_new_org(inst);

    if let Some(llm) = raw.llm {
        if let Some(p) = llm.posture {
            match LlmPosture::parse(&p) {
                None => errs.push(GovernanceError::new(
                    INVALID,
                    "llm.posture",
                    "must be `open` or `sanctioned_only`",
                )),
                Some(v) if v < inst.llm_minimum_posture => errs.push(GovernanceError::new(
                    BELOW_MIN,
                    "llm.posture",
                    "looser than the instance minimum posture",
                )),
                Some(v) => out.llm.posture = v,
            }
        }
        if let Some(eps) = llm.endpoints {
            out.llm.endpoints.clear();
            for (id, e) in eps {
                let path = format!("llm.endpoints.{id}");
                match inst.endpoint(&id) {
                    None => errs.push(GovernanceError::new(
                        INVALID,
                        path,
                        "endpoint is not configured on this instance",
                    )),
                    Some(ie) => {
                        let n = endpoint(&path, e, inst, ie, &mut errs);
                        out.llm.endpoints.insert(id, n);
                    }
                }
            }
        }
    }

    if let Some(eg) = raw.egress {
        if let Some(p) = eg.posture {
            match EgressPosture::parse(&p) {
                None => errs.push(GovernanceError::new(
                    INVALID,
                    "egress.posture",
                    "must be `open` or `credentialed_only`",
                )),
                Some(v) if v < inst.egress_minimum_posture => errs.push(GovernanceError::new(
                    BELOW_MIN,
                    "egress.posture",
                    "looser than the instance minimum posture",
                )),
                Some(v) => out.egress.posture = v,
            }
        }
        if let Some(p) = eg.open_lane {
            match OpenLanePosture::parse(&p) {
                None => errs.push(GovernanceError::new(
                    INVALID,
                    "egress.open_lane",
                    "must be `enabled`, `allowlist_only` or `disabled`",
                )),
                Some(v) if v < inst.open_lane_minimum => errs.push(GovernanceError::new(
                    BELOW_MIN,
                    "egress.open_lane",
                    "looser than the instance minimum open-lane posture",
                )),
                Some(v) => out.egress.open_lane = v,
            }
        }
        if let Some(list) = eg.allowlist {
            if list.len() > EGRESS_ALLOWLIST_MAX as usize {
                errs.push(GovernanceError::new(
                    INVALID,
                    "egress.allowlist",
                    format!("at most {EGRESS_ALLOWLIST_MAX} entries"),
                ));
            }
            for (i, a) in list.into_iter().enumerate() {
                let p = format!("egress.allowlist[{i}]");
                let mut ok = true;
                if let Err(e) = HostPattern::parse(&a.host) {
                    errs.push(GovernanceError::new(
                        INVALID,
                        format!("{p}.host"),
                        e.to_string(),
                    ));
                    ok = false;
                }
                if let Some(pp) = &a.path
                    && let Err(e) = PathPattern::parse(pp)
                {
                    errs.push(GovernanceError::new(
                        INVALID,
                        format!("{p}.path"),
                        e.to_string(),
                    ));
                    ok = false;
                }
                if ok {
                    let n = AllowEntry {
                        host: a.host.to_ascii_lowercase(),
                        path: a.path,
                    };
                    if !out.egress.allowlist.contains(&n) {
                        out.egress.allowlist.push(n);
                    }
                }
            }
        }
    }

    if let Some(b) = raw.byo {
        out.byo = byo(b, inst, &mut errs);
    }
    if let Some(a) = raw.agent_tokens {
        out.agent_tokens = agent_tokens(a, inst, &mut errs);
    }

    if errs.is_empty() { Ok(out) } else { Err(errs) }
}

/// `byo_cidr_outside_default`: an org source entry outside a non-empty instance default list.
pub const BYO_CIDR_OUTSIDE_DEFAULT: &str = "byo_cidr_outside_default";

fn byo(b: RawByo, inst: &Instance, errs: &mut Vec<GovernanceError>) -> OrgByo {
    let mut out = OrgByo::default();
    if let Some(p) = b.posture {
        match ByoPosture::parse(&p) {
            None => errs.push(GovernanceError::new(
                INVALID,
                "byo.posture",
                "must be `enabled`, `owners_only` or `disabled`",
            )),
            Some(v) if v < inst.byo.minimum_posture => errs.push(GovernanceError::new(
                BELOW_MIN,
                "byo.posture",
                "looser than the instance minimum BYO posture",
            )),
            Some(v) => out.posture = Some(v),
        }
    }
    out.require_context_credentials = b.require_context_credentials.unwrap_or(false);
    if let Some(list) = b.source_cidrs {
        if list.len() > BYO_CIDR_MAX {
            errs.push(GovernanceError::new(
                INVALID,
                "byo.source_cidrs",
                format!("at most {BYO_CIDR_MAX} entries"),
            ));
        }
        for (i, s) in dedup(&list).into_iter().enumerate() {
            let path = format!("byo.source_cidrs[{i}]");
            match Cidr::parse(&s) {
                Err(e) => errs.push(GovernanceError::new(INVALID, path, format!("`{s}`: {e}"))),
                Ok(c) => {
                    let d = &inst.byo.default_source_cidrs;
                    if !d.is_empty() && !d.iter().any(|o| o.contains_cidr(&c)) {
                        errs.push(GovernanceError::new(
                            BYO_CIDR_OUTSIDE_DEFAULT,
                            path,
                            "outside the instance default source list",
                        ));
                    } else {
                        out.source_cidrs.push(c.to_string());
                    }
                }
            }
        }
    }
    out
}

fn endpoint(
    path: &str,
    e: RawEndpoint,
    inst: &Instance,
    ie: &InstanceEndpoint,
    errs: &mut Vec<GovernanceError>,
) -> OrgEndpoint {
    let scopes = dedup(&e.key_scopes.unwrap_or_default());
    for (i, s) in scopes.iter().enumerate() {
        if !inst.allowed_key_scopes.contains(s) {
            errs.push(GovernanceError::new(
                INVALID,
                format!("{path}.key_scopes[{i}]"),
                format!("scope `{s}` is not permitted by the instance"),
            ));
        }
    }
    let models = dedup(&e.allowed_models.unwrap_or_default());
    if !ie.allowed_models.is_empty() {
        for (i, m) in models.iter().enumerate() {
            if !ie.allowed_models.contains(m) {
                errs.push(GovernanceError::new(
                    INVALID,
                    format!("{path}.allowed_models[{i}]"),
                    format!("model `{m}` is not on the instance allow-list"),
                ));
            }
        }
    }
    let o = e.origin.unwrap_or_default();
    let origin = OrgOrigin {
        allowed_organization_ids: dedup(&o.allowed_organization_ids),
        allowed_workspace_ids: dedup(&o.allowed_workspace_ids),
        allowed_team_ids: dedup(&o.allowed_team_ids),
    };
    origin_rules(path, &origin, ie, errs);
    OrgEndpoint {
        allowed: e.allowed.unwrap_or(true),
        key_scopes: scopes,
        allowed_models: models,
        origin,
    }
}

fn subset(path: String, org: &[String], inst: &[String], errs: &mut Vec<GovernanceError>) {
    if inst.is_empty() {
        return;
    }
    for (i, v) in org.iter().enumerate() {
        if !inst.contains(v) {
            errs.push(GovernanceError::new(
                INVALID,
                format!("{path}[{i}]"),
                format!("`{v}` is not in the instance rule"),
            ));
        }
    }
}

fn origin_rules(path: &str, o: &OrgOrigin, ie: &InstanceEndpoint, errs: &mut Vec<GovernanceError>) {
    if o.is_empty() {
        return;
    }
    let p = format!("{path}.origin");
    let Some(rule) = &ie.key_origin else {
        if natural_method(ie, o).is_none() {
            errs.push(GovernanceError::new(
                INVALID,
                p,
                "the endpoint's provider cannot report the requested origin ids",
            ));
        }
        return;
    };
    if rule.method == OriginMethod::Endpoint {
        for (f, empty) in [
            (
                "allowed_organization_ids",
                o.allowed_organization_ids.is_empty(),
            ),
            ("allowed_workspace_ids", o.allowed_workspace_ids.is_empty()),
        ] {
            if !empty {
                errs.push(GovernanceError::new(
                    INVALID,
                    format!("{p}.{f}"),
                    "not supported: the instance origin method is `endpoint`",
                ));
            }
        }
        if !o.allowed_team_ids.is_empty() && !ie.origin_methods.contains(&OriginMethod::LitellmTeam)
        {
            errs.push(GovernanceError::new(
                INVALID,
                format!("{p}.allowed_team_ids"),
                "the endpoint's provider cannot verify team membership",
            ));
        }
        return;
    }
    subset(
        format!("{p}.allowed_organization_ids"),
        &o.allowed_organization_ids,
        &rule.allowed_organization_ids,
        errs,
    );
    subset(
        format!("{p}.allowed_workspace_ids"),
        &o.allowed_workspace_ids,
        &rule.allowed_workspace_ids,
        errs,
    );
    subset(
        format!("{p}.allowed_team_ids"),
        &o.allowed_team_ids,
        &rule.allowed_team_ids,
        errs,
    );
}

/// Method used when the instance has no origin rule: the first provider method able to
/// check every dimension the org lists (`provider_account`: organization/workspace ids;
/// `litellm_team`: team ids).
pub(crate) fn natural_method(ie: &InstanceEndpoint, o: &OrgOrigin) -> Option<OriginMethod> {
    let accounts = !o.allowed_organization_ids.is_empty() || !o.allowed_workspace_ids.is_empty();
    let teams = !o.allowed_team_ids.is_empty();
    ie.origin_methods.iter().copied().find(|m| match m {
        OriginMethod::ProviderAccount => !teams,
        OriginMethod::LitellmTeam => !accounts,
        OriginMethod::Endpoint => false,
    })
}
