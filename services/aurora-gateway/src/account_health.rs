//! (P5-UX-14, D-350, C-985, SEC-129; P5-POLISH-3, D-383, C-1033) Account health from real use.
//!
//! When an upstream answers a request in which the gateway injected an account's credential, the answer
//! says something about that account: a 401 or 403 means the app refused what Aurora holds (`rejected`);
//! a 2xx or 3xx means it works (`ok`). Any other status says nothing about the credential. What is written
//! is `last_ok_at` or `last_error_at`/`last_error`: only agent calls reach this path, and a Test is
//! recorded apart (`last_test_*`, written by the API), so a passed Test never reads as use.
//!
//! Writes are informational and bounded: at most once a minute per account **and outcome** (so a failure is
//! written even inside the minute after a success and is never hidden), off the request path, and never to
//! `status` or anything an agent's access reads. A process that provokes refusals can therefore cause one
//! write a minute on one account row and nothing else.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// At most one write per account per this long.
const MIN_INTERVAL: Duration = Duration::from_secs(60);
/// Bound on remembered accounts; beyond it the memory is dropped and rebuilt (a write is then re-attempted,
/// and the statement skips an unchanged row).
const MAX_TRACKED: usize = 50_000;

/// What real use showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Observed {
    Rejected,
    Ok,
}

impl Observed {
    /// The upstream status of a request with an injected credential, as health: 401 or 403 → rejected,
    /// 2xx or 3xx → ok, anything else (a 404, a 5xx) → nothing.
    pub fn of_status(status: u16) -> Option<Self> {
        match status {
            401 | 403 => Some(Self::Rejected),
            200..=399 => Some(Self::Ok),
            _ => None,
        }
    }

    /// The `last_error` value of a failure; `None` for a success.
    pub fn error(self) -> Option<&'static str> {
        match self {
            Self::Rejected => Some("rejected"),
            Self::Ok => None,
        }
    }
}

/// The account whose credential was injected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountId {
    Connection(Uuid),
    Secret(Uuid),
}

impl AccountId {
    fn id(self) -> Uuid {
        match self {
            Self::Connection(i) | Self::Secret(i) => i,
        }
    }
}

/// The in-memory half of the recording: when a write was last sent per account and outcome.
pub struct AccountHealth {
    min_interval: Duration,
    seen: Mutex<HashMap<(Uuid, Observed), Instant>>,
}

impl Default for AccountHealth {
    fn default() -> Self {
        Self::with_min_interval(MIN_INTERVAL)
    }
}

impl AccountHealth {
    pub fn new() -> Self {
        Self::default()
    }

    /// Another spacing between two writes for one account (tests; the contract is a minute, C-985).
    pub fn with_min_interval(min_interval: Duration) -> Self {
        Self {
            min_interval,
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Whether to send a write for `observed` now, and remember that it was sent. False inside the minute
    /// since the last write of the same outcome for this account; the other outcome has its own clock.
    pub fn should_write(&self, id: Uuid, observed: Observed, now: Instant) -> bool {
        let Ok(mut seen) = self.seen.lock() else {
            return false;
        };
        if let Some(at) = seen.get(&(id, observed))
            && now.duration_since(*at) < self.min_interval
        {
            return false;
        }
        if seen.len() >= MAX_TRACKED {
            seen.clear();
        }
        seen.insert((id, observed), now);
        true
    }

    /// Records one upstream answer to a request with `account`'s credential injected. Never blocks the
    /// response: the write is spawned and its failure ignored.
    pub fn observe(&self, pool: &aurora_db::PgPool, account: AccountId, status: u16) {
        let Some(observed) = Observed::of_status(status) else {
            return;
        };
        if !self.should_write(account.id(), observed, Instant::now()) {
            return;
        }
        let pool = pool.clone();
        tokio::spawn(async move {
            let _ = aurora_db::repo::gateway::record_account_use(
                &pool,
                matches!(account, AccountId::Connection(_)),
                account.id(),
                observed.error(),
            )
            .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_refusal_and_a_success_say_anything() {
        for s in [401, 403] {
            assert_eq!(Observed::of_status(s), Some(Observed::Rejected), "{s}");
        }
        for s in [200, 204, 301, 304, 399] {
            assert_eq!(Observed::of_status(s), Some(Observed::Ok), "{s}");
        }
        for s in [100, 400, 404, 429, 500, 503] {
            assert_eq!(Observed::of_status(s), None, "{s}");
        }
    }

    #[test]
    fn at_most_one_write_a_minute_per_outcome_and_a_failure_is_never_hidden() {
        let h = AccountHealth::new();
        let id = Uuid::now_v7();
        let t0 = Instant::now();
        assert!(h.should_write(id, Observed::Ok, t0));
        // A failure inside the minute after a success is written at once.
        assert!(h.should_write(id, Observed::Rejected, t0 + Duration::from_secs(1)));
        // The same outcome again inside the minute is not; after it, it is.
        assert!(!h.should_write(id, Observed::Rejected, t0 + Duration::from_secs(30)));
        assert!(!h.should_write(id, Observed::Ok, t0 + Duration::from_secs(30)));
        assert!(h.should_write(id, Observed::Rejected, t0 + Duration::from_secs(62)));
        // Another account is independent.
        assert!(h.should_write(Uuid::now_v7(), Observed::Ok, t0));
    }
}
