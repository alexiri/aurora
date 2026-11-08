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
    /// Recorded responses (`aurora plugin test`): requests that pass every egress check below are
    /// answered from the cassette instead of the network. Production never sets it.
    pub cassette: Option<Cassette>,
}

/// One recorded exchange.
#[derive(Debug)]
struct CassetteEntry {
    method: String,
    url: String,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    used: bool,
}

/// `(method, url, status, headers, body)` of one recorded exchange.
pub type Recorded = (String, String, u16, Vec<(String, String)>, Vec<u8>);

/// Recorded HTTP, replayed in order: each request consumes the first unused entry with the same
/// method and URL. Clones share state, so one cassette spans several calls of a case.
#[derive(Debug, Clone, Default)]
pub struct Cassette(std::sync::Arc<std::sync::Mutex<Vec<CassetteEntry>>>);

fn norm_url(u: &str) -> String {
    reqwest::Url::parse(u).map_or_else(|_| u.to_owned(), |p| p.to_string())
}

impl Cassette {
    /// `(method, url, status, headers, body)` per entry.
    pub fn new(entries: Vec<Recorded>) -> Self {
        let c = Self::default();
        c.reset(entries);
        c
    }

    /// Replace the recorded entries (a runtime keeps one cassette across the cases of a package).
    pub fn reset(&self, entries: Vec<Recorded>) {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *g = entries
            .into_iter()
            .map(|(m, u, status, headers, body)| CassetteEntry {
                method: m.to_ascii_uppercase(),
                url: norm_url(&u),
                status,
                headers,
                body,
                used: false,
            })
            .collect();
    }

    fn replay(&self, method: &str, url: &str) -> Result<FetchResponse, FetchFail> {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let e = g
            .iter_mut()
            .find(|e| !e.used && e.method == method && e.url == url)
            .ok_or_else(|| {
                FetchFail::Network(format!("no recorded response for {method} {url}"))
            })?;
        e.used = true;
        Ok(FetchResponse {
            status: e.status,
            headers: e.headers.clone(),
            body: e.body.clone(),
        })
    }

    /// `METHOD url` of every recorded entry that no request consumed.
    pub fn unused(&self) -> Vec<String> {
        let g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        g.iter()
            .filter(|e| !e.used)
            .map(|e| format!("{} {}", e.method, e.url))
            .collect()
    }
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
        let method_name = method.to_ascii_uppercase();
        let method = match method_name.as_str() {
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
        if let Some(c) = &self.policy.cassette {
            return c.replay(&method_name, url.as_str());
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

    fn rec(method: &str, url: &str, status: u16, body: &str) -> Recorded {
        (
            method.into(),
            url.into(),
            status,
            vec![],
            body.as_bytes().to_vec(),
        )
    }

    #[tokio::test]
    async fn cassette_replays_in_order_after_the_egress_checks() {
        let cassette = Cassette::new(vec![
            rec("post", "https://auth.example.test/mint", 200, "one"),
            rec("POST", "https://auth.example.test/mint", 200, "two"),
            rec("GET", "https://auth.example.test/never", 200, ""),
        ]);
        let e = Egress::new(
            &["auth.example.test".to_owned()],
            FetchPolicy {
                insecure_local: false,
                cassette: Some(cassette.clone()),
            },
        );
        let t = Duration::from_secs(1);
        let url = "https://auth.example.test/mint";
        assert_eq!(
            e.fetch("POST", url, &[], None, t).await.unwrap().body,
            b"one"
        );
        assert_eq!(
            e.fetch("POST", url, &[], None, t).await.unwrap().body,
            b"two"
        );
        // exhausted: no network fallback
        assert!(matches!(
            e.fetch("POST", url, &[], None, t).await,
            Err(FetchFail::Network(m)) if m.contains("no recorded response")
        ));
        // the recording never widens egress: https, host list and credentials still apply
        assert!(matches!(
            e.fetch("GET", "http://auth.example.test/never", &[], None, t)
                .await,
            Err(FetchFail::Denied(_))
        ));
        assert!(matches!(
            e.fetch("GET", "https://other.example.test/never", &[], None, t)
                .await,
            Err(FetchFail::Denied(_))
        ));
        assert_eq!(
            cassette.unused(),
            vec!["GET https://auth.example.test/never"]
        );
    }

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
