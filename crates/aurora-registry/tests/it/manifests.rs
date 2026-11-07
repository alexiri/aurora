#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::collections::BTreeSet;

use aurora_registry::{Manifest, Violation, validate};

const RUNNING: &str = "0.1.0";

fn fixture(name: &str) -> String {
    let p = format!(
        "{}/tests/fixtures/manifests/{name}.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn run(yaml: &str) -> Vec<Violation> {
    match Manifest::parse(yaml, RUNNING) {
        Err(v) => v,
        Ok(m) => validate::manifest(&m),
    }
}

#[test]
fn valid_full_has_no_violations() {
    let v = run(&fixture("valid-full"));
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn minimal_parses() {
    let v = run(&fixture("minimal"));
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn invalid_fixtures_yield_expected_code() {
    let table: &[(&str, &str)] = &[
        ("unknown-field", "unknown_field"),
        ("contract", "bad_contract"),
        ("min-core", "min_core_too_new"),
        ("duplicate-app-id", "duplicate_app_id"),
        ("duplicate-tool-id", "duplicate_tool_id"),
        ("host-pattern", "host_pattern_invalid"),
        ("tool-host-uncovered", "tool_host_uncovered"),
        ("probe-host", "probe_host_undeclared"),
        ("link-host", "link_host_undeclared"),
        ("link-not-https", "link_not_https"),
        ("token-url-http", "token_url_not_https"),
        ("token-url-no-zone", "token_url_template_no_zone"),
        ("authorize-url-no-zone", "authorize_url_template_no_zone"),
        ("oauth-field-credentials", "oauth_field_store"),
        ("tenant-zone", "bad_zone"),
        ("rewrite-zone", "bad_zone"),
        ("undeclared-credential", "undeclared_credential"),
        ("undeclared-metadata", "undeclared_metadata"),
        ("context-not-permitted", "context_not_permitted"),
        ("ns-not-allowed", "template_ns_not_allowed"),
        ("hooks-missing", "hooks_mismatch"),
        ("hooks-extra", "hooks_mismatch"),
        ("egress-summarize", "egress_summarize"),
        ("env-reserved", "env_name_reserved"),
        ("env-reserved-prefix", "env_name_reserved"),
        ("env-invalid", "env_name_invalid"),
        ("stub-path", "stub_path_invalid"),
        ("stub-absolute", "stub_path_invalid"),
        ("instructions-long", "agent_instructions_too_long"),
        ("summary-title", "summary_title_too_long"),
        ("summary-fields", "summary_too_many_fields"),
        ("header-forbidden", "approval_header_forbidden"),
        ("database-engine", "database_engine"),
        ("unknown-mechanism", "unknown_mechanism"),
        ("unknown-decoder", "unknown_decoder"),
        ("template-syntax", "template_syntax"),
        ("template-namespace", "template_unknown_namespace"),
        ("path-regex", "bad_path_regex"),
        ("bad-id", "bad_id"),
    ];
    for (name, code) in table {
        let v = run(&fixture(&format!("invalid-{name}")));
        let got: BTreeSet<&str> = v.iter().map(|x| x.code).collect();
        assert_eq!(got, BTreeSet::from([*code]), "{name}: {v:#?}");
    }
}

#[test]
fn bidi_controls_are_sanitized() {
    let y = fixture("minimal").replace(
        "name: Todoist }\napps",
        "name: \"Todo\u{202e}ist\u{200b}\" }\napps",
    );
    let m = Manifest::parse(&y, RUNNING).unwrap();
    assert_eq!(m.package.name, "Todoist");
}
