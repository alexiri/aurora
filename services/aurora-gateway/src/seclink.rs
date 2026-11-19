//! (phase 5, P5-ICS, D-273, C-843, SEC-107; `docs/06 §Secret-link lane`) The secret-link lane: a
//! request on `<connection_id>.link.aurora.internal` that the decision path admitted (grant,
//! policy, approvals, Read only) is not forwarded. The gateway fetches exactly the URL the person
//! stored — `GET`, `https`, at most [`REDIRECTS_MAX`] followed hops, the destination guard (inside
//! `Upstream::send`) and the org egress policy ([`Hops`]) on every hop — and answers with the
//! body, from a short in-memory cache when it holds one. The agent controls no part of the
//! destination; the URL appears in no log row, span, approval or error (SEC-107).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use aurora_core::ApiError;
use aurora_core::limits::{
    SECRET_LINK_CACHE_MAX_BYTES, SECRET_LINK_CACHE_TTL, SECRET_LINK_REDIRECTS_MAX,
    SECRET_LINK_RESPONSE_MAX,
};
use bytes::{Bytes, BytesMut};
use http::{HeaderMap, HeaderValue, Method, Response, StatusCode, Uri, header};
use http_body_util::BodyExt;
use lru::LruCache;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::invalidation::{CacheStats, Invalidatable, Invalidate, Scope};
use crate::pipeline::{PreparedRequest, Scheme};
use crate::upstream::Upstream;
use crate::{Body, empty_body, full_body, guidance};

/// Whole-fetch bound (headers of every hop plus the body).
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// The shortest path-with-query that is searched for in a response (a bare `/` is everywhere).
const PATH_NEEDLE_MIN: usize = 12;

pub const REDIRECTS_MAX: usize = SECRET_LINK_REDIRECTS_MAX;

/// Per-hop org egress policy: `Err` refuses the hop (403 `link_destination_refused`).
pub trait Hops: Send + Sync {
    fn check(&self, host: &str, port: u16) -> Result<(), ApiError>;
}

/// The request-log row of one secret-link request, written exactly once.
pub trait Report: Send + Sync {
    /// `code` is the error code of a refusal, `None` for every answer (a cached one has no `upstream`).
    fn finish(self: Box<Self>, status: u16, upstream: Option<u16>, code: Option<&str>);
}

/// Everything the lane needs, produced after admission by the decision path.
pub struct LinkFetch {
    /// The stored URL. Never logged, never in an error.
    pub url: String,
    pub connection_id: Uuid,
    /// The credential's version: part of the cache key, so a reconnect never serves an old body.
    pub version: i64,
    pub org: Uuid,
    pub workspace: Option<Uuid>,
    pub accept: Option<HeaderValue>,
    pub if_none_match: Option<String>,
    pub head: bool,
    pub cache: Arc<LinkCache>,
    pub hops: Box<dyn Hops>,
    pub report: Box<dyn Report>,
    /// (P5-WEBHOOK-2) A send-mode connection's request, rendered and ready: the lane posts it
    /// (`webhook_send`) instead of fetching.
    pub send: Option<crate::webhook_send::SendPlan>,
}

impl std::fmt::Debug for LinkFetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkFetch")
            .field("connection_id", &self.connection_id)
            .finish_non_exhaustive()
    }
}

/// `https` destination parsed from a stored URL or a redirect `Location`.
#[derive(Debug, Clone)]
pub(crate) struct Dest {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) path: String,
    pub(crate) query: Option<String>,
}

pub(crate) fn parse_dest(uri: &Uri) -> Option<Dest> {
    if uri.scheme_str()? != "https" {
        return None;
    }
    let host = uri.host()?.trim_matches(['[', ']']).to_owned();
    Some(Dest {
        host,
        port: uri.port_u16().unwrap_or(443),
        path: if uri.path().is_empty() {
            "/".to_owned()
        } else {
            uri.path().to_owned()
        },
        query: uri.query().map(str::to_owned),
    })
}

pub(crate) enum Next {
    Go(Dest),
    /// A hop that is not `https` (or not parseable): refused.
    Refused,
}

/// Where a redirect `Location` leads, relative to the current hop.
pub(crate) fn follow(cur: &Dest, location: &str) -> Next {
    let abs = |s: &str| {
        s.parse::<Uri>()
            .ok()
            .and_then(|u| parse_dest(&u))
            .map_or(Next::Refused, Next::Go)
    };
    if location.starts_with("https://") {
        return abs(location);
    }
    if location.contains("://") {
        return Next::Refused;
    }
    if let Some(rest) = location.strip_prefix("//") {
        return abs(&format!("https://{rest}"));
    }
    let (path, query) = match location.split_once('?') {
        Some((p, q)) => (p, Some(q.to_owned())),
        None => (location, None),
    };
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        let dir = cur.path.rsplit_once('/').map_or("", |(d, _)| d);
        format!("{dir}/{path}")
    };
    if path.contains(['\r', '\n', ' ']) {
        return Next::Refused;
    }
    Next::Go(Dest {
        host: cur.host.clone(),
        port: cur.port,
        path,
        query,
    })
}

pub(crate) fn refused(host: &str) -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "link_destination_refused",
        "The link's address is not permitted: it is a private or internal address, or an organization policy blocks its host.",
    )
    .with_details(json!({ "host": host }))
}

fn not_found() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "link_not_found",
        "The link no longer serves a calendar: it was probably unpublished or reset. Ask the person to reconnect with the new link.",
    )
}

fn unreachable() -> ApiError {
    ApiError::bad_gateway(
        "link_unreachable",
        "The link's server could not be reached.",
    )
}

fn too_large() -> ApiError {
    ApiError::bad_gateway(
        "link_response_too_large",
        "The link's answer is larger than the gateway reads.",
    )
    .with_details(json!({ "limit_bytes": SECRET_LINK_RESPONSE_MAX }))
}

fn echoed() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_echoed_credential",
        "The upstream answered with the link's address, so the response was withheld.",
    )
}

/// A refusal of the dial (guard) or a failed exchange, with no word of the URL.
fn map_send(e: &ApiError) -> ApiError {
    match e.status() {
        StatusCode::FORBIDDEN => refused(""),
        StatusCode::GATEWAY_TIMEOUT => guidance::upstream_timeout(),
        _ => unreachable(),
    }
}

/// The needles of the echo check: the whole URL and its path-with-query, raw and percent-encoded
/// (never single path segments: a calendar id legitimately appears in the body).
pub(crate) fn needles(url: &str, d: &Dest) -> Vec<Vec<u8>> {
    let pq = match &d.query {
        Some(q) => format!("{}?{q}", d.path),
        None => d.path.clone(),
    };
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut add = |s: &str| {
        for n in [
            s.as_bytes().to_vec(),
            utf8_percent_encode(s, NON_ALPHANUMERIC)
                .to_string()
                .into_bytes(),
        ] {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    };
    add(url);
    if pq.len() >= PATH_NEEDLE_MIN {
        add(&pq);
    }
    out
}

fn etag_of(body: &[u8]) -> String {
    let h = Sha256::digest(body);
    format!("\"{}\"", hex::encode(&h[..16]))
}

/// A body the lane serves: fresh or cached.
struct Served {
    body: Bytes,
    content_type: Option<HeaderValue>,
    etag: String,
    age: u64,
    hit: bool,
}

impl LinkFetch {
    async fn serve(&self, up: &Upstream) -> Result<Served, ApiError> {
        let key = (self.connection_id, self.version);
        if let Some(s) = self.cache.get(key) {
            return Ok(s);
        }
        let first = self
            .url
            .parse::<Uri>()
            .ok()
            .and_then(|u| parse_dest(&u))
            .ok_or_else(|| refused(""))?;
        let needles = needles(&self.url, &first);
        let (body, content_type) =
            tokio::time::timeout(FETCH_TIMEOUT, self.fetch(up, first, &needles))
                .await
                .map_err(|_| guidance::upstream_timeout())??;
        if crate::inject::echoes(&needles, &body) {
            return Err(echoed());
        }
        let etag = etag_of(&body);
        self.cache.put(
            key,
            self.org,
            self.workspace,
            body.clone(),
            content_type.clone(),
            etag.clone(),
        );
        Ok(Served {
            body,
            content_type,
            etag,
            age: 0,
            hit: false,
        })
    }

    /// One `GET` of the stored URL, following at most [`REDIRECTS_MAX`] `https` hops.
    async fn fetch(
        &self,
        up: &Upstream,
        first: Dest,
        needles: &[Vec<u8>],
    ) -> Result<(Bytes, Option<HeaderValue>), ApiError> {
        let mut cur = first;
        for hop in 0..=REDIRECTS_MAX {
            self.hops.check(&cur.host, cur.port)?;
            let mut headers = HeaderMap::new();
            if let Some(a) = &self.accept {
                headers.insert(header::ACCEPT, a.clone());
            }
            let resp = up
                .send(PreparedRequest {
                    scheme: Scheme::Https,
                    host: cur.host.clone(),
                    port: cur.port,
                    method: Method::GET,
                    path: cur.path.clone(),
                    query: cur.query.clone(),
                    headers,
                    upgrade: None,
                    body: empty_body(),
                    hook: None,
                    response_headers: HeaderMap::new(),
                    retry: None,
                    ldap: None,
                    link: None,
                })
                .await
                .map_err(|e| {
                    let mapped = map_send(&e);
                    if mapped.status() == StatusCode::FORBIDDEN {
                        refused(&cur.host)
                    } else {
                        mapped
                    }
                })?;
            let status = resp.status();
            if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
                // The redirect and its `Location` never reach the agent.
                let loc = resp
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                drop(resp);
                if hop == REDIRECTS_MAX {
                    return Err(ApiError::bad_gateway(
                        "link_too_many_redirects",
                        "The link redirects more often than the gateway follows.",
                    ));
                }
                let Some(loc) = loc else {
                    return Err(unreachable());
                };
                match follow(&cur, &loc) {
                    Next::Go(d) => cur = d,
                    Next::Refused => return Err(refused(&cur.host)),
                }
                continue;
            }
            if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
                return Err(not_found());
            }
            if status != StatusCode::OK {
                return Err(ApiError::bad_gateway(
                    "link_upstream_error",
                    "The link's server did not answer with the calendar.",
                )
                .with_details(json!({ "upstream_status": status.as_u16() })));
            }
            let content_type = resp.headers().get(header::CONTENT_TYPE).cloned();
            // Headers are searched for the URL too (a `Content-Location`, say).
            if resp
                .headers()
                .values()
                .any(|v| crate::inject::echoes(needles, v.as_bytes()))
            {
                return Err(echoed());
            }
            let mut body = resp.into_body();
            let mut buf = BytesMut::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|_| unreachable())?;
                if let Some(d) = frame.data_ref() {
                    if buf.len() + d.len() > SECRET_LINK_RESPONSE_MAX {
                        return Err(too_large());
                    }
                    buf.extend_from_slice(d);
                }
            }
            return Ok((buf.freeze(), content_type));
        }
        Err(unreachable())
    }
}

/// Answers an admitted secret-link request; the request-log row is written here, once.
pub async fn answer(up: &Upstream, mut f: LinkFetch) -> Response<Body> {
    if let Some(plan) = f.send.take() {
        return crate::webhook_send::answer(up, f, plan).await;
    }
    let served = f.serve(up).await;
    let LinkFetch {
        if_none_match,
        head,
        report,
        ..
    } = f;
    match served {
        Err(e) => {
            report.finish(e.status().as_u16(), None, Some(e.code()));
            guidance::response(&e)
        }
        Ok(s) => {
            // A hit is an ordinary 200 row (the activity views and the status rail read any
            // non-null `error_code` as a failure); the only trace is that no upstream call was made.
            let upstream = (!s.hit).then_some(200);
            let matches = if_none_match.as_deref().is_some_and(|inm| {
                inm.split(',')
                    .map(str::trim)
                    .any(|t| t == "*" || t.trim_start_matches("W/") == s.etag)
            });
            let mut resp = if matches {
                let mut r = Response::new(empty_body());
                *r.status_mut() = StatusCode::NOT_MODIFIED;
                r
            } else {
                let len = s.body.len();
                let mut r = Response::new(if head {
                    empty_body()
                } else {
                    full_body(s.body)
                });
                if let Some(ct) = s.content_type {
                    r.headers_mut().insert(header::CONTENT_TYPE, ct);
                }
                r.headers_mut()
                    .insert(header::CONTENT_LENGTH, HeaderValue::from(len));
                r
            };
            if let Ok(v) = HeaderValue::from_str(&s.etag) {
                resp.headers_mut().insert(header::ETAG, v);
            }
            resp.headers_mut()
                .insert(header::AGE, HeaderValue::from(s.age));
            report.finish(resp.status().as_u16(), upstream, None);
            resp
        }
    }
}

struct Entry {
    body: Bytes,
    content_type: Option<HeaderValue>,
    etag: String,
    stored: Instant,
    org: Uuid,
    workspace: Option<Uuid>,
}

struct Inner {
    map: LruCache<(Uuid, i64), Entry>,
    bytes: usize,
}

/// The gateway's per-replica memory cache of secret-link bodies (D-273(g), SEC-107): keyed by
/// `(connection id, credential version)`, answered only after admission, bounded in bytes (least
/// recently used evicted), never on disk, dropped on any connection change.
pub struct LinkCache {
    inner: Mutex<Inner>,
    ttl: Duration,
    max_bytes: usize,
    /// Test clock: time added to `Instant::now()`.
    skew_ms: AtomicU64,
}

impl Default for LinkCache {
    fn default() -> Self {
        Self::new(SECRET_LINK_CACHE_TTL, SECRET_LINK_CACHE_MAX_BYTES)
    }
}

impl LinkCache {
    #[must_use]
    pub fn new(ttl: Duration, max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: LruCache::unbounded(),
                bytes: 0,
            }),
            ttl,
            max_bytes,
            skew_ms: AtomicU64::new(0),
        }
    }

    fn now(&self) -> Instant {
        Instant::now() + Duration::from_millis(self.skew_ms.load(Ordering::Relaxed))
    }

    /// Tests: age every entry by `d` without sleeping.
    #[doc(hidden)]
    pub fn advance(&self, d: Duration) {
        self.skew_ms.fetch_add(
            u64::try_from(d.as_millis()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().map_or(0, |i| i.map.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get(&self, key: (Uuid, i64)) -> Option<Served> {
        let now = self.now();
        let mut g = self.inner.lock().ok()?;
        let expired = g
            .map
            .peek(&key)
            .is_some_and(|e| now.saturating_duration_since(e.stored) >= self.ttl);
        if expired && let Some(e) = g.map.pop(&key) {
            g.bytes -= e.body.len();
            return None;
        }
        let e = g.map.get(&key)?;
        Some(Served {
            body: e.body.clone(),
            content_type: e.content_type.clone(),
            etag: e.etag.clone(),
            age: now.saturating_duration_since(e.stored).as_secs(),
            hit: true,
        })
    }

    fn put(
        &self,
        key: (Uuid, i64),
        org: Uuid,
        workspace: Option<Uuid>,
        body: Bytes,
        content_type: Option<HeaderValue>,
        etag: String,
    ) {
        if body.len() > self.max_bytes {
            return;
        }
        let Ok(mut g) = self.inner.lock() else { return };
        // A new credential version replaces the connection's older entries.
        let stale: Vec<_> = g
            .map
            .iter()
            .filter(|(k, _)| k.0 == key.0)
            .map(|(k, _)| *k)
            .collect();
        for k in stale {
            if let Some(e) = g.map.pop(&k) {
                g.bytes -= e.body.len();
            }
        }
        g.bytes += body.len();
        g.map.put(
            key,
            Entry {
                body,
                content_type,
                etag,
                stored: self.now(),
                org,
                workspace,
            },
        );
        while g.bytes > self.max_bytes {
            match g.map.pop_lru() {
                Some((_, e)) => g.bytes -= e.body.len(),
                None => break,
            }
        }
    }

    fn retain(&self, keep: impl Fn(&(Uuid, i64), &Entry) -> bool) {
        let Ok(mut g) = self.inner.lock() else { return };
        let drop: Vec<_> = g
            .map
            .iter()
            .filter(|(k, e)| !keep(k, e))
            .map(|(k, _)| *k)
            .collect();
        for k in drop {
            if let Some(e) = g.map.pop(&k) {
                g.bytes -= e.body.len();
            }
        }
    }
}

impl Invalidatable for LinkCache {
    fn invalidate(&self, ev: &Invalidate) {
        match ev.scope {
            Scope::Connection(id) => self.retain(|k, _| k.0 != id),
            Scope::Org(id) => self.retain(|_, e| e.org != id),
            Scope::Workspace(id) => self.retain(|_, e| e.workspace != Some(id)),
            Scope::All => self.flush_all(),
            Scope::Agent(_) | Scope::Credential(_) | Scope::User(_) => {}
        }
    }

    fn flush_all(&self) {
        self.retain(|_, _| false);
    }

    fn stats(&self) -> CacheStats {
        let Ok(g) = self.inner.lock() else {
            return CacheStats::default();
        };
        let now = self.now();
        CacheStats {
            entries: g.map.len(),
            oldest_age: g
                .map
                .iter()
                .map(|(_, e)| now.saturating_duration_since(e.stored))
                .max(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(path: &str, q: Option<&str>) -> Dest {
        Dest {
            host: "h.example".into(),
            port: 443,
            path: path.into(),
            query: q.map(str::to_owned),
        }
    }

    #[test]
    fn redirect_targets_are_https_only() {
        let cur = d("/a/b.ics", None);
        assert!(matches!(
            follow(&cur, "http://evil.example/x"),
            Next::Refused
        ));
        assert!(matches!(follow(&cur, "ftp://x/y"), Next::Refused));
        match follow(&cur, "https://other.example:8443/z?q=1") {
            Next::Go(n) => {
                assert_eq!(
                    (n.host.as_str(), n.port, n.path.as_str()),
                    ("other.example", 8443, "/z")
                );
                assert_eq!(n.query.as_deref(), Some("q=1"));
            }
            Next::Refused => panic!("absolute https is followed"),
        }
        match follow(&cur, "c.ics") {
            Next::Go(n) => assert_eq!(n.path, "/a/c.ics"),
            Next::Refused => panic!("relative is followed"),
        }
        match follow(&cur, "//cdn.example/p") {
            Next::Go(n) => assert_eq!(n.host, "cdn.example"),
            Next::Refused => panic!("scheme-relative is https"),
        }
    }

    #[test]
    fn needles_are_the_whole_url_and_path_with_query_not_segments() {
        let url = "https://h.example/calendar/ical/CAL%40ID/private-TOKEN123/basic.ics";
        let n = needles(
            url,
            &d("/calendar/ical/CAL%40ID/private-TOKEN123/basic.ics", None),
        );
        let hay = b"ORGANIZER:mailto:CAL@ID CAL%40ID calendar";
        assert!(
            !crate::inject::echoes(&n, hay),
            "a single segment is not a needle"
        );
        assert!(crate::inject::echoes(
            &n,
            b"x /calendar/ical/CAL%40ID/private-TOKEN123/basic.ics y"
        ));
        assert!(crate::inject::echoes(&n, url.as_bytes()));
    }

    #[test]
    fn cache_ttl_lru_and_version() {
        let (o, a, b) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        let c = LinkCache::new(Duration::from_secs(10), 10);
        c.put(
            (a, 1),
            o,
            None,
            Bytes::from_static(b"123456"),
            None,
            "e".into(),
        );
        assert!(c.get((a, 1)).is_some());
        // 6 + 6 > 10: the least recently used (a) is evicted.
        c.put(
            (b, 1),
            o,
            None,
            Bytes::from_static(b"abcdef"),
            None,
            "e".into(),
        );
        assert!(c.get((a, 1)).is_none());
        assert!(c.get((b, 1)).is_some());
        // A new credential version replaces the old entry.
        c.put((b, 2), o, None, Bytes::from_static(b"zz"), None, "e".into());
        assert!(c.get((b, 1)).is_none());
        assert!(c.get((b, 2)).is_some());
        // Over the whole bound: never cached.
        c.put(
            (a, 1),
            o,
            None,
            Bytes::from_static(b"12345678901"),
            None,
            "e".into(),
        );
        assert!(c.get((a, 1)).is_none());
        c.advance(Duration::from_secs(11));
        assert!(c.get((b, 2)).is_none(), "expired after the TTL");
        assert!(c.is_empty());
    }
}
