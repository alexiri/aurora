//! (FIX-166, D-368) The wire-level debug log: tracing target `aurora_gateway::wire`, level
//! `debug`, off at the default `info`. Three events per forwarded request (`received`,
//! `forwarded`, `response`), each inside the request's span, so each carries its `request_id`.
//!
//! They show names, lengths and framing — never values (SEC-01, `06 §Telemetry`): a fixed
//! allow-list of framing and identification headers is logged with its value; every other header
//! is `name: <N bytes>`; query parameters are `name(<N>)` (the length of the value); a header,
//! query parameter or path that the gateway added or changed is `<injected>`; body bytes are never
//! read for the log.

use crate::Body;
use http::{HeaderMap, Method, StatusCode, Version};
use hyper::body::Body as _;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// Header values that may be logged: framing and identification, never credentials.
const VALUE_ALLOW: [&str; 14] = [
    "content-length",
    "transfer-encoding",
    "content-type",
    "content-encoding",
    "accept",
    "accept-encoding",
    "user-agent",
    "te",
    "connection",
    "expect",
    "upgrade",
    "host",
    "server",
    "via",
];

/// Longest allow-listed value logged.
const VALUE_MAX: usize = 200;

/// The wire log is on (`aurora_gateway::wire=debug`).
pub(crate) fn enabled() -> bool {
    tracing::enabled!(target: "aurora_gateway::wire", tracing::Level::DEBUG)
}

/// The names of `h` with, for allow-listed ones, the value; a name whose value is not the one in
/// `base` (or absent there) is `<injected>`.
fn header_names(h: &HeaderMap, base: Option<&HeaderMap>) -> String {
    let mut items: Vec<String> = Vec::with_capacity(h.len());
    for (name, value) in h {
        let changed = base.is_some_and(|b| !b.get_all(name).iter().any(|v| v == value));
        let shown = if changed {
            "<injected>".to_owned()
        } else if VALUE_ALLOW.contains(&name.as_str()) {
            let s: String = String::from_utf8_lossy(value.as_bytes())
                .chars()
                .filter(|c| !c.is_control())
                .take(VALUE_MAX)
                .collect();
            s
        } else {
            format!("<{} bytes>", value.len())
        };
        items.push(format!("{}: {shown}", name.as_str()));
    }
    items.sort();
    items.join(", ")
}

/// Query parameter names with their value lengths; one that is not in `base` as sent is
/// `<injected>`. Values are never shown.
fn query_names(q: Option<&str>, base: Option<&str>) -> String {
    let Some(q) = q else {
        return String::new();
    };
    let has_base = base.is_some();
    let base_pairs: Vec<&str> = base.map(|b| b.split('&').collect()).unwrap_or_default();
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (name, value) = p.split_once('=').unwrap_or((p, ""));
            if has_base && !base_pairs.contains(&p) {
                format!("{name}(<injected>)")
            } else {
                format!("{name}({})", value.len())
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// How the body is framed, from the body itself and the `Content-Length`/`Transfer-Encoding`
/// headers that go with it.
fn framing(body: &Body, h: &HeaderMap) -> String {
    let hint = body.size_hint();
    let header = |n: &str| {
        h.get(n)
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
            .unwrap_or_else(|| "-".to_owned())
    };
    let mut s = String::new();
    let _ = write!(
        s,
        "is_end_stream={} size_hint_lower={} size_hint_upper={} content_length={} transfer_encoding={}",
        body.is_end_stream(),
        hint.lower(),
        hint.upper()
            .map_or_else(|| "none".to_owned(), |u| u.to_string()),
        header("content-length"),
        header("transfer-encoding"),
    );
    s
}

fn version_name(v: Version) -> &'static str {
    match v {
        Version::HTTP_09 => "h0.9",
        Version::HTTP_10 => "h1.0",
        Version::HTTP_11 => "h1.1",
        Version::HTTP_2 => "h2",
        Version::HTTP_3 => "h3",
        _ => "unknown",
    }
}

/// What the gateway knows about the request when it decided: for the `forwarded` event.
#[derive(Default)]
pub struct Decided {
    pub decision: String,
    pub app_id: String,
    pub tool_id: String,
    pub connection_id: String,
}

/// The state of one request's wire log; inert (`on == false`) at the default level.
pub(crate) struct Wire {
    on: bool,
    headers: HeaderMap,
    query: Option<String>,
    path: String,
    host: String,
    sent: Option<Instant>,
    up: Option<Upstream>,
    replaced: Option<&'static str>,
}

struct Upstream {
    version: Version,
    status: StatusCode,
    headers: String,
    framing: String,
    latency: Duration,
}

impl Wire {
    pub(crate) fn off() -> Self {
        Self {
            on: false,
            headers: HeaderMap::new(),
            query: None,
            path: String::new(),
            host: String::new(),
            sent: None,
            up: None,
            replaced: None,
        }
    }

    /// Logs `received` (when on) and remembers what came in, to tell later what was injected.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn received(
        lane: &str,
        version: Version,
        method: &Method,
        host: &str,
        port: u16,
        path: &str,
        query: Option<&str>,
        headers: &HeaderMap,
        body: &Body,
    ) -> Self {
        if !enabled() {
            return Self::off();
        }
        tracing::debug!(
            event = "received",
            lane,
            http_version = version_name(version),
            method = %method,
            authority = %format!("{host}:{port}"),
            path,
            query = %query_names(query, None),
            headers = %header_names(headers, None),
            body = %framing(body, headers),
            "received"
        );
        Self {
            on: true,
            headers: headers.clone(),
            query: query.map(str::to_owned),
            path: path.to_owned(),
            host: host.to_owned(),
            sent: None,
            up: None,
            replaced: None,
        }
    }

    /// Logs `forwarded`: the request as it goes upstream, after injection, rewrite and any body
    /// re-wrap.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn forwarded(
        &self,
        scheme: &str,
        host: &str,
        port: u16,
        method: &Method,
        path: &str,
        query: Option<&str>,
        headers: &HeaderMap,
        body: &Body,
        decided: &Decided,
    ) {
        if !self.on {
            return;
        }
        // A path the gateway rewrote may carry an injected value (`path_template`): never shown.
        let shown_path = if path == self.path {
            path.to_owned()
        } else {
            format!("<injected, {} bytes>", path.len())
        };
        tracing::debug!(
            event = "forwarded",
            upstream_authority = %format!("{host}:{port}"),
            host_rewritten = host != self.host,
            scheme,
            asked_version = "auto (h2 via ALPN, else h1)",
            method = %method,
            path = %shown_path,
            query = %query_names(query, Some(self.query.as_deref().unwrap_or(""))),
            headers = %header_names(headers, Some(&self.headers)),
            body = %framing(body, headers),
            decision = %decided.decision,
            app = %decided.app_id,
            tool = %decided.tool_id,
            connection = %decided.connection_id,
            "forwarded"
        );
    }

    /// The upstream answered (or the retry's answer).
    pub(crate) fn started(&mut self) {
        if self.on {
            self.sent = Some(Instant::now());
        }
    }

    pub(crate) fn upstream_answered(&mut self, resp: &http::Response<hyper::body::Incoming>) {
        if !self.on {
            return;
        }
        let h = resp.headers();
        let body_framing = {
            let hint = resp.body().size_hint();
            format!(
                "is_end_stream={} size_hint_lower={} size_hint_upper={} content_length={} transfer_encoding={}",
                resp.body().is_end_stream(),
                hint.lower(),
                hint.upper()
                    .map_or_else(|| "none".to_owned(), |u| u.to_string()),
                h.get("content-length")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("-"),
                h.get("transfer-encoding")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("-"),
            )
        };
        self.up = Some(Upstream {
            version: resp.version(),
            status: resp.status(),
            headers: header_names(h, None),
            framing: body_framing,
            latency: self.sent.map(|s| s.elapsed()).unwrap_or_default(),
        });
    }

    /// The gateway answers with something other than the upstream's response.
    pub(crate) fn replaced(&mut self, why: &'static str) {
        if self.on {
            self.replaced = Some(why);
        }
    }

    /// Logs `response` once the answer to the agent is known.
    pub(crate) fn response(&self, answered: StatusCode) {
        if !self.on {
            return;
        }
        match &self.up {
            Some(u) => tracing::debug!(
                event = "response",
                upstream_status = u.status.as_u16(),
                negotiated_protocol = version_name(u.version),
                response_headers = %u.headers,
                body = %u.framing,
                latency_ms = u64::try_from(u.latency.as_millis()).unwrap_or(u64::MAX),
                replaced = self.replaced.is_some(),
                replaced_by = self.replaced.unwrap_or("-"),
                answered_status = answered.as_u16(),
                "response"
            ),
            None => tracing::debug!(
                event = "response",
                upstream_status = "none",
                replaced = self.replaced.is_some(),
                replaced_by = self.replaced.unwrap_or("-"),
                answered_status = answered.as_u16(),
                "response"
            ),
        }
    }
}
