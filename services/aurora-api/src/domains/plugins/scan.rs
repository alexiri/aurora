//! Plugin directory scan (05 §Background jobs): `plugins.dir` at startup and every 60 s,
//! singleton across replicas, idempotent by package digest.

use super::ingest::{self, Source};
use crate::app::AppState;
use crate::audit::AuditActor;
use aurora_core::limits::PACKAGE_MAX;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

pub const SCAN_EVERY: Duration = Duration::from_secs(60);
pub const EXTENSION: &str = "aurora-plugin";
const JOB: &str = "plugin_dir_scan";

/// `(modified, len)` per file already processed, so unchanged files are skipped.
pub type Seen = HashMap<PathBuf, (SystemTime, u64)>;

/// Scan `plugins.dir` once; returns the number of newly created package rows.
pub async fn scan_once(state: &AppState, seen: &mut Seen) -> usize {
    scan_dir(
        state,
        &PathBuf::from(&state.config.plugins.dir),
        Source::Directory,
        seen,
    )
    .await
}

/// Scan `plugins.builtin_dir` once (09 §10 `builtin`). Fail closed: only archives that verify
/// against the compiled-in project key are loaded; anything else is refused and logged.
pub async fn scan_builtin_once(state: &AppState, seen: &mut Seen) -> usize {
    scan_dir(
        state,
        &PathBuf::from(&state.config.plugins.builtin_dir),
        Source::Builtin,
        seen,
    )
    .await
}

async fn scan_dir(state: &AppState, dir: &PathBuf, source: Source, seen: &mut Seen) -> usize {
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else {
        return 0;
    };
    let mut files = Vec::new();
    while let Ok(Some(e)) = rd.next_entry().await {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some(EXTENSION) {
            continue;
        }
        let Ok(ft) = e.file_type().await else {
            continue;
        };
        if !ft.is_file() {
            continue;
        }
        files.push(path);
    }
    files.sort();
    let actor = AuditActor::system(match source {
        Source::Builtin => "plugin-builtin",
        _ => "plugin-directory",
    });
    let mut created = 0;
    for path in files {
        let Ok(meta) = tokio::fs::metadata(&path).await else {
            continue;
        };
        let stamp = (
            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            meta.len(),
        );
        if seen.get(&path) == Some(&stamp) {
            continue;
        }
        seen.insert(path.clone(), stamp);
        if usize::try_from(meta.len()).map_or(true, |n| n > PACKAGE_MAX) {
            tracing::warn!(file = %path.display(), "plugin file too large; skipped");
            continue;
        }
        let Ok(bytes) = tokio::fs::read(&path).await else {
            seen.remove(&path);
            continue;
        };
        match ingest::ingest(state, &bytes, source, &actor).await {
            Ok(r) if r.created => {
                created += 1;
                tracing::info!(file = %path.display(), status = %r.row.status, "plugin ingested");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    file = %path.display(), source = source.as_str(), code = e.code(),
                    "plugin file refused"
                );
            }
        }
    }
    created
}

/// Singleton variant: `None` when another replica holds the lock.
pub async fn scan_locked(state: &AppState, seen: &mut Seen) -> Option<usize> {
    // Built-ins first so the catalogue is in place before operator directories are looked at.
    let job = move || async move {
        let b = scan_builtin_once(state, seen).await;
        b + scan_once(state, seen).await
    };
    match aurora_db::jobs::try_singleton(&state.pool, JOB, job).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "plugin scan failed");
            None
        }
    }
}

/// Start the scan loop (immediately, then every [`SCAN_EVERY`]).
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut seen = Seen::new();
        let mut tick = tokio::time::interval(SCAN_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            scan_locked(&state, &mut seen).await;
        }
    });
}
