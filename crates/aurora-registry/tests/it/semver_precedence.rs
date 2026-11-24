#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Real semver precedence for the version comparison (`parse_semver`): numeric core, then
//! pre-release identifiers — a release ranks above any pre-release of the same core, numeric
//! identifiers compare numerically and rank below alphanumeric ones, a longer identifier
//! list ranks above its prefix, build metadata is ignored. Plain `x.y.z` orders as before.

use aurora_registry::manifest::parse_semver;

/// The ordering example from the semver.org spec (§11), as one chain.
#[test]
fn the_spec_ordering_example_holds() {
    let order = [
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
    ];
    for (i, low) in order.iter().enumerate() {
        for high in &order[i + 1..] {
            assert!(
                parse_semver(low) < parse_semver(high),
                "{low} must rank below {high}"
            );
            assert!(
                parse_semver(high) > parse_semver(low),
                "{high} must rank above {low}"
            );
        }
    }
}

#[test]
fn numeric_identifiers_compare_numerically_and_rank_below_alphanumeric_ones() {
    assert!(parse_semver("1.0.0-2") < parse_semver("1.0.0-10"));
    assert!(parse_semver("1.0.0-2") < parse_semver("1.0.0-a"));
    // Byte order inside alphanumeric identifiers (ASCII).
    assert!(parse_semver("1.0.0-alpha") < parse_semver("1.0.0-beta"));
}

#[test]
fn a_longer_identifier_list_ranks_above_its_prefix() {
    assert!(parse_semver("1.0.0-rc.1") < parse_semver("1.0.0-rc.1.1"));
    assert!(parse_semver("1.0.0-alpha") < parse_semver("1.0.0-alpha.1"));
}

#[test]
fn build_metadata_is_ignored_and_a_release_outranks_its_prereleases() {
    assert_eq!(parse_semver("1.0.0+build.1"), parse_semver("1.0.0+build.2"));
    assert_eq!(parse_semver("1.0.0"), parse_semver("1.0.0+build.1"));
    assert!(parse_semver("1.0.0-rc.1") < parse_semver("1.0.0"));
    // The bug this fixes: a new pre-release was reduced to the same key as the old one.
    assert!(parse_semver("1.0.0-rc.1") < parse_semver("1.0.0-rc.2"));
    assert_eq!(parse_semver("1.0.0-rc.1"), parse_semver("1.0.0-rc.1"));
}

#[test]
fn plain_cores_keep_their_ordering_and_garbage_still_fails() {
    assert!(parse_semver("0.9.0") < parse_semver("1.0.0"));
    assert!(parse_semver("1.2.3") < parse_semver("1.10.0"));
    assert!(parse_semver("1.9.9") < parse_semver("2.0.0"));
    for bad in ["1.2", "1.2.3.4", "01.2.3", "1.2.x", "", "1.2.3-"] {
        assert!(parse_semver(bad).is_none(), "{bad} must not parse");
    }
}
