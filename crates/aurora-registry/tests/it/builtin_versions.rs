#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-133/FIX-136 package versions: the shipped `github` and `indico` manifests changed after
//! `github` 2.0.0 / `indico` 1.1.1 (verbatim copies in `tests/fixtures/previous/`). Each shipped
//! version is higher than the previous, is a minor (not a major) upgrade, and widens nothing —
//! so an instance that has the previous version active takes the new one through the built-in
//! upgrade path automatically, without waiting for an administrator (09 §10, SEC-15).
use aurora_registry::Manifest;
use aurora_registry::diff::widens;
use aurora_registry::manifest::parse_semver;
use aurora_registry::upgrade::is_major_upgrade;

fn read(path: &str) -> Manifest {
    let y = std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    Manifest::parse(&y, "9.9.9").unwrap_or_else(|e| panic!("{path}: {e:?}"))
}

fn check(id: &str, previous: &str, shipped: &str) {
    let old = read(&format!("tests/fixtures/previous/{id}-{previous}.yaml"));
    let new = read(&format!("../../plugins/builtin/{id}/plugin.yaml"));
    assert_eq!(old.package.version, previous);
    assert_eq!(new.package.version, shipped, "{id}: shipped version");
    assert!(
        parse_semver(&new.package.version) > parse_semver(&old.package.version),
        "{id}: the changed manifest needs a higher version than {previous}"
    );
    assert!(
        !is_major_upgrade(&old.package.version, &new.package.version),
        "{id}: additive manifest changes are a minor version"
    );
    assert_eq!(old.package.publisher, new.package.publisher);
    let w = widens(&old, &new);
    assert!(
        w.is_empty(),
        "{id} {previous} -> {shipped} widens permissions, so it would be held for approval: {w:?}"
    );
}

#[test]
fn github_2_0_0_still_upgrades_to_the_shipped_version_without_widening() {
    // 2.1.0 (FIX-133/136) is superseded by 2.2.0 (FIX-135); an install still on 2.0.0 takes the
    // shipped version the same way.
    check("github", "2.0.0", "2.5.0");
}

/// FIX-135 (D-242): the packages that declare app families (presentation only) are a minor bump
/// that widens nothing, so they replace their previous version by themselves; every member of a
/// family is in the package, and a family has the same name on all members.
#[test]
fn github_2_2_0_and_atlassian_1_1_0_declare_families_without_widening() {
    check("github", "2.1.0", "2.5.0");
    check("atlassian", "1.0.3", "1.5.0");
    for (id, fams) in [
        ("github", vec![("github", vec!["github", "github-ghes"])]),
        (
            "atlassian",
            vec![
                ("jira", vec!["jira", "jira-dc"]),
                ("confluence", vec!["confluence", "confluence-dc"]),
            ],
        ),
    ] {
        let m = read(&format!("../../plugins/builtin/{id}/plugin.yaml"));
        for (fam, apps) in fams {
            let members: Vec<&str> = m
                .apps
                .iter()
                .filter(|a| a.about.family.as_ref().is_some_and(|f| f.id == fam))
                .map(|a| a.about.id.as_str())
                .collect();
            assert_eq!(members, apps, "{id}: family {fam}");
        }
        assert!(aurora_registry::validate::manifest(&m).is_empty());
    }
}

/// FIX-134 (D-243): `sentry` 1.0.2 had four apps (sentry.io, US, DE, self-hosted); 2.0.0 is one
/// app with two preset instances. Dropping apps is a major version, but it widens nothing (the
/// hosts were all claimed by 1.0.2 already), and the shipped version takes over every old app.
#[test]
fn sentry_2_0_0_replaces_1_0_2_without_widening_and_takes_over_the_old_apps() {
    let old = read("tests/fixtures/previous/sentry-1.0.2.yaml");
    let new = read("../../plugins/builtin/sentry/plugin.yaml");
    assert_eq!(old.package.version, "1.0.2");
    assert_eq!(new.package.version, "2.4.0");
    assert!(is_major_upgrade(&old.package.version, &new.package.version));
    let w = widens(&old, &new);
    assert!(w.is_empty(), "sentry 1.0.2 -> 2.0.0 widens: {w:?}");
    let old_ids: Vec<&str> = old.apps.iter().map(|a| a.about.id.as_str()).collect();
    assert_eq!(
        old_ids,
        ["sentry", "sentry-us", "sentry-de", "sentry-self-hosted"]
    );
    assert_eq!(new.apps.len(), 1);
    // Every old app but `sentry` itself is taken over by an entry the shipped version satisfies.
    let took: Vec<&str> = aurora_registry::upgrade::supersedes(&new)
        .iter()
        .map(|s| s.old_app)
        .collect();
    assert_eq!(took, ["sentry-us", "sentry-de", "sentry-self-hosted"]);
    // The old manifest satisfies none of them (no presets, no instances on `sentry`).
    assert!(aurora_registry::upgrade::supersedes(&old).is_empty());
}

#[test]
fn indico_1_2_0_replaces_1_1_1_without_widening() {
    check("indico", "1.1.1", "1.5.0");
}
