//! (P5-GUIDE-6, `docs/24 §3.5`, C-651, `18 §App guides on demand`) The operations-file linter
//! corpus (one case per violation code), the reserved app id, the snapshot fragment and its
//! compatibility.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use aurora_registry::validate::agent_rules::archive_rules;
use aurora_registry::{Manifest, Package};

use super::agent_guide::{TOOLS, snapshot_of, yaml};

const AGENT_OPS: &str =
    "    agent:\n      summary: \"Things.\"\n      operations: operations.json\n";

fn package(files: &[(&str, &[u8])]) -> Package {
    let mut pkg = Package::default();
    for (f, c) in files {
        pkg.files.insert(format!("agent/demo-app/{f}"), c.to_vec());
    }
    pkg
}

fn lint(y: &str, ops: &str) -> Vec<&'static str> {
    let m = Manifest::parse(y, "99.0.0").unwrap();
    let pkg = package(&[("operations.json", ops.as_bytes())]);
    let mut v = aurora_registry::validate::manifest(&m);
    archive_rules(&m, &pkg, &mut v);
    let mut codes: Vec<&'static str> = v
        .iter()
        .map(|x| x.code)
        .filter(|c| c.starts_with("agent_") || *c == "app_id_reserved")
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

fn one(op: &str) -> String {
    format!("{{\"format\":\"aurora.operations/1\",\"operations\":[{op}]}}")
}

fn op(extra: &str) -> String {
    format!(
        "{{\"id\":\"list_things\",\"method\":\"GET\",\"path\":\"/v1/things\",\"summary\":\"All things\"{extra}}}"
    )
}

fn codes_for(ops: &str) -> Vec<&'static str> {
    lint(&yaml(TOOLS, AGENT_OPS, ""), ops)
}

#[test]
fn a_valid_operations_file_has_no_violations() {
    let full = op(
        ",\"tags\":[\"things\"],\"host\":\"api.example.test\",\"items\":\"$.data\",\"tool\":\"list_things\",\
         \"params\":[{\"name\":\"q\",\"in\":\"query\",\"type\":\"string\",\"enum\":[\"a\",\"b\"],\"default\":\"a\"},\
         {\"name\":\"X-Trace\",\"in\":\"header\",\"type\":\"string\"},\
         {\"name\":\"tags\",\"in\":\"query\",\"type\":\"array\"}],\
         \"pagination\":{\"style\":\"cursor\",\"param\":\"cursor\",\"next\":\"$.next\"}",
    );
    assert!(codes_for(&one(&full)).is_empty());
    let with_path = "{\"id\":\"get_thing\",\"method\":\"GET\",\"path\":\"/v1/things/{id}\",\"summary\":\"One\",\
         \"params\":[{\"name\":\"id\",\"in\":\"path\",\"type\":\"string\",\"required\":true}]}";
    let body = "{\"id\":\"make_thing\",\"method\":\"POST\",\"path\":\"/v1/things\",\"summary\":\"Make\",\
         \"body\":{\"content_type\":\"application/json\",\"fields\":[{\"name\":\"name\",\"type\":\"string\",\"required\":true}]},\
         \"pagination\":{\"style\":\"page_param\",\"param\":\"page\",\"start\":1,\"size_param\":\"per_page\"}}";
    let link = "{\"id\":\"all_things\",\"method\":\"GET\",\"path\":\"/v1/all\",\"summary\":\"All\",\"pagination\":{\"style\":\"link_header\"}}";
    assert!(codes_for(&one(&format!("{with_path},{body},{link}"))).is_empty());
}

#[test]
fn every_violation_code_is_reachable() {
    let big_param = |extra: &str| {
        one(&op(&format!(
            ",\"params\":[{{\"name\":\"q\",\"in\":\"query\",\"type\":\"string\"{extra}}}]"
        )))
    };
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "format",
            "{\"format\":\"aurora.operations/2\",\"operations\":[]}".into(),
            "agent_operations_format",
        ),
        ("json", "not json".into(), "agent_operations_json"),
        (
            "unknown top key",
            "{\"format\":\"aurora.operations/1\",\"operations\":[],\"x\":1}".into(),
            "agent_operations_unknown_key",
        ),
        (
            "unknown op key",
            one(&op(",\"extra\":1")),
            "agent_operations_unknown_key",
        ),
        (
            "unknown param key",
            big_param(",\"x\":1"),
            "agent_operations_unknown_key",
        ),
        (
            "empty list",
            "{\"format\":\"aurora.operations/1\",\"operations\":[]}".into(),
            "agent_operations_too_many",
        ),
        (
            "id",
            one(&op("").replace("list_things", "List-Things")),
            "agent_operations_id",
        ),
        (
            "duplicate id",
            one(&format!("{0},{0}", op(""))),
            "agent_operations_duplicate_id",
        ),
        (
            "method",
            one(&op("").replace("GET", "TRACE")),
            "agent_operations_method",
        ),
        (
            "query in path",
            one(&op("").replace("/v1/things", "/v1/things?a=1")),
            "agent_operations_path",
        ),
        (
            "dot-dot",
            one(&op("").replace("/v1/things", "/v1/../things")),
            "agent_operations_path",
        ),
        (
            "double slash",
            one(&op("").replace("/v1/things", "/v1//things")),
            "agent_operations_path",
        ),
        (
            "relative",
            one(&op("").replace("/v1/things", "v1/things")),
            "agent_operations_path",
        ),
        (
            "undeclared template",
            one(&op("").replace("/v1/things", "/v1/{id}")),
            "agent_operations_path_param",
        ),
        (
            "declared but unused path param",
            one(&op(
                ",\"params\":[{\"name\":\"id\",\"in\":\"path\",\"type\":\"string\"}]",
            )),
            "agent_operations_path_param",
        ),
        (
            "foreign host",
            one(&op(",\"host\":\"evil.example.test\"")),
            "agent_operations_host",
        ),
        (
            "param name",
            big_param("").replace("\"q\"", "\"a b\""),
            "agent_operations_param",
        ),
        (
            "param type",
            big_param("").replace("\"string\"", "\"object\""),
            "agent_operations_json",
        ),
        (
            "param enum",
            big_param(",\"enum\":[[1]]"),
            "agent_operations_param",
        ),
        (
            "header Authorization",
            one(&op(
                ",\"params\":[{\"name\":\"Authorization\",\"in\":\"header\",\"type\":\"string\"}]",
            )),
            "agent_operations_header",
        ),
        (
            "header x-aurora",
            one(&op(
                ",\"params\":[{\"name\":\"X-Aurora-Connection-Id\",\"in\":\"header\",\"type\":\"string\"}]",
            )),
            "agent_operations_header",
        ),
        (
            "body content type",
            one(&op(",\"body\":{\"content_type\":\"text/plain\"}")),
            "agent_operations_body",
        ),
        (
            "pagination param",
            one(&op(
                ",\"pagination\":{\"style\":\"page_param\",\"param\":\"a b\"}",
            )),
            "agent_operations_pagination",
        ),
        (
            "cursor json path",
            one(&op(
                ",\"pagination\":{\"style\":\"cursor\",\"param\":\"c\",\"next\":\"$..next\"}",
            )),
            "agent_operations_json_path",
        ),
        (
            "items json path",
            one(&op(",\"items\":\"$[?(@.a)]\"")),
            "agent_operations_json_path",
        ),
        (
            "tool",
            one(&op(",\"tool\":\"no_such_tool\"")),
            "agent_operations_tool",
        ),
    ];
    for (name, ops, want) in cases {
        let got = codes_for(&ops);
        assert!(got.contains(&want), "{name}: wanted {want}, got {got:?}");
    }
}

#[test]
fn an_inject_stage_header_is_forbidden_from_the_hosts_not_a_fixed_list() {
    // `yaml` injects only `bearer`; a package whose host sets `x-api-key` forbids that name.
    let y = yaml(TOOLS, AGENT_OPS, "").replace(
        "- bearer: \"{credentials.api_token}\"",
        "- header: { name: X-Api-Key, value: \"{credentials.api_token}\" }",
    );
    let h = one(&op(
        ",\"params\":[{\"name\":\"x-api-key\",\"in\":\"header\",\"type\":\"string\"}]",
    ));
    assert!(lint(&y, &h).contains(&"agent_operations_header"));
    let free = one(&op(
        ",\"params\":[{\"name\":\"X-Other\",\"in\":\"header\",\"type\":\"string\"}]",
    ));
    assert!(lint(&y, &free).is_empty());
}

#[test]
fn too_many_and_too_large_files() {
    let many: Vec<String> = (0..2001)
        .map(|i| {
            format!(
                "{{\"id\":\"op_{i}\",\"method\":\"GET\",\"path\":\"/v1/x{i}\",\"summary\":\"s\"}}"
            )
        })
        .collect();
    assert!(codes_for(&one(&many.join(","))).contains(&"agent_operations_too_many"));
    let big = one(&op(&format!(
        ",\"tags\":[],\"items\":\"$\",\"tool\":null,\"pad\":\"{}\"",
        "x".repeat(520 * 1024)
    )));
    assert!(codes_for(&big).contains(&"agent_operations_too_large"));
}

#[test]
fn the_operations_file_is_named_and_present() {
    // (P5-APPSKILL-1) `agent_operations_listed` went with `agent.guide`, which is ignored now.
    // not a .json name
    let y = yaml(
        TOOLS,
        "    agent:\n      summary: \"S\"\n      operations: ops.md\n",
        "",
    );
    assert!(lint(&y, &one(&op(""))).contains(&"agent_operations_file_name"));
    // the summary is required, the file must be in the archive
    let y = yaml(TOOLS, "    agent:\n      operations: operations.json\n", "");
    assert!(lint(&y, &one(&op(""))).contains(&"agent_summary_required"));
    let m = Manifest::parse(&yaml(TOOLS, AGENT_OPS, ""), "99.0.0").unwrap();
    let mut v = Vec::new();
    archive_rules(&m, &package(&[]), &mut v);
    assert!(v.iter().any(|x| x.code == "agent_operations_missing_file"));
}

#[test]
fn the_app_id_aurora_is_reserved() {
    let y = yaml(TOOLS, "", "").replace("id: demo-app", "id: aurora");
    assert!(lint(&y, "{}").contains(&"app_id_reserved"));
}

#[test]
fn the_snapshot_records_the_operations_file_and_a_package_without_one_is_unchanged() {
    let ops = one(&format!(
        "{},{}",
        op(""),
        op("").replace("list_things", "second")
    ));
    let s = snapshot_of(
        &yaml(TOOLS, AGENT_OPS, ""),
        &[("operations.json", ops.as_str())],
    );
    let ag = s.apps[0].agent.as_ref().unwrap();
    let o = ag.operations.as_ref().unwrap();
    assert_eq!(o.file, "operations.json");
    assert_eq!(o.count, 2);
    assert_eq!(o.size, ops.len() as u64);
    assert_eq!(o.sha256.len(), 64);
    // No operations: the key is absent from the serialized fragment (same bytes as before).
    let s2 = snapshot_of(
        &yaml(TOOLS, "    agent:\n      instructions: hi\n", ""),
        &[],
    );
    let json = serde_json::to_string(&s2.apps[0].agent).unwrap();
    assert!(!json.contains("operations"), "{json}");
}
