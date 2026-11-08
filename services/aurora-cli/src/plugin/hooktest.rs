//! Hook fixtures of `aurora plugin test` (docs/09 §12): cases with a `hook:` key run the
//! package's `hooks.wasm` through the same hook-host runtime the platform uses (wasmtime, the
//! egress rules, the memory/time limits and the output validation against the manifest), with
//! `fetch` answered from a recorded HTTP cassette. No network, ever.
//!
//! ```yaml
//! - name: token.mint returns a fresh token
//!   request: { method: GET, url: "https://api.example.test/v1/items" }   # required by the case schema
//!   hook:
//!     point: token.mint                      # connect.authorize | connect.exchange | token.mint | approval.summarize
//!     app_id: my-app                         # optional when the package has one app
//!     http_cassette:                         # recorded responses, consumed in order
//!       - { request: { method: POST, url: "https://auth.example.test/mint" }, response: { status: 200, body: "tok" } }
//!     egress_override: [other.example.test]  # optional: replace the manifest's egress list for this case
//!     input: { credentials: { installation_id: "7" } }      # key/value lists are written as maps
//!     expect: { credentials: { access_token: "tok" } }      # subset of the output; or { error: reauth-required }
//!     # several calls in one case (shared cassette): steps: [ { input: ..., expect: ... }, ... ]
//! ```

use anyhow::{Context, Result, anyhow};
use aurora_hookhost::fetch::{Cassette, FetchPolicy};
use aurora_hookhost::runtime::{CallSpec, Module, Runtime};
use aurora_hookhost::validate::{self, to_json};
use aurora_hookhost::wire::{HookKind, parse_invocation, secrets};
use aurora_registry::Package;
use aurora_registry::snapshot::Snapshot;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookCase {
    point: String,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    http_cassette: Vec<Recorded>,
    #[serde(default)]
    egress_override: Option<Vec<String>>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    expect: Option<Value>,
    #[serde(default)]
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    expect: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recorded {
    request: RecordedRequest,
    response: RecordedResponse,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedRequest {
    method: String,
    url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedResponse {
    status: u16,
    #[serde(default)]
    body: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

/// Replaces every `skipped` hook result of the gateway's `check-snapshot` report with the result
/// of running the case, and recomputes the totals. `files` are the package's `(tests/<name>, yaml)`.
pub fn run_into_report(pkg: &Package, snapshot: &[u8], report: &mut Value) -> Result<()> {
    // The CLI itself runs inside a tokio runtime; the hook host gets a runtime of its own on a
    // separate thread (a runtime cannot be started or dropped inside another one).
    std::thread::scope(|s| {
        s.spawn(|| run_into_report_inner(pkg, snapshot, report))
            .join()
            .unwrap_or_else(|_| Err(anyhow!("hook runner panicked")))
    })
}

fn run_into_report_inner(pkg: &Package, snapshot: &[u8], report: &mut Value) -> Result<()> {
    let mut cases: HashMap<String, Vec<Value>> = HashMap::new();
    for (path, data) in &pkg.files {
        if let Some(name) = path.strip_prefix("tests/") {
            let v: Vec<Value> =
                serde_yaml::from_slice(data).with_context(|| format!("{path}: invalid fixture"))?;
            cases.insert(name.to_owned(), v);
        }
    }
    let snap = Snapshot::from_slice(snapshot).context("compiled snapshot")?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut ctx = Ctx {
        pkg,
        snap: &snap,
        engine: None,
    };
    let mut ordinal: HashMap<String, usize> = HashMap::new();
    let results = report["results"]
        .as_array_mut()
        .ok_or_else(|| anyhow!("check-snapshot report has no results"))?;
    for r in results.iter_mut() {
        let file = r["file"].as_str().unwrap_or_default().to_owned();
        let idx = {
            let n = ordinal.entry(file.clone()).or_insert(0);
            *n += 1;
            *n - 1
        };
        if r["status"] != "skipped" {
            continue;
        }
        let Some(case) = cases.get(&file).and_then(|c| c.get(idx)) else {
            continue;
        };
        let Some(hook) = case.get("hook") else {
            continue;
        };
        let name = r["name"].clone();
        let (failures, observed) = rt.block_on(run_case(&mut ctx, hook));
        *r = json!({
            "name": name, "file": file,
            "status": if failures.is_empty() { "pass" } else { "fail" },
            "failures": failures, "observed": observed,
        });
    }
    let count = |s: &str| results.iter().filter(|r| r["status"] == s).count();
    let (passed, failed, skipped) = (count("pass"), count("fail"), count("skipped"));
    report["passed"] = json!(passed);
    report["failed"] = json!(failed);
    report["skipped"] = json!(skipped);
    report["ok"] = json!(failed == 0);
    Ok(())
}

struct Ctx<'a> {
    pkg: &'a Package,
    snap: &'a Snapshot,
    /// Built on first use inside the tokio runtime (the epoch ticker needs one).
    engine: Option<(Runtime, Module, Cassette)>,
}

async fn run_case(ctx: &mut Ctx<'_>, hook: &Value) -> (Vec<String>, Value) {
    let mut fails = Vec::new();
    let mut observed = Vec::new();
    if let Err(e) = run_case_inner(ctx, hook, &mut fails, &mut observed).await {
        fails.push(e.to_string());
    }
    (fails, json!({ "hook": observed }))
}

async fn run_case_inner(
    ctx: &mut Ctx<'_>,
    hook: &Value,
    fails: &mut Vec<String>,
    observed: &mut Vec<Value>,
) -> Result<()> {
    let hc: HookCase = serde_json::from_value(hook.clone()).context("invalid `hook` block")?;
    let kind =
        HookKind::parse(&hc.point).ok_or_else(|| anyhow!("unknown hook point `{}`", hc.point))?;
    let steps: Vec<(Option<Value>, Option<Value>)> = if hc.steps.is_empty() {
        vec![(hc.input.clone(), hc.expect.clone())]
    } else {
        if hc.input.is_some() || hc.expect.is_some() {
            return Err(anyhow!("use either `input`/`expect` or `steps`, not both"));
        }
        hc.steps
            .iter()
            .map(|s| (s.input.clone(), s.expect.clone()))
            .collect()
    };

    // app: explicit, else the package's only app
    let apps: Vec<&str> = ctx.snap.apps.iter().map(|a| a.app_id.as_str()).collect();
    let app_id = match (&hc.app_id, apps.as_slice()) {
        (Some(a), _) => a.clone(),
        (None, [only]) => (*only).to_owned(),
        _ => {
            return Err(anyhow!(
                "`hook.app_id` is required: the package has several apps"
            ));
        }
    };
    let app = ctx
        .snap
        .app(&app_id)
        .ok_or_else(|| anyhow!("unknown app `{app_id}`"))?;
    let pentry = ctx
        .snap
        .packages
        .iter()
        .find(|p| p.package_id == app.package_id)
        .ok_or_else(|| anyhow!("package entry missing from the snapshot"))?;
    let egress: Vec<String> = if kind.egress_allowed() {
        hc.egress_override.clone().unwrap_or_else(|| {
            pentry
                .egress
                .get(&kind.registry())
                .cloned()
                .unwrap_or_default()
        })
    } else {
        Vec::new()
    };

    let wasm = ctx
        .pkg
        .file("hooks.wasm")
        .ok_or_else(|| anyhow!("hook case, but the package has no hooks.wasm"))?;
    // One runtime and one compiled component per package; only the cassette changes per case.
    if ctx.engine.is_none() {
        let cassette = Cassette::default();
        let rt = Runtime::new(FetchPolicy {
            insecure_local: false,
            cassette: Some(cassette.clone()),
        })
        .map_err(|e| anyhow!("hook runtime: {e}"))?;
        let module = rt
            .compile(wasm)
            .map_err(|e| anyhow!("hooks.wasm is not a loadable component: {e}"))?;
        ctx.engine = Some((rt, module, cassette));
    }
    let (rt, module, cassette) = ctx
        .engine
        .as_ref()
        .ok_or_else(|| anyhow!("no hook runtime"))?;
    cassette.reset(
        hc.http_cassette
            .iter()
            .map(|r| {
                (
                    r.request.method.clone(),
                    r.request.url.clone(),
                    r.response.status,
                    r.response
                        .headers
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    r.response.body.clone().into_bytes(),
                )
            })
            .collect(),
    );

    for (i, (input, expect)) in steps.into_iter().enumerate() {
        let wire = to_wire(kind, input.unwrap_or_else(|| json!({})))?;
        let secret_values = secrets(&wire);
        let inv = parse_invocation(kind, &app.app_id, wire, &pentry.context)
            .map_err(|e| anyhow!("input: {}", e.message))?;
        let spec = CallSpec {
            package_id: pentry.package_id.clone(),
            app_id: app.app_id.clone(),
            hook: kind.name(),
            egress: egress.clone(),
            secrets: secret_values,
            wall: kind.wall(),
        };
        let (res, _stats) = rt.call(module, spec, inv).await;
        let res = res.and_then(|o| {
            validate::validate(
                &validate::Ctx {
                    snapshot: ctx.snap,
                    package: pentry,
                    app,
                },
                o,
            )
        });
        let out = match &res {
            Ok(o) => json!({ "ok": friendly(to_json(o)) }),
            Err(e) => {
                json!({ "error": { "code": e.code.replace('_', "-"), "message": e.message } })
            }
        };
        observed.push(out.clone());
        let label = format!("step {}", i + 1);
        compare(&label, expect.as_ref(), &out, fails);
    }
    for u in cassette.unused() {
        fails.push(format!("recorded response never requested: {u}"));
    }
    Ok(())
}

/// Checks `expect` against the call's outcome: `{ error: <code>, message_contains }` for failures,
/// otherwise a subset match of the output (`url_contains` is a substring check on `url`).
fn compare(label: &str, expect: Option<&Value>, out: &Value, fails: &mut Vec<String>) {
    let Some(expect) = expect.and_then(Value::as_object) else {
        fails.push(format!("{label}: `expect` is required and must be a map"));
        return;
    };
    if let Some(code) = expect.get("error") {
        let err = &out["error"];
        let want = code.as_str().unwrap_or_default().replace('_', "-");
        if err.is_null() {
            fails.push(format!(
                "{label}: expected error `{want}`, the hook succeeded"
            ));
        } else {
            if err["code"] != json!(want) {
                fails.push(format!(
                    "{label}: expected error `{want}`, got {}",
                    err["code"]
                ));
            }
            if let Some(m) = expect.get("message_contains").and_then(Value::as_str)
                && !err["message"].as_str().unwrap_or_default().contains(m)
            {
                fails.push(format!(
                    "{label}: error message {} does not contain `{m}`",
                    err["message"]
                ));
            }
        }
        return;
    }
    let ok = &out["ok"];
    if ok.is_null() {
        fails.push(format!(
            "{label}: the hook failed: {} {}",
            out["error"]["code"], out["error"]["message"]
        ));
        return;
    }
    for (k, want) in expect {
        match k.as_str() {
            "ok" => {}
            "url_contains" => {
                let url = ok["url"].as_str().unwrap_or_default();
                if !url.contains(want.as_str().unwrap_or("\u{0}")) {
                    fails.push(format!("{label}: url `{url}` does not contain {want}"));
                }
            }
            _ => subset(&format!("{label}.{k}"), want, &ok[k], fails),
        }
    }
}

fn subset(path: &str, want: &Value, got: &Value, fails: &mut Vec<String>) {
    match (want, got) {
        (Value::Object(w), Value::Object(g)) => {
            for (k, v) in w {
                subset(
                    &format!("{path}.{k}"),
                    v,
                    g.get(k).unwrap_or(&Value::Null),
                    fails,
                );
            }
        }
        (Value::Object(w), Value::Array(g)) if w.is_empty() && g.is_empty() => {}
        (Value::Array(w), Value::Array(g)) if w.len() == g.len() => {
            for (i, (a, b)) in w.iter().zip(g).enumerate() {
                subset(&format!("{path}[{i}]"), a, b, fails);
            }
        }
        _ if want == got => {}
        _ => fails.push(format!("{path}: expected {want}, got {got}")),
    }
}

/// `[{key, value}, ...]` lists become maps, recursively (the fixture-friendly form of the output).
fn friendly(v: Value) -> Value {
    match v {
        Value::Array(a) => {
            let is_kv = !a.is_empty()
                && a.iter().all(|e| {
                    e.as_object().is_some_and(|o| {
                        o.len() == 2 && o.contains_key("key") && o.contains_key("value")
                    })
                });
            if is_kv {
                Value::Object(
                    a.into_iter()
                        .filter_map(|e| {
                            let k = e["key"].as_str()?.to_owned();
                            Some((k, e["value"].clone()))
                        })
                        .collect(),
                )
            } else {
                Value::Array(a.into_iter().map(friendly).collect())
            }
        }
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, friendly(v))).collect()),
        other => other,
    }
}

/// Key/value inputs are written as maps in fixtures; the wire form is `[{key, value}]`. Fields the
/// fixture leaves out get empty defaults (the hook sees what the host would send for an empty value).
fn to_wire(kind: HookKind, input: Value) -> Result<Value> {
    let Value::Object(mut o) = input else {
        return Err(anyhow!("`input` must be a map"));
    };
    for k in [
        "client",
        "client_secret",
        "fields",
        "context",
        "credentials",
        "metadata",
        "query",
        "headers",
    ] {
        if let Some(v) = o.get_mut(k) {
            *v = kv_list(k, std::mem::take(v))?;
        }
    }
    if let Some(Value::Object(src)) = o.get_mut("source") {
        for (_, v) in src.iter_mut() {
            *v = kv_list("source", std::mem::take(v))?;
        }
    }
    if let Some(Value::String(s)) = o.get("body_prefix") {
        let bytes: Vec<Value> = s.bytes().map(|b| json!(b)).collect();
        o.insert("body_prefix".into(), Value::Array(bytes));
    }
    let defaults: &[(&str, Value)] = match kind {
        HookKind::Authorize => &[
            ("method_id", json!("")),
            ("client", json!([])),
            ("redirect_uri", json!("")),
            ("state", json!("")),
            ("scopes", json!([])),
            ("access_level", Value::Null),
            ("fields", json!([])),
            ("context", json!([])),
        ],
        HookKind::Exchange => &[
            ("method_id", json!("")),
            ("client", json!([])),
            ("client_secret", json!([])),
            ("redirect_uri", json!("")),
            ("pkce_verifier", Value::Null),
            ("continuation", Value::Null),
            ("context", json!([])),
        ],
        HookKind::Mint => &[
            ("credentials", json!([])),
            ("metadata", json!([])),
            ("tenant_host", Value::Null),
            ("client", json!([])),
            ("client_secret", json!([])),
            ("context", json!([])),
        ],
        HookKind::Summarize => &[
            ("tool_id", Value::Null),
            ("method", json!("")),
            ("host", json!("")),
            ("path", json!("")),
            ("query", json!([])),
            ("headers", json!([])),
            ("body_prefix", json!([])),
            ("truncated", json!(false)),
        ],
    };
    for (k, v) in defaults {
        o.entry((*k).to_owned()).or_insert_with(|| v.clone());
    }
    Ok(Value::Object(o))
}

fn kv_list(what: &str, v: Value) -> Result<Value> {
    match v {
        Value::Object(m) => {
            let mut out = Vec::new();
            for (k, v) in m {
                let Value::String(s) = v else {
                    return Err(anyhow!("`{what}.{k}` must be a string"));
                };
                out.push(json!({ "key": k, "value": s }));
            }
            Ok(Value::Array(out))
        }
        // already the wire form
        Value::Array(a) => Ok(Value::Array(a)),
        Value::Null => Ok(Value::Array(Vec::new())),
        _ => Err(anyhow!("`{what}` must be a map of strings")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_become_kv_lists_and_defaults_fill_in() {
        let w = to_wire(
            HookKind::Mint,
            json!({ "credentials": { "installation_id": "7" } }),
        )
        .unwrap();
        assert_eq!(
            w["credentials"],
            json!([{ "key": "installation_id", "value": "7" }])
        );
        assert_eq!(w["metadata"], json!([]));
        assert!(w["tenant_host"].is_null());
        assert!(to_wire(HookKind::Mint, json!({ "credentials": { "a": 1 } })).is_err());
        let x = to_wire(
            HookKind::Exchange,
            json!({ "source": { "oauth_callback": { "code": "c" } } }),
        )
        .unwrap();
        assert_eq!(
            x["source"],
            json!({ "oauth_callback": [{ "key": "code", "value": "c" }] })
        );
    }

    #[test]
    fn output_lists_become_maps() {
        let f = friendly(
            json!({ "credentials": [{ "key": "a", "value": "b" }], "scopes": ["x"], "metadata": [] }),
        );
        assert_eq!(
            f,
            json!({ "credentials": { "a": "b" }, "scopes": ["x"], "metadata": [] })
        );
    }

    #[test]
    fn expectations_are_subsets_and_errors_are_exact() {
        let ok = json!({ "ok": { "url": "https://a.test/x?y=1", "continuation": "c" } });
        let mut f = Vec::new();
        compare(
            "s",
            Some(&json!({ "ok": true, "url_contains": "y=1", "continuation": "c" })),
            &ok,
            &mut f,
        );
        assert!(f.is_empty(), "{f:?}");
        compare("s", Some(&json!({ "continuation": "other" })), &ok, &mut f);
        compare("s", Some(&json!({ "error": "upstream" })), &ok, &mut f);
        assert_eq!(f.len(), 2, "{f:?}");
        let err =
            json!({ "error": { "code": "reauth-required", "message": "installation revoked" } });
        let mut f = Vec::new();
        compare(
            "s",
            Some(&json!({ "error": "reauth_required", "message_contains": "revoked" })),
            &err,
            &mut f,
        );
        assert!(f.is_empty(), "{f:?}");
        compare("s", Some(&json!({ "title": "t" })), &err, &mut f);
        assert_eq!(f.len(), 1, "{f:?}");
    }
}
