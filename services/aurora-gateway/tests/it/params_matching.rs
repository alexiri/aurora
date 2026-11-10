#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-GRANTS (Q8, D-124, SEC-68): parameter-level tool matching through the gateway's own
//! matcher (the `check-snapshot` runner uses `identify_app` / `identify_tool`, the functions of
//! the live pipeline) and the snapshot compatibility rules.
use aurora_gateway::apps::{identify_app, needs_body};
use aurora_gateway::check_snapshot::run;
use aurora_gateway::snapshot::{Loaded, parse};
use aurora_registry::Manifest;
use aurora_registry::snapshot::{ActivePackage, compile};
use serde_json::Value;

const MANIFEST: &str = r#"
package: { id: aws, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: AWS }
apps:
  - about: { id: aws, name: AWS }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    hosts:
      - match: { host: "ec2.*.amazonaws.com" }
        inject: [ { bearer: "{credentials.api_token}" } ]
      - match: { host: "sts.*.amazonaws.com" }
        inject: [ { bearer: "{credentials.api_token}" } ]
      - match: { host: "dynamodb.*.amazonaws.com" }
        inject: [ { bearer: "{credentials.api_token}" } ]
      - match: { host: "q.example.com" }
        inject: [ { bearer: "{credentials.api_token}" } ]
    tools:
      read:
        items:
          - { id: ec2_describe, name: Describe, host: "ec2.*.amazonaws.com", path: /, methods: [POST],
              params: [ { source: form, name: Action, match: { prefix: Describe } } ] }
          - { id: sts_who, name: Who, host: "sts.*.amazonaws.com", path: /, methods: [POST],
              params: [ { source: form, name: Action, match: { equals: GetCallerIdentity } } ] }
          - { id: ddb_query, name: Query, host: "dynamodb.*.amazonaws.com", path: /, methods: [POST],
              params: [ { source: header, name: x-amz-target, match: { prefix: "DynamoDB_20120810.Query" } } ] }
          - { id: q_list, name: List, host: q.example.com, path: /api, methods: [GET, POST],
              params: [ { source: query, name: Action, match: { in: [ListThings, ListOthers] } } ] }
          - { id: two, name: Two, host: q.example.com, path: /two, methods: [POST],
              params: [ { source: query, name: kind, match: { equals: alpha } },
                        { source: form, name: op, match: { equals: read } } ] }
      write:
        items:
          - { id: ec2_call, name: Call, host: "ec2.*.amazonaws.com", path: /, methods: [GET, POST] }
"#;

fn snapshot_json() -> Vec<u8> {
    let p = ActivePackage {
        manifest: Manifest::parse(MANIFEST, "0.1.0").unwrap(),
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
    };
    compile(&[p], &[], &[], 1).unwrap().json
}

const CONN: &str = "  connection: { credentials: { api_token: tok } }\n";

fn run_cases(y: &str) {
    let r = run(&snapshot_json(), &[("p.yaml".into(), y.into())]).unwrap();
    assert!(r.ok, "{}", serde_json::to_string_pretty(&r.json).unwrap());
    assert!(r.json["passed"].as_u64().unwrap() >= 1);
}

fn form(host: &str, body: &str, expect: &str) -> String {
    format!(
        "- name: form {body}\n{CONN}  request:\n    method: POST\n    url: \"https://{host}/\"\n    headers: {{ content-type: application/x-www-form-urlencoded }}\n    body: \"{body}\"\n  expect: {expect}\n"
    )
}

#[test]
fn equals_prefix_in_over_form_query_and_header_sources() {
    let mut y = String::new();
    y += &form(
        "ec2.eu-west-1.amazonaws.com",
        "Action=DescribeInstances",
        "{ tool: ec2_describe }",
    );
    y += &form(
        "ec2.eu-west-1.amazonaws.com",
        "Version=1&Action=DescribeVpcs",
        "{ tool: ec2_describe }",
    );
    y += &form(
        "ec2.eu-west-1.amazonaws.com",
        "Action=RunInstances",
        "{ tool: ec2_call }",
    );
    y += &form(
        "sts.eu-west-1.amazonaws.com",
        "Action=GetCallerIdentity",
        "{ tool: sts_who }",
    );
    y += &form(
        "sts.eu-west-1.amazonaws.com",
        "Action=AssumeRole",
        "{ app: aws, no_tool: true }",
    );
    y += r#"
- name: header prefix
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://dynamodb.eu-west-1.amazonaws.com/", headers: { X-Amz-Target: "DynamoDB_20120810.QueryItems" } }
  expect: { tool: ddb_query }
- name: header other value
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://dynamodb.eu-west-1.amazonaws.com/", headers: { X-Amz-Target: "DynamoDB_20120810.DeleteTable" } }
  expect: { app: aws, no_tool: true }
- name: header missing
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://dynamodb.eu-west-1.amazonaws.com/" }
  expect: { app: aws, no_tool: true }
- name: query in-list
  connection: { credentials: { api_token: tok } }
  request: { method: GET, url: "https://q.example.com/api?Action=ListOthers" }
  expect: { tool: q_list }
- name: query other value
  connection: { credentials: { api_token: tok } }
  request: { method: GET, url: "https://q.example.com/api?Action=DeleteThings" }
  expect: { app: aws, no_tool: true }
- name: query repeated
  connection: { credentials: { api_token: tok } }
  request: { method: GET, url: "https://q.example.com/api?Action=ListThings&Action=DeleteThings" }
  expect: { app: aws, no_tool: true }
- name: query name is case sensitive
  connection: { credentials: { api_token: tok } }
  request: { method: GET, url: "https://q.example.com/api?action=ListThings" }
  expect: { app: aws, no_tool: true }
- name: all conditions must hold (query and form)
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://q.example.com/two?kind=alpha", headers: { content-type: application/x-www-form-urlencoded }, body: "op=read" }
  expect: { tool: two }
- name: one of two conditions failing
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://q.example.com/two?kind=beta", headers: { content-type: application/x-www-form-urlencoded }, body: "op=read" }
  expect: { app: aws, no_tool: true }
"#;
    run_cases(&y);
}

#[test]
fn every_fail_closed_case_falls_through_to_the_generic_write_tool_or_to_none() {
    let ec2 = "ec2.eu-west-1.amazonaws.com";
    let mut y = String::new();
    // missing / repeated
    y += &form(ec2, "Version=1", "{ tool: ec2_call }");
    y += &form(
        ec2,
        "Action=DescribeInstances&Action=TerminateInstances",
        "{ tool: ec2_call }",
    );
    y += &form(
        ec2,
        "Action=TerminateInstances&Action=DescribeInstances",
        "{ tool: ec2_call }",
    );
    // percent-encoding: decoded once, `+` is a space, double encoding is not decoded twice
    y += &form(ec2, "%41ction=DescribeInstances", "{ tool: ec2_describe }");
    y += &form(ec2, "Action=Describe%49nstances", "{ tool: ec2_describe }");
    y += &form(ec2, "Action=%2544escribe", "{ tool: ec2_call }");
    y += &form(ec2, "Action=Describe+Instances", "{ tool: ec2_describe }");
    // case differences never match
    y += &form(ec2, "Action=describeInstances", "{ tool: ec2_call }");
    y += &form(ec2, "action=DescribeInstances", "{ tool: ec2_call }");
    // invalid UTF-8 after decoding, malformed escape
    y += &form(ec2, "Action=Describe%FF", "{ tool: ec2_call }");
    y += &form(ec2, "Action=Describe%zz", "{ tool: ec2_call }");
    y += r#"
- name: query and body conflict
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://ec2.eu-west-1.amazonaws.com/?Action=TerminateInstances", headers: { content-type: application/x-www-form-urlencoded }, body: "Action=DescribeInstances" }
  expect: { tool: ec2_call }
- name: same value in query and body is still a conflict
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://ec2.eu-west-1.amazonaws.com/?Action=DescribeInstances", headers: { content-type: application/x-www-form-urlencoded }, body: "Action=DescribeInstances" }
  expect: { tool: ec2_call }
- name: other content type
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://ec2.eu-west-1.amazonaws.com/", headers: { content-type: application/json }, body: "Action=DescribeInstances" }
  expect: { tool: ec2_call }
- name: no content type
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://ec2.eu-west-1.amazonaws.com/", body: "Action=DescribeInstances" }
  expect: { tool: ec2_call }
- name: charset other than utf-8
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://ec2.eu-west-1.amazonaws.com/", headers: { content-type: "application/x-www-form-urlencoded; charset=iso-8859-1" }, body: "Action=DescribeInstances" }
  expect: { tool: ec2_call }
- name: charset utf-8 is fine
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://ec2.eu-west-1.amazonaws.com/", headers: { content-type: "application/x-www-form-urlencoded; charset=UTF-8" }, body: "Action=DescribeInstances" }
  expect: { tool: ec2_describe }
- name: compressed body
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://ec2.eu-west-1.amazonaws.com/", headers: { content-type: application/x-www-form-urlencoded, content-encoding: gzip }, body: "Action=DescribeInstances" }
  expect: { tool: ec2_call }
- name: truncated at the condition buffer
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://ec2.eu-west-1.amazonaws.com/", headers: { content-type: application/x-www-form-urlencoded }, body: "Action=DescribeInstances", body_truncated: true }
  expect: { tool: ec2_call }
- name: GET Describe is not the POST read tool (query is not a form condition)
  connection: { credentials: { api_token: tok } }
  request: { method: GET, url: "https://ec2.eu-west-1.amazonaws.com/?Action=DescribeInstances" }
  expect: { tool: ec2_call }
- name: header name is case insensitive
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://dynamodb.eu-west-1.amazonaws.com/", headers: { x-amz-target: "DynamoDB_20120810.QueryItems" } }
  expect: { tool: ddb_query }
"#;
    run_cases(&y);
}

#[test]
fn a_query_source_tool_fails_closed_when_the_request_has_a_body() {
    let post = |ct: &str, body: &str, extra: &str, name: &str, expect: &str| {
        format!(
            "- name: {name}\n{CONN}  request: {{ method: POST, url: \"https://q.example.com/api?Action=ListThings\", headers: {{ {ct} }}, body: \"{body}\"{extra} }}\n  expect: {expect}\n"
        )
    };
    let form = "content-type: application/x-www-form-urlencoded";
    let none = "{ app: aws, no_tool: true }";
    let mut y = String::new();
    // an unrelated urlencoded body does not hide anything: still the tool
    y += &post(
        form,
        "other=1",
        "",
        "unrelated form body",
        "{ tool: q_list }",
    );
    // the same parameter in the body: conflicting value, equal value
    y += &post(
        form,
        "Action=DeleteThings",
        "",
        "conflicting form body",
        none,
    );
    y += &post(
        form,
        "Action=ListThings",
        "",
        "equal value in the body",
        none,
    );
    // a body that cannot be checked
    y += &post(
        "content-type: application/json",
        "{}",
        "",
        "non-form body",
        none,
    );
    y += &post(
        "content-type: application/x-www-form-urlencoded, content-encoding: gzip",
        "other=1",
        "",
        "compressed body",
        none,
    );
    y += &post(
        form,
        "other=1",
        ", body_truncated: true",
        "oversized body",
        none,
    );
    run_cases(&y);
}

#[test]
fn credentials_cookies_and_injected_headers_are_never_a_source() {
    // The classifier never reads credentials, cookies or injected headers: a request carrying
    // the parameter only in `authorization` or `cookie` matches nothing.
    let y = r#"
- name: parameter hidden in cookie and authorization
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://dynamodb.eu-west-1.amazonaws.com/", headers: { cookie: "x-amz-target=DynamoDB_20120810.Query", authorization: "DynamoDB_20120810.Query" } }
  expect: { app: aws, no_tool: true }
- name: the agent's own header cannot make the injected bearer a source
  connection: { credentials: { api_token: "DynamoDB_20120810.Query" } }
  request: { method: POST, url: "https://dynamodb.eu-west-1.amazonaws.com/" }
  expect: { app: aws, no_tool: true }
"#;
    run_cases(y);
}

fn loaded() -> Loaded {
    let v: Value = serde_json::from_slice(&snapshot_json()).unwrap();
    parse("h", v).unwrap()
}

#[test]
fn the_body_is_buffered_only_when_a_form_params_tool_may_match() {
    let l = loaded();
    let app = |host: &str, path: &str| identify_app(&l, host, 443, path).unwrap();
    // A form tool (ec2_describe, sts_who) may match: body needed.
    let m = app("ec2.eu-west-1.amazonaws.com", "/");
    assert!(needs_body(
        &m,
        "ec2.eu-west-1.amazonaws.com",
        443,
        "/",
        "POST"
    ));
    // Same host, a method no form tool serves, or another path: nothing to read.
    assert!(!needs_body(
        &m,
        "ec2.eu-west-1.amazonaws.com",
        443,
        "/",
        "DELETE"
    ));
    assert!(!needs_body(
        &m,
        "ec2.eu-west-1.amazonaws.com",
        443,
        "/other",
        "POST"
    ));
    // A header-only or query-only tool never needs the body.
    let m = app("dynamodb.eu-west-1.amazonaws.com", "/");
    assert!(!needs_body(
        &m,
        "dynamodb.eu-west-1.amazonaws.com",
        443,
        "/",
        "POST"
    ));
    let m = app("q.example.com", "/api");
    // A query-only tool must prove the body does not carry the parameter too (SEC-68): buffered.
    assert!(needs_body(&m, "q.example.com", 443, "/api", "GET"));
    // A two-condition tool with a form condition does.
    assert!(needs_body(&m, "q.example.com", 443, "/two", "POST"));
}

#[test]
fn a_snapshot_with_a_classifier_this_gateway_does_not_know_is_refused() {
    let good: Value = serde_json::from_slice(&snapshot_json()).unwrap();
    assert!(parse("h", good.clone()).is_ok());
    // Unknown source (a newer classifier) or an unknown key inside a condition.
    let mut bad = good.clone();
    bad["apps"][0]["tools"]["read"]["items"][0]["params"][0]["source"] = "json_body".into();
    assert!(parse("h", bad).is_err());
    let mut bad = good.clone();
    bad["apps"][0]["tools"]["read"]["items"][0]["params"][0]["regex"] = "x".into();
    assert!(parse("h", bad).is_err());
    // An empty or over-long list is not a classifier this gateway can run: refused, so the tool
    // is never run as a path-only tool.
    let mut bad = good.clone();
    bad["apps"][0]["tools"]["read"]["items"][0]["params"] = Value::Array(vec![]);
    assert!(parse("h", bad).is_err());
    let mut bad = good.clone();
    let c = bad["apps"][0]["tools"]["read"]["items"][0]["params"][0].clone();
    bad["apps"][0]["tools"]["read"]["items"][0]["params"] = Value::Array(vec![c; 5]);
    assert!(parse("h", bad).is_err());
}
