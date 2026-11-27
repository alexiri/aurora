//! (phase 5, P5-CERN-ADDONS, D-251) The manifest's `ldap` section: shape rules, the one
//! reserved-host exception, the snapshot copy and the permission diff.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::collections::BTreeSet;

use aurora_registry::diff::{Widening, widens};
use aurora_registry::{Manifest, Violation, validate};

fn manifest(ldap: &str, hosts: &str, extra: &str) -> String {
    format!(
        r#"package: {{ id: dir-pkg, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: Dir }}
apps:
  - about: {{ id: dir, name: Directory }}
    connect:
      - id: open
        kind: fields
        fields: []
{extra}
    ldap:
{ldap}
    hosts:
{hosts}
    tools:
      read:
        items:
          - {{ id: search, name: Search, host: dir.ldap.aurora.internal, path: /search, methods: [POST] }}
"#
    )
}

const LDAP: &str = r#"      server: xldap.cern.ch
      base_dn: "DC=cern,DC=ch"
      attributes: [uid, mail, displayName]"#;
const HOSTS: &str = "      - match: { host: dir.ldap.aurora.internal }\n        inject: []";

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
fn a_well_formed_ldap_app_is_valid_and_reaches_the_snapshot() {
    let y = manifest(LDAP, HOSTS, "");
    let v = run(&y);
    assert!(v.is_empty(), "{v:#?}");
    let m = Manifest::parse(&y, "0.1.0").unwrap();
    let l = m.apps[0].ldap.as_ref().unwrap();
    assert_eq!((l.port, l.max_entries), (636, 50));
}

#[test]
fn the_synthetic_host_is_the_only_reserved_exception() {
    let foreign = manifest(
        LDAP,
        &format!("{HOSTS}\n      - match: {{ host: other.example.com }}\n        inject: []"),
        "",
    );
    assert_eq!(codes(&foreign), BTreeSet::from(["ldap_host_foreign"]));
    let missing = manifest(
        LDAP,
        "      - match: { host: other.example.com }\n        inject: []",
        "",
    );
    assert!(codes(&missing).contains("ldap_host_missing"));
}

// The LDAP lane applies no response projection, so a tool may not declare one.
#[test]
fn a_projection_on_an_ldap_tool_is_refused() {
    let y = manifest(LDAP, HOSTS, "").replace(
        "methods: [POST] }",
        "methods: [POST], project: [\"$.entries\"] }",
    );
    let v = run(&y);
    assert_eq!(codes(&y), BTreeSet::from(["tool_project_lane"]), "{v:#?}");
    assert_eq!(v[0].path, "/apps/0/tools/read/items/0/project", "{v:#?}");
}

#[test]
fn server_shapes_are_refused() {
    for server in [
        "10.0.0.5",
        "localhost",
        "dir.internal",
        "*.cern.ch",
        "xldap.cern.ch:636",
        "[::1]",
        "intranet",
        "a..b",
    ] {
        let l = LDAP.replace("xldap.cern.ch", &format!("\"{server}\""));
        assert!(
            codes(&manifest(&l, HOSTS, "")).contains("ldap_server_invalid"),
            "{server}"
        );
    }
}

#[test]
fn attribute_and_limit_rules() {
    for attrs in ["[]", "[\"*\"]", "[\"+\"]", "[\"mail;binary\"]", "[\"1x\"]"] {
        let l = LDAP.replace("[uid, mail, displayName]", attrs);
        assert!(
            codes(&manifest(&l, HOSTS, "")).contains("ldap_attributes_invalid"),
            "{attrs}"
        );
    }
    let l = format!("{LDAP}\n      max_entries: 501");
    assert!(codes(&manifest(&l, HOSTS, "")).contains("ldap_max_entries_invalid"));
    let l = format!("{LDAP}\n      max_entries: 0");
    assert!(codes(&manifest(&l, HOSTS, "")).contains("ldap_max_entries_invalid"));
    let l = LDAP.replace("DC=cern,DC=ch", "");
    assert!(codes(&manifest(&l, HOSTS, "")).contains("ldap_base_dn_invalid"));
}

#[test]
fn an_ldap_app_declares_no_credentials_and_injects_nothing() {
    let creds = manifest(
        LDAP,
        HOSTS,
        "    credentials:\n      - { name: pw, secret: true }",
    );
    assert!(codes(&creds).contains("ldap_credentials_declared"));
    let inj = manifest(
        LDAP,
        "      - match: { host: dir.ldap.aurora.internal }\n        inject:\n          - header: { name: x-a, value: v }",
        "",
    );
    assert!(codes(&inj).contains("ldap_host_inject"));
}

#[test]
fn a_new_or_wider_directory_block_is_a_widening_and_a_narrower_one_is_not() {
    let base = Manifest::parse(&manifest(LDAP, HOSTS, ""), "0.1.0").unwrap();
    let empty = Manifest::parse(
        "package: { id: dir-pkg, version: 0.9.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: Dir }\n",
        "0.1.0",
    )
    .unwrap();
    let first = widens(&empty, &base);
    assert!(
        first.iter().any(|w| matches!(
            w,
            Widening::Directory { server, .. } if server == "xldap.cern.ch:636"
        )),
        "{first:?}"
    );
    let only_dir = |a: &Manifest, b: &Manifest| {
        widens(a, b)
            .into_iter()
            .filter(|w| matches!(w, Widening::Directory { .. }))
            .count()
    };
    assert!(widens(&base, &base).is_empty());
    let narrower = Manifest::parse(
        &manifest(
            &LDAP.replace("[uid, mail, displayName]", "[uid]"),
            HOSTS,
            "",
        ),
        "0.1.0",
    )
    .unwrap();
    assert!(widens(&base, &narrower).is_empty());
    for wider in [
        LDAP.replace(
            "[uid, mail, displayName]",
            "[uid, mail, displayName, telephoneNumber]",
        ),
        LDAP.replace("xldap.cern.ch", "other.example.com"),
        LDAP.replace("DC=cern,DC=ch", "DC=ch"),
        format!("{LDAP}\n      port: 3636"),
        format!("{LDAP}\n      max_entries: 100"),
    ] {
        let m = Manifest::parse(&manifest(&wider, HOSTS, ""), "0.1.0").unwrap();
        assert_eq!(only_dir(&base, &m), 1, "{wider}");
    }
}
