//! Guarded outbound fetches for addon sources (SEC-86, D-250(d)): https only, public
//! addresses only (`06 §Destination guard` via the connect `Env`), the manifest fetched
//! **without** redirects, package URLs following **at most one** redirect (the release-asset
//! CDN pattern of git forges). Digest checks happen before anything is parsed.

use super::manifest::ManifestError;
use aurora_core::limits::{ADDON_FETCH_TIMEOUT, ADDON_MANIFEST_MAX, PACKAGE_MAX};
use aurora_core::sanitize::one_line;
use reqwest::StatusCode;

/// Fetches through the API's guarded client.
pub struct Fetcher<'a> {
    http: &'a reqwest::Client,
    env: Option<&'a crate::domains::connect::env::Env>,
}

impl<'a> Fetcher<'a> {
    #[must_use]
    pub fn new(http: &'a reqwest::Client) -> Self {
        Self { http, env: None }
    }

    /// Production constructor: resolve through the connect Env (test override applied,
    /// guard enforced).
    #[must_use]
    pub fn with_env(env: &'a crate::domains::connect::env::Env) -> Self {
        Self {
            http: &env.http,
            env: Some(env),
        }
    }

    fn resolve(&self, raw: &str) -> Result<reqwest::Url, ManifestError> {
        if let Some(env) = self.env {
            // The Env applies the test override first and then refuses non-https,
            // credential URLs and non-public IP literals (`06 §Destination guard`).
            return env.prepare(raw).map_err(|_| ManifestError::UrlRefused);
        }
        // Without the Env (unit tests) the same shape rules apply on the URL itself.
        let url = reqwest::Url::parse(raw).map_err(|_| ManifestError::UrlRefused)?;
        let public_host = url.host_str().is_some_and(|h| {
            match h.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
                Ok(std::net::IpAddr::V4(ip)) => {
                    !(ip.is_loopback()
                        || ip.is_private()
                        || ip.is_link_local()
                        || ip.is_unspecified()
                        || ip.is_broadcast()
                        || ip.is_multicast())
                }
                Ok(std::net::IpAddr::V6(ip)) => {
                    let s = ip.segments();
                    !(ip.is_loopback()
                        || ip.is_unspecified()
                        || ip.is_multicast()
                        || (s[0] & 0xfe00) == 0xfc00
                        || (s[0] & 0xffc0) == 0xfe80)
                }
                Err(_) => !h.is_empty(),
            }
        });
        if url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && public_host
        {
            Ok(url)
        } else {
            Err(ManifestError::UrlRefused)
        }
    }

    /// GET `url`, at most `max_redirects` hops (each still https and public), body capped
    /// at `max` bytes. A bigger body, a non-2xx/3xx answer or a transport error is
    /// [`ManifestError::Unreachable`] (413 for an over-long body).
    async fn get(
        &self,
        url: &str,
        max_redirects: usize,
        max: usize,
    ) -> Result<Vec<u8>, ManifestError> {
        let mut current = self.resolve(url)?;
        for _ in 0..=max_redirects {
            let res = self
                .http
                .get(current.clone())
                .timeout(ADDON_FETCH_TIMEOUT)
                .send()
                .await
                .map_err(|e| ManifestError::Unreachable(one_line(&e.to_string(), 120)))?;
            if res.status().is_redirection() {
                if max_redirects == 0 {
                    // The manifest is a raw file: a redirect answer is never followed.
                    return Err(ManifestError::Unreachable(format!(
                        "unexpected redirect status {}",
                        res.status().as_u16()
                    )));
                }
                let loc = res
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        ManifestError::Unreachable("redirect without a target".into())
                    })?;
                current = self
                    .resolve(loc)
                    .map_err(|_| ManifestError::Unreachable("redirect target refused".into()))?;
                continue;
            }
            let status = res.status();
            let len = res.content_length();
            if let Some(n) = len
                && n > max as u64
            {
                return Err(ManifestError::TooLarge);
            }
            if !status.is_success() {
                return Err(ManifestError::Unreachable(format!(
                    "the server answered {} {}",
                    status.as_u16(),
                    canonical_reason(status)
                )));
            }
            let body = res
                .bytes()
                .await
                .map_err(|e| ManifestError::Unreachable(one_line(&e.to_string(), 120)))?;
            if body.len() > max {
                return Err(ManifestError::TooLarge);
            }
            // The override origin answers on http: accept the body either way (tests);
            // production only ever gets here over https (resolve refused the rest).
            return Ok(body.to_vec());
        }
        Err(ManifestError::Unreachable("too many redirects".to_owned()))
    }

    /// The manifest: no redirects, ≤ [`ADDON_MANIFEST_MAX`].
    pub async fn manifest(&self, url: &str) -> Result<Vec<u8>, ManifestError> {
        self.get(url, 0, ADDON_MANIFEST_MAX).await
    }

    /// A package archive: at most one redirect, ≤ [`PACKAGE_MAX`].
    pub async fn package(&self, url: &str) -> Result<Vec<u8>, ManifestError> {
        self.get(url, 1, PACKAGE_MAX).await
    }
}

fn canonical_reason(s: StatusCode) -> &'static str {
    s.canonical_reason().unwrap_or("status")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn refuses_non_https_and_credential_urls_before_any_request() {
        let c = client();
        let f = Fetcher::new(&c);
        for u in [
            "http://cdn.example/p",
            "https://u:p@cdn.example/p",
            "https://127.0.0.1/p",
            "https://10.1.2.3/p",
            "https://[::1]/p",
            "https://169.254.169.254/p",
        ] {
            assert!(
                matches!(f.resolve(u), Err(ManifestError::UrlRefused)),
                "{u}"
            );
        }
        assert!(f.resolve("https://cdn.example/p").is_ok());
    }
}
