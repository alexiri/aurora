//! The `memory` backend: per-process counters, no infrastructure (the default, and the `local`
//! fallback of the other backends).

use crate::{
    BucketTake, CounterStore, Period, Result, Verdict, WindowProbe, WindowUnit, check_key,
    gcra_new_tat, gcra_wait_ms, interval_ms, normalize_units,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Hard cap on tracked windows and buckets each (memory bound; eviction only ever refills).
const MAX_ENTRIES: usize = 200_000;

type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

#[derive(Default)]
struct Inner {
    /// `(key, period)` -> `(window start ms, count)`.
    windows: HashMap<(String, Period), (i64, u64)>,
    /// key -> theoretical arrival time, ms.
    buckets: HashMap<String, i64>,
    calls: u64,
}

pub struct MemoryStore {
    clock: Clock,
    inner: Mutex<Inner>,
}

fn system_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::with_clock(Arc::new(system_ms))
    }

    /// A store reading time (UTC milliseconds) from `clock` (tests).
    pub fn with_clock(clock: Clock) -> Self {
        Self {
            clock,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Every stored key (key-privacy tests).
    pub fn keys(&self) -> Vec<String> {
        let g = self.lock();
        g.windows
            .keys()
            .map(|(k, _)| k.clone())
            .chain(g.buckets.keys().cloned())
            .collect()
    }

    /// Number of tracked windows and buckets (tests).
    pub fn tracked(&self) -> usize {
        let g = self.lock();
        g.windows.len() + g.buckets.len()
    }
}

fn window_start(now: i64, p: Period) -> i64 {
    let m = i64::try_from(p.millis()).unwrap_or(i64::MAX);
    now - now.rem_euclid(m)
}

fn sweep(g: &mut Inner, now: i64) {
    g.calls += 1;
    if g.calls.is_multiple_of(512) || g.windows.len() + g.buckets.len() > MAX_ENTRIES {
        g.windows
            .retain(|(_, p), (ws, _)| *ws + i64::try_from(p.millis()).unwrap_or(0) + 60_000 > now);
        g.buckets.retain(|_, tat| *tat + 60_000 > now);
        // Still over the cap: drop the oldest (a refill, never a bypass of the memory bound).
        while g.windows.len() > MAX_ENTRIES {
            let Some(k) = g
                .windows
                .iter()
                .min_by_key(|(_, (ws, _))| *ws)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            g.windows.remove(&k);
        }
        while g.buckets.len() > MAX_ENTRIES {
            let Some(k) = g
                .buckets
                .iter()
                .min_by_key(|(_, t)| **t)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            g.buckets.remove(&k);
        }
    }
}

#[async_trait]
impl CounterStore for MemoryStore {
    fn backend(&self) -> &'static str {
        "memory"
    }

    async fn fixed_window_peek(&self, probes: &[WindowProbe]) -> Result<Vec<u64>> {
        for p in probes {
            check_key(&p.key)?;
        }
        let now = (self.clock)();
        let g = self.lock();
        Ok(probes
            .iter()
            .map(|p| match g.windows.get(&(p.key.clone(), p.period)) {
                Some((ws, n)) if *ws == window_start(now, p.period) => *n,
                _ => 0,
            })
            .collect())
    }

    async fn fixed_window_consume(&self, units: &[WindowUnit]) -> Result<Verdict> {
        for u in units {
            check_key(&u.key)?;
        }
        let units = normalize_units(units);
        let now = (self.clock)();
        let mut g = self.lock();
        sweep(&mut g, now);
        let mut worst: Option<(String, u64)> = None;
        for u in &units {
            let ws = window_start(now, u.period);
            let cur = match g.windows.get(&(u.key.clone(), u.period)) {
                Some((w, n)) if *w == ws => *n,
                _ => 0,
            };
            if cur.saturating_add(u.cost) > u.limit {
                let retry = u64::try_from(ws + i64::try_from(u.period.millis()).unwrap_or(0) - now)
                    .unwrap_or(1)
                    .max(1);
                if worst.as_ref().is_none_or(|(_, r)| retry > *r) {
                    worst = Some((u.key.clone(), retry));
                }
            }
        }
        if let Some((key, ms)) = worst {
            return Ok(Verdict::Refused {
                key,
                retry_after: Duration::from_millis(ms),
            });
        }
        for u in &units {
            let ws = window_start(now, u.period);
            let e = g
                .windows
                .entry((u.key.clone(), u.period))
                .or_insert((ws, 0));
            if e.0 != ws {
                *e = (ws, 0);
            }
            e.1 += u.cost;
        }
        Ok(Verdict::Allowed)
    }

    async fn bucket_take(&self, takes: &[BucketTake]) -> Result<Verdict> {
        for t in takes {
            check_key(&t.key)?;
        }
        let now = (self.clock)();
        let mut g = self.lock();
        sweep(&mut g, now);
        let mut worst: Option<(String, u64)> = None;
        // Repeated keys within one call are charged cumulatively.
        let mut pending: HashMap<&str, i64> = HashMap::new();
        for t in takes {
            let tms = interval_ms(t.emission_interval);
            let tat = pending
                .get(t.key.as_str())
                .copied()
                .or_else(|| g.buckets.get(&t.key).copied())
                .unwrap_or(now);
            if let Some(w) = gcra_wait_ms(tat, now, tms, t.burst) {
                if worst.as_ref().is_none_or(|(_, r)| w > *r) {
                    worst = Some((t.key.clone(), w));
                }
            } else {
                pending.insert(t.key.as_str(), gcra_new_tat(tat, now, tms));
            }
        }
        if let Some((key, ms)) = worst {
            return Ok(Verdict::Refused {
                key,
                retry_after: Duration::from_millis(ms),
            });
        }
        for (k, v) in pending {
            g.buckets.insert(k.to_owned(), v);
        }
        Ok(Verdict::Allowed)
    }

    async fn health(&self) -> Result<()> {
        Ok(())
    }
}
