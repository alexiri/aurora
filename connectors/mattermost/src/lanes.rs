//! Ordered dispatch per key (docs/20 §Event ordering).
//!
//! Mattermost delivers a channel's events in order over the WebSocket, but handling them in one task
//! each lets a slow lookup (channel, user, file) let a later event overtake an earlier one: a post
//! deleted right after it was written reached the core as "deleted" (an unknown message) followed by
//! "posted" with the original text. Every event of one key (a channel) goes through one queue and is
//! handled to completion before the next starts; different keys still run in parallel.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::mpsc;

/// A lane's worker ends after this long without events (the next event starts a new one).
const IDLE: Duration = Duration::from_secs(60);
/// Closed lanes are pruned from the map once it holds this many.
const PRUNE_AT: usize = 1024;

pub struct Lanes<T> {
    lanes: Mutex<HashMap<String, mpsc::UnboundedSender<T>>>,
}

impl<T> Default for Lanes<T> {
    fn default() -> Self {
        Self {
            lanes: Mutex::new(HashMap::new()),
        }
    }
}

impl<T: Send + 'static> Lanes<T> {
    /// Queues `item` on the lane of `key`; `handler` runs for it after every earlier item of the
    /// same key has finished.
    pub fn dispatch<F, Fut>(&self, key: &str, item: T, handler: F)
    where
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut lanes = self.lanes.lock().unwrap_or_else(|e| e.into_inner());
        let mut item = item;
        if let Some(tx) = lanes.get(key) {
            match tx.send(item) {
                Ok(()) => return,
                // The lane's worker went idle between events: start a new one.
                Err(mpsc::error::SendError(back)) => item = back,
            }
        }
        if lanes.len() >= PRUNE_AT {
            lanes.retain(|_, tx| !tx.is_closed());
        }
        let (tx, mut rx) = mpsc::unbounded_channel();
        // The receiver is alive, so this cannot fail.
        let _ = tx.send(item);
        lanes.insert(key.to_owned(), tx);
        tokio::spawn(async move {
            loop {
                match tokio::time::timeout(IDLE, rx.recv()).await {
                    Ok(Some(item)) => handler(item).await,
                    Ok(None) => return,
                    Err(_) => {
                        // Idle: refuse new items (the dispatcher then starts a fresh lane), then
                        // finish what slipped in meanwhile.
                        rx.close();
                        while let Ok(item) = rx.try_recv() {
                            handler(item).await;
                        }
                        return;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A slow first event of a key does not let the second overtake it; another key is not held up.
    #[tokio::test]
    async fn one_key_is_ordered_and_keys_run_in_parallel() {
        let lanes: Lanes<(&'static str, u64)> = Lanes::default();
        let seen: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let done = Arc::new(tokio::sync::Notify::new());
        let (seen2, done2) = (seen.clone(), done.clone());
        let handler = move |(name, delay_ms): (&'static str, u64)| {
            let (seen, done) = (seen2.clone(), done2.clone());
            async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                seen.lock().unwrap().push(name);
                done.notify_one();
            }
        };
        lanes.dispatch("c1", ("posted", 150), handler.clone());
        lanes.dispatch("c1", ("deleted", 0), handler.clone());
        lanes.dispatch("c2", ("other", 20), handler);
        for _ in 0..3 {
            tokio::time::timeout(Duration::from_secs(5), done.notified())
                .await
                .unwrap();
        }
        let seen = seen.lock().unwrap().clone();
        let pos = |n: &str| seen.iter().position(|s| *s == n).unwrap();
        assert!(pos("posted") < pos("deleted"), "{seen:?}");
        assert!(
            pos("other") < pos("posted"),
            "another channel is not held back: {seen:?}"
        );
    }
}
