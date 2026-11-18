//! (phase 5, P5-CALDAV, D-275, C-845, SEC-109) The manifest's `own_server` section: shape rules,
//! the `{connection}` tool host, the WebDAV methods, and the snapshot copy.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::collections::BTreeSet;

use aurora_registry::{Manifest, Violation, validate};

const SECTION: &str = "    own_server: { url_field: url, hosts_field: hosts, inject: [ { basic: { user: \"{credentials.username}\", password: \"{credentials.password}\" } } ] }";

fn manifest(section: &str, extra: &str, tools: &str) -> String {
    format!(
        r#"package: {{ id: dav-pkg, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: Dav }}
apps:
  - about: {{ id: dav, name: Dav }}
    credentials:
      - {{ name: username }}
      - {{ name: password, secret: true }}
    metadata:
      - {{ name: url }}
      - {{ name: hosts }}
    connect:
      - id: fields
        kind: fields
        fields:
          - {{ name: username, label: User, type: text, required: true, store: credentials }}
          - {{ name: password, label: Password, type: secret, required: true, store: credentials }}
{section}
{extra}
    tools:
{tools}
"#
    )
}

fn tools(host: &str, methods: &str) -> String {
    format!(
        "      read:\n        items:\n          - {{ id: t, name: T, host: \"{host}\", path: \"/**\", methods: [{methods}] }}"
    )
}

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
fn a_well_formed_own_server_app_is_valid_and_reaches_the_snapshot() {
    let y = manifest(SECTION, "", &tools("{connection}", "PROPFIND"));
    let v = run(&y);
    assert!(v.is_empty(), "{v:#?}");
    let m = Manifest::parse(&y, "0.1.0").unwrap();
    let o = m.apps[0].own_server.as_ref().unwrap();
    assert_eq!(
        (o.url_field.as_str(), o.hosts_field.as_str()),
        ("url", "hosts")
    );
}

#[test]
fn every_webdav_method_loads_and_is_a_valid_tool_method() {
    for m in [
        "PROPFIND",
        "REPORT",
        "MKCALENDAR",
        "PROPPATCH",
        "MOVE",
        "COPY",
        "OPTIONS",
        "PUT",
        "DELETE",
    ] {
        let v = run(&manifest(SECTION, "", &tools("{connection}", m)));
        assert!(v.is_empty(), "{m}: {v:#?}");
    }
}

#[test]
fn the_connection_host_is_refused_without_the_section() {
    let c = codes(&manifest("", "", &tools("{connection}", "GET")));
    assert!(c.contains("connection_placeholder_misuse"), "{c:?}");
}

#[test]
fn an_own_server_tool_must_use_the_connection_host() {
    let c = codes(&manifest(SECTION, "", &tools("example.com", "GET")));
    assert!(c.contains("own_server_tool"), "{c:?}");
}

#[test]
fn the_section_beside_hosts_or_instances_is_refused() {
    let hosts = "    hosts:\n      - match: { host: example.com }\n        inject: []";
    let c = codes(&manifest(SECTION, hosts, &tools("{connection}", "GET")));
    assert!(c.contains("own_server_exclusive"), "{c:?}");
    let inst = "    instances: { label: Server }";
    let c = codes(&manifest(SECTION, inst, &tools("{connection}", "GET")));
    assert!(c.contains("own_server_exclusive"), "{c:?}");
}

#[test]
fn only_basic_bearer_and_header_stages_are_allowed() {
    for stage in [
        "{ query: { name: k, value: \"{credentials.password}\" } }",
        "{ remove_header: x-y }",
        "{ path_template: { prefix: /a, value: \"{credentials.password}\" } }",
    ] {
        let s = format!(
            "    own_server: {{ url_field: url, hosts_field: hosts, inject: [ {stage} ] }}"
        );
        let c = codes(&manifest(&s, "", &tools("{connection}", "GET")));
        assert!(c.contains("own_server_inject"), "{stage}: {c:?}");
    }
    let bearer = "    own_server: { url_field: url, hosts_field: hosts, inject: [ { bearer: \"{credentials.password}\" } ] }";
    assert!(run(&manifest(bearer, "", &tools("{connection}", "GET"))).is_empty());
}

#[test]
fn rewrite_host_and_tenant_have_no_place_in_the_section() {
    for extra in [
        "rewrite_host: { template: x, zone: \"*.x.y\" }",
        "tenant: { field: connection.tenant_host, zone: \"*.x.y\" }",
    ] {
        let s = format!(
            "    own_server: {{ url_field: url, hosts_field: hosts, inject: [], {extra} }}"
        );
        assert!(
            Manifest::parse(&manifest(&s, "", &tools("{connection}", "GET")), "0.1.0").is_err(),
            "{extra}"
        );
    }
}

#[test]
fn the_fields_must_be_declared_metadata_and_the_template_must_name_a_credential() {
    let s = "    own_server: { url_field: nope, hosts_field: hosts, inject: [ { basic: { user: \"{credentials.username}\", password: \"{credentials.password}\" } } ] }";
    assert!(codes(&manifest(s, "", &tools("{connection}", "GET"))).contains("own_server_field"));
    let s = "    own_server: { url_field: url, hosts_field: hosts, inject: [ { bearer: \"{credentials.missing}\" } ] }";
    assert!(!run(&manifest(s, "", &tools("{connection}", "GET"))).is_empty());
}

#[test]
fn the_snapshot_carries_one_synthetic_entry_that_never_claims_a_host() {
    use aurora_registry::identify::{
        claim_transport_ok, identify_own_server_app, identify_tool_in,
    };
    use aurora_registry::snapshot::{ActivePackage, Snapshot, compile};
    let y = manifest(SECTION, "", &tools("{connection}", "PROPFIND"));
    let c = compile(
        &[ActivePackage {
            manifest: Manifest::parse(&y, "0.1.0").unwrap(),
            digest: "d".into(),
            publisher: "aurora".into(),
            hooks_digest: None,
        }],
        &[],
        &[],
        1,
    )
    .unwrap();
    let snap = Snapshot::from_slice(&c.json).unwrap();
    // No static host claim, one `{connection}` entry with the section's inject stages.
    assert!(snap.hosts.is_empty());
    assert_eq!(snap.own_server_hosts.len(), 1);
    assert_eq!(snap.own_server_hosts[0].host, "{connection}");
    assert_eq!(snap.own_server_hosts[0].inject.len(), 1);
    assert!(
        aurora_registry::index::HostIndex::new(&snap)
            .lookup_port("{connection}", 443, "/")
            .is_none()
    );
    let m = identify_own_server_app(&snap, "dav").unwrap();
    assert_eq!(m.entry.app_id, "dav");
    // Its tool serves whatever origin the caller matched, by method.
    let t = identify_tool_in(
        m.app,
        false,
        "dav.example",
        8443,
        "/cal/",
        None,
        "PROPFIND",
        b"",
        false,
        &[],
        None,
    );
    assert_eq!(t.map(|t| t.id.as_str()), Some("t"));
    assert!(
        identify_tool_in(
            m.app,
            false,
            "dav.example",
            8443,
            "/cal/",
            None,
            "DELETE",
            b"",
            false,
            &[],
            None
        )
        .is_none()
    );
    // The transport rule the gateway applies to the matched origin: TLS to that very port.
    assert!(claim_transport_ok(Some(8443), true, 8443));
    assert!(!claim_transport_ok(Some(8443), false, 8443));
    assert!(!claim_transport_ok(Some(8443), true, 443));
}
