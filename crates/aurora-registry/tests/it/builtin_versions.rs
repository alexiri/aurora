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
fn github_2_1_0_replaces_2_0_0_without_widening() {
    check("github", "2.0.0", "2.1.0");
}

#[test]
fn indico_1_2_0_replaces_1_1_1_without_widening() {
    check("indico", "1.1.1", "1.2.0");
}
