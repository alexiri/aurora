#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! D-74: instance-bound apps (`instances`, the `{instance}` placeholder): linter rules and
//! snapshot compilation.
use std::collections::BTreeSet;

use aurora_registry::claims::{claims, conflict_groups};
use aurora_registry::index::{HostIndex, Snapshot};
use aurora_registry::snapshot::{ActivePackage, compile};
use aurora_registry::{Manifest, Violation, validate};

const RUNNING: &str = "0.1.0";

/// A valid instance-bound package; `{{HOSTS}}`-style knobs are replaced by the caller.
fn yaml(instances: &str, host: &str, tool_host: &str, probe_url: &str, oauth: &str) -> String {
    format!(
        r#"package: {{ id: selfhost, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: P }}
oauth_clients:
  - id: oa
    authorize_url: "{oauth}/authorize"
    token_url: "{oauth}/token"
    fields: [ {{ name: client_id, label: Client ID }} ]
apps:
  - about: {{ id: desk, name: Desk, web_hosts: [saas.desk.example] }}
{instances}
    credentials: [ {{ name: access_token, secret: true }} ]
    connect:
      - id: oauth
        kind: oauth2
        client: oa
        scopes: [ {{ scope: read, name: Read, access: read }} ]
        default_scopes: [read]
        probe: {{ url: "{probe_url}" }}
    hosts:
      - match: {{ host: "{host}", path_prefix: /api }}
        inject:
          - bearer: "{{credentials.access_token}}"
    tools:
      read:
        items:
          - {{ id: list_things, name: List, host: "{tool_host}", path: /api/things, methods: [GET] }}
"#
    )
}

const INST: &str = "    instances: { label: Desk instance, default_host: saas.desk.example, probe: { path: /api/health, expect_status: [200, 401] } }";

fn good() -> String {
    yaml(
        INST,
        "{instance}",
        "{instance}",
        "https://{instance}/api/me",
        "https://{instance}/oauth",
    )
}

fn run(y: &str) -> Vec<Violation> {
    match Manifest::parse(y, RUNNING) {
        Err(v) => v,
        Ok(m) => validate::manifest(&m),
    }
}

fn codes(v: &[Violation]) -> BTreeSet<&'static str> {
    v.iter().map(|x| x.code).collect()
}

#[test]
fn a_valid_instance_bound_app_has_no_violations() {
    let v = run(&good());
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn placeholder_rules_are_enforced() {
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "part of a host",
            yaml(
                INST,
                "api.{instance}",
                "api.{instance}",
                "https://{instance}/api/me",
                "https://{instance}/oauth",
            ),
            "instance_placeholder_misuse",
        ),
        (
            "wildcard host",
            yaml(
                INST,
                "*.{instance}",
                "{instance}",
                "https://{instance}/api/me",
                "https://{instance}/oauth",
            ),
            "instance_placeholder_misuse",
        ),
        (
            "app without instances",
            yaml(
                "",
                "{instance}",
                "{instance}",
                "https://{instance}/api/me",
                "https://{instance}/oauth",
            ),
            "instance_placeholder_misuse",
        ),
        (
            "in a probe path",
            yaml(
                INST,
                "{instance}",
                "{instance}",
                "https://{instance}/api/{instance}",
                "https://{instance}/oauth",
            ),
            "instance_placeholder_misuse",
        ),
        (
            "probe on another host",
            yaml(
                INST,
                "{instance}",
                "{instance}",
                "https://evil.example/api/me",
                "https://{instance}/oauth",
            ),
            "probe_host_undeclared",
        ),
        (
            "oauth url not https",
            yaml(
                INST,
                "{instance}",
                "{instance}",
                "https://{instance}/api/me",
                "http://{instance}/oauth",
            ),
            "instance_placeholder_misuse",
        ),
        (
            "oauth url placeholder as a userinfo trick",
            yaml(
                INST,
                "{instance}",
                "{instance}",
                "https://{instance}/api/me",
                "https://evil.example@{instance}/oauth",
            ),
            "instance_placeholder_misuse",
        ),
        (
            "oauth url placeholder in the path",
            yaml(
                INST,
                "{instance}",
                "{instance}",
                "https://{instance}/api/me",
                "https://{instance}/{instance}",
            ),
            "instance_placeholder_misuse",
        ),
    ];
    for (name, y, code) in cases {
        let v = run(&y);
        assert!(codes(&v).contains(code), "{name}: {v:#?}");
    }
}

#[test]
fn instances_block_rules() {
    for (name, inst, code) in [
        (
            "default host with port and capitals",
            "    instances: { label: X, default_host: \"Saas.Desk.example:8443\" }",
            "instance_default_host_invalid",
        ),
        (
            "default host is an IP",
            "    instances: { label: X, default_host: \"10.0.0.1\" }",
            "instance_default_host_invalid",
        ),
        (
            "probe path without slash",
            "    instances: { label: X, probe: { path: health } }",
            "instance_probe_invalid",
        ),
        (
            "empty label",
            "    instances: { label: \"  \" }",
            "instances_label_empty",
        ),
    ] {
        let v = run(&yaml(
            inst,
            "{instance}",
            "{instance}",
            "https://{instance}/api/me",
            "https://{instance}/oauth",
        ));
        assert!(codes(&v).contains(code), "{name}: {v:#?}");
    }
    // `instances` without any `{instance}` host rule is refused.
    let v = run(&yaml(
        INST,
        "saas.desk.example",
        "saas.desk.example",
        "https://saas.desk.example/api/me",
        "https://saas.desk.example/oauth",
    ));
    assert!(
        codes(&v).contains("instances_without_instance_host"),
        "{v:#?}"
    );
}

fn active(y: &str, id: &str) -> ActivePackage {
    let y = y.replace("id: selfhost", &format!("id: {id}"));
    ActivePackage {
        manifest: Manifest::parse(&y, RUNNING).unwrap(),
        digest: format!("d-{id}"),
        publisher: "aurora".into(),
        hooks_digest: None,
    }
}

#[test]
fn snapshot_keeps_instance_entries_apart_and_compiles_default_host_as_a_static_claim() {
    let c = compile(&[active(&good(), "selfhost")], &[], &[], 1).unwrap();
    let s = Snapshot::from_slice(&c.json).unwrap();
    // The `{instance}` entry is not in the static host index ...
    assert!(s.hosts.iter().all(|h| h.host != "{instance}"));
    assert_eq!(s.instance_hosts.len(), 1);
    assert_eq!(s.instance_hosts[0].host, "{instance}");
    // ... while `default_host` is an ordinary static claim of the same rule.
    assert_eq!(s.hosts.len(), 1);
    assert_eq!(s.hosts[0].host, "saas.desk.example");
    assert_eq!(s.hosts[0].path_prefix.as_deref(), Some("/api"));
    let ix = HostIndex::new(&s);
    assert!(ix.lookup("saas.desk.example", "/api/things").is_some());
    // An instance host is never found by the static index (only the gateway's exact instance
    // match serves it).
    assert!(ix.lookup("desk.acme.example", "/api/things").is_none());
    // The `{instance}` tool is found on the default host.
    let t = ix.tool("saas.desk.example", "/api/things", "GET");
    assert_eq!(t.map(|(_, t)| t.id.as_str()), Some("list_things"));
    assert!(ix.tool("desk.acme.example", "/api/things", "GET").is_none());
    assert!(s.app("desk").unwrap().instances.is_some());
}

#[test]
fn an_app_without_a_default_host_claims_no_static_host() {
    let y = good().replace(", default_host: saas.desk.example", "");
    let c = compile(&[active(&y, "selfhost")], &[], &[], 1).unwrap();
    let s = Snapshot::from_slice(&c.json).unwrap();
    assert!(s.hosts.is_empty());
    assert_eq!(s.instance_hosts.len(), 1);
    assert!(claims(&Manifest::parse(&y, RUNNING).unwrap()).is_empty());
}

#[test]
fn default_host_takes_part_in_claim_conflicts_but_instance_entries_do_not() {
    let a = active(&good(), "selfhost");
    // Another package claiming the SaaS host conflicts with the default host.
    let other = good()
        .replace("id: desk", "id: other")
        .replace("{instance}", "saas.desk.example")
        .replace(INST, "");
    let b = active(&other, "other");
    let all: Vec<_> = [&a, &b]
        .iter()
        .flat_map(|p| {
            claims(&p.manifest)
                .into_iter()
                .map(|c| (p.manifest.package.id.clone(), c))
        })
        .collect();
    assert_eq!(conflict_groups(&all).len(), 1);
    // A static claim on an arbitrary host does not conflict with an `{instance}` rule: instance
    // hosts are registered (and refused at registration), not claimed by the manifest.
    let elsewhere = good()
        .replace("id: desk", "id: other")
        .replace("{instance}", "other.example")
        .replace(INST, "");
    let c = active(&elsewhere, "other2");
    let all: Vec<_> = [&a, &c]
        .iter()
        .flat_map(|p| {
            claims(&p.manifest)
                .into_iter()
                .map(|c| (p.manifest.package.id.clone(), c))
        })
        .collect();
    assert!(conflict_groups(&all).is_empty());
}
