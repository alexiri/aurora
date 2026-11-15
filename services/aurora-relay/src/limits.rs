//! The relay's own resource bounds (FIX-072, SEC-82, SEC-85; `docs/02 §Limits`, `docs/16 §W18`).
//!
//! * [`GrantRate`]: `PREVIEW_GRANT_RATE` — grant redemptions per minute per client address and
//!   per share, a fixed 60 s window **per replica** (like `PREVIEW_API_RATE`; the replicas do
//!   not share state, so the effective ceiling is the per-replica value times the replica
//!   count).
//! * [`secs_to_utc_midnight`]: the `Retry-After` of a `preview_quota` answer.
//!
//! The concurrent-stream caps live in the stream table (`stream::Streams::reserve`); the daily
//! byte quota is decided at the API from the shared counters (`resolve` answers
//! `deny`/`preview_quota`), the relay only reports the bytes it moved.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The window of the grant limiter.
const WINDOW: Duration = Duration::from_secs(60);
/// Grant redemptions per minute per share (`docs/02`: "and 10 per minute per share"; the
/// per-address 30 is `PREVIEW_GRANT_RATE`).
const GRANT_RATE_PER_SHARE: u32 = 10;
/// Entries beyond this count trigger a sweep of the expired windows.
const SWEEP_AT: usize = 10_000;

/// Which grant-redemption limit refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantRefusal {
    Address,
    Share,
}

impl GrantRefusal {
    /// The `aurora_relay_refused_total{reason}` label value.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Address => "grant_rate_address",
            Self::Share => "grant_rate_share",
        }
    }
}

/// Fixed-window limiter of grant redemptions (`PREVIEW_GRANT_RATE`).
pub struct GrantRate {
    per_address: u32,
    per_share: u32,
    window: Duration,
    windows: Mutex<HashMap<String, (Instant, u32)>>,
}

impl GrantRate {
    pub fn new(per_address: u32, per_share: u32) -> Self {
        Self {
            per_address,
            per_share,
            window: WINDOW,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// The spec's values: 30 per minute per address, 10 per minute per share.
    pub fn spec() -> Self {
        Self::new(
            aurora_core::limits::PREVIEW_GRANT_RATE,
            GRANT_RATE_PER_SHARE,
        )
    }

    /// Counts one redemption attempt for `(addr, label)`; `Err` carries the refusing limit and
    /// the seconds until its window ends. Nothing is consumed on refusal. An unknown client
    /// address shares one bucket (`unknown`), never an unlimited one.
    pub fn check(&self, addr: Option<IpAddr>, label: &str) -> Result<(), (GrantRefusal, u64)> {
        let now = Instant::now();
        let akey = format!(
            "a:{}",
            addr.map_or_else(|| "unknown".to_owned(), |a| a.to_string())
        );
        let skey = format!("s:{label}");
        let mut g = self.windows.lock().unwrap_or_else(|p| p.into_inner());
        if g.len() >= SWEEP_AT {
            let w = self.window;
            g.retain(|_, (start, _)| now.duration_since(*start) < w);
        }
        let left = |key: &str, limit: u32| -> Option<u64> {
            let (start, n) = g.get(key).copied().unwrap_or((now, 0));
            let live = now.duration_since(start) < self.window;
            let n = if live { n } else { 0 };
            (n >= limit).then(|| {
                let rest = self.window.saturating_sub(now.duration_since(start));
                rest.as_secs() + u64::from(rest.subsec_nanos() > 0)
            })
        };
        if let Some(s) = left(&akey, self.per_address) {
            return Err((GrantRefusal::Address, s.max(1)));
        }
        if let Some(s) = left(&skey, self.per_share) {
            return Err((GrantRefusal::Share, s.max(1)));
        }
        for key in [akey, skey] {
            let e = g.entry(key).or_insert((now, 0));
            if now.duration_since(e.0) >= self.window {
                *e = (now, 0);
            }
            e.1 += 1;
        }
        Ok(())
    }
}

/// The visitor's address (FIX-090): the socket `peer`, or — when the peer is inside
/// `relay.trusted_proxies` — the rightmost `X-Forwarded-For` entry that is not itself a trusted
/// proxy (entries to its left are the visitor's to forge and are ignored). An entry that is not
/// an address ends the walk at the last good one. Same rule as the API's `server.trusted_proxies`.
pub fn client_address(
    peer: Option<IpAddr>,
    xff: &[&str],
    proxies: &[aurora_core::cidr::Cidr],
) -> Option<IpAddr> {
    let peer = peer?;
    let trusted = |ip: IpAddr| proxies.iter().any(|c| c.contains(ip));
    if !trusted(peer) {
        return Some(peer);
    }
    let mut candidate = peer;
    for entry in xff.iter().flat_map(|h| h.split(',')).rev() {
        let Ok(ip) = entry.trim().parse::<IpAddr>() else {
            break;
        };
        candidate = ip;
        if !trusted(ip) {
            break;
        }
    }
    Some(candidate)
}

/// Negative resolves per client address per minute (FIX-090): a resolve that comes back `login`
/// or `deny` is counted against the address that sent the credential; past this many, a request
/// whose credential is not already cached is answered locally without asking the API. Random
/// cookies (each a distinct cache key, each a call) can no longer turn one visitor into the
/// relay plane's whole `PREVIEW_API_RATE` class.
pub const NEGATIVE_RESOLVES_PER_MIN: u32 = 30;

/// Fixed-window counter of negative resolves per address (per replica).
pub struct NegativeResolves {
    per_window: u32,
    window: Duration,
    windows: Mutex<HashMap<Option<IpAddr>, (Instant, u32)>>,
}

impl NegativeResolves {
    pub fn new(per_window: u32) -> Self {
        Self {
            per_window,
            window: WINDOW,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Whether `addr` may still spend an API call on an uncached credential.
    pub fn allows(&self, addr: Option<IpAddr>) -> bool {
        let g = self.windows.lock().unwrap_or_else(|p| p.into_inner());
        g.get(&addr)
            .is_none_or(|(start, n)| start.elapsed() >= self.window || *n < self.per_window)
    }

    /// Counts one negative resolve for `addr`.
    pub fn record(&self, addr: Option<IpAddr>) {
        let mut g = self.windows.lock().unwrap_or_else(|p| p.into_inner());
        if g.len() >= SWEEP_AT {
            let w = self.window;
            g.retain(|_, (start, _)| start.elapsed() < w);
        }
        let e = g.entry(addr).or_insert((Instant::now(), 0));
        if e.0.elapsed() >= self.window {
            *e = (Instant::now(), 0);
        }
        e.1 += 1;
    }
}

/// The relay's time limits (`docs/16 §W18` Limits, `docs/02 §Limits`). Production uses
/// [`Timeouts::spec`]; tests build shortened ones through `Streams::with_timeouts`, so the
/// production code path is the same one and no test-only branch exists.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// A request head must arrive within this (slow-loris; `PREVIEW_HEADER_TIMEOUT`).
    pub header: Duration,
    /// Idle limit of a plain HTTP stream (`PREVIEW_IDLE_TIMEOUT`, 120 s).
    pub idle_http: Duration,
    /// Idle limit of an upgraded stream (`PREVIEW_WS_IDLE_TIMEOUT`, 600 s; pings count).
    pub idle_ws: Duration,
    /// Total lifetime of a stream (`PREVIEW_STREAM_LIFETIME`, 4 h).
    pub lifetime: Duration,
    /// How often open streams are re-validated at the API (`PREVIEW_AUTH_CACHE`, 15 s).
    pub validate_every: Duration,
    /// How long the closing `Close` frame to the runner may take before the leg is dropped
    /// anyway (`docs/16 §W18`: 2 s): a stalled runner never holds a stream slot.
    pub close: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self::spec()
    }
}

impl Timeouts {
    pub const fn spec() -> Self {
        Self {
            header: aurora_core::limits::PREVIEW_HEADER_TIMEOUT,
            idle_http: aurora_core::limits::PREVIEW_IDLE_TIMEOUT,
            idle_ws: aurora_core::limits::PREVIEW_WS_IDLE_TIMEOUT,
            lifetime: aurora_core::limits::PREVIEW_STREAM_LIFETIME,
            validate_every: aurora_core::limits::PREVIEW_AUTH_CACHE,
            close: Duration::from_secs(2),
        }
    }
}

/// Seconds from now to the next UTC midnight (at least 1).
pub fn secs_to_utc_midnight() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    (86_400 - now % 86_400).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_share_and_per_address_windows() {
        let g = GrantRate::new(30, 10);
        let ip: Option<IpAddr> = Some("10.0.0.1".parse().unwrap());
        for _ in 0..10 {
            assert!(g.check(ip, "pa").is_ok());
        }
        let (why, retry) = g.check(ip, "pa").unwrap_err();
        assert_eq!(why, GrantRefusal::Share);
        assert!((1..=60).contains(&retry));
        // Another share from the same address still passes until the address cap.
        for l in ["pb", "pc"] {
            for _ in 0..10 {
                assert!(g.check(ip, l).is_ok());
            }
        }
        let (why, _) = g.check(ip, "pd").unwrap_err();
        assert_eq!(why, GrantRefusal::Address);
        // Another address is unaffected.
        assert!(g.check(Some("10.0.0.2".parse().unwrap()), "pd").is_ok());
    }

    #[test]
    fn client_address_is_the_rightmost_untrusted_hop_of_a_trusted_peer() {
        let proxies = [aurora_core::cidr::Cidr::parse("10.0.0.0/8").unwrap()];
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        // An untrusted peer's header is ignored (it could say anything).
        assert_eq!(
            client_address(Some(ip("1.2.3.4")), &["9.9.9.9"], &proxies),
            Some(ip("1.2.3.4"))
        );
        // No proxies configured: the peer is always the client.
        assert_eq!(
            client_address(Some(ip("10.0.0.1")), &["9.9.9.9"], &[]),
            Some(ip("10.0.0.1"))
        );
        // A trusted peer: the rightmost entry that is not a proxy; forged left entries ignored.
        assert_eq!(
            client_address(
                Some(ip("10.0.0.1")),
                &["6.6.6.6, 5.5.5.5, 10.0.0.2"],
                &proxies
            ),
            Some(ip("5.5.5.5"))
        );
        // Several header lines count as one list; garbage ends the walk.
        assert_eq!(
            client_address(Some(ip("10.0.0.1")), &["7.7.7.7", "5.5.5.5"], &proxies),
            Some(ip("5.5.5.5"))
        );
        assert_eq!(
            client_address(Some(ip("10.0.0.1")), &["5.5.5.5, junk"], &proxies),
            Some(ip("10.0.0.1"))
        );
        assert_eq!(client_address(None, &["5.5.5.5"], &proxies), None);
    }

    #[test]
    fn negative_resolves_are_capped_per_address() {
        let n = NegativeResolves::new(3);
        let a = Some("10.0.0.1".parse().unwrap());
        let b = Some("10.0.0.2".parse().unwrap());
        for _ in 0..3 {
            assert!(n.allows(a));
            n.record(a);
        }
        assert!(
            !n.allows(a),
            "the fourth uncached credential is not sent to the API"
        );
        assert!(n.allows(b), "another address is unaffected");
    }

    #[test]
    fn midnight_is_within_a_day() {
        let s = secs_to_utc_midnight();
        assert!((1..=86_400).contains(&s));
    }
}
