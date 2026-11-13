//! Rules of `grants` and tool `params` (09 §3.4 `grants`, `tools`; 09 §6; phase 4, P4-GRANTS,
//! D-123, D-124): shape, shadowing, and the fixture requirements.
use std::collections::BTreeSet;

use aurora_policy::params::{
    ALLOWED_HEADERS, IN_MAX, MAX_CONDITIONS, NAME_MAX, Outcome, PREFIX_MIN, ParamCondition,
    ParamInput, ParamSource, VALUE_MAX, matches, outcome,
};
use aurora_policy::{HostPattern, PathPattern};
use serde_yaml::Value;

use crate::Violation;
use crate::manifest::{App, INSTANCE_HOST, Manifest, ToolItem};

fn name_ok(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= NAME_MAX
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

fn value_ok(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= VALUE_MAX
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'/' | b'-'))
}

/// Every tool of the app in classification order: `read` items before `write` items, each in
/// declaration order (09 §3.4 "Which tool a request is"), with their JSON-pointer-like path.
fn ordered(app: &App) -> Vec<(String, &ToolItem)> {
    let Some(t) = &app.tools else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (g, grp) in [("read", &t.read), ("write", &t.write)] {
        if let Some(grp) = grp {
            for (j, it) in grp.items.iter().enumerate() {
                out.push((format!("/tools/{g}/items/{j}"), it));
            }
        }
    }
    out
}

fn tool_count(app: &App) -> usize {
    app.tools.as_ref().map_or(0, |t| {
        [&t.read, &t.write]
            .into_iter()
            .flatten()
            .map(|g| g.items.len() + usize::from(g.wildcard.is_some()))
            .sum()
    })
}

fn example_host(h: &str) -> String {
    h.replace('*', "x")
}

fn example_path(p: &str) -> String {
    let p = p.replace('*', "x");
    if p.starts_with('/') {
        p
    } else {
        format!("/{p}")
    }
}

/// Whether every value `later` accepts is accepted by `earlier` (both on the same parameter):
/// `equals` ⊂ `equals` / `in` / a prefix of it; `in` ⊂ a superset (every member accepted);
/// `prefix` ⊂ a shorter-or-equal prefix. Anything else is not provably implied.
fn implies(later: &ParamCondition, earlier: &ParamCondition) -> bool {
    if later.source != earlier.source {
        return false;
    }
    let same_name = if later.source == ParamSource::Header {
        later.name.eq_ignore_ascii_case(&earlier.name)
    } else {
        later.name == earlier.name
    };
    if !same_name {
        return false;
    }
    let (l, e) = (&later.matcher, &earlier.matcher);
    if l.forms() != 1 || e.forms() != 1 {
        return false;
    }
    let accepted_by_earlier = |v: &str| match (&e.equals, &e.any_of, &e.prefix) {
        (Some(x), None, None) => v == x,
        (None, Some(xs), None) => xs.iter().any(|x| x == v),
        (None, None, Some(p)) => v.starts_with(p.as_str()),
        _ => false,
    };
    match (&l.equals, &l.any_of, &l.prefix) {
        (Some(v), None, None) => accepted_by_earlier(v),
        (None, Some(vs), None) => !vs.is_empty() && vs.iter().all(|v| accepted_by_earlier(v)),
        // Every string with prefix `p` starts with `q` iff `p` does.
        (None, None, Some(p)) => e.prefix.as_ref().is_some_and(|q| p.starts_with(q.as_str())),
        _ => false,
    }
}

/// An earlier tool that matches everything `t` can match (so it would shadow it).
fn shadows(earlier: &ToolItem, t: &ToolItem) -> bool {
    if earlier.graphql.is_some() || earlier.git.is_some() {
        return false;
    }
    // (FIX-085) An earlier `params` tool shadows `t` when every one of its conditions is implied
    // by a condition `t` carries: whatever `t` accepts, the earlier tool accepts first. A tool
    // without `params` is matched by anything the earlier one (with `params`) refuses, so it is
    // never shadowed by it.
    if let Some(ec) = &earlier.params {
        let Some(tc) = &t.params else {
            return false;
        };
        if ec.is_empty() || !ec.iter().all(|e| tc.iter().any(|l| implies(l, e))) {
            return false;
        }
    }
    let host_ok = earlier.host == t.host
        || (earlier.host != INSTANCE_HOST
            && t.host != INSTANCE_HOST
            && HostPattern::lenient(&earlier.host).matches(&example_host(&t.host), 443));
    let path_ok = PathPattern::lenient(&earlier.path).matches(&example_path(&t.path));
    let methods_ok = earlier.methods.is_empty()
        || (!t.methods.is_empty()
            && t.methods
                .iter()
                .all(|m| earlier.methods.iter().any(|e| e.eq_ignore_ascii_case(m))));
    host_ok && path_ok && methods_ok
}

/// A tool that cannot carry a body: no methods listed (any method, a GET included, would
/// match), or only `GET` / `HEAD`.
fn carries_no_body(methods: &[String]) -> bool {
    methods.is_empty()
        || methods
            .iter()
            .all(|m| m.eq_ignore_ascii_case("GET") || m.eq_ignore_ascii_case("HEAD"))
}

/// Manifest-level rules: `custom_only_without_tools`, `custom_only_needs_block_untooled`,
/// `params` shape and shadowing.
pub fn app_rules(app: &App, base: &str, out: &mut Vec<Violation>) {
    if app.grants.custom_only && tool_count(app) == 0 {
        out.push(Violation::new(
            "custom_only_without_tools",
            format!("{base}/grants/custom_only"),
            "`custom_only` needs at least one tool in the catalogue",
        ));
    }
    // (FIX-085, SEC-67) The tools of a `custom_only` app are the only way to use it: a request
    // that matches none must be blocked, so the manifest says so.
    if app.grants.custom_only && !app.grants.block_untooled {
        out.push(Violation::new(
            "custom_only_needs_block_untooled",
            format!("{base}/grants/block_untooled"),
            "`custom_only` implies `block_untooled: true`; set both",
        ));
    }
    let tools = ordered(app);
    for (k, (p, t)) in tools.iter().enumerate() {
        let Some(conds) = &t.params else { continue };
        let at = |s: &str| format!("{base}{p}/params{s}");
        if conds.is_empty() || conds.len() > MAX_CONDITIONS {
            out.push(Violation::new(
                "params_invalid",
                at(""),
                format!("`params` needs 1 to {MAX_CONDITIONS} conditions"),
            ));
        }
        for (i, c) in conds.iter().enumerate() {
            condition(c, &at(&format!("/{i}")), out);
            if c.source == ParamSource::Form && carries_no_body(&t.methods) {
                out.push(Violation::new(
                    "params_invalid",
                    at(&format!("/{i}/source")),
                    "a `form` condition needs a tool that lists the methods that carry a body (`methods`, not `GET`/`HEAD` only)",
                ));
            }
        }
        if let Some((ep, _)) = tools
            .iter()
            .take(k)
            .find(|(_, earlier)| shadows(earlier, t))
        {
            out.push(Violation::new(
                "params_tool_shadowed",
                format!("{base}{p}"),
                format!("an earlier tool ({ep}) matches everything this tool matches; declare the `params` tool first"),
            ));
        }
    }
}

fn condition(c: &ParamCondition, p: &str, out: &mut Vec<Violation>) {
    let mut bad = |sub: &str, msg: String| {
        out.push(Violation::new("params_invalid", format!("{p}{sub}"), msg));
    };
    if !name_ok(&c.name) {
        bad(
            "/name",
            "name must be `[A-Za-z0-9_.-]`, 1-64 characters".into(),
        );
    }
    if c.source == ParamSource::Header
        && !ALLOWED_HEADERS
            .iter()
            .any(|h| h.eq_ignore_ascii_case(&c.name))
    {
        bad(
            "/name",
            format!("header conditions may read only {ALLOWED_HEADERS:?}"),
        );
    }
    let m = &c.matcher;
    if m.forms() != 1 {
        bad(
            "/match",
            "`match` needs exactly one of `equals`, `in`, `prefix`".into(),
        );
        return;
    }
    if let Some(v) = &m.equals
        && !value_ok(v)
    {
        bad(
            "/match/equals",
            "value must be `[A-Za-z0-9_.:/-]`, 1-128 characters".into(),
        );
    }
    if let Some(l) = &m.any_of {
        if l.is_empty() || l.len() > IN_MAX {
            bad("/match/in", format!("`in` needs 1 to {IN_MAX} values"));
        }
        if l.iter().any(|v| !value_ok(v)) {
            bad(
                "/match/in",
                "values must be `[A-Za-z0-9_.:/-]`, 1-128 characters".into(),
            );
        }
    }
    if let Some(v) = &m.prefix {
        if !value_ok(v) {
            bad(
                "/match/prefix",
                "value must be `[A-Za-z0-9_.:/-]`, 1-128 characters".into(),
            );
        } else if v.len() < PREFIX_MIN {
            bad(
                "/match/prefix",
                format!("`prefix` needs at least {PREFIX_MIN} characters"),
            );
        }
    }
}

/// One fixture request, as far as the requirements need it.
struct Req {
    method: String,
    host: String,
    path: String,
    query: Option<String>,
    headers: Vec<(String, String)>,
    body: String,
    truncated: bool,
    expect_tool: Option<String>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

fn requests(pkg: &crate::package::Package) -> Vec<Req> {
    let mut out = Vec::new();
    for (name, bytes) in &pkg.files {
        if !(name.starts_with("tests/") && name.ends_with(".yaml")) {
            continue;
        }
        let Ok(cases) = serde_yaml::from_slice::<Vec<Value>>(bytes) else {
            continue;
        };
        for case in cases {
            let Some(rq) = case.get("request") else {
                continue;
            };
            let Some(url) = s(rq, "url") else { continue };
            let Some(rest) = url
                .strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
            else {
                continue;
            };
            let (auth, tail) = rest.split_at(rest.find(['/', '?']).unwrap_or(rest.len()));
            let host = auth
                .rsplit_once(':')
                .map_or(auth, |(h, _)| h)
                .to_ascii_lowercase();
            let (path, query) = match tail.split_once('?') {
                Some((p, q)) => (p.to_owned(), Some(q.to_owned())),
                None => (tail.to_owned(), None),
            };
            let headers = rq
                .get("headers")
                .and_then(Value::as_mapping)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default();
            out.push(Req {
                method: s(rq, "method").unwrap_or_default().to_ascii_uppercase(),
                host,
                path: if path.is_empty() {
                    "/".to_owned()
                } else {
                    path
                },
                query,
                headers,
                body: s(rq, "body").unwrap_or_default(),
                truncated: rq
                    .get("body_truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                expect_tool: case.get("expect").and_then(|e| s(e, "tool")),
            });
        }
    }
    out
}

/// Fixture requirements for every `params` tool (09 §3.4 tools): the positive case and each of
/// other value, missing, repeated, query/body conflict, wrong content type, truncated body.
pub fn fixtures(m: &Manifest, pkg: &crate::package::Package, out: &mut Vec<Violation>) {
    let reqs = requests(pkg);
    for (ai, app) in m.apps.iter().enumerate() {
        for (p, t) in ordered(app) {
            let Some(conds) = &t.params else { continue };
            if conds.is_empty() {
                continue;
            }
            let mut need: BTreeSet<&'static str> = [
                "positive",
                "other value",
                "missing",
                "repeated",
                "wrong content type",
                "truncated body",
            ]
            .into_iter()
            .collect();
            if conds.iter().any(|c| c.source == ParamSource::Form) {
                need.insert("query/body conflict");
            }
            for r in &reqs {
                let serves = HostPattern::lenient(&t.host).matches(&r.host, 443)
                    || (t.host == INSTANCE_HOST)
                    || example_host(&t.host) == r.host;
                if !serves
                    || !PathPattern::lenient(&t.path).matches(&r.path)
                    || !(t.methods.is_empty()
                        || t.methods.iter().any(|x| x.eq_ignore_ascii_case(&r.method)))
                {
                    continue;
                }
                let headers: Vec<(&str, &str)> = r
                    .headers
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                let input = ParamInput {
                    query: r.query.as_deref(),
                    headers: &headers,
                    body: r.body.as_bytes(),
                    truncated: r.truncated,
                };
                if r.expect_tool.as_deref() == Some(t.id.as_str()) {
                    // (FIX-085) A claim is not a match: the fixture counts as the positive case
                    // only when the tool's conditions really hold for the request it describes.
                    if matches(conds, &input) {
                        need.remove("positive");
                    }
                    continue;
                }
                for c in conds {
                    match outcome(c, &input) {
                        Outcome::Match => {}
                        Outcome::OtherValue => {
                            need.remove("other value");
                        }
                        Outcome::Missing => {
                            need.remove("missing");
                        }
                        Outcome::Repeated => {
                            need.remove("repeated");
                        }
                        Outcome::Conflict => {
                            need.remove("query/body conflict");
                        }
                        Outcome::WrongContentType | Outcome::Compressed => {
                            need.remove("wrong content type");
                        }
                        Outcome::Truncated => {
                            need.remove("truncated body");
                        }
                    }
                }
            }
            if !need.is_empty() {
                out.push(Violation::new(
                    "params_fixture_missing",
                    format!("/apps/{ai}{p}/params"),
                    format!(
                        "tool `{}` needs tests/*.yaml fixtures for: {}",
                        t.id,
                        need.into_iter().collect::<Vec<_>>().join(", ")
                    ),
                ));
            }
        }
    }
}
