//! Minimal URL / address checks (no `url` crate dependency).

use std::net::IpAddr;

pub(crate) struct ParsedUrl<'a> {
    pub host: String,
    pub path: &'a str,
    pub has_query_or_fragment: bool,
    pub has_userinfo: bool,
}

/// Parse `scheme://[userinfo@]host[:port][/path][?query][#fragment]`.
pub(crate) fn parse_url<'a>(s: &'a str, schemes: &[&str]) -> Result<ParsedUrl<'a>, String> {
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("must not contain whitespace".into());
    }
    let (scheme, rest) = s
        .split_once("://")
        .ok_or_else(|| format!("must be a URL with scheme {}", schemes.join(" or ")))?;
    if !schemes.iter().any(|x| x.eq_ignore_ascii_case(scheme)) {
        return Err(format!("scheme must be {}", schemes.join(" or ")));
    }
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(auth_end);
    let (has_userinfo, hostport) = match authority.rsplit_once('@') {
        Some((_, h)) => (true, h),
        None => (false, authority),
    };
    let host = if let Some(inner) = hostport.strip_prefix('[') {
        let (h, after) = inner.split_once(']').ok_or("unterminated IPv6 literal")?;
        if !after.is_empty() {
            check_port(after.strip_prefix(':').ok_or("invalid port")?)?;
        }
        h
    } else if let Some((h, p)) = hostport.rsplit_once(':') {
        check_port(p)?;
        h
    } else {
        hostport
    };
    if host.is_empty() {
        return Err("host must not be empty".into());
    }
    let q = tail.find(['?', '#']);
    Ok(ParsedUrl {
        host: host.to_ascii_lowercase(),
        path: &tail[..q.unwrap_or(tail.len())],
        has_query_or_fragment: q.is_some(),
        has_userinfo,
    })
}

fn check_port(p: &str) -> Result<(), String> {
    p.parse::<u16>()
        .map(|_| ())
        .map_err(|_| "invalid port".into())
}

pub(crate) fn host_of(s: &str) -> Option<String> {
    parse_url(s, &["http", "https"]).ok().map(|u| u.host)
}

/// `a.b.c.d/len` or IPv6 equivalent.
pub(crate) fn is_cidr(s: &str) -> bool {
    let Some((ip, len)) = s.split_once('/') else {
        return false;
    };
    let (Ok(ip), Ok(len)) = (ip.parse::<IpAddr>(), len.parse::<u8>()) else {
        return false;
    };
    len <= if ip.is_ipv4() { 32 } else { 128 }
}

pub(crate) fn is_bind_addr(s: &str) -> bool {
    let s = s
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .unwrap_or(s);
    s.parse::<IpAddr>().is_ok()
}
