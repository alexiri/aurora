//! Per-key miss coalescing (docs/06 §Operation during a Postgres outage (b): "misses are
//! coalesced per key"). After a flush every entry is a miss; without this, N concurrent requests
//! for one key would each read the database. The first caller loads, the others wait on the same
//! key and then find the entry in the cache.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

pub struct Coalescer<K> {
    locks: Mutex<HashMap<K, Arc<AsyncMutex<()>>>>,
}

impl<K> Default for Coalescer<K> {
    fn default() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
        }
    }
}

/// Held while one caller loads `key`; dropping it lets the next waiter proceed.
pub struct Flight<'a, K: Hash + Eq + Clone> {
    owner: &'a Coalescer<K>,
    key: K,
    guard: Option<OwnedMutexGuard<()>>,
}

impl<K: Hash + Eq + Clone> Coalescer<K> {
    /// Waits until no other caller is loading `key`, then marks this caller as the loader. The
    /// caller re-checks its cache after this returns: the previous loader has usually filled it.
    pub async fn enter(&self, key: &K) -> Flight<'_, K> {
        let lock = match self.locks.lock() {
            Ok(mut m) => m.entry(key.clone()).or_default().clone(),
            Err(_) => Arc::default(),
        };
        let guard = lock.lock_owned().await;
        Flight {
            owner: self,
            key: key.clone(),
            guard: Some(guard),
        }
    }

    /// Keys with a loader or a waiter.
    pub fn in_flight(&self) -> usize {
        self.locks.lock().map_or(0, |m| m.len())
    }
}

impl<K: Hash + Eq + Clone> Drop for Flight<'_, K> {
    fn drop(&mut self) {
        // Release the lock first, then forget the key when nobody else waits on it.
        drop(self.guard.take());
        if let Ok(mut m) = self.owner.locks.lock()
            && m.get(&self.key).is_some_and(|l| Arc::strong_count(l) == 1)
        {
            m.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn concurrent_misses_for_one_key_load_once() {
        let c = Arc::new(Coalescer::<u8>::default());
        let cache = Arc::new(Mutex::new(None::<u32>));
        let loads = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let (c, cache, loads) = (c.clone(), cache.clone(), loads.clone());
            tasks.push(tokio::spawn(async move {
                if let Some(v) = *cache.lock().unwrap() {
                    return v;
                }
                let _f = c.enter(&1).await;
                if let Some(v) = *cache.lock().unwrap() {
                    return v;
                }
                loads.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                *cache.lock().unwrap() = Some(7);
                7
            }));
        }
        for t in tasks {
            assert_eq!(t.await.unwrap(), 7);
        }
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(c.in_flight(), 0, "keys are forgotten once nobody waits");
    }

    #[tokio::test]
    async fn different_keys_do_not_wait_for_each_other() {
        let c = Coalescer::<u8>::default();
        let a = c.enter(&1).await;
        let b = tokio::time::timeout(Duration::from_millis(200), c.enter(&2)).await;
        assert!(b.is_ok());
        drop(a);
    }
}
