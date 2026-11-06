//! Probe execution (09 §3.4), host/zone checks shared by hook and field flows.
use super::env::Env;
use aurora_policy::HostPattern;
use aurora_registry::manifest::{Inject, Probe, ProbeMethod};
use aurora_registry::snapshot::{AppEntry, HostEntry, Snapshot};
use aurora_registry::template::{Ns, Part, Template};
use base64::Engine;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Map, Value};

const ENC: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
const BODY_MAX: usize = 1024 * 1024;

#[derive(Debug)]
pub struct ProbeFail {
    pub status: Option<u16>,
}

#[derive(Default)]
pub struct ProbeOut {
    pub label: Option<String>,
    pub metadata: Map<String, Value>,
}

pub struct Vars<'a> {
    pub credentials: &'a Map<String, Value>,
    pub metadata: &'a Map<String, Value>,
    pub tenant_host: Option<&'a str>,
    pub fields: &'a Map<String, Value>,
}

fn dns_ok(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        && !s.starts_with('.')
        && !s.contains("..")
}

fn sval(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Render a template; `encode` percent-encodes substituted values (URLs), otherwise raw.
fn render(t: &Template, v: &Vars<'_>, encode: bool) -> Option<String> {
    let mut out = String::new();
    for p in t.parts() {
        match p {
            Part::Lit(l) => out.push_str(l),
            Part::Ref(r) => {
                let val = match (r.ns, r.name.as_str()) {
                    (Ns::Credentials, n) => v.credentials.get(n).and_then(sval).unwrap_or_default(),
                    (Ns::Metadata, n) => v.metadata.get(n).and_then(sval).unwrap_or_default(),
                    (Ns::Fields, n) => v.fields.get(n).and_then(sval).unwrap_or_default(),
                    (Ns::Connection, "tenant_host") => {
                        let h = v.tenant_host?;
                        if !dns_ok(h) {
                            return None;
                        }
                        out.push_str(h);
                        continue;
                    }
                    _ => return None,
                };
                if encode {
                    out.push_str(&utf8_percent_encode(&val, ENC).to_string());
                } else {
                    out.push_str(&val);
                }
            }
        }
    }
    Some(out)
}

/// Render a token URL (host-position substitution, DNS characters only).
pub fn render_url(t: &Template, tenant_host: Option<&str>) -> Option<String> {
    let empty = Map::new();
    render(
        t,
        &Vars {
            credentials: &empty,
            metadata: &empty,
            tenant_host,
            fields: &empty,
        },
        true,
    )
}

/// `https` URL whose host lies in `zone` (SEC-17 / I-20).
pub fn in_zone(url: &str, zone: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(h) = u.host_str() else { return false };
    u.scheme() == "https"
        && HostPattern::parse(zone)
            .is_ok_and(|p| p.matches(&h.to_ascii_lowercase(), u.port().unwrap_or(443)))
}

/// A tenant host must fall in the zone of one of the app's tenant-bound hosts.
pub fn tenant_ok(snap: &Snapshot, app: &AppEntry, host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    dns_ok(&h)
        && snap
            .hosts
            .iter()
            .filter(|e| e.app_id == app.app_id)
            .filter_map(|e| e.tenant.as_ref())
            .any(|t| HostPattern::parse(&t.zone).is_ok_and(|p| p.matches(&h, 443)))
}

fn host_entry<'a>(
    snap: &'a Snapshot,
    app: &AppEntry,
    host: &str,
    path: &str,
) -> Option<&'a HostEntry> {
    snap.hosts
        .iter()
        .filter(|e| e.app_id == app.app_id)
        .filter(|e| {
            HostPattern::lenient(&e.host).matches(host, 443)
                && path.starts_with(e.path_prefix.as_deref().unwrap_or("/"))
        })
        .max_by_key(|e| e.path_prefix.as_deref().map_or(0, str::len))
}

/// Does `url` target https on one of the app's declared hosts?
pub fn declared_host(snap: &Snapshot, app: &AppEntry, url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str()
                .is_some_and(|h| host_entry(snap, app, &h.to_ascii_lowercase(), u.path()).is_some())
    })
}

pub fn clean_label(s: &str) -> Option<String> {
    let t: String = s.chars().filter(|c| !c.is_control()).collect();
    let t = t.trim();
    (!t.is_empty()).then(|| t.chars().take(120).collect())
}

pub async fn run(
    env: &Env,
    snap: &Snapshot,
    app: &AppEntry,
    probe: &Probe,
    v: &Vars<'_>,
) -> Result<ProbeOut, ProbeFail> {
    let none = ProbeFail { status: None };
    let raw = render(&probe.url, v, true).ok_or(ProbeFail { status: None })?;
    let parsed = reqwest::Url::parse(&raw).map_err(|_| ProbeFail { status: None })?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if parsed.scheme() != "https" {
        return Err(none);
    }
    let entry = host_entry(snap, app, &host, parsed.path()).ok_or(ProbeFail { status: None })?;
    if let Some(t) = &entry.tenant
        && t.field == "connection.tenant_host"
        && v.tenant_host.map(str::to_ascii_lowercase).as_deref() != Some(host.as_str())
    {
        return Err(none);
    }
    let mut url = env.prepare(&raw).map_err(|_| ProbeFail { status: None })?;
    let method = match probe.method {
        ProbeMethod::Get => reqwest::Method::GET,
        ProbeMethod::Head => reqwest::Method::HEAD,
        ProbeMethod::Post => reqwest::Method::POST,
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    for inj in &entry.inject {
        match inj {
            Inject::Bearer(t) => {
                headers.push((
                    "authorization".into(),
                    format!(
                        "Bearer {}",
                        render(t, v, false).ok_or(ProbeFail { status: None })?
                    ),
                ));
            }
            Inject::Basic { user, password } => {
                let u = render(user, v, false).ok_or(ProbeFail { status: None })?;
                let p = render(password, v, false).ok_or(ProbeFail { status: None })?;
                let enc = base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"));
                headers.push(("authorization".into(), format!("Basic {enc}")));
            }
            Inject::Header { name, value } => {
                headers.push((
                    name.clone(),
                    render(value, v, false).ok_or(ProbeFail { status: None })?,
                ));
            }
            Inject::Query { name, value } => {
                let val = render(value, v, false).ok_or(ProbeFail { status: None })?;
                url.query_pairs_mut().append_pair(name, &val);
            }
            _ => {}
        }
    }
    let mut req = env
        .http
        .request(method, url)
        .timeout(std::time::Duration::from_secs(10));
    for (k, val) in headers {
        if val.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0)) {
            return Err(none);
        }
        req = req.header(k, val);
    }
    let mut resp = req.send().await.map_err(|_| ProbeFail { status: None })?;
    let status = resp.status().as_u16();
    if !probe.expect_status.contains(&status) {
        return Err(ProbeFail {
            status: Some(status),
        });
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|_| ProbeFail {
        status: Some(status),
    })? {
        if body.len() + chunk.len() > BODY_MAX {
            return Err(ProbeFail {
                status: Some(status),
            });
        }
        body.extend_from_slice(&chunk);
    }
    let mut out = ProbeOut::default();
    if probe.label.is_none() && probe.metadata.is_empty() {
        return Ok(out);
    }
    let doc: Value = serde_json::from_slice(&body).map_err(|_| ProbeFail {
        status: Some(status),
    })?;
    if let Some(p) = &probe.label {
        out.label = doc.pointer(p).and_then(sval).and_then(|s| clean_label(&s));
    }
    for (name, ptr) in &probe.metadata {
        if app.metadata.iter().any(|m| &m.name == name)
            && let Some(s) = doc.pointer(ptr).and_then(sval)
        {
            out.metadata.insert(name.clone(), Value::String(s));
        }
    }
    Ok(out)
}

/// Read a response body with the 1 MiB cap.
pub async fn read_capped(mut resp: reqwest::Response) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if body.len() + chunk.len() > BODY_MAX {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some(body)
}

pub fn sval_pub(v: &Value) -> Option<String> {
    sval(v)
}
