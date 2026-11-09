//! Gateway availability state under a Postgres outage (docs/06 §Operation during a Postgres
//! outage, phase 3, RFE-014, D-61, C-380..C-384).
//!
//! One [`Health`] per process tracks three states:
//! * `ready`: database reachable, LISTEN connection established, snapshot loaded;
//! * `degraded`: snapshot loaded but the database is unreachable (two failed probes or a
//!   hot-path connection-class error) **or** the LISTEN connection has been down for more than
//!   [`LISTEN_GRACE`];
//! * `down`: no snapshot loaded, or draining.
//!
//! The hot path only reads atomics ([`Health::db_unreachable`], [`Health::down_reason`]); the
//! state itself is derived under a mutex by [`Health::evaluate`] (probe ticks, event setters and
//! the `/readyz` handler), which also emits the metrics and the `gateway.state_change` line.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aurora_db::PgPool;
use chrono::{DateTime, SecondsFormat, Utc};
use tokio::sync::Notify;

/// `GATEWAY_DB_PROBE_INTERVAL`.
pub const DB_PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// Probe statement timeout.
pub const DB_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Consecutive failed probes that make the database *unreachable*.
pub const PROBE_FAILURES: u32 = 2;
/// `GATEWAY_LISTEN_GRACE`: a LISTEN connection that is back within this does not flap readiness.
pub const LISTEN_GRACE: Duration = Duration::from_secs(30);
/// `REQUEST_LOG_OUTAGE_BUFFER`.
pub const REQUEST_LOG_OUTAGE_BUFFER: usize = 10_000;
/// Reconnect backoff of the LISTEN connection (jittered, doubling).
pub const RECONNECT_BACKOFF_MIN: Duration = Duration::from_millis(500);
pub const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(10);
/// `retry-after` of every degraded-mode refusal.
pub const RETRY_AFTER_SECS: u64 = 5;

/// Prefix of the error strings that carry a connection-class database failure through the
/// string-typed policy and governance sources (see [`db_err`]).
const DB_UNAVAILABLE_MARK: &str = "database unavailable: ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ready,
    Degraded,
    Down,
}

impl State {
    pub const ALL: [State; 3] = [State::Ready, State::Degraded, State::Down];

    pub const fn as_str(self) -> &'static str {
        match self {
            State::Ready => "ready",
            State::Degraded => "degraded",
            State::Down => "down",
        }
    }

    fn code(self) -> u8 {
        self as u8
    }
}

/// Why the state is not `ready` (the `reason` of `/readyz`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    DatabaseUnavailable,
    ListenerDown,
    SnapshotNotLoaded,
    Draining,
}

impl Reason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Reason::DatabaseUnavailable => "database_unavailable",
            Reason::ListenerDown => "listener_down",
            Reason::SnapshotNotLoaded => "snapshot_not_loaded",
            Reason::Draining => "draining",
        }
    }
}

/// The state with its reason and the instant it was entered.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub state: State,
    pub reason: Option<Reason>,
    pub since: DateTime<Utc>,
}

impl Snapshot {
    pub fn since_rfc3339(&self) -> String {
        self.since.to_rfc3339_opts(SecondsFormat::Millis, true)
    }
}

struct Inner {
    probe_failures: u32,
    db_unreachable_since: Option<(Instant, DateTime<Utc>)>,
    listener_up: bool,
    listener_down_since: Instant,
    current: Snapshot,
}

pub struct Health {
    inner: Mutex<Inner>,
    /// Hot-path mirrors of the inputs.
    db_down: AtomicBool,
    state_code: AtomicU8,
    draining: AtomicBool,
    snapshot_loaded: Box<dyn Fn() -> bool + Send + Sync>,
    listen_grace: Duration,
    probe_interval: Duration,
    probe_timeout: Duration,
    /// Woken when the database becomes reachable again (request-log flush, hold reconcile).
    recovered: Notify,
}

impl Health {
    /// `snapshot_loaded` reports whether a registry snapshot is loaded (`down` until it is).
    pub fn new(snapshot_loaded: impl Fn() -> bool + Send + Sync + 'static) -> Arc<Self> {
        Self::with_timing(
            snapshot_loaded,
            LISTEN_GRACE,
            DB_PROBE_INTERVAL,
            DB_PROBE_TIMEOUT,
        )
    }

    /// Overrides the grace and the probe timing (tests).
    pub fn with_timing(
        snapshot_loaded: impl Fn() -> bool + Send + Sync + 'static,
        listen_grace: Duration,
        probe_interval: Duration,
        probe_timeout: Duration,
    ) -> Arc<Self> {
        let h = Arc::new(Self {
            inner: Mutex::new(Inner {
                probe_failures: 0,
                db_unreachable_since: None,
                listener_up: false,
                listener_down_since: Instant::now(),
                current: Snapshot {
                    state: State::Down,
                    reason: Some(Reason::SnapshotNotLoaded),
                    since: Utc::now(),
                },
            }),
            db_down: AtomicBool::new(false),
            state_code: AtomicU8::new(State::Down.code()),
            draining: AtomicBool::new(false),
            snapshot_loaded: Box::new(snapshot_loaded),
            listen_grace,
            probe_interval,
            probe_timeout,
            recovered: Notify::new(),
        });
        h.publish_state(State::Down);
        metrics::gauge!("aurora_gateway_db_unreachable_seconds").set(0.0);
        h
    }

    // ---- hot path (atomics only) ----------------------------------------------------------

    /// True while the database is considered unreachable: lookups that need it are refused at
    /// once instead of waiting for the pool timeout.
    pub fn db_unreachable(&self) -> bool {
        self.db_down.load(Ordering::Acquire)
    }

    /// True when the last evaluated state is not `ready` (for the served-from-cache counter).
    pub fn is_degraded_fast(&self) -> bool {
        self.state_code.load(Ordering::Relaxed) == State::Degraded.code()
    }

    /// Why new `CONNECT`s are refused (`down`): no snapshot loaded, or draining.
    pub fn down_reason(&self) -> Option<Reason> {
        if self.draining.load(Ordering::Acquire) {
            Some(Reason::Draining)
        } else if !(self.snapshot_loaded)() {
            Some(Reason::SnapshotNotLoaded)
        } else {
            None
        }
    }

    /// Resolves when the database becomes reachable again.
    pub async fn wait_recovered(&self) {
        self.recovered.notified().await;
    }

    pub fn probe_interval(&self) -> Duration {
        self.probe_interval
    }

    // ---- inputs ----------------------------------------------------------------------------

    /// SIGTERM received: `down` (`draining`).
    pub fn set_draining(&self) {
        self.draining.store(true, Ordering::Release);
        self.evaluate();
    }

    /// A hot-path query failed with a connection-class error: unreachable at once.
    pub fn report_db_error(&self) {
        {
            let Ok(mut g) = self.inner.lock() else { return };
            g.probe_failures = g.probe_failures.max(PROBE_FAILURES);
            if g.db_unreachable_since.is_none() {
                g.db_unreachable_since = Some((Instant::now(), Utc::now()));
            }
            self.db_down.store(true, Ordering::Release);
        }
        self.evaluate();
    }

    /// A probe succeeded: the database is reachable again.
    pub fn probe_ok(&self) {
        let recovered = {
            let Ok(mut g) = self.inner.lock() else { return };
            g.probe_failures = 0;
            let was = g.db_unreachable_since.take().is_some();
            self.db_down.store(false, Ordering::Release);
            was
        };
        self.evaluate();
        if recovered {
            self.recovered.notify_waiters();
        }
    }

    /// A probe failed; the second in a row makes the database unreachable.
    pub fn probe_failed(&self) {
        {
            let Ok(mut g) = self.inner.lock() else { return };
            g.probe_failures += 1;
            if g.probe_failures >= PROBE_FAILURES && g.db_unreachable_since.is_none() {
                g.db_unreachable_since = Some((Instant::now(), Utc::now()));
                self.db_down.store(true, Ordering::Release);
            }
        }
        self.evaluate();
    }

    /// The LISTEN connection is established (every channel subscribed) or lost.
    pub fn set_listener(&self, up: bool) {
        {
            let Ok(mut g) = self.inner.lock() else { return };
            if g.listener_up != up {
                g.listener_up = up;
                if !up {
                    g.listener_down_since = Instant::now();
                }
            }
        }
        self.evaluate();
    }

    // ---- derived state ---------------------------------------------------------------------

    /// Derives the state from the inputs, publishes the gauges and logs a transition.
    pub fn evaluate(&self) -> Snapshot {
        let Ok(mut g) = self.inner.lock() else {
            return Snapshot {
                state: State::Down,
                reason: None,
                since: Utc::now(),
            };
        };
        let (state, reason) = if let Some(r) = self.down_reason() {
            (State::Down, Some(r))
        } else if g.db_unreachable_since.is_some() {
            (State::Degraded, Some(Reason::DatabaseUnavailable))
        } else if !g.listener_up && g.listener_down_since.elapsed() > self.listen_grace {
            (State::Degraded, Some(Reason::ListenerDown))
        } else {
            (State::Ready, None)
        };
        let secs = g
            .db_unreachable_since
            .map_or(0.0, |(at, _)| at.elapsed().as_secs_f64());
        metrics::gauge!("aurora_gateway_db_unreachable_seconds").set(secs);
        let prev = g.current;
        if prev.state != state || prev.reason != reason {
            g.current = Snapshot {
                state,
                reason,
                since: Utc::now(),
            };
            self.publish_state(state);
            tracing::warn!(
                event = "gateway.state_change",
                from = prev.state.as_str(),
                to = state.as_str(),
                reason = reason.map_or("recovered", Reason::as_str),
                "gateway.state_change"
            );
        }
        g.current
    }

    fn publish_state(&self, state: State) {
        self.state_code.store(state.code(), Ordering::Relaxed);
        for s in State::ALL {
            metrics::gauge!("aurora_gateway_state", "state" => s.as_str()).set(if s == state {
                1.0
            } else {
                0.0
            });
        }
    }

    pub fn state(&self) -> State {
        self.evaluate().state
    }

    pub fn listener_connected(&self) -> bool {
        self.inner.lock().is_ok_and(|g| g.listener_up)
    }

    /// When the database became unreachable (`None` when reachable).
    pub fn db_unreachable_since(&self) -> Option<DateTime<Utc>> {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.db_unreachable_since.map(|(_, t)| t))
    }

    /// Probes the database every interval until `shutdown`: `SELECT 1`, with a timeout.
    pub async fn run_probe(
        self: Arc<Self>,
        pool: PgPool,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                () = crate::invalidation::stopped(&mut shutdown) => return,
                () = tokio::time::sleep(self.probe_interval) => {}
            }
            let r =
                tokio::time::timeout(self.probe_timeout, sqlx::query("SELECT 1").execute(&pool))
                    .await;
            match r {
                Ok(Ok(_)) => self.probe_ok(),
                // A statement-level failure on `SELECT 1` is not expected; any failure of the
                // probe counts, like a refused connection does.
                Ok(Err(_)) | Err(_) => self.probe_failed(),
            }
        }
    }
}

/// True for failures of the connection, not of the statement: pool acquire timeout, closed
/// pool, I/O and TLS errors, and SQLSTATE `57P01`/`57P02`/`57P03` or class `08`.
pub fn is_connection_error(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed => true,
        sqlx::Error::Database(d) => d
            .code()
            .is_some_and(|c| matches!(&*c, "57P01" | "57P02" | "57P03") || c.starts_with("08")),
        // A server that closes the socket during the TLS negotiation (stopping, or a proxy in
        // front of it dropping the connection) is read by sqlx as a garbled `SSLRequest` answer
        // (`0x00`), not as an I/O error.
        sqlx::Error::Protocol(m) => m.contains("SSLRequest"),
        _ => false,
    }
}

/// [`is_connection_error`] for the repository error type.
pub fn is_db_connection_error(e: &aurora_db::DbError) -> bool {
    matches!(e, aurora_db::DbError::Other(s) if is_connection_error(s))
}

/// The error string of a string-typed source: a connection-class failure is marked so the
/// consumer can answer 503 `gateway_degraded` and the resolver can report it to [`Health`].
pub fn db_err(e: &aurora_db::DbError) -> String {
    if is_db_connection_error(e) {
        format!("{DB_UNAVAILABLE_MARK}{e}")
    } else {
        e.to_string()
    }
}

/// The marked string for "refused without asking: the database is known to be unreachable".
pub fn db_unreachable_err() -> String {
    format!("{DB_UNAVAILABLE_MARK}not attempted")
}

/// True when `s` came from [`db_err`] or [`db_unreachable_err`] for a connection failure.
pub fn is_marked_db_err(s: &str) -> bool {
    s.starts_with(DB_UNAVAILABLE_MARK)
}

/// Doubling backoff with a cap.
pub fn next_backoff(cur: Duration, max: Duration) -> Duration {
    (cur * 2).min(max)
}

/// `d` scaled by a factor in `[0.5, 1.0]`.
pub fn jitter(d: Duration) -> Duration {
    let frac = f64::from(uuid::Uuid::now_v7().as_u128() as u32) / f64::from(u32::MAX);
    d.mul_f64(0.5 + 0.5 * frac)
}

/// Counts a request that a degraded gateway answered from its caches.
pub fn count_served(reason: &'static str) {
    metrics::counter!(
        "aurora_gateway_degraded_requests_total",
        "outcome" => "served_from_cache", "reason" => reason
    )
    .increment(1);
}

/// Counts a request refused because of the outage.
pub fn count_refused(reason: &'static str) {
    metrics::counter!(
        "aurora_gateway_degraded_requests_total",
        "outcome" => "refused", "reason" => reason
    )
    .increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health(loaded: bool, grace: Duration) -> Arc<Health> {
        Health::with_timing(
            move || loaded,
            grace,
            Duration::from_millis(10),
            Duration::from_millis(10),
        )
    }

    #[test]
    fn down_until_snapshot_loaded_then_ready_once_listener_is_up() {
        let h = health(false, Duration::from_secs(30));
        assert_eq!(h.state(), State::Down);
        let h = health(true, Duration::from_secs(30));
        // Within the grace a listener that is not yet up does not make the gateway degraded.
        assert_eq!(h.state(), State::Ready);
        h.set_listener(true);
        assert_eq!(h.state(), State::Ready);
    }

    #[test]
    fn two_failed_probes_make_the_database_unreachable_and_one_success_clears_it() {
        let h = health(true, Duration::from_secs(30));
        h.set_listener(true);
        h.probe_failed();
        assert_eq!(h.state(), State::Ready);
        assert!(!h.db_unreachable());
        h.probe_failed();
        let s = h.evaluate();
        assert_eq!(s.state, State::Degraded);
        assert_eq!(s.reason, Some(Reason::DatabaseUnavailable));
        assert!(h.db_unreachable());
        h.probe_ok();
        assert_eq!(h.state(), State::Ready);
        assert!(!h.db_unreachable());
    }

    #[test]
    fn a_connection_error_on_the_hot_path_is_an_outage_at_once() {
        let h = health(true, Duration::from_secs(30));
        h.set_listener(true);
        h.report_db_error();
        assert_eq!(h.state(), State::Degraded);
        assert!(h.db_unreachable_since().is_some());
    }

    #[test]
    fn a_listener_that_stays_down_past_the_grace_degrades_and_a_reconnect_recovers() {
        let h = health(true, Duration::from_millis(30));
        h.set_listener(true);
        h.set_listener(false);
        assert_eq!(h.state(), State::Ready, "within the grace");
        std::thread::sleep(Duration::from_millis(50));
        let s = h.evaluate();
        assert_eq!(s.state, State::Degraded);
        assert_eq!(s.reason, Some(Reason::ListenerDown));
        h.set_listener(true);
        assert_eq!(h.state(), State::Ready);
    }

    #[test]
    fn recovery_needs_the_probe_and_the_listener() {
        let h = health(true, Duration::from_millis(1));
        h.set_listener(true);
        h.report_db_error();
        h.set_listener(false);
        std::thread::sleep(Duration::from_millis(5));
        h.probe_ok();
        assert_eq!(h.evaluate().reason, Some(Reason::ListenerDown));
        h.set_listener(true);
        assert_eq!(h.state(), State::Ready);
    }

    #[test]
    fn draining_is_down_whatever_else_holds() {
        let h = health(true, Duration::from_secs(30));
        h.set_listener(true);
        h.set_draining();
        let s = h.evaluate();
        assert_eq!(s.state, State::Down);
        assert_eq!(s.reason, Some(Reason::Draining));
    }

    #[test]
    fn backoff_doubles_to_the_cap_and_jitter_stays_in_range() {
        let mut d = RECONNECT_BACKOFF_MIN;
        let mut seen = vec![d];
        for _ in 0..6 {
            d = next_backoff(d, RECONNECT_BACKOFF_MAX);
            seen.push(d);
        }
        assert_eq!(
            seen.iter().map(|d| d.as_millis()).collect::<Vec<_>>(),
            vec![500, 1000, 2000, 4000, 8000, 10_000, 10_000]
        );
        for _ in 0..100 {
            let j = jitter(Duration::from_secs(10));
            assert!(j >= Duration::from_secs(5) && j <= Duration::from_secs(10));
        }
    }

    #[test]
    fn only_connection_class_errors_are_outages() {
        assert!(is_connection_error(&sqlx::Error::PoolTimedOut));
        assert!(is_connection_error(&sqlx::Error::Io(
            std::io::Error::other("reset")
        )));
        assert!(is_connection_error(&sqlx::Error::Protocol(
            "unexpected response from SSLRequest: 0x00".into()
        )));
        assert!(!is_connection_error(&sqlx::Error::Protocol(
            "bad message".into()
        )));
        assert!(!is_connection_error(&sqlx::Error::RowNotFound));
        assert!(!is_connection_error(&sqlx::Error::ColumnNotFound(
            "x".into()
        )));
        let marked = db_err(&aurora_db::DbError::Other(sqlx::Error::PoolTimedOut));
        assert!(is_marked_db_err(&marked));
        assert!(!is_marked_db_err(&db_err(&aurora_db::DbError::NotFound)));
    }
}
