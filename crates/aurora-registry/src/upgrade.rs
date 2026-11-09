//! Plugin major upgrades and live connections (09 §10, RFE-022, D-65, C-340..C-343).
//!
//! Pure functions: which fields a version requires, whether an activation is a major upgrade and
//! which required fields a connection's stored credential keys lack. Only field **names** appear
//! here, never values (SEC-57).

use crate::manifest::{App, Manifest, parse_semver};
use std::collections::BTreeSet;

/// Longest `status_reason` of a flipped connection (C-342).
pub const REASON_MAX: usize = 300;

/// Names of the credential fields every connection of `app` must hold: the `credentials`
/// entries not marked `optional: true`.
pub fn required_fields(app: &App) -> BTreeSet<String> {
    app.credentials
        .iter()
        .filter(|c| !c.optional)
        .map(|c| c.name.clone())
        .collect()
}

/// `true` when activating `new` over the active `old` is a major upgrade (`N > M`).
pub fn is_major_upgrade(old: &str, new: &str) -> bool {
    match (parse_semver(old), parse_semver(new)) {
        (Some((a, ..)), Some((b, ..))) => b > a,
        _ => false,
    }
}

/// The required fields of `app` that a connection holding `keys` lacks, sorted.
///
/// `keys = None` (a legacy row) is resolved from `recorded` = the manifest of the package
/// version the connection recorded (`app_version`), taking its app's required fields as the keys
/// it holds. Without either the connection is **not satisfying**: every required field is
/// reported missing (fail closed).
pub fn missing_fields(
    app: &App,
    keys: Option<&[String]>,
    recorded: Option<&Manifest>,
) -> Vec<String> {
    let required = required_fields(app);
    let held: Option<BTreeSet<String>> = match keys {
        Some(k) => Some(k.iter().cloned().collect()),
        None => recorded
            .and_then(|m| m.apps.iter().find(|a| a.about.id == app.about.id))
            .map(required_fields),
    };
    match held {
        Some(h) => required.into_iter().filter(|r| !h.contains(r)).collect(),
        None => required.into_iter().collect(),
    }
}

/// `plugin_major_upgrade: <package> <old> -> <new>; missing: <names>`, at most [`REASON_MAX`]
/// characters (truncated with `…`). An empty missing list (a fail-closed row of an app with no
/// required field cannot occur) still reads sensibly.
pub fn reason(package_id: &str, old: &str, new: &str, missing: &[String]) -> String {
    let full = format!(
        "plugin_major_upgrade: {package_id} {old} -> {new}; missing: {}",
        missing.join(", ")
    );
    if full.chars().count() <= REASON_MAX {
        return full;
    }
    let mut cut: String = full.chars().take(REASON_MAX - 1).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn manifest(creds: &str) -> Manifest {
        let y = format!(
            "package: {{ id: p, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: a, name: P }}\napps:\n  - about: {{ id: app, name: A }}\n    credentials: [ {creds} ]\n"
        );
        serde_yaml::from_str(&y).unwrap()
    }

    #[test]
    fn major_means_the_first_component_grows() {
        assert!(is_major_upgrade("1.4.0", "2.0.0"));
        assert!(!is_major_upgrade("1.0.0", "1.1.0"));
        assert!(!is_major_upgrade("2.0.0", "1.9.0"));
        assert!(!is_major_upgrade("x", "2.0.0"));
    }

    #[test]
    fn optional_fields_are_not_required() {
        let m = manifest("{ name: a }, { name: b, optional: true }");
        assert_eq!(
            required_fields(&m.apps[0]).into_iter().collect::<Vec<_>>(),
            ["a"]
        );
    }

    #[test]
    fn missing_uses_keys_then_the_recorded_manifest_then_fails_closed() {
        let new = manifest("{ name: a2 }, { name: b }");
        let app = &new.apps[0];
        let keys = ["b".to_owned(), "a".to_owned()];
        assert_eq!(missing_fields(app, Some(&keys), None), ["a2"]);
        let old = manifest("{ name: a2 }, { name: b }");
        assert!(missing_fields(app, None, Some(&old)).is_empty());
        let old = manifest("{ name: a }, { name: b }");
        assert_eq!(missing_fields(app, None, Some(&old)), ["a2"]);
        assert_eq!(missing_fields(app, None, None), ["a2", "b"]);
    }

    #[test]
    fn reason_is_bounded() {
        let r = reason("p", "1.0.0", "2.0.0", &["x".to_owned()]);
        assert_eq!(r, "plugin_major_upgrade: p 1.0.0 -> 2.0.0; missing: x");
        let many: Vec<String> = (0..100).map(|i| format!("field_{i}")).collect();
        let r = reason("p", "1.0.0", "2.0.0", &many);
        assert_eq!(r.chars().count(), REASON_MAX);
        assert!(r.ends_with('…'));
    }
}
