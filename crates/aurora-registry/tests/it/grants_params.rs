//! P4-GRANTS (D-123, D-124; SEC-67, SEC-68): `apps[].grants` and tool `params` in the manifest,
//! the linter and the snapshot.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::collections::{BTreeSet, HashMap};

use aurora_crypto::generate_signing_key;
use aurora_policy::HostPattern;
use aurora_registry::builder::PackageBuilder;
use aurora_registry::snapshot::{ActivePackage, compile};
use aurora_registry::validate::{self, ValidationContext};
use aurora_registry::{Limits, Manifest, Package, TrustStore, Violation};

const HEAD: &str = r#"
package: { id: aws, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: AWS }
apps:
  - about: { id: aws, name: AWS }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: API token, type: secret, required: true } ]
    hosts:
      - match: { host: "ec2.*.amazonaws.com" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

fn with(extra: &str) -> String {
    format!("{HEAD}{extra}")
}

fn codes(yaml: &str) -> BTreeSet<&'static str> {
    match Manifest::parse(yaml, "0.1.0") {
        Err(v) => v.iter().map(|x| x.code).collect(),
        Ok(m) => validate::manifest(&m).iter().map(|x| x.code).collect(),
    }
}

fn violations(yaml: &str) -> Vec<Violation> {
    match Manifest::parse(yaml, "0.1.0") {
        Err(v) => v,
        Ok(m) => validate::manifest(&m),
    }
}

const DESCRIBE: &str = "{ id: ec2_describe, name: Describe, host: \"ec2.*.amazonaws.com\", path: /, methods: [POST], params: [ { source: form, name: Action, match: { prefix: Describe } } ] }";
const GENERIC: &str =
    "{ id: ec2_call, name: Call EC2, host: \"ec2.*.amazonaws.com\", path: /, methods: [POST] }";

fn tools(read: &[&str], write: &[&str]) -> String {
    format!(
        "    tools:\n      read:\n        items: [ {} ]\n      write:\n        items: [ {} ]\n",
        read.join(", "),
        write.join(", ")
    )
}

// ---- grants ----------------------------------------------------------------------------------

#[test]
fn grants_default_false_and_parse() {
    let m = Manifest::parse(&with(&tools(&[DESCRIBE], &[GENERIC])), "0.1.0").unwrap();
    assert!(!m.apps[0].grants.custom_only && !m.apps[0].grants.block_untooled);
    let y = with(&format!(
        "    grants: {{ custom_only: true, block_untooled: true }}\n{}",
        tools(&[DESCRIBE], &[GENERIC])
    ));
    let m = Manifest::parse(&y, "0.1.0").unwrap();
    assert!(m.apps[0].grants.custom_only && m.apps[0].grants.block_untooled);
    assert!(codes(&y).is_empty(), "{:?}", violations(&y));
}

#[test]
fn grants_unknown_keys_and_non_booleans_are_rejected() {
    let t = tools(&[DESCRIBE], &[GENERIC]);
    assert_eq!(
        codes(&with(&format!("    grants: {{ custom_onlyy: true }}\n{t}"))),
        BTreeSet::from(["unknown_field"])
    );
    assert_eq!(
        codes(&with(&format!(
            "    grants: {{ custom_only: \"yes\" }}\n{t}"
        ))),
        BTreeSet::from(["schema"])
    );
    assert_eq!(
        codes(&with(&format!("    grants: {{ block_untooled: 1 }}\n{t}"))),
        BTreeSet::from(["schema"])
    );
}

#[test]
fn custom_only_without_tools() {
    assert_eq!(
        codes(&with("    grants: { custom_only: true }\n")),
        BTreeSet::from(["custom_only_without_tools"])
    );
    // `block_untooled` alone is fine without tools (everything is untooled).
    assert!(codes(&with("    grants: { block_untooled: true }\n")).is_empty());
}

// ---- params shape -----------------------------------------------------------------------------

fn one(cond: &str) -> String {
    let t = format!(
        "{{ id: t1, name: T, host: \"ec2.*.amazonaws.com\", path: /, methods: [POST], params: [ {cond} ] }}"
    );
    with(&tools(&[&t], &[GENERIC]))
}

#[test]
fn params_valid_shapes() {
    for c in [
        "{ source: form, name: Action, match: { equals: GetCallerIdentity } }",
        "{ source: query, name: Action, match: { in: [Describe, List] } }",
        "{ source: header, name: X-Amz-Target, match: { prefix: \"DynamoDB_20120810.Query\" } }",
    ] {
        let v = violations(&one(c));
        assert!(v.is_empty(), "{c}: {v:#?}");
    }
}

#[test]
fn params_invalid_shapes() {
    for (c, why) in [
        (
            "{ source: form, name: Action, match: { prefix: Des } }",
            "prefix too short",
        ),
        (
            "{ source: form, name: Action, match: { equals: a, prefix: Describe } }",
            "two forms",
        ),
        ("{ source: form, name: Action, match: {} }", "no form"),
        (
            "{ source: form, name: \"Act ion\", match: { equals: Abcd } }",
            "bad name",
        ),
        (
            "{ source: form, name: Action, match: { equals: \"A b\" } }",
            "bad value",
        ),
        (
            "{ source: header, name: authorization, match: { prefix: Bearer } }",
            "forbidden header",
        ),
        (
            "{ source: header, name: cookie, match: { equals: abcd } }",
            "forbidden header",
        ),
        (
            "{ source: form, name: Action, match: { in: [] } }",
            "empty in",
        ),
    ] {
        assert!(codes(&one(c)).contains("params_invalid"), "{why}: {c}");
    }
    // unknown source / key / regex-like match are schema errors
    assert!(
        !codes(&one(
            "{ source: cookie, name: Action, match: { equals: Abcd } }"
        ))
        .is_empty()
    );
    assert!(
        !codes(&one(
            "{ source: form, name: Action, match: { regex: \"D.*\" } }"
        ))
        .is_empty()
    );
    assert!(
        !codes(&one(
            "{ source: form, name: Action, match: { equals: Abcd }, extra: 1 }"
        ))
        .is_empty()
    );
}

#[test]
fn params_count_limits() {
    let c = |n: usize| {
        (0..n)
            .map(|i| format!("{{ source: query, name: P{i}, match: {{ equals: Abcd }} }}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let t = |n: usize| {
        let t = format!(
            "{{ id: t1, name: T, host: \"ec2.*.amazonaws.com\", path: /, methods: [GET], params: [ {} ] }}",
            c(n)
        );
        with(&tools(&[&t], &[GENERIC]))
    };
    assert!(codes(&t(4)).is_empty());
    assert!(codes(&t(5)).contains("params_invalid"));
    assert!(codes(&t(0)).contains("params_invalid"));
}

#[test]
fn params_in_list_and_length_limits() {
    let long = "a".repeat(129);
    assert!(
        codes(&one(&format!(
            "{{ source: query, name: Action, match: {{ equals: \"{long}\" }} }}"
        )))
        .contains("params_invalid")
    );
    let many = (0..33)
        .map(|i| format!("V{i}ab"))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        codes(&one(&format!(
            "{{ source: query, name: Action, match: {{ in: [{many}] }} }}"
        )))
        .contains("params_invalid")
    );
}

#[test]
fn form_condition_needs_a_body_method() {
    let t = "{ id: t1, name: T, host: \"ec2.*.amazonaws.com\", path: /, methods: [GET, HEAD], params: [ { source: form, name: Action, match: { equals: Abcd } } ] }";
    assert!(codes(&with(&tools(&[t], &[GENERIC]))).contains("params_invalid"));
}

#[test]
fn group_wildcard_with_params_is_rejected() {
    let y = with(
        "    tools:\n      read:\n        wildcard: { id: all_read, name: All, params: [] }\n        items: [ { id: a, name: A, host: \"ec2.*.amazonaws.com\", path: /, methods: [GET] } ]\n",
    );
    assert_eq!(codes(&y), BTreeSet::from(["unknown_field"]));
}

#[test]
fn params_tool_must_precede_the_tool_that_shadows_it() {
    // generic write tool is classified after read items: the params tool in `write` placed after a
    // broader read tool is shadowed.
    let read =
        "{ id: ec2_all_read, name: All, host: \"ec2.*.amazonaws.com\", path: /, methods: [POST] }";
    let v = codes(&with(&tools(&[read], &[DESCRIBE])));
    assert!(v.contains("params_tool_shadowed"), "{v:?}");
    // declared before the generic tool: fine
    assert!(codes(&with(&tools(&[DESCRIBE], &[GENERIC]))).is_empty());
    // a broader tool after it is fine too; a broader tool with params itself does not shadow
    let other = "{ id: o, name: O, host: \"ec2.*.amazonaws.com\", path: /, methods: [POST], params: [ { source: form, name: Action, match: { equals: Other } } ] }";
    assert!(codes(&with(&tools(&[other], &[DESCRIBE]))).is_empty());
}

// ---- snapshot ---------------------------------------------------------------------------------

fn compile_json(yaml: &str) -> serde_json::Value {
    let m = Manifest::parse(yaml, "0.1.0").unwrap();
    let c = compile(
        &[ActivePackage {
            manifest: m,
            digest: "d".into(),
            publisher: "aurora".into(),
            hooks_digest: None,
        }],
        &[],
        &[],
        1,
    )
    .unwrap();
    serde_json::from_slice(&c.json).unwrap()
}

#[test]
fn snapshot_carries_grants_and_params() {
    let y = with(&format!(
        "    grants: {{ custom_only: true }}\n{}",
        tools(&[DESCRIBE], &[GENERIC])
    ));
    let j = compile_json(&y);
    assert_eq!(
        j["apps"][0]["grants"],
        serde_json::json!({ "custom_only": true, "block_untooled": false })
    );
    let p = &j["apps"][0]["tools"]["read"]["items"][0]["params"];
    assert_eq!(p[0]["source"], "form");
    assert_eq!(p[0]["match"]["prefix"], "Describe");
    // a tool without params carries no `params` key at all
    assert!(
        j["apps"][0]["tools"]["write"]["items"][0]
            .get("params")
            .is_none()
    );
    // and the snapshot parses back typed
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
    let s = aurora_registry::index::Snapshot::from_slice(&c.json).unwrap();
    assert!(s.apps[0].grants.custom_only);
    assert_eq!(
        s.apps[0]
            .tools
            .as_ref()
            .unwrap()
            .read
            .as_ref()
            .unwrap()
            .items[0]
            .params
            .as_ref()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn snapshot_without_flags_differs_only_by_explicit_grants_defaults() {
    let j = compile_json(&with(&tools(&[GENERIC], &[])));
    assert_eq!(
        j["apps"][0]["grants"],
        serde_json::json!({ "custom_only": false, "block_untooled": false })
    );
    let mut stripped = j.clone();
    stripped["apps"][0]
        .as_object_mut()
        .unwrap()
        .remove("grants");
    assert!(!stripped.to_string().contains("params"));
}

#[test]
fn old_snapshot_without_grants_parses_as_both_false() {
    let mut j = compile_json(&with(&tools(&[GENERIC], &[])));
    j["apps"][0].as_object_mut().unwrap().remove("grants");
    let s = aurora_registry::index::Snapshot::from_slice(j.to_string().as_bytes()).unwrap();
    assert!(!s.apps[0].grants.custom_only && !s.apps[0].grants.block_untooled);
}

#[test]
fn snapshot_with_an_unknown_classifier_is_refused() {
    // A reader that does not know the classifier (an unknown `source`) refuses the snapshot, so a
    // `params` tool is never run as a path-only tool.
    let mut j = compile_json(&with(&tools(&[DESCRIBE], &[GENERIC])));
    j["apps"][0]["tools"]["read"]["items"][0]["params"][0]["source"] = "json_body".into();
    assert!(aurora_registry::index::Snapshot::from_slice(j.to_string().as_bytes()).is_err());
    let mut j = compile_json(&with(&tools(&[DESCRIBE], &[GENERIC])));
    j["apps"][0]["tools"]["read"]["items"][0]["params"][0]["regex"] = "x".into();
    assert!(aurora_registry::index::Snapshot::from_slice(j.to_string().as_bytes()).is_err());
}

// ---- fixture requirements ---------------------------------------------------------------------

fn pkg_codes(yaml: &str, tests: Option<&str>) -> Vec<(&'static str, String)> {
    let mut b = PackageBuilder::new(yaml);
    if let Some(t) = tests {
        b.set_file("tests/params.yaml", t.as_bytes());
    }
    let key = generate_signing_key().unwrap();
    b.sign(&key, "builtin");
    let pkg = Package::read(&b.build().unwrap(), &Limits::default()).unwrap();
    let ctx = ValidationContext {
        running_version: "0.1.0".into(),
        reserved_hosts: vec![HostPattern::parse("*.aurora.internal").unwrap()],
        active_app_ids: HashMap::new(),
    };
    let trust = TrustStore {
        project_key: Some(key.verifying_key()),
        keys: vec![],
    };
    validate::package(&pkg, &ctx, &trust, false)
        .into_iter()
        .map(|v| (v.code, v.message))
        .collect()
}

const FORM: &str = "headers: { content-type: application/x-www-form-urlencoded }";

fn case(name: &str, headers: &str, url: &str, body: &str, extra: &str, tool: &str) -> String {
    format!(
        "- name: {name}\n  request: {{ method: POST, url: \"{url}\", {headers}, body: \"{body}\"{extra} }}\n  expect: {{ app: aws, tool: {tool} }}\n"
    )
}

fn full_fixtures() -> String {
    let u = "https://ec2.eu-west-1.amazonaws.com/";
    let ue = "https://ec2.eu-west-1.amazonaws.com/?Action=DescribeInstances";
    [
        case(
            "positive",
            FORM,
            u,
            "Action=DescribeInstances",
            "",
            "ec2_describe",
        ),
        case(
            "other value",
            FORM,
            u,
            "Action=RunInstances",
            "",
            "ec2_call",
        ),
        case("missing", FORM, u, "Version=1", "", "ec2_call"),
        case(
            "repeated",
            FORM,
            u,
            "Action=DescribeX&Action=RunInstances",
            "",
            "ec2_call",
        ),
        case(
            "conflict",
            FORM,
            ue,
            "Action=DescribeInstances",
            "",
            "ec2_call",
        ),
        case(
            "content type",
            "headers: { content-type: application/json }",
            u,
            "Action=DescribeInstances",
            "",
            "ec2_call",
        ),
        case(
            "truncated",
            FORM,
            u,
            "Action=DescribeInstances",
            ", body_truncated: true",
            "ec2_call",
        ),
    ]
    .concat()
}

#[test]
fn fixtures_are_required_for_every_params_tool() {
    let y = with(&tools(&[DESCRIBE], &[GENERIC]));
    let c = pkg_codes(&y, None);
    assert!(
        c.iter()
            .any(|(code, m)| *code == "params_fixture_missing" && m.contains("positive")),
        "{c:?}"
    );
    let c = pkg_codes(&y, Some(&full_fixtures()));
    assert!(c.is_empty(), "{c:?}");
}

#[test]
fn each_missing_fixture_category_is_reported() {
    let y = with(&tools(&[DESCRIBE], &[GENERIC]));
    let all = [
        ("positive", "positive"),
        ("other value", "other value"),
        ("missing", "missing"),
        ("repeated", "repeated"),
        ("conflict", "query/body conflict"),
        ("content type", "wrong content type"),
        ("truncated", "truncated body"),
    ];
    for (drop, label) in all {
        let u = "https://ec2.eu-west-1.amazonaws.com/";
        let ue = "https://ec2.eu-west-1.amazonaws.com/?Action=DescribeInstances";
        let cases = [
            (
                "positive",
                case(
                    "positive",
                    FORM,
                    u,
                    "Action=DescribeInstances",
                    "",
                    "ec2_describe",
                ),
            ),
            (
                "other value",
                case(
                    "other value",
                    FORM,
                    u,
                    "Action=RunInstances",
                    "",
                    "ec2_call",
                ),
            ),
            (
                "missing",
                case("missing", FORM, u, "Version=1", "", "ec2_call"),
            ),
            (
                "repeated",
                case(
                    "repeated",
                    FORM,
                    u,
                    "Action=DescribeX&Action=RunInstances",
                    "",
                    "ec2_call",
                ),
            ),
            (
                "conflict",
                case(
                    "conflict",
                    FORM,
                    ue,
                    "Action=DescribeInstances",
                    "",
                    "ec2_call",
                ),
            ),
            (
                "content type",
                case(
                    "content type",
                    "headers: { content-type: application/json }",
                    u,
                    "Action=DescribeInstances",
                    "",
                    "ec2_call",
                ),
            ),
            (
                "truncated",
                case(
                    "truncated",
                    FORM,
                    u,
                    "Action=DescribeInstances",
                    ", body_truncated: true",
                    "ec2_call",
                ),
            ),
        ];
        let text: String = cases
            .iter()
            .filter(|(k, _)| *k != drop)
            .map(|(_, v)| v.as_str())
            .collect();
        let c = pkg_codes(&y, Some(&text));
        let hit: Vec<_> = c
            .iter()
            .filter(|(code, _)| *code == "params_fixture_missing")
            .collect();
        assert_eq!(hit.len(), 1, "{drop}: {c:?}");
        assert!(hit[0].1.contains(label), "{drop}: {c:?}");
    }
}
