//! (P5-UX-14, D-350, C-985, SEC-129) Account health from real use.
//!
//! When an upstream answers a request in which the gateway injected an account's credential, the answer
//! says something about that account: a 401 means the app no longer accepts what Aurora holds
//! (`rejected`); a later 2xx or 3xx means it works again (`ok`). Nothing else is recorded: a 403 is usually
//! a permission, not a broken credential, and any other status says nothing about the credential.
//!
//! Writes are informational and bounded: only on a change of state (the statement itself skips a row that
//! already holds the result), at most once a minute per account, off the request path, and never to
//! `status` or anything an agent's access reads. A process that provokes refusals can therefore cause one
//! warning on one account row and nothing else.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    Rejected,
    Ok,
}

impl Observed {
    /// The upstream status of a request with an injected credential, as health: 401 → rejected,
    /// 2xx or 3xx → ok, anything else (a 403, a 404, a 5xx) → nothing.
    pub fn of_status(status: u16) -> Option<Self> {
        match status {
            401 => Some(Self::Rejected),
            200..=399 => Some(Self::Ok),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Ok => "ok",
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

struct Seen {
    result: Observed,
    at: Instant,
}

/// The in-memory half of the recording: what was last sent per account, and when.
pub struct AccountHealth {
    min_interval: Duration,
    seen: Mutex<HashMap<Uuid, Seen>>,
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
    /// since the last write for this account, and when the state is what was last sent.
    pub fn should_write(&self, id: Uuid, observed: Observed, now: Instant) -> bool {
        let Ok(mut seen) = self.seen.lock() else {
            return false;
        };
        if let Some(s) = seen.get(&id)
            && (s.result == observed || now.duration_since(s.at) < self.min_interval)
        {
            return false;
        }
        if seen.len() >= MAX_TRACKED {
            seen.clear();
        }
        seen.insert(
            id,
            Seen {
                result: observed,
                at: now,
            },
        );
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
            let _ = aurora_db::repo::gateway::record_account_check(
                &pool,
                matches!(account, AccountId::Connection(_)),
                account.id(),
                observed.as_str(),
            )
            .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_401_and_a_success_say_anything() {
        assert_eq!(Observed::of_status(401), Some(Observed::Rejected));
        for s in [200, 204, 301, 304, 399] {
            assert_eq!(Observed::of_status(s), Some(Observed::Ok), "{s}");
        }
        for s in [100, 400, 403, 404, 429, 500, 503] {
            assert_eq!(Observed::of_status(s), None, "{s}");
        }
    }

    #[test]
    fn writes_only_on_a_change_and_at_most_once_a_minute() {
        let h = AccountHealth::new();
        let id = Uuid::now_v7();
        let t0 = Instant::now();
        assert!(h.should_write(id, Observed::Rejected, t0));
        // The same state again, even after a minute: nothing to write.
        assert!(!h.should_write(id, Observed::Rejected, t0 + Duration::from_secs(120)));
        // A change inside the minute waits for the next request after it.
        assert!(!h.should_write(id, Observed::Ok, t0 + Duration::from_secs(30)));
        assert!(h.should_write(id, Observed::Ok, t0 + Duration::from_secs(61)));
        // Another account is independent.
        assert!(h.should_write(Uuid::now_v7(), Observed::Ok, t0));
    }
}
