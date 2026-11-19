//! (P5-GUIDE-6, `docs/24 §5.4`, C-652, `18 §App guides on demand`) `api.json` under `insta`:
//! package operations, catalogue entries, the gateway's matcher for `tool`, access per account,
//! the drop order and database apps.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::collections::BTreeSet;

use aurora_registry::agent_guide::{GrantAccess, GuideAccount, render_with_operations};
use aurora_registry::operations;

use super::agent_guide::{AGENT, FILES, TOOLS, snapshot_of, yaml};

const OPS: &str = r#"{"format":"aurora.operations/1","operations":[
 {"id":"list_things","method":"GET","path":"/v1/things","summary":"All things","tags":["things"],
  "params":[{"name":"q","in":"query","type":"string","description":"Filter"}],
  "pagination":{"style":"page_param","param":"page","start":1},"items":"$.data"},
 {"id":"fetch_thing","method":"GET","path":"/v1/things/{id}","summary":"One thing","tags":["things"],
  "params":[{"name":"id","in":"path","required":true,"type":"string"}]},
 {"id":"poke","method":"POST","path":"/v1/poke","summary":"Not in the catalogue",
  "body":{"content_type":"application/json","fields":[{"name":"x","type":"string"}]}}
]}"#;

fn acct(id: &str, host: Option<&str>, access: GrantAccess) -> GuideAccount {
    GuideAccount {
        id: id.into(),
        label: format!("Label {id}"),
        host: host.map(Into::into),
        service: None,
        access,
    }
}

fn custom(allow: &[&str], ask: &[&str]) -> GrantAccess {
    let set = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>();
    GrantAccess::Custom {
        allow: set(allow),
        ask: set(ask),
    }
}

fn api(ops: Option<&str>, accounts: &[GuideAccount]) -> String {
    let s = snapshot_of(&yaml(TOOLS, AGENT, ""), &FILES);
    let parsed = ops.map(|o| operations::parse(o.as_bytes()).unwrap());
    let r = render_with_operations(&s.apps[0], &[], accounts, "1.2.3", true, parsed.as_ref());
    let j = r.api_json.expect("api.json");
    // Pretty for a readable snapshot.
    serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(&j).unwrap()).unwrap()
}

#[test]
fn package_operations_map_to_the_catalogue_by_the_gateways_matcher() {
    let a = [
        acct("c_1", Some("api.example.test"), GrantAccess::Full),
        acct(
            "c_2",
            Some("api.example.test"),
            custom(&["list_things"], &["make_thing"]),
        ),
    ];
    insta::assert_snapshot!("api_json_two_accounts", api(Some(OPS), &a));
}

#[test]
fn tool_is_found_by_the_matcher_not_stated() {
    let a = [acct("c_1", Some("api.example.test"), GrantAccess::Full)];
    let v: serde_json::Value = serde_json::from_str(&api(Some(OPS), &a)).unwrap();
    let ops = v["operations"].as_array().unwrap();
    let tool = |id: &str| ops.iter().find(|o| o["id"] == id).unwrap()["tool"].clone();
    assert_eq!(tool("list_things"), "list_things");
    // `{id}` becomes a placeholder segment the catalogue's `/v1/things/*` covers.
    assert_eq!(tool("fetch_thing"), "get_thing");
    // `make_thing` is POST /v1/things; `poke` is a different path: untooled.
    assert!(tool("poke").is_null());
    // The catalogue tool no operation maps to is listed as a catalogue entry.
    let cat: Vec<_> = ops.iter().filter(|o| o["source"] == "catalogue").collect();
    assert_eq!(cat.len(), 1);
    assert_eq!(cat[0]["id"], "make_thing");
}

#[test]
fn without_an_operations_file_the_catalogue_is_the_api() {
    let a = [acct("c_1", Some("api.example.test"), GrantAccess::Full)];
    insta::assert_snapshot!("api_json_catalogue_only", api(None, &a));
}

#[test]
fn rendering_is_deterministic() {
    let a = [acct("c_1", Some("api.example.test"), GrantAccess::Full)];
    assert_eq!(api(Some(OPS), &a), api(Some(OPS), &a));
    // Key order of C-652 in the bytes themselves (the snapshot sorts keys).
    let s = snapshot_of(&yaml(TOOLS, AGENT, ""), &FILES);
    let parsed = operations::parse(OPS.as_bytes()).unwrap();
    let raw = render_with_operations(&s.apps[0], &[], &a, "1", true, Some(&parsed))
        .api_json
        .unwrap();
    assert!(
        raw.starts_with(r#"{"format":"aurora.app-api/1","app":{"id":"demo-app","name":"Demo app","version":"1"},"accounts":["#),
        "{}",
        &raw[..120]
    );
    assert!(raw.ends_with('\n'));
}

#[test]
fn an_oversized_api_json_drops_descriptions_then_params_and_says_so() {
    let mut ops = String::from(r#"{"format":"aurora.operations/1","operations":["#);
    let long = "d".repeat(190);
    for i in 0..400 {
        if i > 0 {
            ops.push(',');
        }
        let params: Vec<String> = (0..64)
            .map(|p| {
                format!(r#"{{"name":"p{p}","in":"query","type":"string","description":"{long}"}}"#)
            })
            .collect();
        ops.push_str(&format!(
            r#"{{"id":"op_{i}","method":"GET","path":"/v1/op{i}","summary":"s","tags":["t{}"],"params":[{}]}}"#,
            i % 4,
            params.join(",")
        ));
    }
    ops.push_str("]}");
    let a = [acct("c_1", Some("api.example.test"), GrantAccess::Full)];
    let s = snapshot_of(&yaml(TOOLS, AGENT, ""), &FILES);
    let parsed = operations::parse(ops.as_bytes()).unwrap();
    let r = render_with_operations(&s.apps[0], &[], &a, "1", true, Some(&parsed));
    let j = r.api_json.unwrap();
    assert!(
        j.len() <= aurora_core::limits::AGENT_API_JSON_MAX,
        "{}",
        j.len()
    );
    let v: serde_json::Value = serde_json::from_str(&j).unwrap();
    assert_eq!(v["truncated"], true);
}

#[test]
fn a_database_app_has_no_api_json() {
    let mut s = snapshot_of(&yaml(TOOLS, AGENT, ""), &FILES);
    s.apps[0].database = Some(aurora_registry::manifest::Database {
        engine: "postgres".into(),
    });
    let r = render_with_operations(&s.apps[0], &[], &[], "1", true, None);
    assert!(r.api_json.is_none());
}
