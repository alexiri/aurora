//! (phase 5, P5-ICS, D-273, C-843, SEC-107) The manifest's `secret_link` section: shape rules,
//! the reserved synthetic host, and the snapshot copy.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::collections::BTreeSet;

use aurora_registry::manifest::SecretLink;
use aurora_registry::{Manifest, Violation, validate};

fn manifest(credentials: &str, section: &str, hosts: &str, tool: &str) -> String {
    format!(
        r#"package: {{ id: link-pkg, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: Link }}
apps:
  - about: {{ id: link, name: Link }}
    credentials:
{credentials}
    metadata:
      - {{ name: host }}
    connect:
      - id: link
        kind: fields
        fields:
          - {{ name: url, label: URL, type: secret, required: true, store: credentials }}
{section}
{hosts}
    tools:
      read:
        items:
          - {tool}
"#
    )
}

const CREDS: &str = "      - { name: url, secret: true }";
const SECTION: &str = "    secret_link: { credential: url, expect_prefix: \"BEGIN:VCALENDAR\" }";
const TOOL: &str =
    "{ id: read, name: Read, host: \"*.link.aurora.internal\", path: /, methods: [GET] }";

fn run(y: &str) -> Vec<Violation> {
    match Manifest::parse(y, "0.1.0") {
        Err(v) => v,
        Ok(m) => validate::manifest(&m),
    }
}

fn codes(y: &str) -> BTreeSet<&'static str> {
    run(y).iter().map(|v| v.code).collect()
}

#[test]
fn a_well_formed_secret_link_app_is_valid_and_reaches_the_snapshot() {
    let y = manifest(CREDS, SECTION, "", TOOL);
    let v = run(&y);
    assert!(v.is_empty(), "{v:#?}");
    let m = Manifest::parse(&y, "0.1.0").unwrap();
    let s = m.apps[0].secret_link.as_ref().unwrap();
    assert_eq!(s.credential, "url");
    assert_eq!(s.expect_prefix.as_deref(), Some("BEGIN:VCALENDAR"));
}

#[test]
fn the_host_is_refused_without_the_section() {
    let y = manifest(CREDS, "", "", TOOL);
    assert!(codes(&y).contains("tool_host_uncovered"), "{:?}", codes(&y));
}

#[test]
fn a_non_secret_or_missing_credential_is_refused() {
    let plain = manifest("      - { name: url }", SECTION, "", TOOL);
    assert!(codes(&plain).contains("secret_link_credential"));
    let other = manifest("      - { name: other, secret: true }", SECTION, "", TOOL);
    assert!(codes(&other).contains("secret_link_credential"));
}

#[test]
fn hosts_beside_the_section_are_refused() {
    let hosts = "    hosts:\n      - match: { host: example.com }\n        inject: []";
    assert!(codes(&manifest(CREDS, SECTION, hosts, TOOL)).contains("secret_link_hosts"));
}

#[test]
fn a_host_entry_may_never_claim_the_synthetic_host() {
    let hosts =
        "    hosts:\n      - match: { host: \"*.link.aurora.internal\" }\n        inject: []";
    assert!(codes(&manifest(CREDS, "", hosts, TOOL)).contains("secret_link_host_reserved"));
}

#[test]
fn only_get_of_the_root_path_is_a_tool() {
    for tool in [
        "{ id: read, name: Read, host: \"*.link.aurora.internal\", path: /, methods: [POST] }",
        "{ id: read, name: Read, host: \"*.link.aurora.internal\", path: /, methods: [GET, POST] }",
        "{ id: read, name: Read, host: \"*.link.aurora.internal\", path: /x }",
        "{ id: read, name: Read, host: \"*.link.aurora.internal\", path: /x, methods: [GET] }",
    ] {
        assert!(
            codes(&manifest(CREDS, SECTION, "", tool)).contains("secret_link_tool"),
            "{tool}"
        );
    }
}

// The secret-link lane applies no response projection, in either mode, so a tool may not
// declare one.
#[test]
fn a_projection_on_a_secret_link_tool_is_refused() {
    let read = TOOL.replace(
        "methods: [GET] }",
        "methods: [GET], project: [\"$.name\"] }",
    );
    let y = manifest(CREDS, SECTION, "", &read);
    assert_eq!(
        codes(&y),
        BTreeSet::from(["tool_project_lane"]),
        "{:#?}",
        run(&y)
    );
    let send = "{ id: send, name: Send, host: \"*.link.aurora.internal\", path: /, methods: [POST], project: [\"$.ok\"] }";
    let y = manifest(
        CREDS,
        "    secret_link: { credential: url, mode: send }",
        "",
        send,
    )
    .replace("      read:\n", "      write:\n");
    assert!(codes(&y).contains("tool_project_lane"), "{:#?}", run(&y));
}

#[test]
fn the_prefix_is_bounded() {
    let long = format!(
        "    secret_link: {{ credential: url, expect_prefix: \"{}\" }}",
        "x".repeat(65)
    );
    assert!(codes(&manifest(CREDS, &long, "", TOOL)).contains("secret_link_prefix"));
}

#[test]
fn the_synthetic_host_round_trips() {
    let id = "9b8c1a52-3f0e-4a57-9a1b-0d8f6b1c2e3d";
    let host = SecretLink::host_of(id);
    assert_eq!(host, format!("{id}.link.aurora.internal"));
    assert_eq!(SecretLink::id_label(&host), Some(id));
    assert_eq!(
        SecretLink::id_label(&host.to_uppercase()),
        Some(id.to_uppercase().as_str())
    );
    assert_eq!(SecretLink::id_label("example.com"), None);
    assert_eq!(SecretLink::id_label("é"), None);
}
