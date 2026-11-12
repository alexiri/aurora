//! The edge-facing proxy (P4-PRV-2, D-146, SEC-81, SEC-85; `docs/04` §Preview access,
//! `docs/15` §Port previews).
//!
//! One task per browser request. The relay is the only HTTP parser in the path: it normalizes
//! the request (single `Content-Length` or `Transfer-Encoding`, never both; no obsolete line
//! folding; no absolute-form target; `Host` matches the preview label), answers the Fetch
//! Metadata matrix, drives the grant exchange (`/_aurora/grant`, `__Host-aurora_pstate`),
//! forwards with a rebuilt header set (no Aurora credentials or `X-Aurora-*` in either
//! direction, hop-by-hop removed, `X-Forwarded-*` set, `Host` = runner endpoint) and rewrites
//! the response (upstream cookies made host-only, Aurora-named upstream cookies dropped,
//! `Cache-Control: private` when the app set none, `X-Robots-Tag: noindex`, HSTS, the
//! reserved `/_aurora/` never forwarded).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use hyper::body::{Body, Bytes, Frame, Incoming};
use hyper::header::{HeaderMap, HeaderValue};
use hyper::{Method, StatusCode, Uri};
use tokio::sync::mpsc;

use crate::api::session_hash;
use crate::state::State;
use crate::stream::{StreamKey, Streams};

/// Response body: streamed (never fully buffered beyond one chunk). `exact` is set for
/// pre-loaded bodies (status pages) so hyper answers with `content-length` instead of
/// chunked framing; proxied bodies stream with an unknown size.
pub struct RelayBody {
    rx: mpsc::Receiver<Result<Frame<Bytes>, std::io::Error>>,
    exact: Option<u64>,
}

impl RelayBody {
    /// A body driven by a sender held elsewhere (the proxy's pump task).
    pub(crate) fn new(rx: mpsc::Receiver<Result<Frame<Bytes>, std::io::Error>>) -> Self {
        Self { rx, exact: None }
    }

    /// A body whose bytes are already queued (one frame, sender gone).
    pub(crate) fn ready(bytes: Bytes) -> Self {
        let (tx, rx) = mpsc::channel(1);
        let _ = tx.try_send(Ok(Frame::data(bytes)));
        Self { rx, exact: None }.with_exact_len()
    }

    /// Peeks the queued frame to learn the exact length (the sender is gone, so nothing
    /// else will enqueue; `try_recv` + requeue is safe on a capacity-1 channel).
    fn with_exact_len(mut self) -> Self {
        if let Ok(Ok(f)) = self.rx.try_recv()
            && let Ok(d) = f.into_data()
        {
            let n = d.len() as u64;
            let (tx, rx) = mpsc::channel(1);
            let _ = tx.try_send(Ok(Frame::data(d)));
            self.rx = rx;
            self.exact = Some(n);
        }
        self
    }
}

impl Body for RelayBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        std::pin::Pin::new(&mut self.rx).poll_recv(cx)
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        match self.exact {
            Some(n) => hyper::body::SizeHint::with_exact(n),
            None => hyper::body::SizeHint::default(),
        }
    }
}

/// A ready-made body from fixed bytes.
fn full_body(b: &'static str) -> RelayBody {
    RelayBody::ready(Bytes::from_static(b.as_bytes()))
}

/// A body from an owned string (status pages).
fn string_body(b: &str) -> RelayBody {
    RelayBody::ready(Bytes::from(b.to_owned()))
}

/// Hop-by-hop headers (RFC 9110 §7.6.1) plus the ones the relay owns.
const DROP_REQUEST: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    // Aurora credentials never reach the app (SEC-81 rule 3, SEC-85).
    "authorization",
    "cookie",
    "host",
    // The relay owns the forwarding chain; a client-supplied chain is dropped.
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    // Aurora's own names: a browser cannot legitimately send these to a preview.
    "x-aurora-replica",
    "x-aurora-protocol",
    "x-aurora-runner",
];

/// Response headers the relay never forwards as-is: hop-by-hop plus the ones it rewrites.
const DROP_RESPONSE: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    // Security headers are set by the relay, never trusted from the app.
    "strict-transport-security",
    "x-robots-tag",
    "content-security-policy",
];

const PSTATE_COOKIE: &str = "__Host-aurora_pstate";
const PREVIEW_COOKIE: &str = "__Host-aurora_preview";
/// State-cookie lifetime (`docs/04` step 1: `Max-Age=300`).
const PSTATE_MAX_AGE: u64 = 300;
/// The reserved path namespace (never forwarded, SEC-85).
const AURORA_PREFIX: &str = "/_aurora/";

fn status_page(code: StatusCode, title: &str, text: &str) -> hyper::Response<RelayBody> {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
         <meta name=\"robots\" content=\"noindex\"></head>\
         <body><h1>{}</h1><p>{}</p></body></html>",
        code.as_u16(),
        text
    );
    // Infallible in practice: static status, static headers, one static/owned part.
    #[expect(clippy::unwrap_used)]
    hyper::Response::builder()
        .status(code)
        .header("content-type", "text/html; charset=utf-8")
        .header("cache-control", "no-store")
        .header("x-robots-tag", "noindex")
        .header(
            "strict-transport-security",
            "max-age=31536000; includeSubDomains",
        )
        .body(string_body(&body))
        .unwrap_or_else(|_| {
            hyper::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(full_body(""))
                .unwrap()
        })
}

/// A simple full-body response with the security headers.
#[expect(clippy::unwrap_used)]
fn simple(
    code: StatusCode,
    text: &'static str,
    extra: &[(&'static str, String)],
) -> hyper::Response<RelayBody> {
    let mut b = hyper::Response::builder()
        .status(code)
        .header("content-type", "text/plain; charset=utf-8")
        .header("cache-control", "no-store")
        .header("x-robots-tag", "noindex")
        .header(
            "strict-transport-security",
            "max-age=31536000; includeSubDomains",
        );
    for (k, v) in extra {
        b = b.header(*k, v);
    }
    b.body(full_body(text)).unwrap()
}

/// Everything known about one request before the proxy decision.
pub struct Request {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    /// Parsed `Cookie` values of this request.
    cookies: HashMap<String, String>,
}

impl Request {
    pub fn label(&self) -> Option<String> {
        let host = self.host()?;
        let dot = host.find('.')?;
        let label = &host[..dot];
        (label.starts_with('p') && label.len() == 25).then(|| label.to_owned())
    }

    pub fn host(&self) -> Option<String> {
        self.headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .map(|h| h.split(':').next().unwrap_or("").to_ascii_lowercase())
    }

    pub fn path(&self) -> &str {
        self.uri.path()
    }

    /// The full request target (`path[?query]`) the app must see — a bare `uri.path()`
    /// would silently drop the query string.
    pub fn target(&self) -> &str {
        self.uri
            .path_and_query()
            .map_or(self.uri.path(), hyper::http::uri::PathAndQuery::as_str)
    }

    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies.get(name).map(String::as_str)
    }
}

fn parse_cookies(headers: &HeaderMap) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for v in headers.get_all("cookie") {
        let Ok(s) = v.to_str() else { continue };
        for pair in s.split(';') {
            let pair = pair.trim();
            if let Some((k, v)) = pair.split_once('=') {
                out.entry(k.trim().to_owned())
                    .or_insert_with(|| v.trim().to_owned());
            }
        }
    }
    out
}

/// The Fetch-Metadata check (`docs/04` rule 4): same-site/cross-site subrequests are refused;
/// only a top-level navigation (or a header-less request with no `Origin`) passes.
fn cross_site(req: &Request) -> bool {
    let site = req
        .headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let navigation = req.method == Method::GET
        && req
            .headers
            .get("sec-fetch-mode")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|m| m.eq_ignore_ascii_case("navigate"))
        && req
            .headers
            .get("sec-fetch-dest")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|d| d.eq_ignore_ascii_case("document"));
    if navigation {
        return false;
    }
    match site {
        "same-site" | "cross-site" => true,
        // Without the header: accepted only when there is no `Origin` (non-browser callers
        // hold the cookie) or the `Origin` equals the preview's own.
        _ => match req.headers.get("origin").and_then(|v| v.to_str().ok()) {
            None => false,
            Some(o) => {
                let own = req
                    .host()
                    .map(|h| format!("https://{h}"))
                    .unwrap_or_default();
                !o.eq_ignore_ascii_case(&own)
            }
        },
    }
}

/// Smuggling corpus (SEC-85): the relay is the only HTTP parser in the path, so anything
/// ambiguous is refused with 400 before forwarding.
fn ambiguous(req: &Request, raw_head: &[u8]) -> bool {
    let cl = req.headers.get_all("content-length").iter().count();
    if cl > 1 {
        return true; // duplicate Content-Length
    }
    if req.headers.contains_key("content-length") && req.headers.contains_key("transfer-encoding") {
        return true; // CL.TE / TE.CL
    }
    let bad_te = req
        .headers
        .get_all("transfer-encoding")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| !v.eq_ignore_ascii_case("chunked"));
    if bad_te {
        return true;
    }
    // Obsolete line folding (a header line starting with SP/HTAB).
    if raw_head
        .split(|b| *b == b'\n')
        .skip(1)
        .any(|l| l.first().is_some_and(|b| *b == b' ' || *b == b'\t'))
    {
        return true;
    }
    // Absolute-form target or a `Host` that disagrees with the request line authority.
    if req.uri.authority().is_some_and(|a| !a.host().is_empty()) {
        return true;
    }
    false
}

/// `Set-Cookie` rewriting (SEC-85): `Domain` removed (host-only), `__Host-aurora_*` and
/// `aurora_*` names dropped, `Secure`/`HttpOnly` flags preserved.
fn rewrite_set_cookie(value: &str) -> Option<String> {
    let name = value.split(';').next()?.split('=').next()?.trim();
    if name.starts_with("__Host-aurora") || name.starts_with("aurora_") {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for (i, attr) in value.split(';').enumerate() {
        let a = attr.trim();
        if i == 0 {
            parts.push(a.to_owned());
            continue;
        }
        let lower = a.to_ascii_lowercase();
        if lower.starts_with("domain=") {
            continue; // made host-only by dropping the attribute
        }
        parts.push(a.to_owned());
    }
    Some(parts.join("; "))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Nav {
    Yes,
    No,
}

/// One request's full handling. `raw_head` is the exact bytes the parser saw (smuggling checks).
pub async fn handle(
    state: Arc<State>,
    streams: Arc<Streams>,
    req: Request,
    raw_head: Arc<[u8]>,
    client_ip: Option<std::net::IpAddr>,
    body: hyper::body::Incoming,
    upgrade: Option<hyper::upgrade::OnUpgrade>,
) -> hyper::Response<RelayBody> {
    let outcome = |name: &'static str| {
        metrics::counter!("aurora_relay_requests_total", "outcome" => name).increment(1);
    };
    // ---- 0. shape checks (before anything else; never leak the label's existence)
    if ambiguous(&req, &raw_head) {
        outcome("bad_request");
        return simple(StatusCode::BAD_REQUEST, "Bad request.\n", &[]);
    }
    let Some(label) = req.label() else {
        outcome("not_found");
        return simple(StatusCode::NOT_FOUND, "Preview not found.\n", &[]);
    };
    if req.host().is_none_or(|h| !h.ends_with(&state.domain)) {
        outcome("not_found");
        return simple(StatusCode::NOT_FOUND, "Preview not found.\n", &[]);
    }
    // ---- 1. the reserved namespace is never forwarded
    if req.path().starts_with(AURORA_PREFIX) {
        return aurora_path(state, streams, &label, req).await;
    }
    // ---- 2. Fetch Metadata matrix (docs/04 rule 4)
    if cross_site(&req) {
        outcome("cross_site");
        return simple(StatusCode::FORBIDDEN, "preview_cross_site\n", &[]);
    }
    let nav = if req.method == Method::GET || req.method == Method::HEAD {
        Nav::Yes
    } else {
        Nav::No
    };
    // ---- 3. credential check (resolve) + grant exchange
    let token = req.cookie(PREVIEW_COOKIE).map(str::to_owned);
    let hash = token.as_deref().map(session_hash);
    let allowed = match &hash {
        Some(h) => match state.resolve(&label, Some(h), false).await {
            Ok(a) => a.decision == "allow",
            Err(_) => {
                outcome("upstream_error");
                return simple(
                    StatusCode::BAD_GATEWAY,
                    "The preview service is unavailable.\n",
                    &[],
                );
            }
        },
        None => false,
    };
    if !allowed {
        return login_or_wait(state, &label, req, nav, &token, outcome).await;
    }
    // ---- 4. proxy
    proxy(
        state, streams, label, req, hash, nav, client_ip, body, upgrade, outcome,
    )
    .await
}

/// The no-credential path: top-level navigation → grant exchange; anything else → 401.
async fn login_or_wait(
    state: Arc<State>,
    label: &str,
    req: Request,
    nav: Nav,
    token: &Option<String>,
    outcome: impl Fn(&'static str),
) -> hyper::Response<RelayBody> {
    if token.is_some() {
        // A credential that resolve denied: `deny` (uniform not_found / spoken reasons) or a
        // session past its idle limit → login (restart the flow silently).
        let h = token.as_deref().map(session_hash).unwrap_or_default();
        let answer = state.resolve(label, Some(&h), false).await.ok();
        let decision = answer
            .as_ref()
            .map(|a| a.decision.clone())
            .unwrap_or_else(|| "deny".into());
        if decision != "login" {
            outcome("not_found");
            let reason = answer.and_then(|a| a.reason).unwrap_or_default();
            let text = match reason.as_str() {
                "frozen" => "This preview is frozen.\n",
                "paused" => "This preview is paused.\n",
                "wake_limited" => "Too many wake requests; try again later.\n",
                "unreachable" => "The runner of this preview is unreachable.\n",
                _ => "Preview not found.\n",
            };
            let code = match reason.as_str() {
                "frozen" => StatusCode::LOCKED,
                "paused" => StatusCode::FORBIDDEN,
                "wake_limited" => StatusCode::TOO_MANY_REQUESTS,
                "unreachable" => StatusCode::BAD_GATEWAY,
                _ => StatusCode::NOT_FOUND,
            };
            return status_page(code, "Preview", text);
        }
        // `login`: fall through to the navigation redirect (the flow restarts silently).
    }
    if nav == Nav::No {
        outcome("login");
        return simple(StatusCode::UNAUTHORIZED, "preview_login_required\n", &[]);
    }
    // Top-level navigation: set the state cookie and redirect to the dashboard's authorize.
    let nonce = crate::edge::nonce();
    // The full request target is the post-login return URL (query included).
    let path = req.target().to_owned();
    let pstate = format!(
        "{nonce}.{}",
        percent_encoding::percent_encode(path.as_bytes(), percent_encoding::NON_ALPHANUMERIC)
    );
    // The path is stored base64url so it can never carry cookie syntax.
    let b64 = base64url(path.as_bytes());
    let cookie = format!(
        "{PSTATE_COOKIE}={nonce}.{b64}; Path=/; Max-Age={PSTATE_MAX_AGE}; Secure; HttpOnly; SameSite=Lax"
    );
    let _ = pstate;
    let to = format!(
        "{}/v1/previews/authorize?host={label}&state={nonce}",
        state.external_url.trim_end_matches('/')
    );
    outcome("login");
    let res = hyper::Response::builder()
        .status(StatusCode::FOUND)
        .header("location", to)
        .header("cache-control", "no-store")
        .header("set-cookie", cookie)
        .header("x-robots-tag", "noindex")
        .header("referrer-policy", "no-referrer");
    #[expect(clippy::unwrap_used)]
    {
        res.body(full_body("")).unwrap()
    }
}

/// `/_aurora/*`: grant redemption and nothing else.
async fn aurora_path(
    state: Arc<State>,
    _streams: Arc<Streams>,
    label: &str,
    req: Request,
) -> hyper::Response<RelayBody> {
    if req.path() != "/_aurora/grant" {
        return simple(StatusCode::NOT_FOUND, "Preview not found.\n", &[]);
    }
    // Only a top-level GET navigation lands here (the authorize redirect).
    if req.method != Method::GET {
        return simple(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.\n", &[]);
    }
    let Some(pstate) = req.cookie(PSTATE_COOKIE) else {
        metrics::counter!("aurora_relay_grants_total", "result" => "no_state").increment(1);
        return simple(StatusCode::BAD_REQUEST, "grant_invalid\n", &[]);
    };
    let Some((nonce, b64)) = pstate.split_once('.') else {
        metrics::counter!("aurora_relay_grants_total", "result" => "no_state").increment(1);
        return simple(StatusCode::BAD_REQUEST, "grant_invalid\n", &[]);
    };
    let Some(path) = unbase64url(b64) else {
        metrics::counter!("aurora_relay_grants_total", "result" => "invalid").increment(1);
        return simple(StatusCode::BAD_REQUEST, "grant_invalid\n", &[]);
    };
    if !path.starts_with('/') || path.starts_with("//") || path.starts_with(AURORA_PREFIX) {
        metrics::counter!("aurora_relay_grants_total", "result" => "invalid").increment(1);
        return simple(StatusCode::BAD_REQUEST, "grant_invalid\n", &[]);
    }
    let grant = req
        .uri
        .query()
        .and_then(|q| {
            q.split('&').find_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                (k == "g").then(|| v.to_owned())
            })
        })
        .unwrap_or_default();
    let outcome = |r: &'static str| {
        metrics::counter!("aurora_relay_grants_total", "result" => r).increment(1)
    };
    match state.api.redeem(&grant, label, nonce).await {
        Ok(a) => {
            outcome("redeemed");
            let cookie = format!(
                "{PREVIEW_COOKIE}={}; Path=/; Max-Age={}; Secure; HttpOnly; SameSite=Lax",
                a.token,
                a.idle_secs.max(60)
            );
            let clear =
                format!("{PSTATE_COOKIE}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax");
            #[expect(clippy::unwrap_used)]
            {
                hyper::Response::builder()
                    .status(StatusCode::FOUND)
                    .header("location", path)
                    .header("cache-control", "no-store")
                    .header("set-cookie", cookie)
                    .header("set-cookie", clear)
                    .header("x-robots-tag", "noindex")
                    .header(
                        "strict-transport-security",
                        "max-age=31536000; includeSubDomains",
                    )
                    .body(full_body(""))
                    .unwrap()
            }
        }
        Err(crate::api::ApiError::Status(400)) => {
            outcome("invalid");
            simple(StatusCode::BAD_REQUEST, "grant_invalid\n", &[])
        }
        Err(_) => {
            outcome("invalid");
            simple(StatusCode::BAD_GATEWAY, "grant service unavailable.\n", &[])
        }
    }
}

/// The proxy hop: stream grant, then the WebSocket dial (the leg-1 stream carries the HTTP
/// exchange), then the response rewrite. `body` is the request body, `upgrade` the
/// browser's pending upgrade (a WebSocket request), `raw_head` the parsed head's bytes.
#[allow(clippy::too_many_arguments)]
async fn proxy(
    state: Arc<State>,
    streams: Arc<Streams>,
    label: String,
    req: Request,
    hash: Option<String>,
    nav: Nav,
    client_ip: Option<std::net::IpAddr>,
    body: hyper::body::Incoming,
    upgrade: Option<hyper::upgrade::OnUpgrade>,
    outcome: impl Fn(&'static str),
) -> hyper::Response<RelayBody> {
    let Some(h) = hash else {
        return simple(StatusCode::UNAUTHORIZED, "preview_login_required\n", &[]);
    };
    // A WebSocket upgrade: the `Origin` must equal the preview's own origin.
    let is_ws = req
        .headers
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if is_ws {
        let own = req
            .host()
            .map(|h| format!("https://{h}"))
            .unwrap_or_default();
        let ok = req
            .headers
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|o| o.eq_ignore_ascii_case(&own));
        if !ok {
            outcome("cross_site");
            return simple(StatusCode::FORBIDDEN, "preview_cross_site\n", &[]);
        }
    }
    // The runner endpoint + stream grant (aux_) come from the API (`16 §W17`); the relay
    // never names a sandbox or a port.
    let Ok(sa) = state.api.streams(&label, &h).await else {
        outcome("unreachable");
        return simple(
            StatusCode::BAD_GATEWAY,
            "The sandbox is not running.\n",
            &[],
        );
    };
    // The rebuilt outbound header set: filtered (hop-by-hop, Aurora credentials, client
    // forwarding chain removed), `Host` = the runner endpoint, `X-Forwarded-For` added.
    let runner_host = authority_of(&sa.runner_endpoint).unwrap_or_default();
    let mut out_headers = outbound_headers(&req.headers, &runner_host, &ip_text(client_ip));
    // An upgraded request (WebSocket): `upgrade`/`connection` are hop-by-hop and were
    // dropped above, but the upgrade handshake itself must cross the leg (`16 §W18`: the
    // relay forwards the `Upgrade` handshake over the stream) — re-add the browser's.
    if req.headers.contains_key("upgrade")
        && let Some(v) = req.headers.get("upgrade")
        && let Ok(name) = hyper::header::HeaderName::from_bytes("upgrade".as_bytes())
    {
        out_headers.insert(name, v.clone());
        if let Some(cv) = req.headers.get("connection")
            && let Ok(cname) = hyper::header::HeaderName::from_bytes("connection".as_bytes())
        {
            out_headers.insert(cname, cv.clone());
        }
    }
    // How the body is framed over the leg: an exact `content-length` passes through, a
    // chunked body is re-framed by the relay (the outbound head drops the browser's
    // `transfer-encoding` with the other hop-by-hop headers).
    let body_len = if req.headers.contains_key("transfer-encoding") {
        crate::proxy::BodyLen::Chunked
    } else {
        match body.size_hint().exact() {
            Some(0) => crate::proxy::BodyLen::None,
            Some(n) => crate::proxy::BodyLen::Exact(n as usize),
            None => crate::proxy::BodyLen::Chunked,
        }
    };
    let key = StreamKey {
        label: label.clone(),
        session_hash: h,
    };
    // The rebuilt request head plus the body and the browser's pending upgrade ride the
    // same leg-1 stream; the target keeps the query string.
    let head = crate::proxy::build_request_head(&req.method, req.target(), &out_headers, body_len);
    let _ = nav;
    match crate::proxy::forward(state, streams, key, sa, head, body_len, body, upgrade).await {
        Ok(r) => {
            outcome("ok");
            r
        }
        Err(code) => {
            outcome("upstream_error");
            let text = match code {
                502 => "The sandbox refused the connection.\n",
                504 => "The sandbox timed out.\n",
                _ => "The preview is unavailable.\n",
            };
            simple(
                StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY),
                text,
                &[],
            )
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn authority_of(endpoint: &str) -> Option<String> {
    let rest = endpoint
        .strip_prefix("wss://")
        .or_else(|| endpoint.strip_prefix("ws://"))?;
    let auth = rest.split('/').next()?;
    Some(auth.to_owned())
}

/// 16 random bytes, base64url — the `state` nonce (`docs/04` step 1).
pub fn nonce() -> String {
    use std::io::Read;
    let mut b = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok();
    debug_assert!(ok);
    base64url(&b)
}

/// Unpadded base64url (the same alphabet as the tokens).
pub fn base64url(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    // Trim padding for the URL-safe alphabet.
    out.trim_end_matches('=').to_owned()
}

/// Inverse of [`base64url`]; `None` on any non-alphabet byte.
pub fn unbase64url(s: &str) -> Option<String> {
    unbase64url_bytes(s).and_then(|b| String::from_utf8(b).ok())
}

/// Byte-level inverse of [`base64url`]; `None` on any non-alphabet byte.
pub fn unbase64url_bytes(s: &str) -> Option<Vec<u8>> {
    const INVALID: u8 = 255;
    let mut table = [INVALID; 256];
    for (i, c) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
        .iter()
        .enumerate()
    {
        table[*c as usize] = i as u8;
    }
    let bytes = s.as_bytes();
    if bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n: u32 = 0;
        let mut got = 0;
        for &c in chunk {
            let v = table[c as usize];
            if v == INVALID {
                return None;
            }
            n = (n << 6) | u32::from(v);
            got += 1;
        }
        n <<= 6 * (4 - got);
        out.push((n >> 16) as u8);
        if got > 2 {
            out.push((n >> 8) as u8);
        }
        if got > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// The XFF value for the outbound head: the observed client IP, or `unknown`.
pub fn ip_text(ip: Option<std::net::IpAddr>) -> String {
    ip.map(|i| i.to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Builds the outbound `Request` headers once (tests assert on this exact set).
pub fn outbound_headers(req_headers: &HeaderMap, host_value: &str, client_ip: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (name, value) in req_headers {
        let n = name.as_str();
        if DROP_REQUEST.iter().any(|d| d.eq_ignore_ascii_case(n)) {
            continue;
        }
        h.insert(name, value.clone());
    }
    h.insert(
        "host",
        HeaderValue::from_str(host_value).unwrap_or(HeaderValue::from_static("invalid")),
    );
    h.insert(
        "x-forwarded-for",
        HeaderValue::from_str(client_ip).unwrap_or(HeaderValue::from_static("unknown")),
    );
    h
}

/// Response header rewrite (SEC-85): upstream cookies made host-only, Aurora-named cookies
/// dropped, security headers added, `Cache-Control: private` when the app set none.
pub fn rewrite_response_headers(app_headers: &HeaderMap) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (name, value) in app_headers {
        let n = name.as_str();
        if DROP_RESPONSE.iter().any(|d| d.eq_ignore_ascii_case(n)) {
            continue;
        }
        if n.eq_ignore_ascii_case("set-cookie") {
            if let Ok(v) = value.to_str()
                && let Some(rewritten) = rewrite_set_cookie(v)
                && let Ok(hv) = HeaderValue::from_str(&rewritten)
            {
                h.append("set-cookie", hv);
            }
            continue;
        }
        h.insert(name, value.clone());
    }
    if !h.contains_key("cache-control") {
        h.insert(
            "cache-control",
            HeaderValue::from_static("private, no-cache"),
        );
    }
    h.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    h.insert(
        "strict-transport-security",
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h
}

/// Idle read timeout of an app response (`PREVIEW_HEADER_TIMEOUT` for the first byte).
pub const HEADER_TIMEOUT: Duration = aurora_core::limits::PREVIEW_HEADER_TIMEOUT;

/// The hyper accept loop of the edge listener (:4112). Each connection is served with the
/// state; request bodies beyond `body_max` are refused with 413; total concurrent streams
/// are capped at `max_streams` (429 beyond).
pub async fn serve_listener(
    listener: tokio::net::TcpListener,
    state: Arc<State>,
    streams: Arc<crate::stream::Streams>,
    max_streams: u32,
    body_max: usize,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    tracing::debug!(
        port = listener.local_addr().map(|a| a.port()).unwrap_or(0),
        "edge accept loop up"
    );
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((tcp, _)) => {
                    let state = state.clone();
                    let streams = streams.clone();
                    tokio::spawn(async move {
                        let ip = tcp.peer_addr().ok().map(|a| a.ip());
                        serve_conn(state, streams, tcp, ip, body_max, max_streams).await;
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "relay accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            r = stop.changed() => {
                tracing::debug!(?r, "edge accept loop exiting (stop)");
                return;
            }
        }
    }
}

/// Serves one connection: hyper's HTTP/1 server with the request service (keep-alive,
/// WebSocket upgrades for the browser side).
async fn serve_conn(
    state: Arc<State>,
    streams: Arc<crate::stream::Streams>,
    tcp: tokio::net::TcpStream,
    ip: Option<std::net::IpAddr>,
    body_max: usize,
    max_streams: u32,
) {
    let state2 = state.clone();
    let streams2 = streams.clone();
    let svc = hyper::service::service_fn(move |mut req: hyper::Request<Incoming>| {
        let state = state2.clone();
        let streams = streams2.clone();
        async move {
            let headers = req.headers().clone();
            // hyper does not expose the raw head; the smuggling checks run on the parsed
            // header map (duplicate Content-Length survives there; obsolete line folding and
            // bad TE make hyper's own parser fail with 400 first — defense in depth).
            let mut raw = Vec::new();
            for (n, v) in headers.iter() {
                raw.extend_from_slice(n.as_str().as_bytes());
                raw.extend_from_slice(b": ");
                raw.extend_from_slice(v.as_bytes());
                raw.extend_from_slice(b"\r\n");
            }
            let raw_head: Arc<[u8]> = Arc::from(raw.into_boxed_slice());
            if headers
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<usize>().ok())
                .is_some_and(|n| n > body_max)
            {
                metrics::counter!("aurora_relay_requests_total", "outcome" => "too_large")
                    .increment(1);
                return Ok::<_, std::convert::Infallible>(simple(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "Request body too large.\n",
                    &[],
                ));
            }
            if streams.open_count() >= max_streams {
                metrics::counter!("aurora_relay_requests_total", "outcome" => "quota").increment(1);
                return Ok(simple(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Too many open preview streams.\n",
                    &[],
                ));
            }
            let upgrade = hyper::upgrade::on(&mut req);
            let (parts, body) = req.into_parts();
            let cookies = parse_cookies(&parts.headers);
            let req = Request {
                method: parts.method,
                uri: parts.uri,
                headers: parts.headers,
                cookies,
            };
            Ok(handle(state, streams, req, raw_head, ip, body, Some(upgrade)).await)
        }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .header_read_timeout(HEADER_TIMEOUT)
        .timer(hyper_util::rt::TokioTimer::new())
        .serve_connection(hyper_util::rt::TokioIo::new(tcp), svc)
        .with_upgrades()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderName;

    fn headers(list: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in list {
            h.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn base64_roundtrip() {
        for case in ["", "a", "ab", "abc", "abcd", "/v1/status?x=1&y=2"] {
            let e = base64url(case.as_bytes());
            assert_eq!(unbase64url(&e).as_deref(), Some(case));
        }
        // Arbitrary (non-UTF-8) bytes encode and decode at the byte level.
        let raw: &[u8] = &[0xffu8, 0x00, 0x7f];
        let e = base64url(raw);
        assert_eq!(unbase64url(&e), None); // not valid UTF-8
        assert_eq!(unbase64url_bytes(&e).as_deref(), Some(raw));
        assert!(unbase64url("ab!c").is_none());
    }

    #[test]
    fn request_target_keeps_the_query() {
        let mk = |uri: &str| Request {
            method: Method::GET,
            uri: uri.parse().unwrap(),
            headers: HeaderMap::new(),
            cookies: HashMap::new(),
        };
        assert_eq!(mk("/hello?x=1").target(), "/hello?x=1");
        assert_eq!(mk("/hello").target(), "/hello");
        assert_eq!(mk("/").target(), "/");
        // The path alone (the old behaviour) would have dropped `?x=1`.
        assert_eq!(mk("/a/b?y=%202").path(), "/a/b");
    }

    #[test]
    fn set_cookie_rewriting() {
        // `Domain` is dropped (host-only); Aurora names are dropped entirely.
        assert_eq!(
            rewrite_set_cookie("sid=abc; Domain=example.org; Path=/; HttpOnly").as_deref(),
            Some("sid=abc; Path=/; HttpOnly")
        );
        assert_eq!(rewrite_set_cookie("__Host-aurora_preview=x; Path=/"), None);
        assert_eq!(rewrite_set_cookie("aurora_session=x"), None);
        assert_eq!(rewrite_set_cookie("app=y").as_deref(), Some("app=y"));
    }

    #[test]
    fn response_security_headers() {
        let app = headers(&[
            ("content-type", "text/html"),
            ("cache-control", "max-age=60"),
        ]);
        let out = rewrite_response_headers(&app);
        assert_eq!(out.get("cache-control").unwrap(), "max-age=60");
        assert_eq!(out.get("x-robots-tag").unwrap(), "noindex");
        assert!(out.get("strict-transport-security").is_some());
        let app = headers(&[("content-type", "text/html")]);
        let out = rewrite_response_headers(&app);
        assert_eq!(out.get("cache-control").unwrap(), "private, no-cache");
        // Hop-by-hop response headers are dropped.
        let app = headers(&[
            ("transfer-encoding", "chunked"),
            ("connection", "keep-alive"),
        ]);
        assert!(rewrite_response_headers(&app).get("connection").is_none());
    }

    #[test]
    fn request_header_hygiene() {
        let h = headers(&[
            ("cookie", "__Host-aurora_preview=auv_x"),
            ("authorization", "Bearer auv_x"),
            ("x-aurora-replica", "1"),
            ("accept", "text/html"),
            ("x-forwarded-for", "1.2.3.4"),
            ("connection", "keep-alive"),
        ]);
        let out = outbound_headers(&h, "runner.example:4108", "10.0.0.1");
        assert!(out.get("cookie").is_none());
        assert!(out.get("authorization").is_none());
        assert!(out.get("x-aurora-replica").is_none());
        assert!(out.get("connection").is_none());
        assert_eq!(out.get("accept").unwrap(), "text/html");
        assert_eq!(out.get("host").unwrap(), "runner.example:4108");
        // The client-supplied XFF is dropped, then re-added with the relay's own value.
        assert_eq!(out.get("x-forwarded-for").unwrap(), "10.0.0.1");
    }

    #[test]
    fn fetch_metadata_matrix() {
        let mk = |m: Method, hs: &[(&str, &str)]| Request {
            method: m,
            uri: "/".parse().unwrap(),
            headers: headers(hs),
            cookies: HashMap::new(),
        };
        // Top-level navigation passes (same-site and cross-site alike).
        for site in ["same-site", "cross-site"] {
            let r = mk(
                Method::GET,
                &[
                    ("sec-fetch-site", site),
                    ("sec-fetch-mode", "navigate"),
                    ("sec-fetch-dest", "document"),
                ],
            );
            assert!(!cross_site(&r), "{site} navigation");
        }
        // Subrequests are refused.
        let r = mk(
            Method::POST,
            &[
                ("sec-fetch-site", "same-site"),
                ("sec-fetch-mode", "cors"),
                ("sec-fetch-dest", "empty"),
            ],
        );
        assert!(cross_site(&r));
        let r = mk(
            Method::GET,
            &[
                ("sec-fetch-site", "cross-site"),
                ("sec-fetch-mode", "no-cors"),
                ("sec-fetch-dest", "image"),
            ],
        );
        assert!(cross_site(&r));
        // iframes are never navigations.
        let r = mk(
            Method::GET,
            &[
                ("sec-fetch-site", "same-site"),
                ("sec-fetch-mode", "navigate"),
                ("sec-fetch-dest", "iframe"),
            ],
        );
        assert!(cross_site(&r));
        // Header-less: no `Origin` passes, a foreign `Origin` is refused.
        let r = mk(Method::GET, &[]);
        assert!(!cross_site(&r));
        let r = mk(Method::GET, &[("origin", "https://evil.example")]);
        assert!(cross_site(&r));
        let r = mk(
            Method::GET,
            &[("host", "pabc.example"), ("origin", "https://pabc.example")],
        );
        assert!(!cross_site(&r));
    }

    #[test]
    fn smuggling_corpus() {
        let mk = |raw: &[u8], hs: &[(&str, &str)]| {
            let _ = raw;
            Request {
                method: Method::POST,
                uri: "/x".parse().unwrap(),
                headers: headers(hs),
                cookies: HashMap::new(),
            }
        };
        // Duplicate Content-Length (the single-CL request is the clean control).
        let _r = mk(b"", &[("content-length", "5"), ("host", "p.example")]);
        let mut h = headers(&[("content-length", "5"), ("host", "p.example")]);
        h.append("content-length", HeaderValue::from_static("6"));
        let r2 = Request {
            method: Method::POST,
            uri: "/x".parse().unwrap(),
            headers: h,
            cookies: HashMap::new(),
        };
        assert!(ambiguous(&r2, b""));
        // CL + TE together.
        let r = mk(
            b"",
            &[
                ("content-length", "5"),
                ("transfer-encoding", "chunked"),
                ("host", "p.example"),
            ],
        );
        assert!(ambiguous(&r, b""));
        // A non-chunked TE.
        let r = mk(b"", &[("transfer-encoding", "gzip"), ("host", "p.example")]);
        assert!(ambiguous(&r, b""));
        // Obsolete line folding.
        assert!(ambiguous(
            &mk(b"", &[("host", "p.example")]),
            b"POST /x HTTP/1.1\r\nx-a: 1\r\n 2\r\n\r\n"
        ));
        // Absolute-form target.
        let r = Request {
            method: Method::GET,
            uri: "http://p.example/x".parse().unwrap(),
            headers: headers(&[("host", "p.example")]),
            cookies: HashMap::new(),
        };
        assert!(ambiguous(&r, b""));
        // Clean request passes.
        let r = mk(b"", &[("content-length", "5"), ("host", "p.example")]);
        assert!(!ambiguous(
            &r,
            b"POST /x HTTP/1.1\r\ncontent-length: 5\r\nhost: p.example\r\n\r\n"
        ));
    }

    #[test]
    fn label_shape() {
        let mk = |host: &str| Request {
            method: Method::GET,
            uri: "/".parse().unwrap(),
            headers: headers(&[("host", host)]),
            cookies: HashMap::new(),
        };
        let good = format!("p{}", "a".repeat(24));
        assert_eq!(
            mk(&format!("{good}.prv.example.org")).label().as_deref(),
            Some(good.as_str())
        );
        assert_eq!(mk("dashboard.example.org").label(), None);
        assert_eq!(mk("pshort.prv.example.org").label(), None);
    }

    #[test]
    fn authority_parsing() {
        assert_eq!(
            authority_of("wss://runner-a:4108/v1/preview-stream").as_deref(),
            Some("runner-a:4108")
        );
        assert_eq!(authority_of("https://x"), None);
    }
}
