//! `aurora-gateway check-snapshot` (docs/06 §Subcommands, docs/09 §12): offline fixture
//! runner. Each fixture request goes through the same matcher (`Loaded::lookup`,
//! `apps::identify_app/identify_tool`), inject/route/finalize stages
//! (`decide::inject_and_route`, `decide::finalize_request`) and generic summarizer the
//! live pipeline uses; only credentials, connection data and the upstream are recorded.

use crate::apps::{identify_app, identify_tool};
use crate::decide::{finalize_request, inject_and_route};
use crate::mechanisms::route::{GuardError, tenant_guard};
use crate::mechanisms::template::Vars;
use crate::pipeline::Scheme;
use crate::snapshot::Loaded;
use aurora_registry::index::Snapshot;
use aurora_registry::manifest::Source;
use aurora_summary::{ApprovalSummary, SummaryField};
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    #[serde(default)]
    connection: Connection,
    request: Request,
    #[serde(default)]
    expect: Expect,
    /// Hook-host cases are not runnable offline yet; reported as skipped.
    #[serde(default)]
    hook: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    #[serde(default)]
    credentials: serde_json::Map<String, Value>,
    #[serde(default)]
    metadata: Value,
    tenant_host: Option<String>,
    #[serde(default)]
    context: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    method: String,
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    app: Option<String>,
    tool: Option<String>,
    /// Expect no app match at all.
    #[serde(default)]
    unmatched: bool,
    upstream: Option<ExpectUpstream>,
    summary: Option<ExpectSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectUpstream {
    #[serde(default)]
    headers: BTreeMap<String, String>,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectSummary {
    title: Option<String>,
    #[serde(default)]
    fields: BTreeMap<String, String>,
}

/// JSON report: `{ ok, passed, failed, skipped, results: [...] }`.
pub struct Report {
    pub json: Value,
    pub ok: bool,
}

/// Parses every `*.yaml` of `dir` (sorted) and runs it against the snapshot file.
pub fn run_paths(snapshot: &Path, fixtures: &Path) -> Result<Report, String> {
    let bytes = std::fs::read(snapshot).map_err(|e| format!("{}: {e}", snapshot.display()))?;
    let mut files = Vec::new();
    let rd = std::fs::read_dir(fixtures).map_err(|e| format!("{}: {e}", fixtures.display()))?;
    for ent in rd {
        let p = ent.map_err(|e| e.to_string())?.path();
        if p.extension().is_some_and(|x| x == "yaml") {
            let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let name = p
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            files.push((name, text));
        }
    }
    files.sort();
    run(&bytes, &files)
}

/// `files` are `(file name, yaml text)`.
pub fn run(snapshot_json: &[u8], files: &[(String, String)]) -> Result<Report, String> {
    let snap = Snapshot::from_slice(snapshot_json).map_err(|e| format!("snapshot: {e}"))?;
    let hash = format!("{:x}", Sha256::digest(snapshot_json));
    let loaded = Loaded::new(&hash, snap)?;
    let mut results = Vec::new();
    for (file, text) in files {
        let cases: Vec<Case> =
            serde_yaml::from_str(text).map_err(|e| format!("{file}: invalid fixture: {e}"))?;
        for c in cases {
            let mut r = run_case(&loaded, &c);
            if let Some(o) = r.as_object_mut() {
                o.insert("file".into(), json!(file));
            }
            results.push(r);
        }
    }
    let count = |s: &str| results.iter().filter(|r| r["status"] == s).count();
    let (passed, failed, skipped) = (count("pass"), count("fail"), count("skipped"));
    Ok(Report {
        ok: failed == 0,
        json: json!({
            "ok": failed == 0, "snapshot_hash": hash,
            "passed": passed, "failed": failed, "skipped": skipped, "results": results,
        }),
    })
}

fn run_case(l: &Loaded, c: &Case) -> Value {
    if c.hook.is_some() {
        return json!({ "name": c.name, "status": "skipped", "reason": "hook cases need the hook host" });
    }
    let mut failures = Vec::new();
    let observed = execute(l, c, &mut failures).unwrap_or_else(|e| {
        failures.push(e);
        Value::Null
    });
    if failures.is_empty() {
        failures = compare(&c.expect, &observed);
    }
    json!({
        "name": c.name,
        "status": if failures.is_empty() { "pass" } else { "fail" },
        "failures": failures, "observed": observed,
    })
}

fn execute(l: &Loaded, c: &Case, failures: &mut Vec<String>) -> Result<Value, String> {
    let method = Method::from_bytes(c.request.method.to_ascii_uppercase().as_bytes())
        .map_err(|_| "invalid request.method".to_owned())?;
    let uri: http::Uri = c
        .request
        .url
        .parse()
        .map_err(|e| format!("invalid request.url: {e}"))?;
    let scheme = match uri.scheme_str() {
        Some("http") => Scheme::Http,
        Some("https") => Scheme::Https,
        _ => return Err("request.url must be http(s)".into()),
    };
    let host = uri
        .host()
        .ok_or("request.url has no host")?
        .to_ascii_lowercase();
    let port = uri.port_u16().unwrap_or_else(|| scheme.default_port());
    let path = uri.path().to_owned();
    let query = uri.query().map(str::to_owned);
    let mut headers = HeaderMap::new();
    for (k, v) in &c.request.headers {
        headers.insert(
            HeaderName::from_bytes(k.as_bytes()).map_err(|_| format!("bad header name `{k}`"))?,
            HeaderValue::from_str(v).map_err(|_| format!("bad header value for `{k}`"))?,
        );
    }
    let body = c.request.body.as_deref().unwrap_or("").as_bytes();

    let Some(m) = identify_app(l, &host, port, &path) else {
        return Ok(json!({ "app": null, "tool": null }));
    };
    let tool = identify_tool(
        m.app,
        &host,
        port,
        &path,
        query.as_deref(),
        method.as_str(),
        body,
        false,
    );
    let mut obs = json!({ "app": m.app.app_id, "tool": tool.map(|t| t.id.as_str()) });

    if let Some(t) = &m.entry.tenant {
        match tenant_guard(t, c.connection.tenant_host.as_deref(), &host) {
            Ok(()) => {}
            Err(GuardError::Mismatch { expected }) => {
                failures.push(format!("tenant guard: host mismatch (expected {expected})"));
            }
            Err(GuardError::Unavailable) => failures.push("tenant guard: unavailable".into()),
        }
    }

    let creds = &c.connection.credentials;
    let optional: BTreeSet<&str> = m
        .app
        .credentials
        .iter()
        .filter(|d| d.optional)
        .map(|d| d.name.as_str())
        .collect();
    let allowed_context: &[String] = l
        .snapshot
        .packages
        .iter()
        .find(|p| p.package_id == m.app.package_id)
        .map_or(&[], |p| p.context.as_slice());
    let metadata = if c.connection.metadata.is_null() {
        json!({})
    } else {
        c.connection.metadata.clone()
    };
    let vars = Vars {
        credentials: creds,
        optional: &optional,
        metadata: &metadata,
        tenant_host: c.connection.tenant_host.as_deref(),
        allowed_context,
        context: &c.connection.context,
    };
    let (mut up_path, mut up_query) = (path.clone(), query.clone());
    let rewritten =
        match inject_and_route(m.entry, &vars, &mut headers, &mut up_path, &mut up_query) {
            Ok(h) => h,
            Err(()) => {
                failures.push("credentials unavailable: inject/route failed".into());
                return Ok(obs);
            }
        };
    let up_host = rewritten.unwrap_or_else(|| host.clone());
    if finalize_request(
        m.entry,
        &vars,
        &method,
        &up_host,
        port,
        scheme,
        &up_path,
        up_query.as_deref(),
        &mut headers,
    )
    .is_err()
    {
        failures.push("credentials unavailable: finalize failed".into());
        return Ok(obs);
    }
    let authority = if port == scheme.default_port() {
        up_host
    } else {
        format!("{up_host}:{port}")
    };
    let url = format!(
        "{}://{authority}{up_path}{}",
        scheme.as_str(),
        up_query
            .as_deref()
            .map(|q| format!("?{q}"))
            .unwrap_or_default()
    );
    let hdrs: BTreeMap<String, String> = headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_owned(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();

    let summary = summarize(
        m.app,
        tool.map(|t| t.id.as_str()),
        &method,
        &host,
        &path,
        query.as_deref(),
        &c.request.headers,
        body,
    );
    if let Some(o) = obs.as_object_mut() {
        o.insert("upstream".into(), json!({ "url": url, "headers": hdrs }));
        o.insert(
            "summary".into(),
            serde_json::to_value(&summary).unwrap_or(Value::Null),
        );
    }
    Ok(obs)
}

/// Approval template for the tool when declared (path/query/header sources), else the
/// generic summary. Body-source fields need the decoders and fall back to the generic
/// summary (the live gateway has no template summarizer yet).
#[allow(clippy::too_many_arguments)]
fn summarize(
    app: &aurora_registry::snapshot::AppEntry,
    tool: Option<&str>,
    method: &Method,
    host: &str,
    path: &str,
    query: Option<&str>,
    req_headers: &BTreeMap<String, String>,
    body: &[u8],
) -> ApprovalSummary {
    if let Some(a) = tool.and_then(|t| app.approvals.get(t))
        && a.fields
            .iter()
            .all(|f| !matches!(f.source, Source::Body { .. }))
    {
        let fields = a
            .fields
            .iter()
            .map(|f| {
                let value = match &f.source {
                    Source::Path { segment } => path
                        .split('/')
                        .nth(*segment as usize)
                        .unwrap_or_default()
                        .to_owned(),
                    Source::Query { name } => query
                        .into_iter()
                        .flat_map(|q| q.split('&'))
                        .filter_map(|kv| kv.split_once('='))
                        .find(|(k, _)| k == name)
                        .map(|(_, v)| v.to_owned())
                        .unwrap_or_default(),
                    Source::Header { name } => req_headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default(),
                    Source::Body { .. } => String::new(),
                };
                SummaryField {
                    label: f.label.clone(),
                    value,
                }
            })
            .collect();
        return ApprovalSummary {
            title: a.title.clone(),
            fields,
            text: None,
            link: None,
            group: None,
            source: aurora_summary::SummarySource::Template,
        }
        .clamp();
    }
    aurora_summary::generic(method.as_str(), host, path, query, None, body, false)
}

fn compare(e: &Expect, o: &Value) -> Vec<String> {
    let mut f = Vec::new();
    let got = |k: &str| o.get(k).and_then(Value::as_str);
    if e.unmatched {
        if got("app").is_some() {
            f.push(format!("expected no app match, got {:?}", got("app")));
        }
        return f;
    }
    if let Some(a) = &e.app
        && got("app") != Some(a)
    {
        f.push(format!("app: expected `{a}`, got {:?}", got("app")));
    }
    if let Some(t) = &e.tool
        && got("tool") != Some(t)
    {
        f.push(format!("tool: expected `{t}`, got {:?}", got("tool")));
    }
    if let Some(u) = &e.upstream {
        let up = &o["upstream"];
        if let Some(url) = &u.url
            && up["url"].as_str() != Some(url)
        {
            f.push(format!("upstream.url: expected `{url}`, got {}", up["url"]));
        }
        for (k, v) in &u.headers {
            let have = up["headers"][k.to_ascii_lowercase()].as_str();
            if have != Some(v) {
                f.push(format!(
                    "upstream.headers.{k}: expected `{v}`, got {have:?}"
                ));
            }
        }
    }
    if let Some(s) = &e.summary {
        let sm = &o["summary"];
        if let Some(t) = &s.title
            && sm["title"].as_str() != Some(t)
        {
            f.push(format!(
                "summary.title: expected `{t}`, got {}",
                sm["title"]
            ));
        }
        for (label, v) in &s.fields {
            let have = sm["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|x| x["label"] == *label)
                .and_then(|x| x["value"].as_str());
            if have != Some(v) {
                f.push(format!(
                    "summary.fields.{label}: expected `{v}`, got {have:?}"
                ));
            }
        }
    }
    f
}
