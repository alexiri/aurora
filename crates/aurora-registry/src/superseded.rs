//! Superseded built-in apps (FIX-123, D-195(a), D-215, 09 §10 *Superseded apps*).
//!
//! A built-in app that became a **connect method** of another app: activating the version of
//! `new_package` that has `new_app` with a connect method `method` moves the connections (and
//! instances, libraries, availability) of `old_app` to it and retires `old_package`. This is the
//! one place the core names built-in packages: an upgrade shim for installs that still hold the
//! old apps, listed in `scripts/guards/integration-names.allow` for that reason. Nothing here
//! reads a credential.

use crate::manifest::Manifest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superseded {
    pub old_package: &'static str,
    pub old_app: &'static str,
    pub new_package: &'static str,
    pub new_app: &'static str,
    pub method: &'static str,
}

/// The table of superseded apps. Append-only: an entry stays after the retired package is gone
/// from every database that matters, so a late upgrade still finds it.
pub const SUPERSEDED: &[Superseded] = &[
    Superseded {
        old_package: "github-app",
        old_app: "github-app",
        new_package: "github",
        new_app: "github",
        method: "github_app",
    },
    Superseded {
        old_package: "github-app",
        old_app: "github-app-ghes",
        new_package: "github",
        new_app: "github-ghes",
        method: "github_app",
    },
    Superseded {
        old_package: "aws-role",
        old_app: "aws-role",
        new_package: "aws-keys",
        new_app: "aws-keys",
        method: "assume_role",
    },
];

/// The superseded entries `manifest` takes over: its package is the `new_package`, it has the
/// `new_app` and that app has a connect method `method`.
pub fn supersedes(manifest: &Manifest) -> Vec<&'static Superseded> {
    SUPERSEDED
        .iter()
        .filter(|s| manifest.package.id == s.new_package)
        .filter(|s| {
            manifest
                .apps
                .iter()
                .find(|a| a.about.id == s.new_app)
                .is_some_and(|a| a.connect.iter().any(|c| c.id() == s.method))
        })
        .collect()
}

/// Package ids retired by (a version of) `new_package`, whatever its manifest.
pub fn retired_by(new_package: &str) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = SUPERSEDED
        .iter()
        .filter(|s| s.new_package == new_package)
        .map(|s| s.old_package)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// `status_reason` of a retired package (≤ 64 characters).
pub fn superseded_reason(new_package: &str) -> String {
    format!("superseded_by_{new_package}")
}

/// Is this `status_reason` the one [`superseded_reason`] writes?
pub fn is_superseded_reason(reason: Option<&str>) -> bool {
    reason.is_some_and(|r| r.starts_with("superseded_by_"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_version_supersedes_only_with_the_target_app_and_method() {
        let y = |app: &str, method: &str| {
            format!(
                "package: {{ id: github, version: 2.0.0, contract: 1, min_core: 0.1.0, publisher: a, name: P }}\napps:\n  - about: {{ id: {app}, name: A }}\n    connect:\n      - id: {method}\n        kind: fields\n        fields: []\n"
            )
        };
        let full: Manifest = serde_yaml::from_str(&y("github", "github_app")).unwrap();
        assert_eq!(
            supersedes(&full)
                .iter()
                .map(|s| s.old_app)
                .collect::<Vec<_>>(),
            ["github-app"]
        );
        // The old 1.x package has the app but not the method: nothing moves yet.
        let old: Manifest = serde_yaml::from_str(&y("github", "oauth")).unwrap();
        assert!(supersedes(&old).is_empty());
        let other: Manifest = serde_yaml::from_str(&y("other", "github_app")).unwrap();
        assert!(supersedes(&other).is_empty());
        assert_eq!(retired_by("github"), ["github-app"]);
        assert_eq!(retired_by("aws-keys"), ["aws-role"]);
        assert!(retired_by("sentry").is_empty());
        assert!(superseded_reason("aws-keys").chars().count() <= 64);
        assert!(is_superseded_reason(Some(&superseded_reason("github"))));
        assert!(!is_superseded_reason(Some("disabled_by_admin")));
    }
}
