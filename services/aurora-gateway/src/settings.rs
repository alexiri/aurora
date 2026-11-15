//! The gateway's instance-settings snapshot (phase 5, SET-1, D-236(f), `06 §Caches`): the
//! sections the gateway reads (`governance`, `llm`, `approvals`, `limits`, `secrets`) merged with
//! its own configuration exactly as the API merges them (`helm > stored > default`), reloaded on
//! `aurora_settings`, by a 30 s poll of the generations and on every LISTEN reconnect. A reload
//! that fails validation keeps the previous snapshot; a swap of `governance`, `llm`, `limits` or
//! `secrets` flushes every gateway cache (scope `all`). SET-1 wires no consumer: SET-2, SET-3,
//! SET-5 and FIX-124 read `snapshot()`.

use crate::invalidation::Hub;
use arc_swap::ArcSwap;
use aurora_config::{ConfigSource, Effective, Stored};
use aurora_core::limits::SETTINGS_RELOAD_POLL;
use aurora_db::PgPool;
use aurora_db::bus::{BusEvent, Channel, Listener};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The sections the gateway reads (`03 §instance_settings`).
pub const SECTIONS: [&str; 5] = ["governance", "llm", "approvals", "limits", "secrets"];

/// Is `section` one the gateway reads (the test sections too where the registry has them)?
fn reads(section: &str) -> bool {
    SECTIONS.contains(&section)
        || (section == aurora_config::TEST_SECTION
            && aurora_config::registry().has_section(section))
}

/// Sections whose swap flushes every gateway cache.
const FLUSHING: [&str; 4] = ["governance", "llm", "limits", "secrets"];

/// What the gateway serves from: the generation of each section read and the merged view.
#[derive(Debug)]
pub struct Snapshot {
    pub generations: BTreeMap<String, i64>,
    pub effective: Effective,
}

/// Why a reload did not swap.
#[derive(Debug)]
pub enum ReloadError {
    /// Rows not readable (outage, or the gateway role has no grant yet during a rollout).
    Database(String),
    /// The merged view is invalid: never swapped in.
    Invalid(String),
}

impl std::fmt::Display for ReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(m) | Self::Invalid(m) => f.write_str(m),
        }
    }
}

pub struct Settings {
    source: ConfigSource,
    /// Run `gateway_config()` on the merged view (false for hand-built tests).
    validate: bool,
    snap: ArcSwap<Snapshot>,
    failed: std::sync::atomic::AtomicU64,
}

impl Settings {
    /// Over the gateway's own configuration source (`None`: nothing locked, no validation).
    pub fn new(source: Option<ConfigSource>) -> Arc<Self> {
        let validate = source.is_some();
        let source = source.unwrap_or_else(ConfigSource::empty);
        let effective = source.effective(&Stored::default()).unwrap_or(Effective {
            raw: aurora_config::RawConfig::default(),
            keys: BTreeMap::new(),
        });
        Arc::new(Self {
            source,
            validate,
            snap: ArcSwap::from_pointee(Snapshot {
                generations: BTreeMap::new(),
                effective,
            }),
            failed: std::sync::atomic::AtomicU64::new(0),
        })
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snap.load_full()
    }

    /// Reloads rejected for failing validation since start (also `aurora_settings_reload_failed_total`).
    pub fn reload_failures(&self) -> u64 {
        self.failed.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn evaluate(&self, stored: &Stored) -> Result<Effective, String> {
        let problems = stored.type_problems();
        if let Some((k, m)) = problems.first() {
            return Err(format!("{k}: {m}"));
        }
        let eff = self.source.effective(stored).map_err(|e| e.to_string())?;
        if self.validate
            && let Err(e) = eff.raw.gateway_config()
        {
            return Err(e.to_string());
        }
        Ok(eff)
    }

    /// Read the rows, validate the merged view and swap. Returns the sections whose generation
    /// changed. A failure keeps the previous snapshot and counts
    /// `aurora_settings_reload_failed_total{service="gateway"}`.
    pub async fn reload(&self, pool: &PgPool) -> Result<Vec<String>, ReloadError> {
        use aurora_db::repo::instance_settings as repo;
        let rows = repo::list_all(pool)
            .await
            .map_err(|e| ReloadError::Database(e.to_string()))?;
        let generations: BTreeMap<String, i64> = rows
            .iter()
            .filter(|r| reads(&r.section))
            .map(|r| (r.section.clone(), r.generation))
            .collect();
        let read: Vec<&str> = rows
            .iter()
            .map(|r| r.section.as_str())
            .filter(|s| reads(s))
            .collect();
        let (stored, ignored) = Stored::from_rows(
            rows.iter().map(|r| (r.section.as_str(), &r.value)),
            Some(&read),
        );
        for what in ignored {
            tracing::warn!(%what, "stored instance setting ignored (unknown to this release)");
        }
        let effective = match self.evaluate(&stored) {
            Ok(e) => e,
            Err(why) => {
                self.failed
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                metrics::counter!(
                    "aurora_settings_reload_failed_total",
                    "service" => "gateway",
                    "section" => "all"
                )
                .increment(1);
                return Err(ReloadError::Invalid(why));
            }
        };
        let before = self.snapshot();
        let changed: Vec<String> = generations
            .iter()
            .filter(|(s, g)| before.generations.get(*s) != Some(*g))
            .map(|(s, _)| s.clone())
            .chain(
                before
                    .generations
                    .keys()
                    .filter(|s| !generations.contains_key(*s))
                    .cloned(),
            )
            .collect();
        self.snap.store(Arc::new(Snapshot {
            generations,
            effective,
        }));
        Ok(changed)
    }

    /// Reload and flush the caches the changed sections feed.
    async fn reload_and_flush(&self, pool: &PgPool, hub: &Hub) {
        match self.reload(pool).await {
            Ok(changed) => {
                if changed.iter().any(|s| FLUSHING.contains(&s.as_str())) {
                    metrics::counter!("aurora_gateway_cache_flush_total", "reason" => "settings")
                        .increment(1);
                    hub.flush_all();
                }
            }
            Err(ReloadError::Database(e)) => {
                tracing::warn!(error = %e, "settings not readable; keeping the previous snapshot (the configuration serves until the table is granted)");
            }
            Err(ReloadError::Invalid(e)) => {
                tracing::error!(error = %e, "stored settings failed validation; keeping the previous snapshot");
            }
        }
    }

    /// The reload task: LISTEN `aurora_settings`, the generation poll, and a reload on every
    /// (re)connect. Returns when `shutdown` flips to true.
    pub async fn run(
        self: Arc<Self>,
        pool: PgPool,
        hub: Arc<Hub>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            let mut listener = tokio::select! {
                () = crate::invalidation::stopped(&mut shutdown) => return,
                r = Listener::connect(&pool, &[Channel::Settings]) => match r {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::warn!(error = %e, "settings listener connect failed; retrying");
                        tokio::select! {
                            () = crate::invalidation::stopped(&mut shutdown) => return,
                            () = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
                        }
                        continue;
                    }
                },
            };
            let mut tick = tokio::time::interval(SETTINGS_RELOAD_POLL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    () = crate::invalidation::stopped(&mut shutdown) => return,
                    ev = listener.next() => match ev {
                        BusEvent::Resync => self.reload_and_flush(&pool, &hub).await,
                        BusEvent::Notification { payload, .. } => {
                            let section = payload.get("section").and_then(|v| v.as_str()).unwrap_or("");
                            let generation = payload.get("generation").and_then(serde_json::Value::as_i64).unwrap_or(i64::MAX);
                            let known = self.snapshot().generations.get(section).copied().unwrap_or(0);
                            if known < generation {
                                self.reload_and_flush(&pool, &hub).await;
                            }
                        }
                    },
                    _ = tick.tick() => {
                        if self.generations_differ(&pool).await {
                            self.reload_and_flush(&pool, &hub).await;
                        }
                    }
                }
            }
        }
    }

    async fn generations_differ(&self, pool: &PgPool) -> bool {
        let Ok(gens) = aurora_db::repo::instance_settings::generations(pool).await else {
            return false;
        };
        let snap = self.snapshot();
        let theirs: BTreeMap<&str, i64> = gens
            .iter()
            .filter(|(s, _)| reads(s))
            .map(|(s, g)| (s.as_str(), *g))
            .collect();
        let ours: BTreeMap<&str, i64> = snap
            .generations
            .iter()
            .map(|(s, g)| (s.as_str(), *g))
            .collect();
        theirs != ours
    }
}
