//! (P5-GUIDE-6, `docs/24 §5.4`, C-652, `18 §App guides on demand`) `api.json` under `insta`:
//! package operations, catalogue entries, the gateway's matcher for `tool`, access per account,
//! the drop order and database apps.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::collections::BTreeSet;

use aurora_registry::agent_guide::{GrantAccess, GuideAccount, render_with_operations};
use aurora_registry::operations;

use super::agent_guide::{AGENT, TOOLS, snapshot_of, yaml};

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
        webhook: None,
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
    let s = snapshot_of(&yaml(TOOLS, AGENT, ""), &[]);
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
    let s = snapshot_of(&yaml(TOOLS, AGENT, ""), &[]);
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
    let s = snapshot_of(&yaml(TOOLS, AGENT, ""), &[]);
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
    let mut s = snapshot_of(&yaml(TOOLS, AGENT, ""), &[]);
    s.apps[0].database = Some(aurora_registry::manifest::Database {
        engine: "postgres".into(),
    });
    let r = render_with_operations(&s.apps[0], &[], &[], "1", true, None);
    assert!(r.api_json.is_none());
}

// ---- P5-WEBHOOK-3 (C-911): a webhook is an account-bound operation ----

fn slots(v: serde_json::Value) -> Vec<aurora_webhook::Slot> {
    serde_json::from_value(v).unwrap()
}

fn webhook_acct(id: &str, name: &str, notes: &str, s: serde_json::Value) -> GuideAccount {
    GuideAccount {
        webhook: Some(aurora_registry::agent_guide::GuideWebhook {
            name: name.into(),
            description: notes.into(),
            method: "POST".into(),
            slots: slots(s),
        }),
        ..acct(id, None, GrantAccess::Full)
    }
}

fn all_slots() -> serde_json::Value {
    serde_json::json!([
        {"name":"text","type":"string","required":true,"description":"The message","max_length":200},
        {"name":"level","type":"enum","required":false,"enum":["info","warn"]},
        {"name":"count","type":"integer","required":false},
        {"name":"ratio","type":"number","required":false},
        {"name":"urgent","type":"boolean","required":false},
        {"name":"tags","type":"string_list","required":false,"max_items":5,"max_length":20},
        {"name":"card","type":"json","required":false}
    ])
}

fn webhook_api(accounts: &[GuideAccount]) -> serde_json::Value {
    serde_json::from_str(&api(None, accounts)).unwrap()
}

#[test]
fn a_webhook_account_is_one_account_bound_operation() {
    let a = [webhook_acct(
        "c_1",
        "Ops alerts",
        "Deploy results.\nSecond line.",
        all_slots(),
    )];
    insta::assert_snapshot!("api_json_webhook", api(None, &a));
    let v = webhook_api(&a);
    let op = v["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "send_ops_alerts")
        .unwrap();
    assert_eq!(op["account"], "A");
    assert_eq!(op["notes"], "Deploy results. Second line.");
    assert_eq!(op["source"], "connection");
    assert_eq!(op["host"], "c_1.link.aurora.internal");
    assert_eq!(
        v["accounts"][0]["base_url"],
        "https://c_1.link.aurora.internal"
    );
    let ty = |n: &str| {
        op["body"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == n)
            .unwrap()["type"]
            .clone()
    };
    assert_eq!(ty("text"), "string");
    assert_eq!(ty("level"), "string");
    assert_eq!(ty("count"), "integer");
    assert_eq!(ty("ratio"), "number");
    assert_eq!(ty("urgent"), "boolean");
    assert_eq!(ty("tags"), "array");
    assert_eq!(ty("card"), "json");
    assert_eq!(op["params"][0]["name"], "X-Aurora-Dry-Run");
    assert_eq!(op["params"][0]["enum"][0], "1");
    // The `send` tool is not listed twice.
    assert!(
        v["operations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["source"] != "catalogue" || o["id"] != "send")
    );
}

#[test]
fn webhook_ids_take_a_suffix_in_account_order_and_every_other_operation_has_null_keys() {
    let a = [
        webhook_acct("c_1", "Team X!", "", serde_json::json!([])),
        webhook_acct("c_2", "team   x", "", serde_json::json!([])),
        webhook_acct("c_3", "Team X", "", serde_json::json!([])),
    ];
    let v: serde_json::Value = serde_json::from_str(&api(Some(OPS), &a)).unwrap();
    let ids: Vec<&str> = v["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["source"] == "connection")
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["send_team_x", "send_team_x_2", "send_team_x_3"]);
    for o in v["operations"].as_array().unwrap() {
        if o["source"] != "connection" {
            assert!(o["account"].is_null() && o["notes"].is_null(), "{o}");
        }
    }
    // Each operation carries its own letter's access only.
    let b = v["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "send_team_x_2")
        .unwrap();
    assert_eq!(b["account"], "B");
    assert_eq!(b["access"].as_object().unwrap().len(), 1);
    assert!(b["access"]["B"].is_string());
}

#[test]
fn webhook_notes_are_clamped_and_rendering_is_deterministic() {
    let long = "n".repeat(3000);
    let a = [webhook_acct("c_1", "Long", &long, serde_json::json!([]))];
    let v = webhook_api(&a);
    let op = v["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "send_long")
        .unwrap();
    assert!(op["notes"].as_str().unwrap().chars().count() <= 1000);
    assert_eq!(api(None, &a), api(None, &a));
}

/// The render input has no field for the template, headers, URL or secret values: the type's
/// serialization is exactly name, description, method, slots; and nothing of a connection's secret
/// material reaches either file.
#[test]
fn the_webhook_render_input_has_no_template_headers_url_or_secret_field() {
    let w = aurora_registry::agent_guide::GuideWebhook {
        name: "n".into(),
        description: "d".into(),
        method: "POST".into(),
        slots: vec![],
    };
    let v = serde_json::to_value(&w).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["description", "method", "name", "slots"]);
}

#[test]
fn tools_md_lists_each_webhook_with_its_slots_and_a_curl_example() {
    let s = snapshot_of(&yaml(TOOLS, AGENT, ""), &[]);
    let a = [webhook_acct(
        "c_1",
        "Ops alerts",
        "Deploy results.",
        all_slots(),
    )];
    let r = render_with_operations(&s.apps[0], &[], &a, "1", true, None);
    insta::assert_snapshot!("tools_md_webhook", r.tools_md);
    assert_eq!(
        r.tools_md,
        render_with_operations(&s.apps[0], &[], &a, "1", true, None).tools_md
    );
}
