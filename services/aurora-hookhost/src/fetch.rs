//! Egress-filtered `fetch` for hooks (docs/09 §8.3): https only, host in the hook's
//! egress list, public destinations only, no redirects, caps on size and time.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use aurora_core::limits::HOOK_FETCH_RESPONSE_MAX;
use aurora_policy::HostPattern;

/// Why a fetch did not happen or finished early.
#[derive(Debug, PartialEq, Eq)]
pub enum FetchFail {
    Denied(String),
    Timeout,
    TooLarge,
    Network(String),
}

pub struct FetchResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Production policy is `Default`; only tests relax it.
#[derive(Debug, Clone, Default)]
pub struct FetchPolicy {
    /// Allow `http://` and loopback/private destinations (tests only).
    pub insecure_local: bool,
}

fn v4_public(a: Ipv4Addr) -> bool {
    let o = a.octets();
    !(a.is_private()
        || a.is_loopback()
        || a.is_link_local()
        || a.is_unspecified()
        || a.is_broadcast()
        || a.is_multicast()
        || a.is_documentation()
        || o[0] == 0
        || (o[0] == 100 && (64..128).contains(&o[1]))
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        || o[0] >= 240)
}

fn v6_public(a: Ipv6Addr) -> bool {
    if let Some(v4) = a.to_ipv4_mapped() {
        return v4_public(v4);
    }
    let s = a.segments();
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        let o = a.octets();
        return v4_public(Ipv4Addr::new(o[12], o[13], o[14], o[15]));
    }
    !(a.is_loopback()
        || a.is_unspecified()
        || a.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] == 0x2001 && s[1] == 0x0db8)
        || s[0] == 0x0100 && s[1..4] == [0, 0, 0])
}

/// True for globally routable addresses.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => v4_public(a),
        IpAddr::V6(a) => v6_public(a),
    }
}

/// Everything one invocation may reach.
pub struct Egress {
    pub patterns: Vec<HostPattern>,
    pub policy: FetchPolicy,
}

impl Egress {
    pub fn new(hosts: &[String], policy: FetchPolicy) -> Self {
        Self {
            patterns: hosts.iter().map(|h| HostPattern::lenient(h)).collect(),
            policy,
        }
    }

    pub fn allows(&self, host: &str, port: u16) -> bool {
        self.patterns.iter().any(|p| p.matches(host, port))
    }

    pub async fn fetch(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<FetchResponse, FetchFail> {
        let url = reqwest::Url::parse(url).map_err(|_| FetchFail::Denied("invalid url".into()))?;
        let https = url.scheme() == "https";
        if !https && !(self.policy.insecure_local && url.scheme() == "http") {
            return Err(FetchFail::Denied("https required".into()));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(FetchFail::Denied("credentials in url".into()));
        }
        let host = url
            .host_str()
            .ok_or_else(|| FetchFail::Denied("no host".into()))?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        let port = url.port_or_known_default().unwrap_or(443);
        if !self.allows(&host, port) {
            return Err(FetchFail::Denied(format!(
                "host `{host}` not in egress list"
            )));
        }
        let method = match method.to_ascii_uppercase().as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "PATCH" => reqwest::Method::PATCH,
            "DELETE" => reqwest::Method::DELETE,
            "HEAD" => reqwest::Method::HEAD,
            _ => return Err(FetchFail::Denied("method not allowed".into())),
        };
        if body
            .as_ref()
            .is_some_and(|b| b.len() > HOOK_FETCH_RESPONSE_MAX)
        {
            return Err(FetchFail::TooLarge);
        }
        let host_bare = host.trim_start_matches('[').trim_end_matches(']');
        let addrs: Vec<SocketAddr> = if let Ok(ip) = host_bare.parse::<IpAddr>() {
            vec![SocketAddr::new(ip, port)]
        } else {
            tokio::time::timeout(timeout, tokio::net::lookup_host((host_bare, port)))
                .await
                .map_err(|_| FetchFail::Timeout)?
                .map_err(|e| FetchFail::Network(format!("dns: {e}")))?
                .collect()
        };
        if addrs.is_empty() {
            return Err(FetchFail::Network("no address".into()));
        }
        if !self.policy.insecure_local && addrs.iter().any(|a| !is_public(a.ip())) {
            return Err(FetchFail::Denied("destination not allowed".into()));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(timeout)
            .resolve_to_addrs(&host, &addrs)
            .build()
            .map_err(|e| FetchFail::Network(e.to_string()))?;
        let mut rb = client.request(method, url);
        for (k, v) in headers {
            let lk = k.to_ascii_lowercase();
            if matches!(
                lk.as_str(),
                "host" | "connection" | "content-length" | "transfer-encoding" | "upgrade"
            ) {
                continue;
            }
            rb = rb.header(k.as_str(), v.as_str());
        }
        if let Some(b) = body {
            rb = rb.body(b);
        }
        let started = tokio::time::Instant::now();
        let mut resp = rb.send().await.map_err(map_err)?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .take(64)
            .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) =
            tokio::time::timeout(timeout.saturating_sub(started.elapsed()), resp.chunk())
                .await
                .map_err(|_| FetchFail::Timeout)?
                .map_err(map_err)?
        {
            if body.len() + chunk.len() > HOOK_FETCH_RESPONSE_MAX {
                return Err(FetchFail::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(FetchResponse {
            status,
            headers,
            body,
        })
    }
}

fn map_err(e: reqwest::Error) -> FetchFail {
    if e.is_timeout() {
        FetchFail::Timeout
    } else {
        FetchFail::Network(e.without_url().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guard() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "192.168.0.1",
            "172.16.0.1",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:127.0.0.1",
            "64:ff9b::7f00:1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700::1111"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }
}
