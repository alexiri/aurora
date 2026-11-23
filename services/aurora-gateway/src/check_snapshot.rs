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
    /// (FIX-123, D-214) The `connect[]` method the connection was made with: its own `token`
    /// drives the token case, and tools restricted by `connect_methods` apply by it. Absent =
    /// unknown (every tool applies, the app's `token`).
    method: Option<String>,
    /// (P2-CAT-T) The registered OAuth client's fields (fake values) for the refresh request.
    #[serde(default)]
    oauth_client: serde_json::Map<String, Value>,
    tenant_host: Option<String>,
    /// (FIX-136, D-241) The scopes the connection has (granted by OAuth, or declared for a pasted
    /// token); a tool that needs others is refused with `tool_scope_missing`. Empty = unknown.
    #[serde(default)]
    scopes: Vec<String>,
    /// (D-74) The connection is bound to a registered app instance with this host: requests to
    /// exactly this host are served by the app's `{instance}` host entries, any other host is
    /// refused by the instance guard.
    instance_host: Option<String>,
    /// (D-231) The registration's base path (`/wiki`): requests to `instance_host` are served by the
    /// instance only under it, and the manifest is matched on the path below it (the app path).
    instance_base_path: Option<String>,
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
    /// (P4-GRANTS, Q8) The body is longer than the gateway's condition buffer: the classifier
    /// sees a truncated prefix (a fixture cannot carry 256 KiB).
    #[serde(default)]
    body_truncated: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    app: Option<String>,
    tool: Option<String>,
    /// (P4-GRANTS, Q8) Expect the app to match but no catalogue tool (`tool = None`).
    #[serde(default)]
    no_tool: bool,
    /// Expect no app match at all.
    #[serde(default)]
    unmatched: bool,
    /// (D-74) Expect the instance guard to refuse: `host_mismatch`.
    refused: Option<String>,
    /// With `refused: host_mismatch`: the host the connection is bound to.
    expected_host: Option<String>,
    /// (D-231) With `refused: host_mismatch` on an instance with a base path: where the connection
    /// is bound (`https://<host><base_path>`).
    expected_base_url: Option<String>,
    /// (P2-DBL-2) Database statements: the class `aurora-sqlclass` gave the body
    /// (`read` | `write` | `ddl` | `admin` | `transaction`).
    class: Option<String>,
    upstream: Option<ExpectUpstream>,
    summary: Option<ExpectSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectUpstream {
    #[serde(default)]
    headers: BTreeMap<String, String>,
    /// Header names (case-insensitive) the upstream request must not carry.
    #[serde(default)]
    headers_absent: Vec<String>,
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
    let hash = hex::encode(Sha256::digest(snapshot_json));
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
    let query = uri.query().map(str::to_owned);
    let mut headers = HeaderMap::new();
    for (k, v) in &c.request.headers {
        headers.insert(
            HeaderName::from_bytes(k.as_bytes()).map_err(|_| format!("bad header name `{k}`"))?,
            HeaderValue::from_str(v).map_err(|_| format!("bad header value for `{k}`"))?,
        );
    }
    // (FIX-216) Step 3, the gateway's own function: method overrides and ambiguous paths are
    // refused, and everything below matches on the normalized path, exactly as in production.
    let path = match crate::hygiene::check_request_line(&headers, uri.path(), query.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            let err = match e {
                crate::hygiene::PathError::MethodOverride => {
                    crate::guidance::method_override_refused()
                }
                crate::hygiene::PathError::Ambiguous => crate::guidance::ambiguous_path(),
            };
            return Ok(json!({ "refused": { "code": err.code(), "message": err.message() } }));
        }
    };
    let body = c.request.body.as_deref().unwrap_or("").as_bytes();

    // (P2-DBL-2) A database statement: the synthetic request of the database lane.
    if let Some(obs) = database_case(l, &host, &path, method.as_str(), c.request.body.as_deref()) {
        return Ok(obs);
    }

    // (P5-ICS) A secret-link request: the synthetic request of the secret-link lane.
    if let Some(mut obs) = link_case(l, &host, &path, method.as_str()) {
        link_summary(l, &mut obs, &host, body);
        return Ok(obs);
    }

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
    // (D-231) The registration's base path: the instance serves only the paths under it, and the
    // manifest is matched on the app path (the path below it).
    let base = c.connection.instance_base_path.as_deref();
    let on_instance = instance_host
        .as_deref()
        .filter(|h| *h == host)
        .filter(|_| aurora_registry::instance::covers(base, &path))
        .and(instance_app.as_deref())
        .filter(|app| {
            !l.entries_for_host(&host, port)
                .iter()
                .any(|e| e.app_id != *app)
        });
    let inst_path = aurora_registry::instance::app_path(base, &path).to_owned();
    let found = on_instance
        .and_then(|app| crate::apps::identify_instance_app(l, app, &inst_path))
        .or_else(|| identify_app(l, &host, port, &path));
    // (D-275, SEC-109) No static claim or instance serves the origin and it is not a reserved
    // host: an `own_server` app whose fixture connection pinned exactly this `host:port`.
    let pinned = if found.is_none() && !l.is_reserved(&host, port) {
        own_server_pin(l, &c.connection.metadata, &host, port)
    } else {
        None
    };
    let found = found.or_else(|| {
        pinned
            .as_deref()
            .and_then(|a| aurora_registry::identify::identify_own_server_app(&l.snapshot, a))
    });
    let Some(m) = found else {
        return Ok(json!({ "app": null, "tool": null }));
    };
    // The path every manifest decision below uses; the upstream URL keeps the full `path`.
    let app_path = if m.instance { inst_path } else { path.clone() };
    let mismatch = |expected: String| {
        let mut r = json!({ "code": "host_mismatch", "expected_host": expected });
        if let Some(b) = base {
            r["expected_base_url"] = json!(format!("https://{expected}{b}"));
        }
        r
    };
    let mut refusal: Option<Value> = None;
    if instance_host.is_none() {
        // (D-86) No registered instance: a connection of an instance-bound app is the default-host
        // connection, injected on the app's `default_host` only.
        let dh = m
            .app
            .instances
            .as_ref()
            .and_then(|i| i.default_host.as_deref());
        let on = on_instance.map(|_| uuid::Uuid::nil());
        match instance_guard(None, dh, None, on, &host, &path) {
            Ok(()) => {}
            Err(GuardError::Mismatch { expected }) => {
                refusal = Some(json!({ "code": "host_mismatch", "expected_host": expected }));
            }
            Err(GuardError::Unavailable) => {
                refusal = Some(json!({ "code": "credential_unavailable" }));
            }
        }
    }
    if let Some(ih) = instance_host.as_deref() {
        let on = on_instance.map(|_| uuid::Uuid::nil());
        match instance_guard(Some(uuid::Uuid::nil()), Some(ih), base, on, &host, &path) {
            Ok(()) => {}
            Err(GuardError::Mismatch { expected }) => {
                refusal = Some(mismatch(expected));
            }
            Err(GuardError::Unavailable) => {
                refusal = Some(json!({ "code": "credential_unavailable" }));
            }
        }
    }
    let param_headers: Vec<(&str, &str)> = c
        .request
        .headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let tool = identify_tool(
        &m,
        &host,
        port,
        &app_path,
        query.as_deref(),
        method.as_str(),
        body,
        c.request.body_truncated,
        &param_headers,
        c.connection.method.as_deref(),
    );
    let mut obs = json!({ "app": m.app.app_id, "tool": tool.map(|t| t.id.as_str()) });
    // The instance guard refused: nothing is injected (compared against `expect.refused`).
    if let Some(r) = refusal {
        if let Some(o) = obs.as_object_mut() {
            o.insert("refused".into(), r);
        }
        return Ok(obs);
    }

    // (FIX-136, D-241) The token cannot serve the tool: refused before injection.
    if let Some(t) = tool
        && !t.scopes.is_empty()
        && !c.connection.scopes.is_empty()
        && !t.scopes.iter().any(|s| c.connection.scopes.contains(s))
    {
        if let Some(o) = obs.as_object_mut() {
            o.insert(
                "refused".into(),
                json!({ "code": "tool_scope_missing", "needs": t.scopes }),
            );
        }
        return Ok(obs);
    }

    // (FIX-104, D-180) The transport rule the gateway applies: a claimed host's credential leaves
    // only over TLS to the claimed port; anything else is an uncredentialed request.
    if !crate::mechanisms::route::claim_transport_ok(
        // An own-server origin was matched by host and port: TLS to that very port.
        if pinned.is_some() {
            Some(port)
        } else {
            crate::mechanisms::route::claim_port(&m.entry.host)
        },
        scheme == Scheme::Https,
        port,
    ) {
        if let Some(o) = obs.as_object_mut() {
            o.insert(
                "refused".into(),
                json!({ "code": "uncredentialed_transport",
                        "detail": "credentials are injected only over TLS to the claimed port" }),
            );
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
            // (D-467) `{request.host}` in a link carries the registration's base path.
            base_path: base.filter(|_| m.instance),
            // `{path.N}` counts from the app path (D-231).
            path: &app_path,
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

/// (P2-DBL-2, docs/06 §Database lane) A request to `<connection id>.db.aurora.internal`. A path
/// of `/` with a body is a SQL statement: it is classified by `aurora-sqlclass` (the gateway's own
/// classifier) and the case then continues as `POST /<class>`. The tool is the app's tool for that
/// host and path, exactly as `DbSession::authorize` identifies it; a `transaction` statement is
/// never policy checked, so it selects no tool. Nothing is injected: the database lane never puts
/// a stored secret on a request. `None` when the host is not a synthetic database host or the
/// snapshot has no database app (the case then falls through to the HTTP lane).
fn database_case(
    l: &Loaded,
    host: &str,
    path: &str,
    method: &str,
    body: Option<&str>,
) -> Option<Value> {
    let id = host.strip_suffix(crate::db::HOST_SUFFIX)?;
    uuid::Uuid::parse_str(id).ok()?;
    let apps: Vec<_> = l
        .snapshot
        .apps
        .iter()
        .filter(|a| a.database.is_some())
        .collect();
    let first = *apps.first()?;
    let mut class: Option<&'static str> = None;
    let mut path = path.to_owned();
    if path == "/"
        && let Some(sql) = body.filter(|b| !b.is_empty())
    {
        match aurora_sqlclass::classify(sql) {
            Ok(c) => {
                class = Some(c.class.as_str());
                path = format!("/{}", c.class.as_str());
            }
            Err(r) => {
                return Some(json!({
                    "app": first.app_id, "tool": null,
                    "refused": { "code": "statement_refused", "message": r.message() }
                }));
            }
        }
    }
    let transaction = class == Some("transaction");
    let found = apps.iter().find_map(|a| {
        if transaction {
            return None;
        }
        crate::apps::identify_tool_in(
            a,
            false,
            host,
            443,
            &path,
            None,
            method,
            &[],
            false,
            &[],
            None,
        )
        .map(|t| (a.app_id.as_str(), t.id.as_str()))
    });
    Some(json!({
        "app": found.map_or(first.app_id.as_str(), |(a, _)| a),
        "tool": found.map(|(_, t)| t),
        "class": class,
    }))
}

/// (D-275, SEC-109) The `own_server` app whose fixture connection pinned exactly `host:port` (its
/// metadata `hosts_field` array holds the origin, case-insensitively), as the resolver decides it.
fn own_server_pin(l: &Loaded, metadata: &Value, host: &str, port: u16) -> Option<String> {
    let origin = aurora_registry::identify::origin_of(host, port);
    l.snapshot.apps.iter().find_map(|a| {
        let o = a.own_server.as_ref()?;
        metadata
            .get(&o.hosts_field)?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .any(|h| h.eq_ignore_ascii_case(&origin))
            .then(|| a.app_id.clone())
    })
}

/// (P5-ICS, D-273, docs/06 §Secret-link lane) A request to `<connection id>.link.aurora.internal`:
/// only `GET`/`HEAD` are served (405 otherwise, as the lane answers), decided as the `GET` the
/// request stands for; the tool is the app's tool for that host and path, exactly as the lane
/// identifies it. Nothing is injected (the stored URL is the lane's own business). `None` when the
/// host is not a synthetic link host or the snapshot has no secret-link app.
fn link_case(l: &Loaded, host: &str, path: &str, method: &str) -> Option<Value> {
    let id = aurora_registry::manifest::SecretLink::id_label(host)?;
    uuid::Uuid::parse_str(id).ok()?;
    let first = l.snapshot.apps.iter().find(|a| a.secret_link.is_some())?;
    // (P5-WEBHOOK-1, C-909) A send-mode app (an outgoing webhook) serves `POST`/`PUT` instead; the
    // rendered send itself is P5-WEBHOOK-2's.
    let send = |a: &aurora_registry::snapshot::AppEntry| {
        a.secret_link
            .as_ref()
            .is_some_and(|s| s.mode() == aurora_registry::manifest::SecretLinkMode::Send)
    };
    let sending = send(first);
    let served = if sending {
        matches!(method, "POST" | "PUT")
    } else {
        matches!(method, "GET" | "HEAD")
    };
    if !served {
        return Some(json!({
            "app": first.app_id, "tool": null,
            "refused": {
                "code": "method_not_allowed",
                "message": if sending {
                    "An outgoing webhook can only be sent with POST or PUT."
                } else {
                    "A calendar link can only be read with GET or HEAD."
                }
            }
        }));
    }
    // The agent's path is ignored by the lane: the request is always `<method> /`.
    let _ = path;
    let id_method = if sending { method } else { "GET" };
    let found = l
        .snapshot
        .apps
        .iter()
        .filter(|a| a.secret_link.is_some() && send(a) == sending)
        .find_map(|a| {
            crate::apps::identify_tool_in(
                a,
                false,
                host,
                443,
                "/",
                None,
                id_method,
                &[],
                false,
                &[],
                None,
            )
            .map(|t| (a.app_id.as_str(), t.id.as_str()))
        });
    Some(json!({
        "app": found.map_or(first.app_id.as_str(), |(a, _)| a),
        "tool": found.map(|(_, t)| t),
    }))
}

/// (P5-WEBHOOK-1) The approval summary of a link case whose tool the app gives a template (a
/// send-mode app's `send`): the template over the request's body, as for an HTTP request. The
/// lane's `slots` decoder replaces it in P5-WEBHOOK-2.
fn link_summary(l: &Loaded, obs: &mut Value, host: &str, body: &[u8]) {
    use aurora_summary::template;
    let (Some(app_id), Some(tool)) = (obs["app"].as_str(), obs["tool"].as_str()) else {
        return;
    };
    let Some(app) = l.snapshot.app(app_id) else {
        return;
    };
    let Some(approval) = app.approvals.get(tool) else {
        return;
    };
    let summary = template::summarize(
        approval,
        &template::Input {
            host,
            base_path: None,
            path: "/",
            query: None,
            header: &|_| None,
            body_prefix: body,
            truncated: false,
            declared_hosts: &[],
            web_hosts: &app.about.web_hosts,
            injected_headers: &[],
        },
    );
    if let Some(o) = obs.as_object_mut() {
        o.insert(
            "summary".into(),
            serde_json::to_value(&summary).unwrap_or(Value::Null),
        );
    }
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
        .token_for(c.connection.method.as_deref())
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
    // (FIX-233) The case's client fields split as the store keeps them: a field the manifest
    // declares non-secret is public (it may fill a `{client.<field>}` path segment).
    let (public, secret): (serde_json::Map<_, _>, serde_json::Map<_, _>) = conn
        .oauth_client
        .clone()
        .into_iter()
        .partition(|(k, _)| client.fields.iter().any(|f| &f.name == k && !f.secret));
    let outcome = match build_exchange(
        client,
        token,
        &conn.credentials,
        &public,
        &secret,
        conn.tenant_host.as_deref(),
        // (D-86) A connection without a registered instance resolves `{instance}` to the app's
        // `default_host`, as the gateway resolver does.
        // (D-231) A registered instance resolves to its host and base path.
        match conn.instance_host.as_deref() {
            Some(h) => Some(aurora_registry::instance::InstanceBase::new(
                h,
                conn.instance_base_path.as_deref(),
            )),
            None => app
                .instances
                .as_ref()
                .and_then(|i| i.default_host.as_deref())
                .map(|h| aurora_registry::instance::InstanceBase::new(h, None)),
        },
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
            if let Some(u) = &e.expected_base_url
                && o["refused"]["expected_base_url"].as_str() != Some(u)
            {
                f.push(format!(
                    "refused.expected_base_url: expected `{u}`, got {}",
                    o["refused"]["expected_base_url"]
                ));
            }
        }
        (Some(want), have) => f.push(format!("expected refusal `{want}`, got {have:?}")),
        (None, Some(have)) => f.push(format!("refused: {have}")),
        (None, None) => {}
    }
    if let Some(want) = &e.class
        && got("class") != Some(want)
    {
        f.push(format!("class: expected `{want}`, got {:?}", got("class")));
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
    if e.no_tool && got("tool").is_some() {
        f.push(format!("tool: expected none, got {:?}", got("tool")));
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
        // (D-86) None of these headers may be on the upstream request (case-insensitive).
        for k in &u.headers_absent {
            if up["headers"][k.to_ascii_lowercase()].is_string() {
                f.push(format!(
                    "upstream.headers_absent.{k}: the upstream request carried `{k}`"
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
