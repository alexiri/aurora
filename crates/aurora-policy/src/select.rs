//! Injection selection (07 §Injection selection).
use crate::facts::{Credential, CredentialKey, RequestIdentity};
use crate::rule::{Action, Rule};
use std::collections::HashSet;

/// A candidate is selected if at least one enabled allow rule (any scope, any
/// order) whose identities match has a target that names it. Targets of kind
/// `app`/`network` never select; tool constraints and conditions are ignored.
pub fn selected_candidates<'a>(
    rules: impl IntoIterator<Item = &'a Rule>,
    identity: &RequestIdentity,
    candidates: &[Credential],
) -> HashSet<CredentialKey> {
    let mut out = HashSet::new();
    for r in rules {
        if !r.enabled || r.action != Action::Allow || !r.identity_matches(identity) {
            continue;
        }
        for c in candidates {
            if r.targets.iter().any(|t| t.selects(c)) {
                out.insert(c.key());
            }
        }
    }
    out
}
