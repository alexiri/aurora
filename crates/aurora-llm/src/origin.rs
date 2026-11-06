//! Key-origin compliance (08 §11.3).
use crate::{KeyOrigin, KeyOriginRule, OriginMethod};

fn allowed(list: &[String], value: &Option<String>) -> bool {
    list.is_empty() || value.as_ref().is_some_and(|v| list.contains(v))
}

/// Whether a verified origin satisfies the rule. Empty lists are unconstrained; a listed
/// dimension with a missing id does not comply. Method `endpoint` complies by construction.
pub fn complies(origin: &KeyOrigin, rule: &KeyOriginRule) -> bool {
    if rule.method == OriginMethod::Endpoint {
        return true;
    }
    allowed(&rule.allowed_organization_ids, &origin.organization_id)
        && allowed(&rule.allowed_workspace_ids, &origin.workspace_id)
        && allowed(&rule.allowed_team_ids, &origin.team_id)
}
