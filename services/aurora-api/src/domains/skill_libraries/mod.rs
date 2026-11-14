//! Git skill libraries (phase 4, P4-GSL, D-148, D-149, SEC-83, SEC-84; docs/10 §Git skill
//! library). A library is an org-scoped app connection of a git-hosting package plus
//! repository, ref and path: a *source*, never a tier. Only skills people add are stored (pinned,
//! read-only); a refresh only writes proposals. Reads go through the gateway
//! (`POST /internal/v1/library-fetch`), so the API never holds a git credential.
pub mod content;
pub mod dto;
pub mod gateway;
pub mod refresh;
pub(crate) mod routes;
pub mod service;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub use routes::router;
use uuid::Uuid;

/// Per-replica state of the feature: the gateway client and the 5-minute browse cache.
pub struct LibraryEnv {
    pub gateway: Arc<dyn gateway::LibraryGateway>,
    pub(crate) cache: Mutex<HashMap<Uuid, CachedListing>>,
    /// One cold listing at a time per library (FIX-096): later browsers wait for the first and
    /// then read the cache.
    pub(crate) flights: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    /// Identifies this replica as the owner of refresh leases.
    pub replica: Uuid,
}

pub(crate) struct CachedListing {
    pub at: Instant,
    pub fingerprint: String,
    pub listing: Arc<service::Listing>,
}

impl LibraryEnv {
    #[must_use]
    pub fn new(gateway: Arc<dyn gateway::LibraryGateway>) -> Self {
        Self {
            gateway,
            cache: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
            replica: Uuid::now_v7(),
        }
    }

    pub(crate) fn forget(&self, library: Uuid) {
        if let Ok(mut c) = self.cache.lock() {
            c.remove(&library);
        }
    }

    /// The single-flight lock of a library's cold listing.
    pub(crate) fn flight(&self, library: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        let mut f = self
            .flights
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f.entry(library).or_default().clone()
    }
}
