//! In-memory token buckets keyed `(bucket name, key)` (docs/04 §Rate limits (auth)).
//! Per API replica; the key is a client IP or a lower-cased email.

use aurora_core::ApiError;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct Quota {
    pub capacity: u32,
    pub per: Duration,
}

/// The `04` quotas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    SignInIp,
    SignInEmail,
    SignUpIp,
    ResetEmail,
}

impl Bucket {
    pub const fn name(self) -> &'static str {
        match self {
            Self::SignInIp => "signin_ip",
            Self::SignInEmail => "signin_email",
            Self::SignUpIp => "signup_ip",
            Self::ResetEmail => "reset_email",
        }
    }
    pub const fn quota(self) -> Quota {
        match self {
            Self::SignInIp => Quota {
                capacity: 10,
                per: Duration::from_secs(60),
            },
            Self::SignInEmail => Quota {
                capacity: 5,
                per: Duration::from_secs(60),
            },
            Self::SignUpIp => Quota {
                capacity: 5,
                per: Duration::from_secs(3600),
            },
            Self::ResetEmail => Quota {
                capacity: 3,
                per: Duration::from_secs(3600),
            },
        }
    }
}

struct State {
    tokens: f64,
    last: Instant,
}

const PRUNE_AT: usize = 10_000;

#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<(&'static str, String), State>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume one token from a standard bucket.
    pub fn check(&self, bucket: Bucket, key: &str) -> Result<(), ApiError> {
        self.check_quota(bucket.name(), key, bucket.quota())
    }

    /// Consume one token with an explicit quota; 429 + `Retry-After` when empty.
    pub fn check_quota(&self, name: &'static str, key: &str, q: Quota) -> Result<(), ApiError> {
        let now = Instant::now();
        let cap = f64::from(q.capacity);
        let rate = cap / q.per.as_secs_f64();
        let mut map = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        if map.len() > PRUNE_AT {
            map.retain(|_, s| s.tokens + now.duration_since(s.last).as_secs_f64() * rate < cap);
        }
        let s = map.entry((name, key.to_owned())).or_insert(State {
            tokens: cap,
            last: now,
        });
        s.tokens = (s.tokens + now.duration_since(s.last).as_secs_f64() * rate).min(cap);
        s.last = now;
        if s.tokens >= 1.0 {
            s.tokens -= 1.0;
            Ok(())
        } else {
            let wait = ((1.0 - s.tokens) / rate).ceil().max(1.0) as u64;
            Err(ApiError::rate_limited(
                wait,
                "Too many requests. Try again later.",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausts_and_isolates_keys() {
        let rl = RateLimiter::new();
        let q = Quota {
            capacity: 2,
            per: Duration::from_secs(60),
        };
        assert!(rl.check_quota("t", "a", q).is_ok());
        assert!(rl.check_quota("t", "a", q).is_ok());
        let e = rl.check_quota("t", "a", q).unwrap_err();
        assert_eq!(e.status(), 429);
        assert!(e.retry_after_secs().unwrap() >= 1);
        assert!(rl.check_quota("t", "b", q).is_ok());
    }
}
