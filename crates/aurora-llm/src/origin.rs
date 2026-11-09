//! Key-origin compliance (08 §11.3).
use crate::{KeyOrigin, KeyOriginRule, OriginMethod};

fn allowed(list: &[String], value: &Option<String>) -> bool {
    list.is_empty() || value.as_ref().is_some_and(|v| list.contains(v))
}

/// `origin.detail` for a key whose provider reports no team while the rule requires team ids
/// (08 §11.3, FIX-019). Such a key is refused as unverifiable, not as a mismatch.
pub const KEY_REPORTS_NO_TEAM: &str = "key_reports_no_team";

/// Whether the rule needs a team id the key does not report.
pub fn requires_team(origin: &KeyOrigin, rule: &KeyOriginRule) -> bool {
    rule.method == OriginMethod::LitellmTeam
        && !rule.allowed_team_ids.is_empty()
        && origin.team_id.is_none()
}

/// Key-free, ≤ 200 char `origin.detail` for a failed verification (C-502).
pub fn detail_of(e: &crate::LlmError) -> String {
    use crate::LlmError as E;
    let d = match e {
        E::Unreachable(_) => "endpoint_unreachable".to_owned(),
        E::InvalidKey => "key_rejected".to_owned(),
        E::Status(s) => format!("key_info_http_{s}"),
        E::BadResponse(_) => "bad_response".to_owned(),
        E::Unverifiable(s) => s.clone(),
        E::UnsupportedMethod(_) | E::Config(_) => "unsupported_method".to_owned(),
    };
    d.chars().take(200).collect()
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
