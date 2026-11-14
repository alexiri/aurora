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

// ---- FIX-123 (D-195, D-214): connect methods with their own token, presentation, tool filter ----

/// One app with an access-token method and a hook method that mints its own tokens.
const METHODS: &str = r#"package: { id: methods-pkg, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: M }
permissions:
  hooks: [connect.exchange, token.mint]
  egress:
    connect.exchange: [api.example.com]
    token.mint: [api.example.com]
apps:
  - about: { id: svc, name: Service }
    credentials:
      - { name: access_token, secret: true }
      - { name: private_key, secret: true, optional: true }
    connect:
      - id: token
        kind: fields
        label: Personal token
        description: Acts as you.
        recommended: true
        fields:
          - { name: access_token, label: Token, type: secret, required: true }
      - id: app
        kind: hook
        label: Installed app
        description: Acts as an app installed on chosen repositories.
        token: { kind: hook, max_lifetime_secs: 3600 }
        fields:
          - { name: private_key, label: Key, type: file, required: true }
    hosts:
      - match: { host: api.example.com }
        inject:
          - bearer: "{credentials.access_token}"
    tools:
      read:
        items:
          - { id: list_user, name: List user, host: api.example.com, path: /user, methods: [GET], connect_methods: [token] }
"#;

fn codes(yaml: &str) -> BTreeSet<&'static str> {
    run(yaml).iter().map(|x| x.code).collect()
}

#[test]
fn connect_methods_with_token_presentation_and_tool_filter_are_valid() {
    let v = run(METHODS);
    assert!(v.is_empty(), "{v:#?}");
    let m = Manifest::parse(METHODS, RUNNING).unwrap();
    let app = &m.apps[0];
    // The method's own token wins; a method without one (and no method at all) uses the app's.
    assert!(app.token.is_none());
    assert_eq!(
        app.token_for(Some("app")).map(|t| t.kind),
        Some(aurora_registry::manifest::TokenKind::Hook)
    );
    assert!(app.token_for(Some("token")).is_none());
    assert!(app.token_for(None).is_none());
    let tools = app.tools.as_ref().unwrap().read.as_ref().unwrap();
    assert!(tools.items[0].applies_to(Some("token")));
    assert!(!tools.items[0].applies_to(Some("app")));
    assert!(
        tools.items[0].applies_to(None),
        "an unknown method hides nothing"
    );
}

#[test]
fn connect_method_rules_reject_each_invalid_case() {
    let cases = [
        (
            "tool names an unknown method",
            METHODS.replace("connect_methods: [token]", "connect_methods: [nope]"),
            "tool_method_unknown",
        ),
        (
            "two recommended",
            METHODS.replace(
                "        token: { kind: hook",
                "        recommended: true\n        token: { kind: hook",
            ),
            "recommended_multiple",
        ),
        (
            "blank description",
            METHODS.replace("description: Acts as you.", "description: \"  \""),
            "method_text_invalid",
        ),
        (
            "unknown oauth client in a method token",
            METHODS.replace(
                "token: { kind: hook, max_lifetime_secs: 3600 }",
                "token: { kind: oauth2_refresh, client: nope }",
            ),
            "unknown_oauth_client",
        ),
        (
            "duplicate method id",
            METHODS.replace("id: app\n", "id: token\n"),
            "duplicate_connect_method",
        ),
        (
            "a required credential a method does not produce",
            METHODS.replace(
                "- { name: access_token, label: Token, type: secret, required: true }",
                "- { name: other, label: Other, type: secret, required: true }",
            ),
            "method_credential_unproduced",
        ),
        (
            "a method token hook without the token.mint permission",
            METHODS.replace(
                "hooks: [connect.exchange, token.mint]",
                "hooks: [connect.exchange]",
            ),
            "hooks_mismatch",
        ),
    ];
    for (what, yaml, code) in cases {
        assert!(
            codes(&yaml).contains(code),
            "{what}: expected {code}, got {:#?}",
            run(&yaml)
        );
    }
}
