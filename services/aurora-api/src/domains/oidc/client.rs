//! Provider metadata cache and the HTTP client used for discovery and token exchange.
use aurora_config::{ApiConfig, OidcProvider};
use openidconnect::core::CoreProviderMetadata;
use openidconnect::{AsyncHttpClient, HttpRequest, IssuerUrl, reqwest};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const METADATA_TTL: Duration = Duration::from_secs(600);

/// `reqwest` plus an optional URL-prefix rewrite for the development IdP (`auth.dev_idp.internal_url`):
/// the browser reaches the IdP at its issuer URL (e.g. through the ingress), the API reaches it at
/// the in-cluster address. Requests to anything under the issuer go to `internal_url` instead; the
/// issuer string in discovery and in the ID token stays the public one.
pub struct OidcHttp {
    inner: reqwest::Client,
    /// `(public prefix, internal prefix)`, no trailing slash.
    rewrite: Option<(String, String)>,
}

impl<'c> AsyncHttpClient<'c> for OidcHttp {
    type Error = <reqwest::Client as AsyncHttpClient<'c>>::Error;
    type Future = <reqwest::Client as AsyncHttpClient<'c>>::Future;

    fn call(&'c self, mut request: HttpRequest) -> Self::Future {
        if let Some((public, internal)) = &self.rewrite {
            let uri = request.uri().to_string();
            let matches = uri
                .strip_prefix(public.as_str())
                .filter(|rest| rest.is_empty() || rest.starts_with(['/', '?']));
            if let Some(rest) = matches
                && let Ok(u) = format!("{internal}{rest}").parse()
            {
                *request.uri_mut() = u;
            }
        }
        self.inner.call(request)
    }
}

pub struct OidcClients {
    pub(super) http: OidcHttp,
    cache: Mutex<HashMap<String, (Instant, CoreProviderMetadata)>>,
}

impl OidcClients {
    pub fn new(cfg: &ApiConfig) -> Result<Self, String> {
        let inner = reqwest::Client::builder()
            // Following redirects would open SSRF paths (openidconnect guidance).
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("oidc http client: {e}"))?;
        let d = &cfg.auth.dev_idp;
        let rewrite = match (d.enabled, d.issuer.as_deref(), d.internal_url.as_deref()) {
            (true, Some(public), Some(internal)) => Some((
                public.trim_end_matches('/').to_owned(),
                internal.trim_end_matches('/').to_owned(),
            )),
            _ => None,
        };
        Ok(Self {
            http: OidcHttp { inner, rewrite },
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Discovery document + JWKS, cached for ten minutes.
    pub(super) async fn metadata(&self, p: &OidcProvider) -> Result<CoreProviderMetadata, String> {
        if let Some((at, m)) = self.cache.lock().await.get(&p.id)
            && at.elapsed() < METADATA_TTL
        {
            return Ok(m.clone());
        }
        let issuer = IssuerUrl::new(p.issuer.clone()).map_err(|e| e.to_string())?;
        let m = CoreProviderMetadata::discover_async(issuer, &self.http)
            .await
            .map_err(|e| format!("discovery failed: {e}"))?;
        self.cache
            .lock()
            .await
            .insert(p.id.clone(), (Instant::now(), m.clone()));
        Ok(m)
    }

    /// (P4-IDP-1) The provider's userinfo response (plain JSON) fetched with the flow's own access
    /// token. Never follows redirects; a non-200 or non-JSON answer is an error.
    pub(super) async fn userinfo(
        &self,
        p: &OidcProvider,
        access_token: &str,
    ) -> Result<serde_json::Value, String> {
        let meta = self.metadata(p).await?;
        let url = meta
            .userinfo_endpoint()
            .ok_or("the provider has no userinfo endpoint")?
            .url()
            .to_string();
        let req = http::Request::builder()
            .method(http::Method::GET)
            .uri(url)
            .header(
                http::header::AUTHORIZATION,
                format!("Bearer {access_token}"),
            )
            .header(http::header::ACCEPT, "application/json")
            .body(Vec::new())
            .map_err(|e| e.to_string())?;
        let res = self.http.call(req).await.map_err(|e| e.to_string())?;
        if res.status() != http::StatusCode::OK {
            return Err(format!("userinfo answered {}", res.status()));
        }
        serde_json::from_slice(res.body()).map_err(|e| format!("userinfo is not JSON: {e}"))
    }

    /// Drop cached metadata (e.g. after a signature failure, to pick up rotated keys).
    pub(super) async fn forget(&self, id: &str) {
        self.cache.lock().await.remove(id);
    }
}
