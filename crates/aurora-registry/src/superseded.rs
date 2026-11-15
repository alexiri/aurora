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
    /// The connect method of `new_app` the old app became (FIX-123); `None` when the old app
    /// became an instance of the new one instead (FIX-134), whose connect methods are unchanged.
    pub method: Option<&'static str>,
    /// (FIX-134, D-243) The preset instance host of `new_app` the old app's connections move to:
    /// a region app that became a preset instance. `None` for the others.
    pub preset_host: Option<&'static str>,
}

/// The table of superseded apps. Append-only: an entry stays after the retired package is gone
/// from every database that matters, so a late upgrade still finds it.
pub const SUPERSEDED: &[Superseded] = &[
    Superseded {
        old_package: "github-app",
        old_app: "github-app",
        new_package: "github",
        new_app: "github",
        method: Some("github_app"),
        preset_host: None,
    },
    Superseded {
        old_package: "github-app",
        old_app: "github-app-ghes",
        new_package: "github",
        new_app: "github-ghes",
        method: Some("github_app"),
        preset_host: None,
    },
    Superseded {
        old_package: "aws-role",
        old_app: "aws-role",
        new_package: "aws-keys",
        new_app: "aws-keys",
        method: Some("assume_role"),
        preset_host: None,
    },
    // (FIX-134, D-243) One Sentry app: the region apps became preset instances and the
    // self-hosted app became the registered instances of the same app. The old apps live in the
    // same package, so no package is retired (`retired_by` skips the package itself).
    Superseded {
        old_package: "sentry",
        old_app: "sentry-us",
        new_package: "sentry",
        new_app: "sentry",
        method: None,
        preset_host: Some("us.sentry.io"),
    },
    Superseded {
        old_package: "sentry",
        old_app: "sentry-de",
        new_package: "sentry",
        new_app: "sentry",
        method: None,
        preset_host: Some("de.sentry.io"),
    },
    Superseded {
        old_package: "sentry",
        old_app: "sentry-self-hosted",
        new_package: "sentry",
        new_app: "sentry",
        method: None,
        preset_host: None,
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
                .is_some_and(|a| match (s.method, s.preset_host) {
                    (Some(m), _) => a.connect.iter().any(|c| c.id() == m),
                    (None, Some(h)) => a
                        .instances
                        .as_ref()
                        .is_some_and(|i| i.presets.iter().any(|p| p.host == h)),
                    // An old app that became the registered instances of the new one.
                    (None, None) => a.instances.is_some(),
                })
        })
        .collect()
}

/// Package ids retired by (a version of) `new_package`, whatever its manifest. A package never
/// retires itself: an entry whose old app lives in the same package only removes the app.
pub fn retired_by(new_package: &str) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = SUPERSEDED
        .iter()
        .filter(|s| s.new_package == new_package && s.old_package != new_package)
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
