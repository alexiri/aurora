//! Provider metadata cache and the HTTP client used for discovery and token exchange.
use aurora_config::OidcProvider;
use openidconnect::core::CoreProviderMetadata;
use openidconnect::{IssuerUrl, reqwest};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const METADATA_TTL: Duration = Duration::from_secs(600);

pub struct OidcClients {
    pub(super) http: reqwest::Client,
    cache: Mutex<HashMap<String, (Instant, CoreProviderMetadata)>>,
}

impl OidcClients {
    pub fn new() -> Result<Self, String> {
        let http = reqwest::Client::builder()
            // Following redirects would open SSRF paths (openidconnect guidance).
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("oidc http client: {e}"))?;
        Ok(Self {
            http,
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

    /// Drop cached metadata (e.g. after a signature failure, to pick up rotated keys).
    pub(super) async fn forget(&self, id: &str) {
        self.cache.lock().await.remove(id);
    }
}
