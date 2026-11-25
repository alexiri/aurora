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

// (lane review 2026-11-25) A `project` of plain `$`-paths is a valid tool field.
#[test]
fn a_tool_project_of_plain_paths_is_valid() {
    let v = run(&METHODS.replace(
        "connect_methods: [token] }",
        "connect_methods: [token], project: [\"$.name\", \"$.type\"] }",
    ));
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
        // (FIX-233, C-1159) `{client.<field>}` needs a declared non-secret field with a default;
        // a secret field has no default.
        ("client-field-url", "oauth_url_client_field_invalid"),
        ("client-field-default", "client_field_default_invalid"),
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

/// METHODS with extra form fields appended to the `token` (fields) method.
fn with_token_fields(extra: &str) -> String {
    METHODS.replace(TOKEN_FIELD, &format!("{TOKEN_FIELD}{extra}"))
}

const LABEL_FIELD: &str = "          - { name: name, label: Name, store: label }\n";

/// (FIX-194, C-1091) A `store: label` field is valid in a `fields` method, and adding or removing
/// one widens nothing.
#[test]
fn label_field_is_valid_and_never_widens() {
    let y = with_token_fields(LABEL_FIELD);
    let v = run(&y);
    assert!(v.is_empty(), "{v:#?}");
    let new = Manifest::parse(&y, RUNNING).unwrap();
    let old = Manifest::parse(METHODS, RUNNING).unwrap();
    assert!(aurora_registry::diff::widens(&old, &new).is_empty());
    assert!(aurora_registry::diff::widens(&new, &old).is_empty());
}

#[test]
fn label_field_outside_a_fields_method_is_refused() {
    // The hook method (`app`) comes after the token method's fields.
    let y = METHODS.replace(
        "          - { name: private_key, label: Key, type: file, required: true }\n",
        "          - { name: private_key, label: Key, type: file, required: true }\n          - { name: name, label: Name, store: label }\n",
    );
    assert_eq!(codes(&y), BTreeSet::from(["label_field_method"]));
}

#[test]
fn two_label_fields_in_one_method_are_refused() {
    let y = with_token_fields(&format!(
        "{LABEL_FIELD}          - {{ name: other, label: Other, store: label }}\n"
    ));
    assert_eq!(codes(&y), BTreeSet::from(["label_field_duplicate"]));
}

#[test]
fn label_field_must_be_text() {
    let y =
        with_token_fields("          - { name: name, label: Name, store: label, type: secret }\n");
    assert_eq!(codes(&y), BTreeSet::from(["label_field_type"]));
}

/// (FIX-136) A `fields` method declares the scopes a pasted token may carry, with read probes;
/// tools name them.
#[test]
fn tool_scopes_and_declared_token_scopes_are_valid() {
    let y = METHODS
        .replace(
            "          - { name: access_token, label: Token, type: secret, required: true }\n",
            "          - { name: access_token, label: Token, type: secret, required: true }\n        scopes:\n          - { scope: read, name: Read, access: read }\n          - { scope: write, name: Write, access: write }\n        probe_scopes:\n          read: { url: \"https://api.example.com/user\" }\n",
        )
        .replace("connect_methods: [token] }", "connect_methods: [token], scopes: [read, write] }");
    let v = run(&y);
    assert!(v.is_empty(), "{v:#?}");
    let m = Manifest::parse(&y, RUNNING).unwrap();
    let Some(aurora_registry::manifest::Connect::Fields(f)) = m.apps[0].connect.first() else {
        panic!("first method is the fields method")
    };
    assert_eq!(f.scopes.len(), 2);
    assert_eq!(f.probe_scopes.len(), 1);
    let t = &m.apps[0]
        .tools
        .as_ref()
        .unwrap()
        .read
        .as_ref()
        .unwrap()
        .items[0];
    assert_eq!(t.scopes, ["read", "write"]);
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

/// (P5-TOOLASK, C-1042) `approval: ask` is valid on a write tool, only `ask` is accepted, and a
/// tool without it serializes exactly as before (the package hashes of other packages hold).
#[test]
fn a_write_tool_may_default_to_ask_and_the_field_is_additive() {
    let write = METHODS.replace(
        "    tools:\n      read:",
        "    tools:\n      write:\n        items:\n          - { id: send, name: Send, host: api.example.com, path: /send, methods: [POST], approval: ask }\n      read:",
    );
    let v = run(&write);
    assert!(v.is_empty(), "{v:#?}");
    let m = Manifest::parse(&write, RUNNING).unwrap();
    let send = &m.apps[0]
        .tools
        .as_ref()
        .unwrap()
        .write
        .as_ref()
        .unwrap()
        .items[0];
    assert_eq!(
        send.approval,
        Some(aurora_registry::manifest::ToolDefaultApproval::Ask)
    );
    assert!(
        serde_json::to_string(send)
            .unwrap()
            .contains(r#""approval":"ask""#)
    );
    let plain = Manifest::parse(METHODS, RUNNING).unwrap();
    let read = &plain.apps[0]
        .tools
        .as_ref()
        .unwrap()
        .read
        .as_ref()
        .unwrap()
        .items[0];
    assert!(read.approval.is_none());
    assert!(!serde_json::to_string(read).unwrap().contains("approval"));
    // Only `ask` exists: `allow` is not a package default.
    assert!(Manifest::parse(&write.replace("approval: ask", "approval: allow"), RUNNING).is_err());
}

/// (P5-TOOLASK, D-397) The shipped packages that act on other people ask first by default, and the
/// policy tag follows the catalogue: `send_scheduling_message` (CalDAV) and the webhook `send`.
#[test]
fn caldav_scheduling_messages_and_webhook_sends_default_to_ask() {
    use aurora_registry::manifest::ToolDefaultApproval::Ask;
    for (pkg, ask_tool) in [("caldav", "send_scheduling_message"), ("webhook", "send")] {
        let y = std::fs::read_to_string(format!(
            "{}/../../plugins/builtin/{pkg}/plugin.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let m = Manifest::parse(&y, RUNNING).unwrap();
        assert!(validate::manifest(&m).is_empty(), "{pkg}");
        let tools = m.apps[0].tools.as_ref().unwrap();
        let asked: Vec<&str> = tools
            .read
            .iter()
            .chain(tools.write.iter())
            .flat_map(|g| g.items.iter())
            .filter(|i| i.approval == Some(Ask))
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(asked, [ask_tool], "{pkg}: exactly one tool asks first");
        assert!(
            tools
                .write
                .as_ref()
                .unwrap()
                .items
                .iter()
                .any(|i| i.id == ask_tool),
            "{pkg}: it is a write tool"
        );
    }
}

/// (FIX-184, D-400) `google-workspace` and `microsoft-365` lint clean and ask first for exactly the
/// write tools that send mail or invitations to other people, and for no read tool.
#[test]
fn google_workspace_and_microsoft_365_ask_first_for_mail_and_invitations_only() {
    use aurora_registry::manifest::ToolDefaultApproval::Ask;
    let cases: [(&str, &[&str]); 2] = [
        (
            "google-workspace",
            &["create_event", "send_draft", "send_message", "update_event"],
        ),
        (
            "microsoft-365",
            &["create_calendar_event", "create_event", "send_mail"],
        ),
    ];
    for (pkg, expected) in cases {
        let y = std::fs::read_to_string(format!(
            "{}/../../plugins/builtin/{pkg}/plugin.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let m = Manifest::parse(&y, RUNNING).unwrap();
        assert!(validate::manifest(&m).is_empty(), "{pkg}");
        let mut asked: Vec<&str> = Vec::new();
        for app in &m.apps {
            let tools = app.tools.as_ref().unwrap();
            for item in tools.write.iter().flat_map(|g| g.items.iter()) {
                if item.approval == Some(Ask) {
                    asked.push(item.id.as_str());
                }
            }
            for item in tools.read.iter().flat_map(|g| g.items.iter()) {
                assert!(item.approval.is_none(), "{pkg}: read tool {}", item.id);
            }
        }
        asked.sort_unstable();
        assert_eq!(asked, expected, "{pkg}: the tools that ask first");
    }
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
            "tool needs a scope no method declares",
            METHODS.replace("connect_methods: [token] }", "connect_methods: [token], scopes: [nope] }"),
            "tool_scope_unknown",
        ),
        (
            "scope declared twice",
            METHODS.replace(
                "          - { name: access_token, label: Token, type: secret, required: true }\n",
                "          - { name: access_token, label: Token, type: secret, required: true }\n        scopes:\n          - { scope: a, name: A, access: read }\n          - { scope: a, name: A2, access: read }\n",
            ),
            "duplicate_scope",
        ),
        (
            "probe for a scope the method does not declare",
            METHODS.replace(
                "          - { name: access_token, label: Token, type: secret, required: true }\n",
                "          - { name: access_token, label: Token, type: secret, required: true }\n        probe_scopes:\n          nope: { url: \"https://api.example.com/x\" }\n",
            ),
            "probe_scope_unknown",
        ),
        (
            "probe for a write scope",
            METHODS.replace(
                "          - { name: access_token, label: Token, type: secret, required: true }\n",
                "          - { name: access_token, label: Token, type: secret, required: true }\n        scopes:\n          - { scope: w, name: W, access: write }\n        probe_scopes:\n          w: { url: \"https://api.example.com/x\" }\n",
            ),
            "probe_scope_write",
        ),
        (
            "scope probe that is not a GET",
            METHODS.replace(
                "          - { name: access_token, label: Token, type: secret, required: true }\n",
                "          - { name: access_token, label: Token, type: secret, required: true }\n        scopes:\n          - { scope: r, name: R, access: read }\n        probe_scopes:\n          r: { method: POST, url: \"https://api.example.com/x\" }\n",
            ),
            "probe_scope_method",
        ),
        (
            "form field uses the reserved aurora_ key space",
            METHODS.replace(
                "{ name: private_key, label: Key, type: file, required: true }",
                "{ name: private_key, label: Key, type: file, required: true }\n          - { name: aurora_access, label: Access, type: text, store: metadata }",
            ),
            "field_name_reserved",
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
        // (P5-TOOLASK, C-1042) A read tool cannot default to Ask.
        (
            "a read tool with approval: ask",
            METHODS.replace("connect_methods: [token] }", "connect_methods: [token], approval: ask }"),
            "tool_approval_read",
        ),
        // A `body_check` key name is what the refusal message quotes: an identifier only.
        (
            "a body_check key that is not an identifier",
            METHODS.replace(
                "connect_methods: [token] }",
                "connect_methods: [token], body_check: { refuse_keys: [bad-key] } }",
            ),
            "body_check_key",
        ),
        (
            "a body_check with more than eight refuse keys",
            METHODS.replace(
                "connect_methods: [token] }",
                &format!(
                    "connect_methods: [token], body_check: {{ refuse_keys: [{}] }} }}",
                    (0..9).map(|i| format!("k{i}")).collect::<Vec<_>>().join(", ")
                ),
            ),
            "body_check_refuse_keys",
        ),
        (
            "a body_check exact_len of zero",
            METHODS.replace(
                "connect_methods: [token] }",
                "connect_methods: [token], body_check: { exact_len: { data: 0 } } }",
            ),
            "body_check_exact_len",
        ),
        // (lane review 2026-11-25) A `project` selects named fields of every answer item:
        // plain `$`-paths only, at most 8.
        (
            "a project with a wildcard path",
            METHODS.replace(
                "connect_methods: [token] }",
                "connect_methods: [token], project: [\"$.a[*].b\"] }",
            ),
            "tool_project",
        ),
        (
            "a project with an empty path list",
            METHODS.replace(
                "connect_methods: [token] }",
                "connect_methods: [token], project: [] }",
            ),
            "tool_project",
        ),
        (
            "a project with more than eight paths",
            METHODS.replace(
                "connect_methods: [token] }",
                &format!(
                    "connect_methods: [token], project: [{}] }}",
                    (0..9).map(|i| format!("\"$.k{i}\"")).collect::<Vec<_>>().join(", ")
                ),
            ),
            "tool_project",
        ),
        // (P5-POLISH-3, C-1031) The package text for people has limits.
        (
            "summary over 100 characters",
            METHODS.replace(
                "about: { id: svc, name: Service }",
                &format!("about: {{ id: svc, name: Service, summary: \"{}\" }}", "x".repeat(101)),
            ),
            "about_text_invalid",
        ),
        (
            "blank first_prompt",
            METHODS.replace(
                "about: { id: svc, name: Service }",
                "about: { id: svc, name: Service, first_prompt: \"  \" }",
            ),
            "about_text_invalid",
        ),
        (
            "more than twelve permission hints",
            METHODS.replace(
                "        recommended: true\n",
                &format!(
                    "        recommended: true\n        access_levels:\n          - {{ id: read, label: Read only, permission_hints: [{}] }}\n",
                    (0..13).map(|i| format!("\"H{i}\"")).collect::<Vec<_>>().join(", ")
                ),
            ),
            "permission_hints_invalid",
        ),
        (
            "a permission hint over 60 characters",
            METHODS.replace(
                "        recommended: true\n",
                &format!(
                    "        recommended: true\n        access_levels:\n          - {{ id: read, label: Read only, permission_hints: [\"{}\"] }}\n",
                    "x".repeat(61)
                ),
            ),
            "permission_hints_invalid",
        ),
        (
            "a token method's access level that is neither read nor write",
            METHODS.replace(
                "        recommended: true\n",
                "        recommended: true\n        access_levels:\n          - { id: admin, label: Admin, permission_hints: [\"A\"] }\n",
            ),
            "access_level_unknown",
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

// ---- P5-UX-2 (C-956): `audience` and `scope_presets` ----

const TOKEN_FIELD: &str =
    "          - { name: access_token, label: Token, type: secret, required: true }\n";

fn with_token_scopes(extra: &str) -> String {
    METHODS.replace(
        TOKEN_FIELD,
        &format!(
            "{TOKEN_FIELD}        scopes:\n          - {{ scope: r, name: R, access: read }}\n          - {{ scope: w, name: W, access: write }}\n{extra}"
        ),
    )
}

#[test]
fn scope_presets_and_audience_parse_default_and_validate() {
    use aurora_registry::manifest::{Audience, Connect};
    // Old packages: no key, `person`, and the derived presets (read = read scopes, write = all).
    let m = Manifest::parse(&with_token_scopes(""), RUNNING).unwrap();
    let c = &m.apps[0].connect[0];
    assert_eq!(c.audience(), Audience::Person);
    let p = c.effective_scope_presets();
    assert_eq!(p.read, ["r"]);
    assert_eq!(p.write, ["r", "w"]);
    // Declared.
    let y = with_token_scopes("        scope_presets: { read: [r], write: [w] }\n").replace(
        "        recommended: true\n",
        "        recommended: true\n        audience: admin\n",
    );
    let v = run(&y);
    assert!(v.is_empty(), "{v:#?}");
    let m = Manifest::parse(&y, RUNNING).unwrap();
    let Connect::Fields(f) = &m.apps[0].connect[0] else {
        panic!("fields method")
    };
    assert_eq!(f.audience, Audience::Admin);
    let p = m.apps[0].connect[0].effective_scope_presets();
    assert_eq!((p.read, p.write), (vec!["r".into()], vec!["w".into()]));
    // An unknown audience is refused.
    assert!(!run(&y.replace("audience: admin", "audience: robot")).is_empty());
}

#[test]
fn scope_presets_must_name_method_scopes_and_read_only_read_ones() {
    for (what, extra, code) in [
        (
            "a write scope in read",
            "        scope_presets: { read: [w], write: [w] }\n",
            "scope_preset_write",
        ),
        (
            "not a method scope",
            "        scope_presets: { read: [r], write: [nope] }\n",
            "scope_preset_unknown",
        ),
    ] {
        let c = codes(&with_token_scopes(extra));
        assert!(c.contains(code), "{what}: {c:?}");
    }
    // Presets on a method without scopes name nothing.
    let c = codes(&METHODS.replace(
        TOKEN_FIELD,
        &format!("{TOKEN_FIELD}        scope_presets: {{ read: [r] }}\n"),
    ));
    assert!(c.contains("scope_preset_unknown"), "{c:?}");
}

/// Old packages stay valid: every builtin package and every valid fixture parses and lints clean
/// with the new keys in the model, and the two builtin values the journey needs are set.
#[test]
fn every_builtin_and_fixture_validates_and_the_journey_keys_are_set() {
    use aurora_registry::manifest::{Audience, Connect};
    let root = format!("{}/../../plugins/builtin", env!("CARGO_MANIFEST_DIR"));
    let mut files: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| format!("{}/plugin.yaml", e.unwrap().path().display()))
        .filter(|p| std::path::Path::new(p).exists())
        .collect();
    for name in ["valid-full", "minimal"] {
        files.push(format!(
            "{}/tests/fixtures/manifests/{name}.yaml",
            env!("CARGO_MANIFEST_DIR")
        ));
    }
    assert!(files.len() > 10, "{files:?}");
    for f in &files {
        let y = std::fs::read_to_string(f).unwrap();
        let m = Manifest::parse(&y, "9.9.9").unwrap_or_else(|e| panic!("{f}: {e:#?}"));
        let v = validate::manifest(&m);
        assert!(v.is_empty(), "{f}: {v:#?}");
    }
    let github = Manifest::parse(
        &std::fs::read_to_string(format!("{root}/github/plugin.yaml")).unwrap(),
        "9.9.9",
    )
    .unwrap();
    let methods: Vec<(&str, Audience)> = github
        .apps
        .iter()
        .flat_map(|a| a.connect.iter().map(|c| (c.id(), c.audience())))
        .collect();
    assert!(
        methods.contains(&("github_app", Audience::Admin)),
        "{methods:?}"
    );
    assert!(
        methods
            .iter()
            .all(|(id, a)| (*id == "github_app") == (*a == Audience::Admin)),
        "only the GitHub App methods are admin: {methods:?}"
    );
    let linear = Manifest::parse(
        &std::fs::read_to_string(format!("{root}/linear/plugin.yaml")).unwrap(),
        "9.9.9",
    )
    .unwrap();
    let Connect::Oauth2(o) = &linear.apps[0].connect[0] else {
        panic!("linear's first method is oauth2")
    };
    assert_eq!(o.scope_presets.as_ref().unwrap().write, ["read", "write"]);
}

// ---- P5-UX-13 (C-939, C-942): `show_context` and `tool_description_missing` ----

#[test]
fn show_context_accepts_only_the_org_external_id_and_needs_the_permission() {
    let with = |ctx: &str, v: &str| {
        METHODS
            .replace(
                "permissions:\n",
                &format!("permissions:\n  context: [{ctx}]\n"),
            )
            .replace(
                "        token: { kind: hook, max_lifetime_secs: 3600 }\n",
                &format!(
                    "        token: {{ kind: hook, max_lifetime_secs: 3600 }}\n        show_context: [{v}]\n"
                ),
            )
    };
    let ok = with("org_external_id", "org_external_id");
    assert!(run(&ok).is_empty(), "{:#?}", run(&ok));
    let m = Manifest::parse(&ok, RUNNING).unwrap();
    assert_eq!(m.apps[0].connect[1].show_context(), ["org_external_id"]);
    assert!(m.apps[0].connect[0].show_context().is_empty());
    // Another value is refused; so is the right one without `permissions.context`.
    assert_eq!(
        codes(&with("org_external_id, org_id", "org_id")),
        BTreeSet::from(["show_context_unknown"])
    );
    assert_eq!(
        codes(&with("org_id", "org_external_id")),
        BTreeSet::from(["show_context_not_permitted"])
    );
}

#[test]
fn a_tool_without_a_description_is_a_warning_not_a_rejection() {
    let m = Manifest::parse(METHODS, RUNNING).unwrap();
    assert!(validate::manifest(&m).is_empty());
    let w = validate::warnings(&m);
    assert_eq!(
        w.iter()
            .map(|x| (x.code, x.path.as_str()))
            .collect::<Vec<_>>(),
        [(
            "tool_description_missing",
            "/apps/0/tools/read/items/0/description"
        )]
    );
    let y = METHODS.replace(
        "name: List user,",
        "name: List user, description: Lists the signed-in user.,",
    );
    let m = Manifest::parse(&y, RUNNING).unwrap();
    assert!(validate::warnings(&m).is_empty());
}
