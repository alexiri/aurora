//! Validation and normalization of the org governance document (08 §11.2).
use super::instance::{Instance, InstanceEndpoint};
use super::model::{
    AllowEntry, BELOW_MIN, EgressPosture, GovernanceError, INVALID, LlmPosture, OrgEgress,
    OrgEndpoint, OrgGovernance, OrgLlm, OrgOrigin,
};
use crate::OriginMethod;
use aurora_core::limits::EGRESS_ALLOWLIST_MAX;
use aurora_policy::{HostPattern, PathPattern};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDoc {
    llm: Option<RawLlm>,
    egress: Option<RawEgress>,
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
        },
    }
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

    if errs.is_empty() { Ok(out) } else { Err(errs) }
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
