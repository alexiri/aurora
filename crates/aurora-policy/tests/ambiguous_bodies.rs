//! The decision is made on a parsed body, the receiver gets the bytes. A body that two parsers
//! may read differently has no single answer: a condition on it is unknown (which applies a
//! restrictive rule and never an allow), and a GraphQL request is a mutation.
#![allow(clippy::unwrap_used)]

use aurora_policy::graphql::{GraphqlKind, classify};
use aurora_policy::rule::{Condition, ConditionOn, ConditionOp, Tri};
use aurora_policy::{RequestFacts, RequestIdentity};

fn eval(op: ConditionOp, pointer: &str, value: &str, body: &str) -> Tri {
    let facts: RequestFacts = serde_json::from_value(serde_json::json!({
        "host": "api.example.com", "port": 443, "path": "/x", "method": "POST",
        "identity": serde_json::to_value(RequestIdentity::default()).unwrap(),
    }))
    .map(|mut f: RequestFacts| {
        f.body_prefix = body.as_bytes().to_vec();
        f
    })
    .unwrap();
    Condition {
        on: ConditionOn::Body,
        op,
        value: value.to_owned(),
        json_pointer: Some(pointer.to_owned()),
    }
    .eval(&facts)
}

/// `serde_json`'s own `Value` parser reads an object with this one member as the JSON written
/// in the string. No receiver does.
const RAW: &str = "$serde_json::private::RawValue";

#[test]
fn a_private_raw_value_wrapper_is_not_unwrapped() {
    // The receiver sees one member with a strange name, and no `channel`.
    let body = format!(r##"{{"{RAW}":"{{\"channel\":\"#safe\"}}"}}"##);
    assert_eq!(
        eval(ConditionOp::Equals, "/channel", "#safe", &body),
        Tri::False
    );
    // Nested: the receiver's `mode` is an object, not the string "safe".
    let body = format!(r#"{{"mode":{{"{RAW}":"\"safe\""}}}}"#);
    assert_eq!(
        eval(ConditionOp::Equals, "/mode", "safe", &body),
        Tri::False
    );
}

#[test]
fn a_repeat_inside_the_selected_value_is_unknown() {
    // A first-wins receiver reads "evil", `Value` reads "ok".
    let body = r#"{"a":{"x":"evil","x":"ok"}}"#;
    assert_eq!(
        eval(ConditionOp::NotContains, "/a", "evil", body),
        Tri::Unknown
    );
    let body = r#"{"a":[{"deep":{"x":"evil","x":"ok"}}]}"#;
    assert_eq!(
        eval(ConditionOp::NotContains, "/a", "evil", body),
        Tri::Unknown
    );
    // The whole document, selected by the empty pointer.
    let body = r#"{"x":"evil","x":"ok"}"#;
    assert_eq!(
        eval(ConditionOp::NotContains, "", "evil", body),
        Tri::Unknown
    );
}

#[test]
fn a_name_that_differs_only_in_case_is_unknown() {
    // A receiver that matches field names without case reads the other spelling.
    for body in [
        r##"{"channel":"#safe","Channel":"#evil"}"##,
        r##"{"CHANNEL":"#evil","channel":"#safe"}"##,
        r##"{"Channel":"#evil"}"##,
    ] {
        assert_eq!(
            eval(ConditionOp::NotContains, "/channel", "#evil", body),
            Tri::Unknown,
            "{body}"
        );
    }
    // The long s folds to `s` and the Kelvin sign to `k`.
    let body = "{\"to\":\"a\",\"ta\u{17f}k\":\"x\"}";
    assert_eq!(eval(ConditionOp::Equals, "/task", "x", body), Tri::Unknown);
    // A case variant off the path decides nothing.
    let body = r#"{"a":{"b":"y"},"other":1,"Other":2}"#;
    assert_eq!(eval(ConditionOp::Equals, "/a/b", "y", body), Tri::True);
}

#[test]
fn a_whole_number_not_written_as_an_integer_is_unknown() {
    // 1e2, 100.0 and 100 are one number to a receiver and three texts here.
    for n in ["1e2", "100.0", "1.0e2", "100.000000000000000000001"] {
        let body = format!(r#"{{"amount":{n}}}"#);
        assert_eq!(
            eval(ConditionOp::Equals, "/amount", "100", &body),
            Tri::Unknown,
            "{n}"
        );
    }
    let body = r#"{"a":{"amount":1e2}}"#;
    assert_eq!(eval(ConditionOp::Contains, "/a", "100", body), Tri::Unknown);
    // Integers and true fractions keep their one reading.
    assert_eq!(
        eval(ConditionOp::Equals, "/amount", "100", r#"{"amount":100}"#),
        Tri::True
    );
    assert_eq!(
        eval(ConditionOp::Equals, "/amount", "0.5", r#"{"amount":5e-1}"#),
        Tri::True
    );
}

#[test]
fn unicode_escapes_are_one_spelling() {
    let body = r#"{"a":"x","a":"y"}"#;
    assert_eq!(eval(ConditionOp::Equals, "/a", "y", body), Tri::Unknown);
    let body = r#"{"a":"delete"}"#;
    assert_eq!(eval(ConditionOp::Equals, "/a", "delete", body), Tri::True);
}

fn gql(body: &str) -> GraphqlKind {
    classify(body.as_bytes(), false)
}

#[test]
fn a_graphql_body_with_two_readings_is_a_mutation() {
    for body in [
        r#"{"query":"mutation { a }","query":"{ a }"}"#.to_owned(),
        r#"{"query":"query Q { a } mutation M { b }","operationName":"M","operationName":"Q"}"#
            .to_owned(),
        // A receiver that matches names without case takes the other one.
        r#"{"query":"{ a }","Query":"mutation { b }"}"#.to_owned(),
        r#"{"query":"query Q { a } mutation M { b }","operationName":"Q","OperationName":"M"}"#
            .to_owned(),
        r#"[{"query":"{ a }"},{"query":"mutation { b }","query":"{ b }"}]"#.to_owned(),
        // The receiver sees no `query` here at all.
        format!(r#"{{"{RAW}":"{{\"query\":\"{{ a }}\"}}"}}"#),
    ] {
        assert_eq!(gql(&body), GraphqlKind::Mutation, "{body}");
    }
    // The variables decide nothing about the kind of operation.
    for body in [
        r#"{"query":"{ a }","variables":{"Query":1}}"#,
        r#"{"query":"{ a }","variables":{"id":1,"id":2}}"#,
    ] {
        assert_eq!(gql(body), GraphqlKind::Query, "{body}");
    }
}
