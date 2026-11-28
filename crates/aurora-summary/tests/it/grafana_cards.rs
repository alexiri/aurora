//! The built-in Grafana alert-rule cards, rendered from the package's own manifest: which
//! rows each kind of rule shows, and that a log query as long as the gateway accepts is
//! shown whole.
use aurora_core::limits::SUMMARY_VALUE_MAX;
use aurora_registry::manifest::Manifest;
use aurora_summary::template::{Input, summarize};
use serde_json::{Value, json};

fn manifest() -> Manifest {
    let path = format!(
        "{}/../../plugins/builtin/grafana/plugin.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    Manifest::parse(&std::fs::read_to_string(path).unwrap(), "9.9.9").unwrap()
}

fn card(tool: &str, body: &Value) -> Vec<(String, String)> {
    let m = manifest();
    let approval = m.apps[0].approvals.get(tool).unwrap();
    let body = serde_json::to_vec(body).unwrap();
    let header = |_: &str| None;
    let inp = Input {
        host: "grafana.example.org",
        base_path: None,
        path: "/api/v1/provisioning/alert-rules/rule-1",
        query: None,
        header: &header,
        body_prefix: &body,
        truncated: false,
        declared_hosts: &[],
        web_hosts: &[],
        injected_headers: &[],
    };
    summarize(approval, &inp)
        .fields
        .into_iter()
        .map(|f| (f.label, f.value))
        .collect()
}

fn rule(model: Value) -> Value {
    let step = |model: Value| {
        json!({ "datasourceUid": "ds-1", "relativeTimeRange": { "from": 960, "to": 60 },
                "model": model })
    };
    json!({
        "title": "t", "folderUID": "f", "ruleGroup": "g", "condition": "C", "for": "120s",
        "data": [
            step(model),
            step(json!({ "reducer": "sum" })),
            step(json!({ "conditions": [{ "evaluator": { "type": "within", "params": [1, 5] } }] })),
        ],
        "noDataState": "OK", "execErrState": "Error", "isPaused": false,
        "keep_firing_for": "0s", "missingSeriesEvalsToResolve": 2,
        "labels": { "managed_by": "night", "service": "web", "expires": "2026-12-02" },
        "annotations": { "summary": "t", "expires": "2026-12-02" },
        "notification_settings": { "receiver": "oncall" }
    })
}

fn prometheus() -> Value {
    rule(json!({ "editorMode": "code", "expr": "up", "refId": "A" }))
}

fn logs(query: &str) -> Value {
    rule(json!({
        "refId": "A", "queryType": "lucene", "query": query, "timeField": "@timestamp",
        "metrics": [{ "id": "1", "type": "sum", "field": "ms" }],
        "bucketAggs": [{ "id": "2", "type": "date_histogram", "field": "@timestamp",
                         "settings": { "interval": "1m", "min_doc_count": "0" } }]
    }))
}

const HEAD: [&str; 5] = ["Title", "Folder", "Group", "Evaluate for", "Data source"];
const LOG_ROWS: [&str; 7] = [
    "Query language",
    "Log query",
    "Metric",
    "Metric field",
    "Buckets",
    "Bucket field",
    "Time field",
];
const TAIL: [&str; 15] = [
    "Looks back (seconds)",
    "Ends before now (seconds)",
    "Comparison",
    "Threshold",
    "Reducer",
    "Second threshold",
    "Condition step",
    "No data state",
    "On query error",
    "Paused",
    "Managed by",
    "Service",
    "Expires",
    "Summary",
    "Expires annotation",
];

fn expected(tool: &str, query_rows: &[&str]) -> Vec<String> {
    let mut want: Vec<&str> = Vec::new();
    if tool == "update_alert_rule" {
        want.push("Rule");
    }
    want.extend(HEAD);
    want.extend(query_rows);
    want.extend(TAIL);
    want.push("Receiver");
    if tool == "update_alert_rule" {
        want.extend(["Keep firing for", "Evaluations to resolve"]);
    }
    want.into_iter().map(str::to_owned).collect()
}

#[test]
fn each_kind_of_rule_shows_its_own_query_rows_and_every_other_row() {
    for tool in ["create_alert_rule", "update_alert_rule"] {
        let labels = |body: &Value| {
            card(tool, body)
                .into_iter()
                .map(|f| f.0)
                .collect::<Vec<_>>()
        };
        assert_eq!(labels(&prometheus()), expected(tool, &["Query"]), "{tool}");
        assert_eq!(
            labels(&logs("status:500")),
            expected(tool, &LOG_ROWS),
            "{tool}"
        );
    }
}

#[test]
fn a_log_query_as_long_as_the_gateway_accepts_is_shown_whole() {
    let query = format!("message:{}", "é".repeat(SUMMARY_VALUE_MAX - 8));
    for tool in ["create_alert_rule", "update_alert_rule"] {
        let shown = card(tool, &logs(&query));
        assert!(
            shown.contains(&("Log query".to_owned(), query.clone())),
            "{shown:?}"
        );
    }
}

#[test]
fn every_length_the_package_holds_a_card_value_to_is_the_cards_own() {
    let m = manifest();
    let mut rules = 0;
    let writes = m.apps[0].tools.as_ref().and_then(|t| t.write.as_ref());
    for tool in &writes.unwrap().items {
        let Some(check) = &tool.body_check else {
            continue;
        };
        for shape in std::iter::once(check).chain(&check.one_of) {
            for l in shape.entry_max_len.values().flatten() {
                assert_eq!(l.max as usize, SUMMARY_VALUE_MAX, "{}", l.member);
                rules += 1;
            }
        }
    }
    // Both rule tools, both OpenSearch variants, five values each.
    assert_eq!(rules, 20);
}
