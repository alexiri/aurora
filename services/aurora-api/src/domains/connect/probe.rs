//! Probe execution (09 §3.4), host/zone checks shared by hook and field flows.
use super::env::Env;
use aurora_policy::HostPattern;
use aurora_registry::index::path_prefix_matches;
use aurora_registry::instance::{InstanceBase, app_path, covers};
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
/// A probe value read from a response header is cut to this many characters.
const HEADER_META_MAX: usize = 500;

/// (P5-UX-2, C-958) Why a probe failed, for the journey's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeCause {
    /// The provider answered 401 or 403: it refused the credential.
    Rejected,
    /// DNS, connect, TLS or timeout: nothing answered.
    Unreachable,
    /// Anything else (another status, a malformed answer, a probe that could not be built).
    Unexpected,
}

impl ProbeCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Unreachable => "unreachable",
            Self::Unexpected => "unexpected",
        }
    }
}

#[derive(Debug)]
pub struct ProbeFail {
    pub status: Option<u16>,
    pub cause: ProbeCause,
    /// The probed host, when one was determined.
    pub host: Option<String>,
}

impl ProbeFail {
    /// A failure with an upstream `status` (or none): 401 and 403 are `rejected`.
    pub fn of(status: Option<u16>) -> Self {
        Self {
            status,
            cause: match status {
                Some(401 | 403) => ProbeCause::Rejected,
                _ => ProbeCause::Unexpected,
            },
            host: None,
        }
    }

    /// Nothing answered `host` (DNS, connect, TLS or timeout).
    pub fn unreachable(host: &str) -> Self {
        Self {
            status: None,
            cause: ProbeCause::Unreachable,
            host: Some(host.to_owned()),
        }
    }

    fn at(mut self, host: &str) -> Self {
        self.host = Some(host.to_owned());
        self
    }
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
    /// The host `{instance}` stands for: the connection's registered instance, or the app's
    /// `default_host` (D-74).
    pub instance_host: Option<&'a str>,
    /// (D-231) The registration's base path; `{instance}` in a URL is `<host><base_path>`.
    pub instance_base_path: Option<&'a str>,
    /// `instance_host` is a *registered* instance (not the static `default_host`): its
    /// `{instance}` host entries apply to it.
    pub instance_registered: bool,
}

fn dns_ok(s: &str) -> bool {
    !s.is_empty()
        && s.parse::<std::net::IpAddr>().is_err()
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
                    (Ns::Instance, _) => {
                        let h = v.instance_host?;
                        if !dns_ok(h) {
                            return None;
                        }
                        out.push_str(h);
                        // In a URL the placeholder is the registration's address; elsewhere (a
                        // header value) it stays the host.
                        if encode && let Some(b) = v.instance_base_path {
                            out.push_str(b);
                        }
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
            instance_host: None,
            instance_base_path: None,
            instance_registered: false,
        },
        true,
    )
}

/// Substitute `{instance}` in a fixed OAuth URL string (`https://{instance}/oauth/token`) with the
/// registration's address, through the one shared resolver (D-231). The result must be https and
/// its host exactly the instance host, no port or userinfo (SEC-04/SEC-59: a token exchange never
/// leaves the instance host).
pub fn instance_url(raw: &str, instance: InstanceBase<'_>) -> Option<String> {
    let url = instance.resolve(raw)?;
    let u = reqwest::Url::parse(&url).ok()?;
    (u.scheme() == "https"
        && u.port().is_none()
        && u.username().is_empty()
        && u.host_str()
            .is_some_and(|h| h.eq_ignore_ascii_case(instance.host)))
    .then_some(url)
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

/// The app's host entry serving `host`/`path`: a static entry, or — only when `instance` is the
/// registered instance host being connected and equals `host` exactly — an `{instance}` entry.
fn host_entry<'a>(
    snap: &'a Snapshot,
    app: &AppEntry,
    host: &str,
    path: &str,
    instance: Option<InstanceBase<'_>>,
) -> Option<&'a HostEntry> {
    // The `{instance}` entries are judged on the app path: the path below the base path (D-231).
    let on_instance = instance
        .filter(|i| i.host.eq_ignore_ascii_case(host) && covers(i.base_path, path))
        .map(|i| app_path(i.base_path, path));
    let statics = snap
        .hosts
        .iter()
        .filter(|e| e.app_id == app.app_id)
        .filter(|e| HostPattern::lenient(&e.host).matches(host, 443))
        .filter(|e| path_prefix_matches(e.path_prefix.as_deref(), path));
    let instanced = on_instance.into_iter().flat_map(|ap| {
        snap.instance_entries(&app.app_id)
            .filter(move |e| path_prefix_matches(e.path_prefix.as_deref(), ap))
    });
    statics
        .chain(instanced)
        .max_by_key(|e| e.path_prefix.as_deref().map_or(0, str::len))
}

/// Does `url` target https on one of the app's declared hosts (or the registered `instance`)?
pub fn declared_host(
    snap: &Snapshot,
    app: &AppEntry,
    url: &str,
    instance: Option<InstanceBase<'_>>,
) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some_and(|h| {
                host_entry(snap, app, &h.to_ascii_lowercase(), u.path(), instance).is_some()
            })
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
    let none = ProbeFail::of(None);
    let raw = render(&probe.url, v, true).ok_or(ProbeFail::of(None))?;
    let parsed = reqwest::Url::parse(&raw).map_err(|_| ProbeFail::of(None))?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if parsed.scheme() != "https" {
        return Err(none);
    }
    let entry = host_entry(
        snap,
        app,
        &host,
        parsed.path(),
        v.instance_host
            .filter(|_| v.instance_registered)
            .map(|h| InstanceBase::new(h, v.instance_base_path)),
    )
    .ok_or(ProbeFail::of(None))?;
    if let Some(t) = &entry.tenant
        && t.field == "connection.tenant_host"
        && v.tenant_host.map(str::to_ascii_lowercase).as_deref() != Some(host.as_str())
    {
        return Err(none);
    }
    let mut url = env.prepare(&raw).map_err(|_| ProbeFail::of(None))?;
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
                    format!("Bearer {}", render(t, v, false).ok_or(ProbeFail::of(None))?),
                ));
            }
            Inject::Basic { user, password } => {
                let u = render(user, v, false).ok_or(ProbeFail::of(None))?;
                let p = render(password, v, false).ok_or(ProbeFail::of(None))?;
                let enc = base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"));
                headers.push(("authorization".into(), format!("Basic {enc}")));
            }
            Inject::Header { name, value } => {
                headers.push((
                    name.clone(),
                    render(value, v, false).ok_or(ProbeFail::of(None))?,
                ));
            }
            Inject::Query { name, value } => {
                let val = render(value, v, false).ok_or(ProbeFail::of(None))?;
                url.query_pairs_mut().append_pair(name, &val);
            }
            _ => {}
        }
    }
    // A registered instance is dialled through the guard that honours `gateway.allowed_private`.
    let client = if v.instance_registered {
        &env.http_instance
    } else {
        &env.http
    };
    let mut req = client
        .request(method, url)
        .timeout(std::time::Duration::from_secs(10));
    for (k, val) in headers {
        if val.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0)) {
            return Err(none);
        }
        req = req.header(k, val);
    }
    // (C-958) DNS, connect, TLS and timeout are `unreachable`; the rest of a failed send is not.
    let mut resp = req.send().await.map_err(|e| {
        if e.is_connect() || e.is_timeout() {
            ProbeFail::unreachable(&host)
        } else {
            ProbeFail::of(None).at(&host)
        }
    })?;
    let status = resp.status().as_u16();
    if !probe.expect_status.contains(&status) {
        return Err(ProbeFail::of(Some(status)).at(&host));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|_| ProbeFail::of(Some(status)).at(&host))?
    {
        if body.len() + chunk.len() > BODY_MAX {
            return Err(ProbeFail::of(Some(status)).at(&host));
        }
        body.extend_from_slice(&chunk);
    }
    let mut out = ProbeOut::default();
    if probe.label.is_none() && probe.metadata.is_empty() {
        return Ok(out);
    }
    let doc: Value =
        serde_json::from_slice(&body).map_err(|_| ProbeFail::of(Some(status)).at(&host))?;
    if let Some(p) = &probe.label {
        out.label = doc.pointer(p).and_then(sval).and_then(|s| clean_label(&s));
    }
    for (name, ptr) in &probe.metadata {
        if !app.metadata.iter().any(|m| &m.name == name) {
            continue;
        }
        // (FIX-133) `header:<name>` reads a response header instead of a JSON pointer: some
        // providers report a token's scopes in `X-OAuth-Scopes`.
        let v = match ptr.strip_prefix("header:") {
            Some(h) => resp
                .headers()
                .get(h)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().chars().take(HEADER_META_MAX).collect::<String>())
                .filter(|s| !s.is_empty() && !s.chars().any(char::is_control)),
            None => doc.pointer(ptr).and_then(sval),
        };
        if let Some(s) = v {
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
