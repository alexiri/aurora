//! Shared state of one terminator replica: the registration's configuration and CA keys, the
//! in-memory session table (nothing survives a restart; the API's lease sweeps the rows), the
//! heartbeat loop with its fail-closed rule (`SSH_HEARTBEAT_FAILURES` consecutive failures close
//! every session and refuse new authentications until one succeeds) and close reporting (once
//! per session; the API's guarded transition makes a second report a no-op).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use aurora_core::limits::SSH_HEARTBEAT_FAILURES;
use aurora_protocol::terminator as wire;
use ssh_key::{Fingerprint, HashAlg, PublicKey};
use tokio::sync::watch;
use uuid::Uuid;

use crate::limits::PreAuth;
use crate::plane::Plane;

/// The knobs tests shorten; production uses [`Settings::default`].
#[derive(Debug, Clone)]
pub struct Settings {
    /// The delay of every authentication failure (`SSH_AUTH_FAILURE_DELAY`).
    pub auth_failure_delay: Duration,
    /// From accept to authenticated (`SSH_AUTH_TIMEOUT`).
    pub auth_timeout: Duration,
    /// Overrides the registration's heartbeat period.
    pub heartbeat_period: Option<Duration>,
    /// Server keepalive on the SSH transport (every 30 s).
    pub keepalive: Duration,
    /// How long a notice may wait for the client's `CHANNEL_CLOSE` before `DISCONNECT`.
    pub notice_wait: Duration,
    /// Overrides the registration's idle timeout / maximum lifetime.
    pub idle: Option<Duration>,
    pub lifetime: Option<Duration>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auth_failure_delay: aurora_core::limits::SSH_AUTH_FAILURE_DELAY,
            auth_timeout: aurora_core::limits::SSH_AUTH_TIMEOUT,
            heartbeat_period: None,
            keepalive: Duration::from_secs(30),
            notice_wait: Duration::from_secs(2),
            idle: None,
            lifetime: None,
        }
    }
}

/// Counts of one session (never content).
#[derive(Default)]
pub struct Stats {
    pub bytes_in: AtomicU64,
    pub bytes_out: AtomicU64,
    pub channels_total: AtomicU32,
    pub channels_open: AtomicU32,
    pub shell: AtomicU32,
    pub exec: AtomicU32,
    pub sftp: AtomicU32,
    /// Unix seconds of the last channel byte (0 = none yet).
    pub last_activity: AtomicI64,
}

impl Stats {
    fn touch(&self) {
        self.last_activity
            .store(chrono::Utc::now().timestamp(), Ordering::Relaxed);
    }
    pub fn add_in(&self, n: usize) {
        self.bytes_in.fetch_add(n as u64, Ordering::Relaxed);
        metrics::counter!("aurora_terminator_bytes_total", "direction" => "to_sandbox")
            .increment(n as u64);
        self.touch();
    }
    pub fn add_out(&self, n: usize) {
        self.bytes_out.fetch_add(n as u64, Ordering::Relaxed);
        metrics::counter!("aurora_terminator_bytes_total", "direction" => "from_sandbox")
            .increment(n as u64);
        self.touch();
    }
    pub fn count_mode(&self, m: wire::ChannelMode) {
        self.channels_total.fetch_add(1, Ordering::Relaxed);
        match m {
            wire::ChannelMode::Shell => &self.shell,
            wire::ChannelMode::Exec => &self.exec,
            wire::ChannelMode::Sftp => &self.sftp,
        }
        .fetch_add(1, Ordering::Relaxed);
        metrics::counter!("aurora_terminator_channels_total", "mode" => m.as_str()).increment(1);
    }
    fn last_activity_rfc3339(&self) -> Option<String> {
        let t = self.last_activity.load(Ordering::Relaxed);
        (t > 0)
            .then(|| chrono::DateTime::from_timestamp(t, 0))
            .flatten()
            .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    }
    fn idle_for(&self, opened: Instant) -> Duration {
        let t = self.last_activity.load(Ordering::Relaxed);
        if t <= 0 {
            return opened.elapsed();
        }
        let ago = chrono::Utc::now().timestamp().saturating_sub(t).max(0) as u64;
        Duration::from_secs(ago).min(opened.elapsed())
    }
}

/// One open SSH session of this replica.
pub struct Sess {
    pub id: Uuid,
    pub opened: Instant,
    pub stats: Arc<Stats>,
    /// Set once to end the connection (reason for the close report and the notice).
    pub close_tx: watch::Sender<Option<String>>,
    reported: AtomicBool,
}

impl Sess {
    pub fn new(id: Uuid) -> Arc<Self> {
        Arc::new(Self {
            id,
            opened: Instant::now(),
            stats: Arc::new(Stats::default()),
            close_tx: watch::channel(None).0,
            reported: AtomicBool::new(false),
        })
    }

    /// Asks the connection to end with `reason` (first reason wins).
    pub fn request_close(&self, reason: &str) {
        self.close_tx.send_if_modified(|v| {
            if v.is_none() {
                *v = Some(reason.to_owned());
                true
            } else {
                false
            }
        });
    }

    pub fn idle_for(&self) -> Duration {
        self.stats.idle_for(self.opened)
    }
}

pub struct Shared {
    pub plane: Plane,
    pub settings: Settings,
    pub cfg: RwLock<wire::TerminatorConfig>,
    cas: RwLock<Vec<Fingerprint>>,
    /// `false` after `SSH_HEARTBEAT_FAILURES` failed heartbeats: no new authentication.
    pub healthy: AtomicBool,
    pub sessions: Mutex<HashMap<Uuid, Arc<Sess>>>,
    pub pre: Arc<PreAuth>,
    pub stop: watch::Receiver<bool>,
    /// The host public key line, re-sent at every registration.
    pub host_public_key: String,
}

impl Shared {
    pub fn new(
        plane: Plane,
        settings: Settings,
        pre: Arc<PreAuth>,
        stop: watch::Receiver<bool>,
        host_public_key: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            plane,
            settings,
            cfg: RwLock::new(wire::TerminatorConfig {
                ca_public_keys: vec![],
                heartbeat_secs: aurora_core::limits::SSH_HEARTBEAT.as_secs() as u32,
                lease_secs: aurora_core::limits::SSH_SESSION_LEASE.as_secs() as u32,
                wake_wait_secs: aurora_core::limits::SSH_WAKE_WAIT.as_secs() as u32,
                idle_timeout_secs: aurora_core::limits::SSH_IDLE_TIMEOUT.as_secs() as u32,
                max_lifetime_secs: aurora_core::limits::SSH_MAX_LIFETIME.as_secs() as u32,
                channels_per_session: aurora_core::limits::SSH_CHANNELS_PER_SESSION,
            }),
            cas: RwLock::new(vec![]),
            healthy: AtomicBool::new(false),
            sessions: Mutex::new(HashMap::new()),
            pre,
            stop,
            host_public_key,
        })
    }

    /// Takes the API's configuration (register and heartbeat answers): CA keys included, so a
    /// rotation reaches the replica within one heartbeat.
    pub fn apply_config(&self, c: wire::TerminatorConfig) {
        let fps: Vec<Fingerprint> = c
            .ca_public_keys
            .iter()
            .filter_map(|l| PublicKey::from_openssh(l.trim()).ok())
            .map(|k| k.fingerprint(HashAlg::Sha256))
            .collect();
        *self.cas.write().unwrap_or_else(|e| e.into_inner()) = fps;
        *self.cfg.write().unwrap_or_else(|e| e.into_inner()) = c;
    }

    pub fn cas(&self) -> Vec<Fingerprint> {
        self.cas.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn cfg(&self) -> wire::TerminatorConfig {
        self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn idle_limit(&self) -> Duration {
        self.settings
            .idle
            .unwrap_or_else(|| Duration::from_secs(u64::from(self.cfg().idle_timeout_secs)))
    }

    pub fn lifetime_limit(&self) -> Duration {
        self.settings
            .lifetime
            .unwrap_or_else(|| Duration::from_secs(u64::from(self.cfg().max_lifetime_secs)))
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    pub fn add_session(&self, s: Arc<Sess>) {
        let mut g = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        g.insert(s.id, s);
        metrics::gauge!("aurora_terminator_sessions_open").set(g.len() as f64);
    }

    /// Removes the session and reports its close once (idempotent at the API as well).
    pub async fn finish(&self, s: &Arc<Sess>, reason: &str) {
        {
            let mut g = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            g.remove(&s.id);
            metrics::gauge!("aurora_terminator_sessions_open").set(g.len() as f64);
        }
        if s.reported.swap(true, Ordering::AcqRel) {
            return;
        }
        let label = reason.split(':').next().unwrap_or(reason).to_owned();
        metrics::counter!("aurora_terminator_session_closed_total", "reason" => label).increment(1);
        let st = &s.stats;
        let req = wire::SessionCloseRequest {
            reason: reason.to_owned(),
            channels_total: st.channels_total.load(Ordering::Relaxed),
            channels_by_mode: wire::ChannelsByMode {
                shell: st.shell.load(Ordering::Relaxed),
                exec: st.exec.load(Ordering::Relaxed),
                sftp: st.sftp.load(Ordering::Relaxed),
            },
            bytes_in: st.bytes_in.load(Ordering::Relaxed),
            bytes_out: st.bytes_out.load(Ordering::Relaxed),
        };
        // Best effort: the lease sweeps a session whose close never arrives.
        if let Err(e) = self.plane.close(s.id, &req).await {
            tracing::warn!(session = %s.id, error = %e, "close report failed");
        }
    }

    fn beats(&self) -> Vec<wire::SessionBeat> {
        let g = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        g.values()
            .map(|s| wire::SessionBeat {
                id: s.id,
                last_activity_at: s.stats.last_activity_rfc3339(),
                channels_open: s.stats.channels_open.load(Ordering::Relaxed),
                channels_total: s.stats.channels_total.load(Ordering::Relaxed),
                bytes_in: s.stats.bytes_in.load(Ordering::Relaxed),
                bytes_out: s.stats.bytes_out.load(Ordering::Relaxed),
            })
            .collect()
    }

    fn close_all(&self, reason: &str) {
        let g = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        for s in g.values() {
            s.request_close(reason);
        }
    }

    /// Registers (retrying until it works or `stop`), then heartbeats forever. After any 401 the
    /// registration is repeated. Returns when `stop` turns true.
    pub async fn run_plane(self: Arc<Self>) {
        let mut stop = self.stop.clone();
        let mut failures = 0u32;
        loop {
            // Register (also after a restart of the API or a 401).
            let reg = loop {
                match self.plane.register(&self.host_public_key).await {
                    Ok(r) => break r,
                    Err(e) => {
                        tracing::warn!(error = %e, "terminator registration failed; retrying");
                        tokio::select! {
                            () = tokio::time::sleep(Duration::from_secs(2)) => {}
                            _ = stop.wait_for(|v| *v) => return,
                        }
                    }
                }
            };
            self.apply_config(reg.config);
            self.healthy.store(true, Ordering::Relaxed);
            tracing::info!(replica = %reg.replica_id, "terminator registered");
            'beat: loop {
                let period = self
                    .settings
                    .heartbeat_period
                    .unwrap_or_else(|| Duration::from_secs(u64::from(self.cfg().heartbeat_secs)));
                tokio::select! {
                    () = tokio::time::sleep(period) => {}
                    _ = stop.wait_for(|v| *v) => return,
                }
                match self.plane.heartbeat(self.beats()).await {
                    Ok(h) => {
                        failures = 0;
                        self.healthy.store(true, Ordering::Relaxed);
                        self.apply_config(h.config);
                        let g = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
                        for r in h.revoked {
                            if let Some(s) = g.get(&r.id) {
                                s.request_close(&format!("revoked:{}", r.reason));
                            }
                        }
                    }
                    Err(e) => {
                        metrics::counter!("aurora_terminator_heartbeat_failures_total")
                            .increment(1);
                        failures += 1;
                        tracing::warn!(error = %e, failures, "heartbeat failed");
                        if failures >= SSH_HEARTBEAT_FAILURES {
                            // Fail closed: the leases have run out server-side anyway.
                            self.healthy.store(false, Ordering::Relaxed);
                            self.close_all("lease_expired");
                        }
                        if e.needs_register() {
                            break 'beat;
                        }
                    }
                }
            }
        }
    }
}
