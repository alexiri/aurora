#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-135 (D-242): `about.family` — apps of one package that are the same product in different
//! deployments (GitHub on github.com and on a GitHub Enterprise Server). Linter rules, the
//! cross-package rule and the snapshot carrying the family.
use std::collections::BTreeSet;

use aurora_registry::snapshot::{ActivePackage, compile};
use aurora_registry::{Manifest, Violation, validate};

const RUNNING: &str = "0.1.0";

/// A package of one hosted app and one instance-bound app; `fam_a` / `fam_b` are their `family:`
/// lines (or empty).
fn yaml(fam_a: &str, fam_b: &str) -> String {
    format!(
        r#"package: {{ id: fam, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: P }}
apps:
  - about:
      id: desk
      name: Desk
{fam_a}
    credentials: [ {{ name: access_token, secret: true }} ]
    connect:
      - id: token
        kind: fields
        fields: [ {{ name: access_token, label: T, type: secret, store: credentials, required: true }} ]
    hosts:
      - match: {{ host: api.desk.example }}
        inject: [ {{ bearer: "{{credentials.access_token}}" }} ]
  - about:
      id: desk-self
      name: Desk Server
{fam_b}
    instances: {{ label: Desk server }}
    credentials: [ {{ name: access_token, secret: true }} ]
    connect:
      - id: token
        kind: fields
        fields: [ {{ name: access_token, label: T, type: secret, store: credentials, required: true }} ]
    hosts:
      - match: {{ host: "{{instance}}" }}
        inject: [ {{ bearer: "{{credentials.access_token}}" }} ]
"#
    )
}

const A: &str = "      family: { id: desk, name: Desk, variant_label: desk.example }";
const B: &str = "      family: { id: desk, name: Desk, variant_label: Server }";

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
fn a_family_of_a_hosted_and_an_instance_bound_app_is_valid() {
    let v = run(&yaml(A, B));
    assert!(v.is_empty(), "{v:#?}");
    // Apps without a family stay valid.
    assert!(run(&yaml("", "")).is_empty());
}

#[test]
fn family_rules_are_enforced() {
    for (name, a, b, code) in [
        (
            "different names",
            A,
            "      family: { id: desk, name: Desktop, variant_label: Server }",
            "family_name_mismatch",
        ),
        (
            "duplicate variant label",
            A,
            "      family: { id: desk, name: Desk, variant_label: desk.example }",
            "family_variant_duplicate",
        ),
        (
            "bad id",
            "      family: { id: Desk Co, name: Desk, variant_label: desk.example }",
            B,
            "family_id_invalid",
        ),
        (
            "empty variant label",
            A,
            "      family: { id: desk, name: Desk, variant_label: \" \" }",
            "family_label_empty",
        ),
    ] {
        let v = run(&yaml(a, b));
        assert!(codes(&v).contains(code), "{name}: {v:#?}");
    }
    // Two members that are not instance-bound.
    let y = yaml(A, B).replace("    instances: { label: Desk server }\n", "");
    let v = run(&y);
    assert!(codes(&v).contains("family_two_hosted"), "{v:#?}");
}

#[test]
fn the_snapshot_carries_the_family() {
    let m = Manifest::parse(&yaml(A, B), RUNNING).unwrap();
    let p = ActivePackage {
        manifest: m,
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
    };
    let c = compile(&[p], &[], &[], 1).unwrap();
    let s = aurora_registry::index::Snapshot::from_slice(&c.json).unwrap();
    let f = |id: &str| s.app(id).unwrap().about.family.clone().unwrap();
    assert_eq!(f("desk").variant_label, "desk.example");
    assert_eq!(f("desk-self").variant_label, "Server");
    assert_eq!(f("desk-self").id, f("desk").id);
}
