//! Snapshot and module catalogue pulled from the API's internal endpoint (D-23).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aurora_registry::snapshot::Snapshot;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OnceCell, RwLock};

use crate::SUPPORTED_CONTRACT;
use crate::runtime::{CallError, Module, Runtime};

pub const POLL: Duration = Duration::from_secs(60);
const MISS_REFRESH_GAP: Duration = Duration::from_secs(2);
const BLOB_MAX: usize = 64 * 1024 * 1024;
const SNAPSHOT_MAX: usize = 64 * 1024 * 1024;

pub struct Loaded {
    pub snapshot: Snapshot,
    pub hash: String,
}

pub struct Catalog {
    http: reqwest::Client,
    base: String,
    bearer: String,
    key_id: Option<String>,
    runtime: Runtime,
    current: RwLock<Option<Arc<Loaded>>>,
    modules: Mutex<HashMap<String, Arc<OnceCell<Module>>>>,
    last_refresh: Mutex<Option<Instant>>,
}

fn strip(d: &str) -> &str {
    d.strip_prefix("sha256:").unwrap_or(d)
}

impl Catalog {
    pub fn new(api_internal_url: &str, bearer: String, runtime: Runtime) -> Arc<Self> {
        Self::with_key_id(api_internal_url, bearer, None, runtime)
    }

    /// As [`Catalog::new`], presenting `X-Aurora-Key-Id: key_id` (RFE-007).
    pub fn with_key_id(
        api_internal_url: &str,
        bearer: String,
        key_id: Option<String>,
        runtime: Runtime,
    ) -> Arc<Self> {
        Arc::new(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            base: api_internal_url.trim_end_matches('/').to_owned(),
            bearer,
            key_id,
            runtime,
            current: RwLock::new(None),
            modules: Mutex::new(HashMap::new()),
            last_refresh: Mutex::new(None),
        })
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub async fn is_ready(&self) -> bool {
        self.current.read().await.is_some()
    }

    pub async fn current(&self) -> Option<Arc<Loaded>> {
        self.current.read().await.clone()
    }

    async fn get(&self, path: &str, max: usize) -> Result<Vec<u8>, String> {
        let mut resp = self
            .http
            .get(format!("{}{path}", self.base))
            .bearer_auth(&self.bearer)
            .headers(
                self.key_id
                    .iter()
                    .filter_map(|i| {
                        reqwest::header::HeaderValue::from_str(i).ok().map(|v| {
                            (
                                reqwest::header::HeaderName::from_static(
                                    aurora_core::limits::KEY_ID_HEADER,
                                ),
                                v,
                            )
                        })
                    })
                    .collect(),
            )
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        if !resp.status().is_success() {
            return Err(format!("status {}", resp.status()));
        }
        let mut out = Vec::new();
        while let Some(c) = resp
            .chunk()
            .await
            .map_err(|e| e.without_url().to_string())?
        {
            if out.len() + c.len() > max {
                return Err("response too large".into());
            }
            out.extend_from_slice(&c);
        }
        Ok(out)
    }

    /// Pull the active snapshot; unchanged hash is a no-op. Returns true if replaced.
    pub async fn refresh(&self) -> Result<bool, String> {
        *self.last_refresh.lock().await = Some(Instant::now());
        let body = self
            .get(
                &format!("/internal/v1/registry/active?contract_max={SUPPORTED_CONTRACT}"),
                SNAPSHOT_MAX,
            )
            .await?;
        let hash = hex::encode(Sha256::digest(&body));
        if self
            .current
            .read()
            .await
            .as_ref()
            .is_some_and(|c| c.hash == hash)
        {
            return Ok(false);
        }
        let snapshot = Snapshot::from_slice(&body).map_err(|e| format!("snapshot: {e}"))?;
        if snapshot.contract > SUPPORTED_CONTRACT {
            return Err(format!(
                "unsupported snapshot contract {}",
                snapshot.contract
            ));
        }
        let live: HashSet<&str> = snapshot
            .packages
            .iter()
            .filter_map(|p| p.hooks_digest.as_deref().map(strip))
            .collect();
        self.modules
            .lock()
            .await
            .retain(|d, _| live.contains(d.as_str()));
        *self.current.write().await = Some(Arc::new(Loaded { snapshot, hash }));
        Ok(true)
    }

    /// Refresh because of a cache miss, at most once per `MISS_REFRESH_GAP`.
    pub async fn refresh_on_miss(&self) {
        let recent = self
            .last_refresh
            .lock()
            .await
            .is_some_and(|t| t.elapsed() < MISS_REFRESH_GAP);
        if !recent && let Err(e) = self.refresh().await {
            tracing::warn!(error = %e, "snapshot refresh failed");
        }
    }

    /// Compiled module for a `hooks_digest`, pulling and verifying the blob on miss.
    ///
    /// Pull + compile run in a detached task: a caller that gives up (the gateway's 1 s
    /// summarize budget) must not cancel a cold compile, or the cache would never warm.
    pub async fn module(self: &Arc<Self>, digest: &str) -> Result<Module, CallError> {
        let this = self.clone();
        let digest = digest.to_owned();
        tokio::spawn(async move { this.module_inner(&digest).await })
            .await
            .map_err(|_| CallError::new("internal", "hook module load failed"))?
    }

    async fn module_inner(self: &Arc<Self>, digest: &str) -> Result<Module, CallError> {
        let key = strip(digest).to_ascii_lowercase();
        // (FIX-107) One cell per digest: the map lock is held only to read or insert the cell, so
        // a cold pull/compile of one package never delays another package's call or a snapshot
        // refresh; concurrent callers of the same digest share one load, and a failed load is
        // retried by the next caller.
        let cell = self
            .modules
            .lock()
            .await
            .entry(key.clone())
            .or_default()
            .clone();
        cell.get_or_try_init(|| self.load_module(digest, &key))
            .await
            .cloned()
    }

    async fn load_module(self: &Arc<Self>, digest: &str, key: &str) -> Result<Module, CallError> {
        let bytes = self
            .get(&format!("/internal/v1/registry/blobs/{digest}"), BLOB_MAX)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "module pull failed");
                CallError::new("internal", "hook module unavailable")
            })?;
        if hex::encode(Sha256::digest(&bytes)) != key {
            tracing::error!("hook module digest mismatch");
            return Err(CallError::new("internal", "hook module digest mismatch"));
        }
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.runtime.compile(&bytes))
            .await
            .map_err(|_| CallError::new("internal", "hook module compile failed"))?
            .map_err(|e| {
                tracing::error!(error = ?e, "hook module compile failed");
                CallError::new("internal", "hook module compile failed")
            })
    }

    /// Background poll: fast retries until the first snapshot, then every 60 s.
    pub fn spawn_poller(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                let wait = match this.refresh().await {
                    Ok(_) => POLL,
                    Err(e) => {
                        tracing::warn!(error = %e, "snapshot pull failed");
                        if this.is_ready().await {
                            POLL
                        } else {
                            Duration::from_secs(2)
                        }
                    }
                };
                tokio::time::sleep(wait).await;
            }
        })
    }
}
