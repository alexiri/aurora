//! The refresh job (phase 4, P4-GSL, D-149, SEC-83; docs/05 §Background jobs): re-reads each
//! library that has at least one added skill under a per-library lease and writes **proposed
//! updates only**. It never writes `skills` or `skill_files`, never bumps a home-sync generation,
//! and never applies anything: applying is a person's act (`skills::library`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use aurora_core::limits;
use aurora_core::sanitize::one_line;
use aurora_db::repo::skill_libraries::{self as repo, LibraryRow, NewUpdate, RefreshDone};
use aurora_db::repo::skills as skills_repo;
use base64::Engine as _;
use serde_json::json;
use uuid::Uuid;

use super::content::{self, Candidate, LoadError, Reader};
use super::service::block_for;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};

const TICK: Duration = Duration::from_secs(60);
const RETENTION_EVERY: Duration = Duration::from_secs(24 * 3600);
const RETENTION_JOB: &str = "skill_library_retention";

struct Done {
    head: String,
    over_budget: bool,
    proposed: usize,
}

/// Starts the refresh and retention loops of this replica.
pub fn spawn(state: AppState) {
    let s = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = run_once(&s).await {
                tracing::warn!(error = %e.message(), "skill library refresh failed");
            }
        }
    });
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(RETENTION_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = retention(&state).await {
                tracing::warn!(error = %e.message(), "skill library retention failed");
            }
        }
    });
}

/// Daily retention (singleton): prune decided proposals older than 90 days, expire leases, mark
/// libraries whose connection vanished `error`.
pub async fn retention(state: &AppState) -> Result<Option<(u64, u64)>, aurora_core::ApiError> {
    let ran = aurora_db::jobs::try_singleton(&state.pool, RETENTION_JOB, || async {
        let mut c = state.pool.acquire().await?;
        repo::retention(&mut c).await
    })
    .await?;
    ran.transpose().map_err(Into::into)
}

/// One pass: claim due libraries (lease, `SKIP LOCKED`, at most `LIBRARY_REFRESH_CONCURRENCY`) and
/// refresh them. Returns how many were refreshed. Safe on every replica at once.
pub async fn run_once(state: &AppState) -> Result<usize, aurora_core::ApiError> {
    if !state.config.skills_library.enabled {
        return Ok(0);
    }
    let owner = state.library.replica;
    let mut c = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    let claimed = repo::claim_due(
        &mut c,
        owner,
        limits::LIBRARY_REFRESH_CONCURRENCY as i64,
        limits::LIBRARY_LEASE.as_secs_f64(),
    )
    .await?;
    if let Ok(n) = repo::pending_total(&mut c).await {
        metrics::gauge!("aurora_skill_library_pending_updates").set(n as f64);
    }
    drop(c);
    let n = claimed.len();
    futures::future::join_all(
        claimed
            .into_iter()
            .map(|l| refresh_library(state, l, owner)),
    )
    .await;
    Ok(n)
}

/// Seconds until the next refresh after `failures` consecutive failures: the interval, then 1 h,
/// then 6 h; ±10 % jitter.
#[must_use]
pub fn next_in_secs(interval: u64, failures: i32) -> f64 {
    let base = match failures {
        0 | 1 => interval as f64,
        2 => (interval as f64).max(3600.0),
        _ => (interval as f64).max(21_600.0),
    };
    let jitter = (Uuid::now_v7().as_u128() % 2001) as f64 / 10_000.0 - 0.1;
    base * (1.0 + jitter)
}

/// Refreshes one claimed library and records the outcome (only the lease owner may).
pub async fn refresh_library(state: &AppState, lib: LibraryRow, owner: Uuid) {
    refresh_library_within(state, lib, owner, limits::LIBRARY_REFRESH_BUDGET).await;
}

/// [`refresh_library`] with an explicit overall bound. A read of a library is many gateway
/// calls (tree pages, up to 50 blobs per skill, each with its own timeout and busy retries);
/// the whole pass must end — and record its outcome, releasing the lease — before
/// `LIBRARY_LEASE` runs out, or another replica claims the library and repeats the work. On the
/// bound the pass is recorded as a failure (backoff) and the next run starts from scratch.
pub async fn refresh_library_within(
    state: &AppState,
    lib: LibraryRow,
    owner: Uuid,
    budget: Duration,
) {
    let started = Instant::now();
    let interval = state
        .config
        .skills_library
        .refresh_interval_secs
        .max(limits::LIBRARY_REFRESH_INTERVAL_MIN.as_secs());
    let result = match tokio::time::timeout(budget, compare(state, &lib, owner)).await {
        Ok(r) => r,
        Err(_) => Err("the refresh took too long and was stopped; it will retry".to_owned()),
    };
    metrics::histogram!("aurora_skill_library_refresh_duration_seconds")
        .record(started.elapsed().as_secs_f64());
    let (status, error, failures, head, label) = match &result {
        Ok(d) => (
            if d.over_budget {
                "over_budget"
            } else {
                "active"
            },
            None,
            0,
            Some(d.head.as_str()),
            if d.proposed > 0 {
                "proposed"
            } else {
                "unchanged"
            },
        ),
        Err(reason) => (
            "error",
            Some(one_line(reason, 200)),
            lib.refresh_failures + 1,
            None,
            "error",
        ),
    };
    metrics::counter!("aurora_skill_library_refresh_total", "result" => label).increment(1);
    let next = next_in_secs(interval, failures);
    let done = RefreshDone {
        id: lib.id,
        owner,
        head_sha: head,
        status,
        error: error.as_deref(),
        failures,
        next_in_secs: next,
    };
    match state.pool.acquire().await {
        Ok(mut c) => {
            if let Err(e) = repo::finish_refresh(&mut c, &done).await {
                tracing::warn!(library = %lib.id, error = %e, "recording a library refresh failed");
            }
        }
        Err(e) => {
            tracing::warn!(library = %lib.id, error = %e, "recording a library refresh failed")
        }
    }
}

fn blobs_of(v: &Option<serde_json::Value>) -> BTreeMap<String, String> {
    v.as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

async fn compare(state: &AppState, lib: &LibraryRow, _owner: Uuid) -> Result<Done, String> {
    fn db<E>(_: E) -> String {
        "the database is unavailable".to_owned()
    }
    let conn = lib.connection_id.ok_or("connection_deleted")?;
    let mut c = state.pool.acquire().await.map_err(db)?;
    let brief = repo::connection_brief(&mut c, conn)
        .await
        .map_err(db)?
        .ok_or("connection_deleted")?;
    if brief.workspace_id.is_some() {
        return Err("the connection is not organization-scoped".into());
    }
    match brief.status.as_str() {
        "active" => {}
        "reauth_required" => {
            return Err("reauth_required: the connection needs to be reconnected".into());
        }
        _ => return Err("the connection is not active".into()),
    }
    let block = block_for(state, &lib.app_id).await?;
    let reader = Reader {
        gw: state.library.gateway.as_ref(),
        lib: &block,
        org: lib.org_id,
        conn,
        repo: &lib.repo,
    };
    let head = reader
        .resolve(lib.reference.as_deref())
        .await
        .map_err(|e| e.reason())?;
    // An unchanged head costs this one call (a library over budget re-checks: space may be free).
    if lib.last_head_sha.as_deref() == Some(head.as_str()) && lib.status != "over_budget" {
        return Ok(Done {
            head,
            over_budget: false,
            proposed: 0,
        });
    }
    let skills = repo::library_skills(&mut c, lib.id).await.map_err(db)?;
    let entries = reader.tree(&head).await.map_err(|e| e.reason())?;
    let cands = content::assemble(&block, &lib.path, &entries);
    let by_dir: BTreeMap<&str, &Candidate> = cands.iter().map(|c| (c.dir.as_str(), c)).collect();
    let mut pending_bytes = repo::pending_bytes_org(&mut c, lib.org_id)
        .await
        .map_err(db)?;
    let mut done = Done {
        head: head.clone(),
        over_budget: false,
        proposed: 0,
    };
    for s in &skills {
        let dir = s.library_path.clone().unwrap_or_default();
        let pinned = blobs_of(&s.library_blobs);
        let pending = repo::pending_update(&mut c, s.id).await.map_err(db)?;
        // What the repository says now about this skill.
        let (kind, reason, proposed, hash, bytes): (
            &str,
            Option<String>,
            Option<serde_json::Value>,
            Vec<u8>,
            i32,
        ) = match by_dir.get(dir.as_str()) {
            None => (
                "removed",
                None,
                None,
                content::marker_sha256("removed", "", &BTreeMap::new()).to_vec(),
                0,
            ),
            Some(cand) => {
                let files = skills_repo::files(&mut c, &[s.id]).await.map_err(db)?;
                let exec_same = cand.files.iter().all(|f| {
                    files
                        .iter()
                        .find(|x| x.path == f.rel)
                        .is_none_or(|x| x.executable == f.executable)
                });
                if cand.problems.is_empty() && cand.blobs() == pinned && exec_same {
                    if let Some(p) = &pending {
                        let _ = p;
                        repo::supersede_pending(&mut c, s.id).await.map_err(db)?;
                    }
                    continue;
                }
                match content::load(&reader, cand).await {
                    Err(LoadError::Read(e)) => return Err(e.reason()),
                    Err(LoadError::Invalid(r)) => {
                        let r = one_line(&r, 300);
                        let h = content::marker_sha256("invalid", &r, &cand.blobs());
                        ("invalid", Some(r), None, h.to_vec(), 0)
                    }
                    Ok(l) => {
                        if Some(l.content_sha256.as_slice()) == s.library_content_sha256.as_deref()
                        {
                            if pending.is_some() {
                                repo::supersede_pending(&mut c, s.id).await.map_err(db)?;
                            }
                            continue;
                        }
                        let proposed = json!({
                            "body": l.body, "description": l.description,
                            "files": l.files.iter().map(|f| json!({
                                "path": f.path,
                                "content_b64": base64::engine::general_purpose::STANDARD.encode(&f.content),
                                "executable": f.executable,
                            })).collect::<Vec<_>>(),
                            "blobs": l.blobs,
                        });
                        let bytes = i32::try_from(proposed.to_string().len()).unwrap_or(i32::MAX);
                        (
                            "changed",
                            None,
                            Some(proposed),
                            l.content_sha256.to_vec(),
                            bytes,
                        )
                    }
                }
            }
        };
        if pending
            .as_ref()
            .is_some_and(|p| p.content_sha256.as_deref() == Some(hash.as_slice()))
        {
            continue;
        }
        if repo::was_rejected(&mut c, s.id, &hash).await.map_err(db)? {
            continue;
        }
        let replaced = pending.as_ref().map_or(0, |p| i64::from(p.bytes));
        if pending_bytes - replaced + i64::from(bytes)
            > limits::LIBRARY_PENDING_BYTES_PER_ORG as i64
        {
            done.over_budget = true;
            break;
        }
        let u = repo::insert_update(
            &mut c,
            &NewUpdate {
                skill_id: s.id,
                library_id: lib.id,
                kind,
                commit_sha: &head,
                proposed: proposed.as_ref(),
                content_sha256: &hash,
                reason: reason.as_deref(),
                bytes,
            },
        )
        .await;
        let u = match u {
            Ok(u) => u,
            // The partial unique index on the skill's pending proposal: a concurrent refresh
            // (a lease that expired under a slow pass) proposed it first — nothing to add.
            Err(aurora_db::DbError::UniqueViolation { .. }) => continue,
            Err(e) => return Err(db(e)),
        };
        pending_bytes += i64::from(bytes) - replaced;
        done.proposed += 1;
        audit::record(
            state,
            AuditEvent::new(
                AuditActor::system("skill-library-refresh"),
                "skill.library.update_proposed",
                Outcome::Success,
            )
            .org(lib.org_id)
            .target("skill", s.id)
            .metadata(
                json!({"update_id": u.id, "kind": kind, "library_id": lib.id,
                "commit_sha": head, "name": s.name}),
            ),
        );
    }
    Ok(done)
}
