//! Evaluator corpus: tests/corpus/*.yaml (one case per file).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use aurora_policy::eval::{MemoryCounter, RateCounter, ScopePolicy};
use aurora_policy::rule::Rule;
use aurora_policy::{
    BlockReason, Credential, RequestFacts, Verdict, evaluate, selected_candidates,
};
use serde::Deserialize;
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    #[allow(dead_code)]
    description: String,
    #[serde(default)]
    org: ScopePolicy,
    #[serde(default)]
    workspace: ScopePolicy,
    facts: serde_yaml::Value,
    #[serde(default)]
    counters: Vec<Preload>,
    #[serde(default)]
    select: Option<Select>,
    #[serde(default)]
    expect: Option<Expect>,
}

#[derive(Deserialize)]
struct Preload {
    rule: Uuid,
    agent: Uuid,
    n: u32,
}

#[derive(Deserialize)]
struct Select {
    candidates: Vec<Credential>,
    expect: BTreeSet<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    verdict: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    org_rule: Option<String>,
    #[serde(default)]
    workspace_rule: Option<String>,
    #[serde(default)]
    hits: Option<usize>,
    /// (RFE-011a) the effective approval timeout the decision carries (`null` = no preference).
    #[serde(default, deserialize_with = "some_opt")]
    approval_timeout_secs: Option<Option<u32>>,
    #[serde(default)]
    graphql: Option<String>,
    #[serde(default)]
    git: Option<String>,
    /// (RFE-023) how the scope decided, per scope: `{organization: <via>, workspace: <via>}`.
    #[serde(default)]
    trace_via: Option<std::collections::BTreeMap<String, String>>,
}

fn load_facts(mut v: serde_yaml::Value) -> RequestFacts {
    let body = v.as_mapping_mut().and_then(|m| m.remove("body"));
    let mut f: RequestFacts = serde_yaml::from_value(v).unwrap();
    if let Some(b) = body {
        f.body_prefix = b.as_str().unwrap().as_bytes().to_vec();
    }
    f
}

fn name_of(v: Verdict) -> (&'static str, Option<&'static str>) {
    match v {
        Verdict::Allow => ("allow", None),
        Verdict::NeedsApproval => ("needs_approval", None),
        Verdict::RateLimited => ("rate_limited", None),
        Verdict::Block { reason } => (
            "block",
            Some(match reason {
                BlockReason::Rule => "rule",
                BlockReason::DefaultDenied => "default_denied",
                BlockReason::EgressDenied => "egress_denied",
            }),
        ),
    }
}

/// Distinguishes an absent key (`None`) from an explicit `null` (`Some(None)`).
fn some_opt<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<u32>>, D::Error> {
    Option::<u32>::deserialize(d).map(Some)
}

fn run_case(label: &str, case: Case) {
    let facts = load_facts(case.facts);
    let counters = MemoryCounter::new();
    for p in &case.counters {
        let window = case
            .org
            .ordered_rules
            .iter()
            .chain(&case.workspace.ordered_rules)
            .find(|r| r.logical_id == p.rule)
            .and_then(|r| r.rate_limit)
            .unwrap();
        for _ in 0..p.n {
            counters.record(p.rule, p.agent, &window);
        }
    }
    if let Some(s) = &case.select {
        let rules: Vec<&Rule> = case
            .org
            .ordered_rules
            .iter()
            .chain(&case.workspace.ordered_rules)
            .collect();
        let got: BTreeSet<Uuid> = selected_candidates(rules, &facts.identity, &s.candidates)
            .into_iter()
            .map(|k| match k {
                aurora_policy::CredentialKey::Connection(i)
                | aurora_policy::CredentialKey::Secret(i) => i,
            })
            .collect();
        assert_eq!(got, s.expect, "{label}: selection");
    }
    if let Some(e) = &case.expect {
        let d = evaluate(&case.org, &case.workspace, &facts, &counters);
        let (v, r) = name_of(d.verdict);
        assert_eq!(v, e.verdict, "{label}: verdict (trace {:?})", d.trace);
        if let Some(reason) = &e.reason {
            assert_eq!(r, Some(reason.as_str()), "{label}: reason");
        }
        if let Some(w) = &e.org_rule {
            assert_eq!(
                d.org_matched.as_ref().map(|m| m.name.as_str()),
                Some(w.as_str()),
                "{label}: org rule"
            );
        }
        if let Some(w) = &e.workspace_rule {
            assert_eq!(
                d.workspace_matched.as_ref().map(|m| m.name.as_str()),
                Some(w.as_str()),
                "{label}: ws rule"
            );
        }
        if let Some(h) = e.hits {
            assert_eq!(d.rate_limit_hits().len(), h, "{label}: hits");
        }
        if let Some(t) = e.approval_timeout_secs {
            assert_eq!(d.approval_timeout_secs, t, "{label}: approval timeout");
        }
        if let Some(tv) = &e.trace_via {
            for (scope, want) in tv {
                let t = match scope.as_str() {
                    "organization" => &d.trace.organization,
                    "workspace" => &d.trace.workspace,
                    other => panic!("{label}: unknown scope {other}"),
                };
                let got = serde_json::to_value(t.via).unwrap();
                assert_eq!(got.as_str(), Some(want.as_str()), "{label}: via {scope}");
            }
        }
        if let Some(g) = &e.graphql {
            let got = serde_json::to_value(facts.graphql_kind()).unwrap();
            assert_eq!(got.as_str(), Some(g.as_str()), "{label}: graphql");
        }
        if let Some(g) = &e.git {
            let got = serde_json::to_value(facts.git_op()).unwrap();
            assert_eq!(got.as_str(), Some(g.as_str()), "{label}: git");
        }
    }
}

/// `tests/corpus/connect_time.yaml`: a list of open-lane cases (RFE-003; each
/// case's facts carry `connect_time: true`).
#[test]
fn connect_time_corpus() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/connect_time.yaml");
    let text = std::fs::read_to_string(path).unwrap();
    let cases: Vec<Case> = serde_yaml::from_str(&text).unwrap();
    assert!(cases.len() >= 20, "connect-time corpus too small");
    for (i, c) in cases.into_iter().enumerate() {
        let label = format!("connect_time#{i}: {}", c.description);
        run_case(&label, c);
    }
}

#[test]
fn corpus() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let mut n = 0;
    for path in files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        .filter(|p| p.file_name().is_some_and(|n| n != "connect_time.yaml"))
    {
        let label = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path).unwrap();
        let case: Case = serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("{label}: {e}"));
        run_case(&label, case);
        n += 1;
    }
    assert!(n >= 20, "corpus too small: {n}");
}
