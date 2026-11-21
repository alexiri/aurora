//! Per-organization token-bucket limiter of the internal check routes (the account Test, C-943).
//! (It was the skill-library fetch route's limiter until P5-SKLIB-1 removed that route, D-448.)

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// Rate limits: per gateway and per organization, per second.
pub const RATE_GLOBAL: f64 = 20.0;
pub const RATE_ORG: f64 = 5.0;

/// A token bucket refilled at `rate` per second with a burst of `rate`.
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// In-memory limiter of the route (the volume is bounded per replica, `LIBRARY_FETCH_RATE`).
pub struct Limiter {
    inner: Mutex<(Option<Bucket>, HashMap<Uuid, Bucket>)>,
    global_rate: f64,
    org_rate: f64,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::with_rates(RATE_GLOBAL, RATE_ORG)
    }
}

impl Limiter {
    /// Per-second rates (burst = rate) per gateway and per organization.
    #[must_use]
    pub fn with_rates(global: f64, org: f64) -> Self {
        Self {
            inner: Mutex::new((None, HashMap::new())),
            global_rate: global,
            org_rate: org,
        }
    }

    fn take(b: &mut Bucket, rate: f64, now: Instant) -> bool {
        let dt = now.saturating_duration_since(b.at).as_secs_f64();
        b.tokens = (b.tokens + dt * rate).min(rate);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Both buckets must have a token; a refused request consumes none.
    pub fn allow(&self, org: Uuid) -> bool {
        self.allow_at(org, Instant::now())
    }

    /// [`Limiter::allow`] at an explicit instant (the clock is injected by tests).
    pub fn allow_at(&self, org: Uuid, now: Instant) -> bool {
        let Ok(mut g) = self.inner.lock() else {
            return false;
        };
        let (global, orgs) = &mut *g;
        if orgs.len() > 10_000 {
            orgs.retain(|_, b| now.saturating_duration_since(b.at) < Duration::from_secs(5));
        }
        let (gr, or) = (self.global_rate, self.org_rate);
        let gb = global.get_or_insert(Bucket {
            tokens: gr,
            at: now,
        });
        let ob = orgs.entry(org).or_insert(Bucket {
            tokens: or,
            at: now,
        });
        // Peek both before consuming either.
        let g_ok =
            (gb.tokens + now.saturating_duration_since(gb.at).as_secs_f64() * gr).min(gr) >= 1.0;
        let o_ok =
            (ob.tokens + now.saturating_duration_since(ob.at).as_secs_f64() * or).min(or) >= 1.0;
        if !(g_ok && o_ok) {
            return false;
        }
        Self::take(gb, gr, now) && Self::take(ob, or, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_per_organization_and_per_gateway() {
        // The production limiter (20/s per gateway, 5/s per organization) on an injected clock.
        let t0 = Instant::now();
        let l = Limiter::default();
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        assert_eq!((0..8).filter(|_| l.allow_at(a, t0)).count(), 5, "per-org burst of 5");
        assert!(l.allow_at(b, t0), "another organization has its own bucket");
        assert!(!l.allow_at(a, t0));
        let later = t0 + Duration::from_secs(1);
        assert_eq!((0..8).filter(|_| l.allow_at(a, later)).count(), 5, "refilled at 5 per second");
        let g = Limiter::default();
        let allowed = (0..25).filter(|_| g.allow_at(Uuid::now_v7(), t0)).count();
        assert_eq!(allowed, 20, "per-gateway burst of 20");
    }
}
