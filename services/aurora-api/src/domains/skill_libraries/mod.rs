//! Git skill libraries (phase 4, P4-GSL, D-148, SEC-83; docs/10 §Git skill library; phase 5,
//! P5-SKLIB-1, D-448, SEC-148). A library is an https git URL (with an optional sealed,
//! read-only token) plus ref and path: a *source*, never a tier. Only skills people add are
//! stored (pinned, read-only); a refresh only writes proposals. The API reads repositories
//! itself with the git smart-HTTP protocol (`git`), on its guarded `http_instance` client.
pub mod access;
pub mod content;
pub mod dto;
pub mod git;
pub mod refresh;
pub(crate) mod routes;
pub mod service;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use routes::router;
use uuid::Uuid;

/// Per-replica state of the feature: the 5-minute browse cache and the refresh lease owner.
pub struct LibraryEnv {
    pub(crate) cache: Mutex<HashMap<Uuid, CachedListing>>,
    /// One cold listing at a time per library (FIX-096): later browsers wait for the first and
    /// then read the cache.
    pub(crate) flights: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    /// Identifies this replica as the owner of refresh leases.
    pub replica: Uuid,
    /// The timeout of one git request (`LIBRARY_FETCH_TIMEOUT`; tests shorten it).
    pub fetch_timeout: Duration,
}

pub(crate) struct CachedListing {
    pub at: Instant,
    pub fingerprint: String,
    pub listing: Arc<service::Listing>,
}

impl Default for LibraryEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl LibraryEnv {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
            replica: Uuid::now_v7(),
            fetch_timeout: aurora_core::limits::LIBRARY_FETCH_TIMEOUT,
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
