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
    /// (SEC-148) The permits of every repository read of this replica.
    pub reads: ReadSlots,
    /// Test hook: how many skill directories refresh passes of this replica have loaded.
    pub loads: std::sync::atomic::AtomicU64,
}

/// (SEC-148) One shared cap on the repository reads running at once on a replica
/// (`LIBRARY_READS_CONCURRENT`): one read can hold ~200 MiB, so check, create, patch, browse, add
/// and refresh draw from the same permits. An interactive read waits for a permit **within its
/// deadline** (`LIBRARY_READ_DEADLINE`) and answers 502 `library_unavailable` when none frees up
/// or the read itself runs out of time; a refresh waits as long as it takes.
pub struct ReadSlots {
    sem: Arc<tokio::sync::Semaphore>,
    deadline: Duration,
}

impl ReadSlots {
    #[must_use]
    pub fn new(permits: usize, deadline: Duration) -> Self {
        Self {
            sem: Arc::new(tokio::sync::Semaphore::new(permits)),
            deadline,
        }
    }

    /// Permits free right now (tests).
    #[must_use]
    pub fn available(&self) -> usize {
        self.sem.available_permits()
    }

    /// Runs the read `fut` holding a permit, all of it (the wait included) under the deadline.
    pub async fn interactive<T>(
        &self,
        fut: impl std::future::Future<Output = Result<T, aurora_core::ApiError>>,
    ) -> Result<T, aurora_core::ApiError> {
        let run = async {
            let _permit = self.sem.acquire().await.map_err(|_| busy())?;
            fut.await
        };
        match tokio::time::timeout(self.deadline, run).await {
            Ok(r) => r,
            Err(_) => Err(busy()),
        }
    }

    /// A permit for a background read (refresh): waits without a deadline.
    pub async fn background(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        self.sem.clone().acquire_owned().await.ok()
    }

    /// A permit for a read that sits under another deadline (browse): waits for one.
    pub async fn permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, aurora_core::ApiError> {
        self.sem.clone().acquire_owned().await.map_err(|_| busy())
    }
}

fn busy() -> aurora_core::ApiError {
    aurora_core::ApiError::bad_gateway(
        "library_unavailable",
        "The repository could not be read: too many repositories are being read right now, or the git host did not answer in time. Try again shortly.",
    )
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
            reads: ReadSlots::new(
                aurora_core::limits::LIBRARY_READS_CONCURRENT,
                aurora_core::limits::LIBRARY_READ_DEADLINE,
            ),
            loads: std::sync::atomic::AtomicU64::new(0),
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// (SEC-148) At most `permits` reads run at once; a read that finds none free within the
    /// deadline answers `library_unavailable`; permits come back afterwards.
    #[tokio::test]
    async fn reads_are_capped_and_a_full_replica_answers_unavailable() {
        let slots = Arc::new(ReadSlots::new(2, Duration::from_secs(10)));
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let (slots, running, peak) = (slots.clone(), running.clone(), peak.clone());
            tasks.push(tokio::spawn(async move {
                slots
                    .interactive(async {
                        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        running.fetch_sub(1, Ordering::SeqCst);
                        Ok::<_, aurora_core::ApiError>(())
                    })
                    .await
            }));
        }
        for t in tasks {
            t.await.unwrap().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(slots.available(), 2);

        // Both permits held: a third read waits its deadline, then the existing 502.
        let short = Arc::new(ReadSlots::new(2, Duration::from_millis(150)));
        let held = (
            short.background().await.unwrap(),
            short.background().await.unwrap(),
        );
        let err = short
            .interactive(async { Ok::<_, aurora_core::ApiError>(()) })
            .await
            .unwrap_err();
        assert_eq!(err.status(), axum::http::StatusCode::BAD_GATEWAY);
        assert_eq!(err.code(), "library_unavailable");
        drop(held);
        short
            .interactive(async { Ok::<_, aurora_core::ApiError>(()) })
            .await
            .unwrap();
        assert_eq!(short.available(), 2);

        // A read that runs past its deadline gives its permit back too.
        let err = short
            .interactive(async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok::<_, aurora_core::ApiError>(())
            })
            .await
            .unwrap_err();
        assert_eq!(err.code(), "library_unavailable");
        assert_eq!(short.available(), 2);
    }
}
