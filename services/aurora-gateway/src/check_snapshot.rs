//! `aurora-gateway check-snapshot` (docs/06 §Subcommands, docs/09 §12): offline fixture
//! runner. Each fixture request goes through the same matcher (`Loaded::lookup`,
//! `apps::identify_app/identify_tool`), inject/route/finalize stages
//! (`decide::inject_and_route`, `decide::finalize_request`) and generic summarizer the
//! live pipeline uses; only credentials, connection data and the upstream are recorded.

use crate::apps::{identify_app, identify_tool};
use crate::decide::{finalize_request, inject_and_route};
use crate::hold::{SummaryRequest, build_summary};
use crate::mechanisms::route::{GuardError, instance_guard, tenant_guard};
use crate::mechanisms::template::Vars;
use crate::pipeline::Scheme;
use crate::snapshot::Loaded;
use aurora_registry::index::Snapshot;
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
    /// (P2-CAT-T, D-80) Token lifecycle replay against a recorded token endpoint.
    #[serde(default)]
    token: Option<TokenCase>,
}

/// Recorded stale answer + token-endpoint cassette + expected outcome (docs/09 §12).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenCase {
    /// The upstream's recorded first answer; it must match `token.stale_codes`.
    upstream: RecordedAnswer,
    #[serde(default)]
    http_cassette: Vec<Recorded>,
    expect: TokenExpect,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedAnswer {
    status: u16,
    #[serde(default)]
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recorded {
    request: RecordedRequest,
    response: RecordedAnswer,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedRequest {
    method: String,
    url: String,
    /// Subset match on the form/JSON parameters of the refresh request.
    #[serde(default)]
    form: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenExpect {
    /// `refreshed` | `reauth_required` | `unavailable`.
    outcome: String,
    /// Subset of the refreshed credentials.
    #[serde(default)]
    credentials: BTreeMap<String, String>,
    /// The retried request (only after `refreshed`).
    retried: Option<ExpectUpstream>,
    /// The URL the refresh request was built for (shown even when nothing matched).
    request_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    #[serde(default)]
    credentials: serde_json::Map<String, Value>,
    #[serde(default)]
    metadata: Value,
    /// (P2-CAT-T) The registered OAuth client's fields (fake values) for the refresh request.
    #[serde(default)]
    oauth_client: serde_json::Map<String, Value>,
    tenant_host: Option<String>,
    /// (D-74) The connection is bound to a registered app instance with this host: requests to
    /// exactly this host are served by the app's `{instance}` host entries, any other host is
    /// refused by the instance guard.
    instance_host: Option<String>,
    /// The app the instance belongs to; defaults to the snapshot's only instance-bound app.
    instance_app: Option<String>,
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
    /// (D-74) Expect the instance guard to refuse: `host_mismatch`.
    refused: Option<String>,
    /// With `refused: host_mismatch`: the host the connection is bound to.
    expected_host: Option<String>,
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
    let mut observed = execute(l, c, None, &mut failures).unwrap_or_else(|e| {
        failures.push(e);
        Value::Null
    });
    if let Some(t) = &c.token
        && failures.is_empty()
    {
        token_lifecycle(l, c, t, &mut observed, &mut failures);
    }
    if failures.is_empty() {
        failures = compare(&c.expect, &observed);
    }
    json!({
        "name": c.name,
        "status": if failures.is_empty() { "pass" } else { "fail" },
        "failures": failures, "observed": observed,
    })
}

/// Runs the request through matcher, guards and injection. `creds_override` replaces the
/// connection's credentials (the retry after a refresh).
fn execute(
    l: &Loaded,
    c: &Case,
    creds_override: Option<&serde_json::Map<String, Value>>,
    failures: &mut Vec<String>,
) -> Result<Value, String> {
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

    // A registered instance serves its exact host unless another app's static claim covers it.
    let instance_host = c
        .connection
        .instance_host
        .as_deref()
        .map(str::to_ascii_lowercase);
    let instance_app: Option<String> = instance_host.as_ref().and_then(|_| {
        c.connection.instance_app.clone().or_else(|| {
            let mut bound = l
                .snapshot
                .apps
                .iter()
                .filter(|a| a.instances.is_some())
                .map(|a| a.app_id.clone());
            let first = bound.next();
            if bound.next().is_some() { None } else { first }
        })
    });
    let on_instance = instance_host
        .as_deref()
        .filter(|h| *h == host)
        .and(instance_app.as_deref())
        .filter(|app| {
            !l.entries_for_host(&host, port)
                .iter()
                .any(|e| e.app_id != *app)
        });
    let found = on_instance
        .and_then(|app| crate::apps::identify_instance_app(l, app, &path))
        .or_else(|| identify_app(l, &host, port, &path));
    let Some(m) = found else {
        return Ok(json!({ "app": null, "tool": null }));
    };
    let mut refusal: Option<Value> = None;
    if let Some(ih) = instance_host.as_deref() {
        let on = on_instance.map(|_| uuid::Uuid::nil());
        match instance_guard(Some(uuid::Uuid::nil()), Some(ih), on, &host) {
            Ok(()) => {}
            Err(GuardError::Mismatch { expected }) => {
                refusal = Some(json!({ "code": "host_mismatch", "expected_host": expected }));
            }
            Err(GuardError::Unavailable) => {
                refusal = Some(json!({ "code": "credential_unavailable" }));
            }
        }
    }
    let tool = identify_tool(
        &m,
        &host,
        port,
        &path,
        query.as_deref(),
        method.as_str(),
        body,
        false,
    );
    let mut obs = json!({ "app": m.app.app_id, "tool": tool.map(|t| t.id.as_str()) });
    // The instance guard refused: nothing is injected (compared against `expect.refused`).
    if let Some(r) = refusal {
        if let Some(o) = obs.as_object_mut() {
            o.insert("refused".into(), r);
        }
        return Ok(obs);
    }

    if let Some(t) = &m.entry.tenant {
        match tenant_guard(t, c.connection.tenant_host.as_deref(), &host) {
            Ok(()) => {}
            Err(GuardError::Mismatch { expected }) => {
                failures.push(format!("tenant guard: host mismatch (expected {expected})"));
            }
            Err(GuardError::Unavailable) => failures.push("tenant guard: unavailable".into()),
        }
    }

    let creds = creds_override.unwrap_or(&c.connection.credentials);
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

    let hdr = |n: &str| {
        c.request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(n))
            .map(|(_, v)| v.clone())
    };
    let summary = build_summary(
        Some(l),
        Some(&m),
        tool.map(|t| t.id.as_str()),
        &SummaryRequest {
            method: method.as_str(),
            host: &host,
            path: &path,
            query: query.as_deref(),
            content_type: c
                .request
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                .map(|(_, v)| v.as_str()),
            header: &hdr,
            body_prefix: body,
            truncated: false,
            headers: &[],
        },
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

/// Token lifecycle through the gateway's own refresh functions (`refresh::build_exchange`,
/// `refresh::interpret_exchange`): stale answer -> refresh request against the recorded token
/// endpoint -> outcome -> the retried request with the new credentials.
fn token_lifecycle(
    l: &Loaded,
    c: &Case,
    t: &TokenCase,
    obs: &mut Value,
    failures: &mut Vec<String>,
) {
    use crate::refresh::{RefreshError, build_exchange, interpret_exchange};
    if obs.get("refused").is_some() {
        failures.push("token case: the request was refused by the instance guard".into());
        return;
    }
    let Some(app) = obs["app"].as_str().and_then(|a| l.snapshot.app(a)) else {
        failures.push("token case: the request matched no app".into());
        return;
    };
    let Some(token) = app
        .token
        .as_ref()
        .filter(|k| k.kind == aurora_registry::manifest::TokenKind::Oauth2Refresh)
    else {
        failures.push(format!(
            "token case: app `{}` has no oauth2_refresh token",
            app.app_id
        ));
        return;
    };
    let stale = token.stale_codes.iter().any(|s| {
        s.status == t.upstream.status
            && s.body_contains
                .as_ref()
                .is_none_or(|n| t.upstream.body.contains(n.as_str()))
    });
    if !stale {
        failures.push(format!(
            "token case: the recorded upstream answer ({}) matches no token.stale_codes entry, so the gateway would not refresh",
            t.upstream.status
        ));
        return;
    }
    let Some(client) = token.client.as_ref().and_then(|k| {
        l.snapshot
            .oauth_clients
            .iter()
            .find(|oc| oc.package_id == app.package_id && &oc.client_key == k)
    }) else {
        failures.push("token case: the app's token.client is not in the snapshot".into());
        return;
    };
    let conn = &c.connection;
    let mut used = vec![false; t.http_cassette.len()];
    let mut tobs = json!({ "stale": true });
    let outcome = match build_exchange(
        client,
        token,
        &conn.credentials,
        &serde_json::Map::new(),
        &conn.oauth_client,
        conn.tenant_host.as_deref(),
        conn.instance_host.as_deref(),
    ) {
        // Fail closed before any request (e.g. no instance host to bind the token URL to).
        Err(_) => "unavailable".to_owned(),
        Ok(req) => {
            let form: BTreeMap<&str, &str> = req
                .params
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            tobs["request_url"] = json!(req.url);
            let hit = t.http_cassette.iter().enumerate().find(|(i, r)| {
                !used[*i]
                    && r.request.method.eq_ignore_ascii_case("POST")
                    && r.request.url == req.url
                    && r.request
                        .form
                        .iter()
                        .all(|(k, v)| form.get(k.as_str()) == Some(&v.as_str()))
            });
            match hit {
                // No recorded answer = the request went somewhere that was not recorded.
                None => "unavailable".to_owned(),
                Some((i, rec)) => {
                    used[i] = true;
                    let body: Value =
                        serde_json::from_str(&rec.response.body).unwrap_or(Value::Null);
                    match interpret_exchange(token, &conn.credentials, rec.response.status, &body) {
                        Ok((creds, _secs)) => {
                            tobs["credentials"] = Value::Object(creds.clone());
                            match execute(l, c, Some(&creds), failures) {
                                Ok(r) => tobs["retried"] = r["upstream"].clone(),
                                Err(e) => failures.push(e),
                            }
                            "refreshed".to_owned()
                        }
                        Err(RefreshError::Reauth) => "reauth_required".to_owned(),
                        Err(_) => "unavailable".to_owned(),
                    }
                }
            }
        }
    };
    tobs["outcome"] = json!(outcome);
    for (i, u) in used.iter().enumerate() {
        if !u {
            let r = &t.http_cassette[i].request;
            failures.push(format!("unused cassette entry: {} {}", r.method, r.url));
        }
    }
    let e = &t.expect;
    if outcome != e.outcome {
        failures.push(format!(
            "token.outcome: expected `{}`, got `{outcome}`",
            e.outcome
        ));
    }
    if let Some(u) = &e.request_url
        && tobs["request_url"].as_str() != Some(u)
    {
        failures.push(format!(
            "token.request_url: expected `{u}`, got {}",
            tobs["request_url"]
        ));
    }
    for (k, v) in &e.credentials {
        if tobs["credentials"][k].as_str() != Some(v) {
            failures.push(format!(
                "token.credentials.{k}: expected `{v}`, got {}",
                tobs["credentials"][k]
            ));
        }
    }
    match (&e.retried, outcome.as_str()) {
        (Some(r), "refreshed") => {
            for (k, v) in &r.headers {
                let have = tobs["retried"]["headers"][k.to_ascii_lowercase()].as_str();
                if have != Some(v) {
                    failures.push(format!(
                        "token.retried.headers.{k}: expected `{v}`, got {have:?}"
                    ));
                }
            }
            if let Some(u) = &r.url
                && tobs["retried"]["url"].as_str() != Some(u)
            {
                failures.push(format!(
                    "token.retried.url: expected `{u}`, got {}",
                    tobs["retried"]["url"]
                ));
            }
        }
        (Some(_), o) => failures.push(format!("token.retried: nothing is retried after `{o}`")),
        (None, _) => {}
    }
    // Never echo credentials of the refresh back into reports beyond what the case asserts.
    if let Some(m) = tobs.get_mut("credentials").and_then(Value::as_object_mut) {
        for v in m.values_mut() {
            *v = json!("[redacted]");
        }
    }
    if let Some(o) = obs.as_object_mut() {
        o.insert("token".into(), tobs);
    }
}

fn compare(e: &Expect, o: &Value) -> Vec<String> {
    let mut f = Vec::new();
    let got = |k: &str| o.get(k).and_then(Value::as_str);
    let refused = o.get("refused").and_then(|r| r["code"].as_str());
    match (&e.refused, refused) {
        (Some(want), Some(have)) if want == have => {
            if let Some(h) = &e.expected_host
                && o["refused"]["expected_host"].as_str() != Some(h)
            {
                f.push(format!(
                    "refused.expected_host: expected `{h}`, got {}",
                    o["refused"]["expected_host"]
                ));
            }
        }
        (Some(want), have) => f.push(format!("expected refusal `{want}`, got {have:?}")),
        (None, Some(have)) => f.push(format!("refused by the instance guard: {have}")),
        (None, None) => {}
    }
    if e.refused.is_some() {
        return f;
    }
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
