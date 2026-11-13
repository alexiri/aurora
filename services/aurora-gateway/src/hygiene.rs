//! Pipeline steps 1 and 3: header hygiene, method-override and path hygiene.

use aurora_core::limits::BATCH_HEADER_MAX;
use http::{HeaderMap, HeaderName, HeaderValue, header};
use percent_encoding::percent_decode_str;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuroraHeaders {
    pub connection_id: Option<String>,
    pub batch_id: Option<String>,
    pub batch_label: Option<String>,
    pub batch_total: Option<u32>,
}

fn valid_value(h: &HeaderValue) -> Option<String> {
    let s = h.to_str().ok()?;
    let s = s.trim();
    if s.is_empty() || s.len() > BATCH_HEADER_MAX || s.chars().any(char::is_control) {
        return None;
    }
    Some(s.to_owned())
}

const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
];

fn connection_tokens(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Returns `websocket` if the message asks for a WebSocket upgrade (FIX-086, `06 §WebSocket
/// upgrades`): `Connection` names `upgrade` and `Upgrade` is exactly one token, `websocket`, ASCII
/// case-insensitively. Any other `Upgrade` value (`h2c`, `foo`, a list) is not honoured: hygiene
/// strips it and the request is forwarded as a plain one, so nothing after a `101` can bypass the
/// per-request pipeline.
pub fn requested_upgrade(headers: &HeaderMap) -> Option<HeaderValue> {
    if !connection_tokens(headers).iter().any(|t| t == "upgrade") {
        return None;
    }
    let mut values = headers.get_all(header::UPGRADE).iter();
    let only = values.next()?;
    if values.next().is_some() {
        return None;
    }
    only.to_str()
        .ok()
        .filter(|v| v.trim().eq_ignore_ascii_case("websocket"))
        .map(|_| HeaderValue::from_static("websocket"))
}

/// Removes hop-by-hop headers, `Proxy-*`, `Upgrade` and anything named by
/// `Connection`. `TE: trailers` is kept (gRPC).
pub fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let named = connection_tokens(headers);
    let te_trailers = headers
        .get(header::TE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("trailers"));
    for n in HOP_BY_HOP.iter().copied().chain(["upgrade"]) {
        headers.remove(n);
    }
    for n in named {
        if let Ok(name) = HeaderName::from_bytes(n.as_bytes()) {
            headers.remove(name);
        }
    }
    let proxy: Vec<HeaderName> = headers
        .keys()
        .filter(|k| k.as_str().starts_with("proxy-"))
        .cloned()
        .collect();
    for k in proxy {
        headers.remove(k);
    }
    if te_trailers {
        headers.insert(header::TE, HeaderValue::from_static("trailers"));
    }
}

/// Step 1. Removes every `x-aurora-*` header (SEC-05), remembering the known
/// ones, then removes hop-by-hop and `Proxy-*` headers.
pub fn strip_request_headers(headers: &mut HeaderMap) -> AuroraHeaders {
    let get = |h: &HeaderMap, n: &str| h.get(n).and_then(valid_value);
    let out = AuroraHeaders {
        connection_id: get(headers, "x-aurora-connection-id"),
        batch_id: get(headers, "x-aurora-batch-id"),
        batch_label: get(headers, "x-aurora-batch-label"),
        batch_total: get(headers, "x-aurora-batch-total").and_then(|s| s.parse().ok()),
    };
    let aurora: Vec<HeaderName> = headers
        .keys()
        .filter(|k| k.as_str().starts_with("x-aurora-"))
        .cloned()
        .collect();
    for k in aurora {
        headers.remove(k);
    }
    strip_hop_by_hop(headers);
    out
}

#[derive(Debug, PartialEq, Eq)]
pub enum PathError {
    MethodOverride,
    Ambiguous,
}

const OVERRIDE_HEADERS: [&str; 3] = [
    "x-http-method-override",
    "x-http-method",
    "x-method-override",
];

/// Step 3: refuses method overrides and ambiguous paths; returns the
/// normalized path and the untouched query.
pub fn check_request_line(
    headers: &HeaderMap,
    raw_path: &str,
    query: Option<&str>,
) -> Result<String, PathError> {
    if OVERRIDE_HEADERS.iter().any(|h| headers.contains_key(*h)) {
        return Err(PathError::MethodOverride);
    }
    if let Some(q) = query {
        let hit = q.split('&').any(|kv| {
            let k = kv.split('=').next().unwrap_or("");
            percent_decode_str(k)
                .decode_utf8_lossy()
                .eq_ignore_ascii_case("_method")
        });
        if hit {
            return Err(PathError::MethodOverride);
        }
    }
    normalize_path(raw_path)
}

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

pub fn normalize_path(raw: &str) -> Result<String, PathError> {
    if !raw.starts_with('/') || raw.contains('\\') || raw.contains('\0') {
        return Err(PathError::Ambiguous);
    }
    let b = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = raw.get(i + 1..i + 3).ok_or(PathError::Ambiguous)?;
            let v = u8::from_str_radix(hex, 16).map_err(|_| PathError::Ambiguous)?;
            if matches!(v, 0 | b'/' | b'\\') {
                return Err(PathError::Ambiguous);
            }
            if is_unreserved(v) {
                out.push(char::from(v));
            } else {
                out.push('%');
                out.push_str(&hex.to_ascii_uppercase());
            }
            i += 3;
        } else {
            let ch = raw[i..].chars().next().ok_or(PathError::Ambiguous)?;
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    let mut res = String::with_capacity(out.len());
    let trailing = out.ends_with('/');
    for seg in out.split('/').filter(|s| !s.is_empty()) {
        if seg == "." || seg == ".." {
            return Err(PathError::Ambiguous);
        }
        res.push('/');
        res.push_str(seg);
    }
    if (res.is_empty() || trailing) && !res.ends_with('/') {
        res.push('/');
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(normalize_path("//a//b").unwrap(), "/a/b");
        assert_eq!(normalize_path("/a/b/").unwrap(), "/a/b/");
        assert_eq!(normalize_path("/").unwrap(), "/");
        assert_eq!(normalize_path("/%61%2d%20").unwrap(), "/a-%20");
        for bad in [
            "/a/../b",
            "/a/./b",
            "/a/%2Fb",
            "/a/%2fb",
            "/a%5Cb",
            "/a%00",
            "/%2e%2e/x",
            "/a/%2E",
            "x",
            "/a\\b",
            "/%zz",
        ] {
            assert_eq!(normalize_path(bad), Err(PathError::Ambiguous), "{bad}");
        }
    }

    #[test]
    fn override_refused() {
        let mut h = HeaderMap::new();
        assert!(check_request_line(&h, "/a", Some("x=1&_method=DELETE")).is_err());
        assert!(check_request_line(&h, "/a", Some("x=1")).is_ok());
        h.insert("x-http-method-override", "PUT".parse().unwrap());
        assert_eq!(
            check_request_line(&h, "/a", None),
            Err(PathError::MethodOverride)
        );
    }

    #[test]
    fn only_websocket_is_an_upgrade() {
        let req = |conn: &str, ups: &[&str]| {
            let mut h = HeaderMap::new();
            h.insert("connection", conn.parse().unwrap());
            for u in ups {
                h.append("upgrade", u.parse().unwrap());
            }
            requested_upgrade(&h)
        };
        assert_eq!(req("Upgrade", &["websocket"]).unwrap(), "websocket");
        assert_eq!(
            req("keep-alive, upgrade", &[" WebSocket "]).unwrap(),
            "websocket"
        );
        for ups in [
            &["h2c"][..],
            &["foo"],
            &["websocket, h2c"],
            &["websocket", "h2c"],
            &[],
        ] {
            assert_eq!(req("upgrade", ups), None, "{ups:?}");
        }
        // Without `Connection: upgrade` the header asks for nothing.
        let mut h = HeaderMap::new();
        h.insert("upgrade", "websocket".parse().unwrap());
        assert_eq!(requested_upgrade(&h), None);
    }

    #[test]
    fn strips() {
        let mut h = HeaderMap::new();
        h.insert("x-aurora-connection-id", "c1".parse().unwrap());
        h.insert("x-aurora-batch-total", "abc".parse().unwrap());
        h.insert("x-aurora-other", "1".parse().unwrap());
        h.insert("proxy-foo", "1".parse().unwrap());
        h.insert("connection", "keep-alive, x-custom".parse().unwrap());
        h.insert("x-custom", "1".parse().unwrap());
        h.insert("keep-alive", "1".parse().unwrap());
        h.insert("accept", "*/*".parse().unwrap());
        let a = strip_request_headers(&mut h);
        assert_eq!(a.connection_id.as_deref(), Some("c1"));
        assert_eq!(a.batch_total, None);
        assert_eq!(h.len(), 1);
        assert!(h.contains_key("accept"));
    }
}
