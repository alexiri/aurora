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
pub const SECTIONS: [&str; 6] = [
    "governance",
    "llm",
    "approvals",
    "limits",
    "secrets",
    "outgoing_webhooks",
];

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
    secrets_target: std::sync::OnceLock<SecretsTarget>,
    /// Live readers run after a swap with the old and the new merged view, before the caches
    /// are flushed (`reload_and_flush`), so a flush never recomputes from the old ceiling.
    hooks: std::sync::Mutex<Vec<SwapHook>>,
}

/// A live reader of the merged view (SET-2: the instance governance; SET-3: the LLM registry).
pub type SwapHook = Arc<dyn Fn(&Effective, &Effective) + Send + Sync>;

/// Did any key of `section` (its dotted prefixes) change between two merged views?
pub fn section_changed(old: &Effective, new: &Effective, prefixes: &[&str]) -> bool {
    let keys = || {
        old.keys
            .keys()
            .chain(new.keys.keys())
            .filter(|k| prefixes.iter().any(|p| k.starts_with(p)))
    };
    keys().any(|k| old.keys.get(k).map(|e| &e.value) != new.keys.get(k).map(|e| &e.value))
}

/// (D-262) The host names (without port) of the LLM endpoints the deployment (Helm) defines: the
/// implicit allowance of the destination guard. A dashboard endpoint, however private its name,
/// is never in it: it is judged by `gateway.allowed_private` alone.
pub fn helm_endpoint_hosts(e: &Effective) -> Result<Vec<String>, String> {
    let helm: std::collections::BTreeSet<&str> = e
        .keys
        .get("llm.endpoints")
        .and_then(|k| k.entries.as_ref())
        .into_iter()
        .flatten()
        .filter(|x| x.source == aurora_config::Source::Helm)
        .map(|x| x.id.as_str())
        .collect();
    let registry =
        aurora_llm::ProviderRegistry::from_config(&e.raw.llm).map_err(|e| e.to_string())?;
    Ok(registry
        .endpoint_hosts_where(|id| helm.contains(id))
        .iter()
        .map(|h| h.rsplit_once(':').map_or(h.as_str(), |(b, _)| b).to_owned())
        .filter(|b| !b.is_empty())
        .collect())
}

const KEYS_OF_SECRETS: [&str; 2] = ["secrets.stores", "secrets.default_store"];

struct SecretsTarget {
    vault: Arc<aurora_secrets::SecretVault>,
    keyring: Arc<aurora_crypto::Keyring>,
    extra_ca: Option<String>,
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
            secrets_target: std::sync::OnceLock::new(),
            hooks: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Register a live reader; it runs after every swap.
    pub fn on_swap(&self, hook: SwapHook) {
        self.hooks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(hook);
    }

    /// Rebuild the live LLM readers whenever a `governance.*`, `byo.*` or `llm.*` key changes
    /// (SET-2, SET-3): the provider table (endpoints, reserved LLM hosts, the secrets' recipes) of
    /// the policy resolver and of the governance step, the instance ceiling that carries the
    /// endpoints and the known LLM hosts (built-in + `known_llm_hosts_extra` − endpoint hosts, from
    /// the merged configuration) and the BYO ceiling. All of it is replaced before the caches are
    /// flushed as `all` (`reload_and_flush`), and nothing is replaced when any part does not
    /// resolve (the previous state stays in force and the failure is logged).
    pub fn rebuild_governance_on_change(
        &self,
        governance: Arc<crate::governance::Governance>,
        resolver: Arc<crate::resolver::DbCredentialResolver>,
        policy: Arc<crate::resolve::Resolver>,
    ) {
        self.on_swap(Arc::new(move |old, new| {
            let llm_changed = section_changed(old, new, &["llm."]);
            if !llm_changed && !section_changed(old, new, &["governance.", "byo."]) {
                return;
            }
            let registry = if llm_changed {
                match aurora_llm::ProviderRegistry::from_config(&new.raw.llm) {
                    Ok(r) => Arc::new(r),
                    Err(e) => {
                        tracing::error!(error = %e, "llm settings did not resolve; keeping the previous endpoint table");
                        return;
                    }
                }
            } else {
                governance.registry()
            };
            match new.raw.governance_config() {
                Ok(g) => {
                    resolver.set_byo(aurora_llm::governance::ByoInstance::from_config(&g.byo));
                    policy.set_llm(registry.clone());
                    let instance = crate::governance::instance_from_config(&g, &registry);
                    governance.set_registry_and_instance(registry, instance);
                }
                Err(e) => {
                    tracing::error!(error = %e, "governance settings did not resolve; keeping the previous instance ceiling");
                }
            }
        }));
    }

    /// (SET-5) The destination guard's list (`gateway.allowed_private` plus the LLM endpoints'
    /// hosts), the condition body cap and the upstream header timeout follow the `limits`
    /// settings live. Applied once now (the stored values loaded before the proxy existed) and
    /// after every swap that changes one of them, for requests begun afterwards.
    pub fn apply_limits_on_change(
        &self,
        guard: Arc<crate::guard::Guard>,
        handler: Arc<crate::decide::PolicyHandler>,
        proxy: Arc<crate::proxy::Proxy>,
    ) {
        self.apply_guard_on_change(guard);
        let apply = move |e: &Effective| {
            handler.set_condition_body_limit(e.raw.gateway.condition_body_bytes as usize);
            proxy.set_header_timeout(std::time::Duration::from_secs(
                e.raw.gateway.upstream_header_timeout_secs,
            ));
        };
        apply(&self.snapshot().effective);
        self.on_swap(Arc::new(move |old, new| {
            if section_changed(old, new, &["gateway."]) {
                apply(new);
            }
        }));
    }

    /// (SET-5, D-262) The destination guard follows `gateway.allowed_private` and the endpoint
    /// table live: the operator's list, and the implicit allowance of the Helm-defined LLM
    /// endpoints ([`helm_endpoint_hosts`]), rebuilt with the endpoint table on every `llm` or
    /// `limits` reload. Applied once now and after each swap that changes either.
    pub fn apply_guard_on_change(&self, guard: Arc<crate::guard::Guard>) {
        let apply = move |e: &Effective| {
            if let Err(err) = guard.set_allowed_private(&e.raw.gateway.allowed_private) {
                tracing::error!(error = %err, "gateway.allowed_private did not resolve; keeping the previous list");
            }
            match helm_endpoint_hosts(e) {
                Ok(hosts) => {
                    if let Err(err) = guard.set_derived_hosts(&hosts) {
                        tracing::error!(error = %err, "LLM endpoint hosts did not resolve for the destination guard");
                    }
                }
                Err(err) => {
                    tracing::error!(error = %err, "llm settings did not resolve; keeping the previous endpoint allowance");
                }
            }
        };
        apply(&self.snapshot().effective);
        self.on_swap(Arc::new(move |old, new| {
            if section_changed(old, new, &["gateway.allowed_private", "llm."]) {
                apply(new);
            }
        }));
    }

    /// (P5-WEBHOOK-2, C-912) The instance ceilings of the outgoing-webhook send limits follow the
    /// `outgoing_webhooks` settings live: a lowered ceiling applies to the next send.
    pub fn apply_webhook_ceiling_on_change(&self, cell: Arc<ArcSwap<(u32, u32)>>) {
        let apply = move |e: &Effective| {
            let w = &e.raw.outgoing_webhooks;
            cell.store(Arc::new((w.per_minute_max, w.per_day_max)));
        };
        apply(&self.snapshot().effective);
        self.on_swap(Arc::new(move |old, new| {
            if section_changed(old, new, &["outgoing_webhooks."]) {
                apply(new);
            }
        }));
    }

    /// (SET-5) The hold timeouts follow the `approvals` settings live.
    pub fn apply_approvals_on_change(&self, holds: Arc<crate::hold::Holds>) {
        self.on_swap(Arc::new(move |old, new| {
            if section_changed(old, new, &["approvals."]) {
                let a = &new.raw.approvals;
                holds.set_timeouts(
                    std::time::Duration::from_secs(a.timeout_secs),
                    std::time::Duration::from_secs(a.max_timeout_secs),
                );
                // (P4-011B, D-265(c)) `approvals.park.*` are read per request from here.
                holds.set_park(crate::park::ParkConfig::from(&a.park));
            }
        }));
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
        let secrets_changed = KEYS_OF_SECRETS.iter().any(|k| {
            before.effective.keys.get(*k).map(|e| &e.value)
                != effective.keys.get(*k).map(|e| &e.value)
        });
        if secrets_changed && let Some(t) = self.secrets_target.get() {
            t.vault
                .rebuild_from_effective(&effective, t.keyring.clone(), t.extra_ca.as_deref());
        }
        let hooks: Vec<SwapHook> = self
            .hooks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for h in &hooks {
            h(&before.effective, &effective);
        }
        self.snap.store(Arc::new(Snapshot {
            generations,
            effective,
        }));
        Ok(changed)
    }

    /// Rebuild `vault`'s store registry whenever the `secrets` settings change (FIX-124).
    pub fn rebuild_vault_on_change(
        &self,
        vault: Arc<aurora_secrets::SecretVault>,
        keyring: Arc<aurora_crypto::Keyring>,
        extra_ca: Option<String>,
    ) {
        let _ = self.secrets_target.set(SecretsTarget {
            vault,
            keyring,
            extra_ca,
        });
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
