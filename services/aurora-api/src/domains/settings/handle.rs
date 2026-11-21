//! The settings snapshot every replica serves from (phase 5, SET-1, D-236): the stored rows,
//! the effective view (`helm > stored > default`) and the merged configuration, swapped
//! atomically on `aurora_settings`, by a 30 s poll and on LISTEN reconnect. A reload that fails
//! validation keeps the previous snapshot. Helm mirroring at start lives here too (D-236(c)).

use arc_swap::ArcSwap;
use aurora_config::{ConfigSource, Effective, ListKind, Source, Stored, registry};
use aurora_core::limits;
use aurora_db::PgPool;
use aurora_db::bus::{BusEvent, Channel, Listener};
use aurora_db::repo::instance_settings as repo;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One section row's bookkeeping.
#[derive(Debug, Clone)]
pub struct RowMeta {
    pub generation: i64,
    pub updated_at: DateTime<Utc>,
    pub updated_by_label: Option<String>,
    /// (C-1102) The writer, null when the user is gone.
    pub updated_by: Option<(uuid::Uuid, String)>,
}

/// A problem found while validating a merged view: a dotted configuration key and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub key: String,
    pub code: String,
    pub message: String,
}

impl Problem {
    pub fn new(key: impl Into<String>, code: &str, message: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// Why a reload did not swap the snapshot.
#[derive(Debug)]
pub enum ReloadError {
    /// The database could not be read: the last snapshot stays in force.
    Database(String),
    /// The stored values do not validate on the merged view: never swapped in.
    Invalid(String),
}

impl std::fmt::Display for ReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(m) | Self::Invalid(m) => f.write_str(m),
        }
    }
}

/// What every replica serves from.
#[derive(Debug)]
pub struct Snapshot {
    pub meta: BTreeMap<String, RowMeta>,
    pub stored: Stored,
    pub effective: Effective,
}

impl Snapshot {
    pub fn generation(&self, section: &str) -> i64 {
        self.meta.get(section).map_or(0, |m| m.generation)
    }
}

/// The process's settings state: the deployment configuration (with presence) and the snapshot.
pub struct SettingsHandle {
    pub source: ConfigSource,
    /// Run `api_config()` on the merged view (false when the state was built from an
    /// already-validated `ApiConfig` without its source: tests and tools).
    validate: bool,
    /// Set once the configuration's locked keys have been mirrored (start, or the first reload
    /// after a database outage at start).
    mirrored: std::sync::atomic::AtomicBool,
    snap: ArcSwap<Snapshot>,
    /// Run after every swap with the old and the new snapshot (live readers: FIX-124's store
    /// registry, and the section briefs' own).
    hooks: std::sync::Mutex<Vec<SwapHook>>,
}

/// A startup-validation error on the merged view as a settings [`Problem`]. A key into a keyed
/// list (`llm.endpoints[1].allowed_paths`) is named by the entry's id and field
/// (`llm.endpoints/<id>/allowed_paths`, C-773), and the stable code of a rule written as
/// `<code> (<field>)` (the shared endpoint rules, C-800) is the problem's code.
fn entry_problem(eff: &Effective, key: String, message: String) -> Problem {
    // The startup rule "at least one provider" is the stable `last_oidc_provider` of a write (D-238(b)).
    let code = if message.starts_with("no_oidc_provider") {
        Some("last_oidc_provider")
    } else {
        None
    };
    let code = code.map(str::to_owned).unwrap_or_else(|| {
        message
            .split_once(" (")
            .filter(|(c, rest)| {
                rest.ends_with(')')
                    && !c.is_empty()
                    && c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
            })
            .map_or("invalid", |(c, _)| c)
            .to_owned()
    });
    let Some((base, rest)) = key.split_once('[') else {
        return Problem::new(key, &code, message);
    };
    let Some((idx, tail)) = rest.split_once(']') else {
        return Problem::new(key, &code, message);
    };
    let id = idx
        .parse::<usize>()
        .ok()
        .and_then(|i| {
            eff.keys
                .get(base)?
                .entries
                .as_ref()?
                .get(i)
                .map(|e| e.id.clone())
        })
        .filter(|id| !id.is_empty());
    let Some(id) = id else {
        return Problem::new(key, &code, message);
    };
    let field = tail
        .trim_start_matches('.')
        .replace(['.', '['], "/")
        .replace(']', "");
    let path = if field.is_empty() {
        format!("{base}/{id}")
    } else {
        format!("{base}/{id}/{field}")
    };
    Problem::new(path, &code, message)
}

/// A live reader notified when the snapshot is swapped.
pub type SwapHook = Arc<dyn Fn(&Snapshot, &Snapshot) + Send + Sync>;

impl std::fmt::Debug for SettingsHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsHandle").finish_non_exhaustive()
    }
}

impl SettingsHandle {
    /// A handle over `source` (`None`: no deployment configuration known, nothing locked).
    pub fn new(source: Option<ConfigSource>) -> Self {
        let validate = source.is_some();
        let source = source.unwrap_or_else(ConfigSource::empty);
        let effective = source.effective(&Stored::default()).unwrap_or_else(|_| {
            // An empty table always deserializes; a configuration that does not is refused at
            // start by `load`, so this is only reachable through a hand-built source.
            Effective {
                raw: aurora_config::RawConfig::default(),
                keys: BTreeMap::new(),
            }
        });
        Self {
            source,
            validate,
            mirrored: std::sync::atomic::AtomicBool::new(false),
            snap: ArcSwap::from_pointee(Snapshot {
                meta: BTreeMap::new(),
                stored: Stored::default(),
                effective,
            }),
            hooks: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Register a live reader; it runs after every swap.
    pub fn on_swap(&self, hook: SwapHook) {
        self.hooks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(hook);
    }

    fn run_hooks(&self, old: &Snapshot, new: &Snapshot) {
        let hooks: Vec<SwapHook> = self
            .hooks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for h in hooks {
            h(old, new);
        }
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snap.load_full()
    }

    /// `helm > stored > default` over `stored`, then the merged-view validation: the same
    /// functions as startup validation plus the cross-section rules (D-236(d)).
    pub fn evaluate(&self, stored: &Stored) -> Result<Effective, Vec<Problem>> {
        // A stored value of the wrong shape or out of its bounds never becomes effective.
        let type_problems: Vec<Problem> = stored
            .type_problems()
            .into_iter()
            .map(|(k, m)| Problem::new(k, "invalid_value", m))
            .collect();
        if !type_problems.is_empty() {
            return Err(type_problems);
        }
        let eff = self.source.effective(stored).map_err(|e| {
            e.0.into_iter()
                .map(|e| Problem::new(e.key, "invalid", e.message))
                .collect::<Vec<_>>()
        })?;
        let mut problems = Vec::new();
        if self.validate
            && let Err(e) = eff.raw.api_config()
        {
            problems.extend(
                e.0.into_iter()
                    .map(|e| entry_problem(&eff, e.key, e.message)),
            );
        }
        problems.extend(super::rules::cross_rules(&eff));
        if problems.is_empty() {
            Ok(eff)
        } else {
            Err(problems)
        }
    }

    /// Re-read every row, validate the merged view and swap. A failure keeps the old snapshot.
    pub async fn reload(&self, pool: &PgPool, service: &'static str) -> Result<(), ReloadError> {
        let rows = repo::list_all_named(pool)
            .await
            .map_err(|e| ReloadError::Database(e.to_string()))?;
        let (stored, meta) = stored_from_rows(&rows);
        match self.evaluate(&stored) {
            Ok(effective) => {
                let new = Arc::new(Snapshot {
                    meta,
                    stored,
                    effective,
                });
                let old = self.snap.swap(new.clone());
                self.run_hooks(&old, &new);
                Ok(())
            }
            Err(problems) => {
                let section = problems
                    .first()
                    .and_then(|p| section_of(&p.key))
                    .unwrap_or("all");
                metrics::counter!(
                    "aurora_settings_reload_failed_total",
                    "service" => service,
                    "section" => section.to_string()
                )
                .increment(1);
                Err(ReloadError::Invalid(describe(&problems)))
            }
        }
    }

    /// Reload when the row generations differ from the snapshot's (the poll's cheap check).
    pub async fn reload_if_changed(
        &self,
        pool: &PgPool,
        service: &'static str,
    ) -> Result<bool, ReloadError> {
        let gens = repo::generations(pool)
            .await
            .map_err(|e| ReloadError::Database(e.to_string()))?;
        let snap = self.snapshot();
        let same = gens.len() == snap.meta.len()
            && gens
                .iter()
                .all(|(s, g)| snap.meta.get(s).is_some_and(|m| m.generation == *g));
        if same {
            return Ok(false);
        }
        self.reload(pool, service).await.map(|()| true)
    }

    /// Mirror the configuration's locked keys once (D-236(c)); a no-op after the first success.
    pub async fn ensure_mirrored(&self, pool: &PgPool) {
        use std::sync::atomic::Ordering;
        if self.mirrored.load(Ordering::Acquire) {
            return;
        }
        match mirror(self, pool).await {
            Ok(changed) => {
                self.mirrored.store(true, Ordering::Release);
                for (section, keys) in changed {
                    tracing::info!(section, keys = ?keys, "settings mirrored from the configuration");
                    record_helm_sync(pool, &section, &keys).await;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "settings: mirroring the configuration failed; will retry")
            }
        }
    }

    /// The reload task of an API replica: the LISTEN consumer, the poll and the reload on every
    /// reconnect (D-236(f)).
    pub fn spawn_reload(self: &Arc<Self>, pool: PgPool, service: &'static str) {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                let mut listener = match Listener::connect(&pool, &[Channel::Settings]).await {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::warn!(error = %e, "settings listener connect failed; retrying");
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        continue;
                    }
                };
                let mut tick = tokio::time::interval(limits::SETTINGS_RELOAD_POLL);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        ev = listener.next() => match ev {
                            BusEvent::Resync => {
                                me.ensure_mirrored(&pool).await;
                                if let Err(e) = me.reload(&pool, service).await {
                                    tracing::error!(error = %e, "settings reload failed; keeping the previous snapshot");
                                }
                            }
                            BusEvent::Notification { payload, .. } => {
                                let section = payload.get("section").and_then(Value::as_str).unwrap_or("");
                                let generation = payload.get("generation").and_then(Value::as_i64).unwrap_or(i64::MAX);
                                if me.snapshot().generation(section) < generation
                                    && let Err(e) = me.reload(&pool, service).await
                                {
                                    tracing::error!(error = %e, section, "settings reload failed; keeping the previous snapshot");
                                }
                            }
                        },
                        _ = tick.tick() => {
                            if let Err(e) = me.reload_if_changed(&pool, service).await {
                                tracing::error!(error = %e, "settings poll failed; keeping the previous snapshot");
                            }
                        }
                    }
                }
            }
        });
    }
}

/// The section a dotted key belongs to (registered keys and their children).
pub fn section_of(key: &str) -> Option<&'static str> {
    registry()
        .defs()
        .iter()
        .find(|d| {
            key == d.key
                || key
                    .strip_prefix(d.key)
                    .is_some_and(|r| r.starts_with('.') || r.starts_with('['))
        })
        .map(|d| d.section)
}

/// Stored values by section from the rows. Unknown sections and keys (written by a newer
/// release, or hand-edited: SEC-89) are logged and ignored (D-238(c)).
pub fn stored_from_rows(rows: &[repo::SettingsRow]) -> (Stored, BTreeMap<String, RowMeta>) {
    let meta = rows
        .iter()
        .map(|row| {
            (
                row.section.clone(),
                RowMeta {
                    generation: row.generation,
                    updated_at: row.updated_at,
                    updated_by_label: row.updated_by_label.clone(),
                    updated_by: row.updated_by.zip(row.updated_by_name.clone()),
                },
            )
        })
        .collect();
    let (stored, ignored) =
        Stored::from_rows(rows.iter().map(|r| (r.section.as_str(), &r.value)), None);
    for what in ignored {
        tracing::warn!(%what, "stored instance setting ignored (unknown to this release)");
    }
    (stored, meta)
}

/// A human-readable list of problems naming the section, the key and the reset command.
pub fn describe(problems: &[Problem]) -> String {
    problems
        .iter()
        .map(|p| match section_of(&p.key) {
            Some(s) => format!(
                "{}: {} (section `{s}`; reset it with `aurora-api settings reset {s}`)",
                p.key, p.message
            ),
            None => format!("{}: {}", p.key, p.message),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Mirror the configuration's locked keys into the stored rows (D-236(c)) under an advisory
/// lock, so removing a key from the values file hands it to the dashboard with the value Helm had.
/// Returns the sections that changed (their generation was bumped and notified).
pub async fn mirror(
    handle: &SettingsHandle,
    pool: &PgPool,
) -> Result<Vec<(String, Vec<String>)>, String> {
    let reg = registry();
    let mut changed: Vec<(String, i64, Vec<String>)> = Vec::new();
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    repo::lock_mirroring(&mut tx)
        .await
        .map_err(|e| e.to_string())?;
    let mut sections: Vec<&str> = reg.defs().iter().map(|d| d.section).collect();
    sections.sort_unstable();
    sections.dedup();
    for section in sections {
        let defs: Vec<_> = reg
            .section(section)
            .filter(|d| !d.secret && handle.source.sets(d.key))
            .collect();
        if defs.is_empty() {
            continue;
        }
        let row = repo::get_for_update(&mut tx, section)
            .await
            .map_err(|e| e.to_string())?;
        let generation = row.as_ref().map_or(0, |r| r.generation);
        let mut value: Map<String, Value> = match row.map(|r| r.value) {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
        let mut keys = Vec::new();
        for d in defs {
            let Some(helm) = handle.source.helm_value(d.key) else {
                continue;
            };
            match d.list {
                Some(ListKind::Keyed { id_field }) => {
                    let mut list: Vec<Value> = value
                        .get(d.key)
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let mut touched = false;
                    for e in &aurora_config::list_entries(d, helm) {
                        let id = e.get(id_field).and_then(Value::as_str).unwrap_or_default();
                        match list
                            .iter_mut()
                            .find(|x| x.get(id_field).and_then(Value::as_str) == Some(id))
                        {
                            Some(slot) if slot == e => {}
                            Some(slot) => {
                                *slot = e.clone();
                                touched = true;
                            }
                            None => {
                                list.push(e.clone());
                                touched = true;
                            }
                        }
                    }
                    if touched {
                        value.insert(d.key.to_string(), Value::Array(list));
                        keys.push(d.key.to_string());
                    }
                }
                _ => {
                    if value.get(d.key) != Some(helm) {
                        value.insert(d.key.to_string(), helm.clone());
                        keys.push(d.key.to_string());
                    }
                }
            }
        }
        if keys.is_empty() {
            continue;
        }
        let new_gen = repo::cas_write(
            &mut tx,
            section,
            generation,
            &Value::Object(value),
            None,
            "system",
        )
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("section `{section}` changed during mirroring; restart to retry"))?;
        changed.push((section.to_string(), new_gen, keys));
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    for (section, generation, _) in &changed {
        if let Err(e) = repo::notify(pool, section, *generation).await {
            tracing::warn!(error = %e, section, "settings notify after mirroring failed");
        }
    }
    Ok(changed.into_iter().map(|(s, _, k)| (s, k)).collect())
}

/// The best-effort system audit row of a mirroring (`instance_settings.helm_sync`).
pub async fn record_helm_sync(pool: &PgPool, section: &str, keys: &[String]) {
    use aurora_db::repo::audit::{NewAudit, insert};
    let metadata = json!({ "section": section, "keys": keys });
    let r = insert(
        pool,
        &NewAudit {
            org_id: None,
            workspace_id: None,
            actor_kind: "system",
            actor_id: None,
            actor_label: "settings",
            action: "instance_settings.helm_sync",
            target_kind: Some("instance_settings"),
            target_id: Some(section),
            outcome: "success",
            source: "system",
            ip: None,
            metadata: &metadata,
        },
    )
    .await;
    if let Err(e) = r {
        tracing::warn!(error = %e, "helm_sync audit row not written");
    }
}

/// Start-up sequence of the API: mirror, load the snapshot (invalid → the message to refuse
/// with, D-238(c)), and report the locked keys.
pub async fn start(handle: &Arc<SettingsHandle>, pool: &PgPool) -> Result<(), String> {
    // The process starts without a database (readiness reports it): the reload task mirrors and
    // loads as soon as it is reachable. Defaults and the configuration serve until then.
    handle.ensure_mirrored(pool).await;
    match handle.reload(pool, "api").await {
        Ok(()) => Ok(()),
        Err(ReloadError::Database(e)) => {
            tracing::warn!(error = %e, "settings: stored settings not loaded yet (database unavailable)");
            Ok(())
        }
        Err(ReloadError::Invalid(e)) => Err(format!("instance settings are invalid: {e}")),
    }
}

/// Which keys are locked right now (the configuration sets them): `(section, key, entry id?)`.
pub fn locked_keys(
    handle: &SettingsHandle,
) -> Vec<(&'static str, &'static str, Option<String>, String)> {
    let mut out = Vec::new();
    let snap = handle.snapshot();
    for d in registry().defs() {
        let Some(k) = snap.effective.keys.get(d.key) else {
            continue;
        };
        match &k.entries {
            Some(entries) => {
                for e in entries.iter().filter(|e| e.source == Source::Helm) {
                    out.push((d.section, d.key, Some(e.id.clone()), d.helm_key.clone()));
                }
            }
            None => {
                if k.source == Source::Helm {
                    out.push((d.section, d.key, None, d.helm_key.clone()));
                }
            }
        }
    }
    out
}
