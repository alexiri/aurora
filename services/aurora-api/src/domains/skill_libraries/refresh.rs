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

use super::content::{self, Candidate, LoadError, ReadError, Reader};
use super::dto::SkillInvalidReason;
use super::git::GitRepo;
use super::service::{Offered, catalogue, default_branch_of, open_token, row_url};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};

const TICK: Duration = Duration::from_secs(60);
const RETENTION_EVERY: Duration = Duration::from_secs(24 * 3600);
const RETENTION_JOB: &str = "skill_library_retention";

struct Done {
    head: String,
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

/// Daily retention (singleton): prune decided proposals older than 90 days, expire leases.
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
    if !state.ops().skills_library.enabled {
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
    if n == 0 {
        // (FIX-146, D-232 (6)) An iteration that finds nothing opens no span.
        return Ok(0);
    }
    tracing::Instrument::instrument(
        futures::future::join_all(
            claimed
                .into_iter()
                .map(|l| refresh_library(state, l, owner)),
        ),
        aurora_core::telemetry::job_span("skill library refresh"),
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

/// [`refresh_library`] with an explicit overall bound. A read of a library is several git
/// requests (discovery, refs, the tree pack, the blob pack, each with its own timeout);
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
        .ops()
        .skills_library
        .refresh_interval_secs
        .max(limits::LIBRARY_REFRESH_INTERVAL_MIN.as_secs());
    // (SEC-148) The replica's shared read permits: a refresh waits for one (it never fails for
    // want of one), and the budget starts once it has it.
    let _permit = state.library.reads.background().await;
    let result = match tokio::time::timeout(budget, compare(state, &lib, owner)).await {
        Ok(r) => r,
        Err(_) => Err("the refresh took too long and was stopped; it will retry".into()),
    };
    metrics::histogram!("aurora_skill_library_refresh_duration_seconds")
        .record(started.elapsed().as_secs_f64());
    let (status, error, failures, head, label, next) = match &result {
        // (D-449) The budget is per organization (`skill_library_org_state`): the library's own
        // status is `active` after a pass that read the repository, whoever was over budget.
        Ok(d) => (
            "active",
            None,
            0,
            Some(d.head.as_str()),
            if d.proposed > 0 {
                "proposed"
            } else {
                "unchanged"
            },
            next_in_secs(interval, 0),
        ),
        // The source changed under the pass: nothing it found is recorded (`finish_refresh`
        // only releases the lease, and the library stays due).
        Err(Fail {
            superseded: true, ..
        }) => ("active", None, 0, None, "superseded", 0.0),
        // (FIX-096) The git host throttled us: not a failure of the library. Keep its state and
        // failure count and come back after the host's (capped) `Retry-After`.
        Err(Fail {
            retry_after: Some(secs),
            ..
        }) => (
            if lib.status == "error" {
                "error"
            } else {
                "active"
            },
            lib.last_error.clone(),
            lib.refresh_failures,
            None,
            "throttled",
            *secs as f64,
        ),
        Err(Fail { reason, .. }) => (
            "error",
            Some(one_line(reason, 200)),
            lib.refresh_failures + 1,
            None,
            "error",
            next_in_secs(interval, lib.refresh_failures + 1),
        ),
    };
    metrics::counter!("aurora_skill_library_refresh_total", "result" => label).increment(1);
    let done = RefreshDone {
        id: lib.id,
        owner,
        head_sha: head,
        head_epoch: lib.head_epoch,
        source_epoch: lib.source_epoch,
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

/// Why a pass did not finish; with `retry_after` the git host throttled it (not a failure), with
/// `superseded` the library's source changed under it (not a failure either).
struct Fail {
    reason: String,
    retry_after: Option<u64>,
    superseded: bool,
}

impl From<String> for Fail {
    fn from(reason: String) -> Self {
        Self {
            reason,
            retry_after: None,
            superseded: false,
        }
    }
}

impl From<&str> for Fail {
    fn from(reason: &str) -> Self {
        reason.to_owned().into()
    }
}

impl From<ReadError> for Fail {
    fn from(e: ReadError) -> Self {
        Self {
            reason: e.reason(),
            retry_after: match e {
                ReadError::Throttled(s) => Some(s),
                _ => None,
            },
            superseded: false,
        }
    }
}

fn db<E>(_: E) -> Fail {
    "the database is unavailable".into()
}

/// A transaction in which the library still reads the source this pass read: its row is locked
/// for share, so a change of source waits for the pass's write and then drops it. After a change
/// the pass writes nothing more.
async fn while_current<'a>(
    state: &'a AppState,
    lib: &LibraryRow,
) -> Result<sqlx::Transaction<'a, sqlx::Postgres>, Fail> {
    let mut tx = state.pool.begin().await.map_err(db)?;
    ensure_current(&mut tx, lib).await?;
    Ok(tx)
}

async fn ensure_current(conn: &mut sqlx::PgConnection, lib: &LibraryRow) -> Result<(), Fail> {
    if repo::lock_source(conn, lib.id).await.map_err(db)? == Some(lib.source_epoch) {
        return Ok(());
    }
    Err(Fail {
        superseded: true,
        ..Fail::from("the library's source changed during the refresh")
    })
}

/// The skill is back to what is pinned: its open proposal no longer applies.
async fn supersede(state: &AppState, lib: &LibraryRow, skill: Uuid) -> Result<(), Fail> {
    let mut tx = while_current(state, lib).await?;
    repo::supersede_pending(&mut tx, skill).await.map_err(db)?;
    tx.commit().await.map_err(db)
}

/// What loading one skill directory gave in this pass (kept for the other organizations' skills
/// of the same directory).
enum Loaded {
    Ok(content::LoadedSkill),
    Invalid(content::Invalid),
}

/// What a pass found about one skill: `(kind, why it is invalid, proposed content, hash, bytes)`.
type Finding = (
    &'static str,
    Option<(SkillInvalidReason, String)>,
    Option<serde_json::Value>,
    Vec<u8>,
    i32,
);

async fn compare(state: &AppState, lib: &LibraryRow, _owner: Uuid) -> Result<Done, Fail> {
    // (P5-SKLIB-1, C-1125) The API reads the repository itself, with the library's token when
    // it has one (opened for this pass only).
    let url = row_url(lib)?;
    let token = open_token(state, lib).await?;
    let git = GitRepo::new(
        &state.connect,
        url,
        token.as_ref(),
        state.library.fetch_timeout,
    )?;
    let reader = Reader::new(&git, &lib.path);
    let resolved = reader.resolve_full(lib.reference.as_deref()).await?;
    let default_branch = default_branch_of(&resolved, lib.reference.as_deref());
    let head = resolved.sha;
    let mut tx = while_current(state, lib).await?;
    repo::record_head(&mut tx, lib.id, &head)
        .await
        .map_err(db)?;
    // (The branch may be renamed while its head stays.)
    if default_branch != lib.default_branch {
        repo::set_default_branch(&mut tx, lib.id, default_branch.as_deref())
            .await
            .map_err(db)?;
    }
    tx.commit().await.map_err(db)?;
    let mut c = state.pool.acquire().await.map_err(db)?;
    // An unchanged head costs this one call, unless an organization is over budget for the library
    // (space may be free: its skipped proposals are compared again).
    // (C-1176) A library whose counts are not stored yet is read once to store them.
    if lib.last_head_sha.as_deref() == Some(head.as_str())
        && lib.skills_offered.is_some()
        && !repo::has_org_state(&mut c, lib.id).await.map_err(db)?
    {
        return Ok(Done { head, proposed: 0 });
    }
    // Every organization's skills: an instance library is added in many (D-449).
    let skills = repo::library_skills(&mut c, lib.id, None)
        .await
        .map_err(db)?;
    let entries = reader.tree(&head).await?;
    let cands = content::assemble(&lib.path, &entries);
    let by_dir: BTreeMap<&str, &Candidate> = cands.iter().map(|c| (c.dir.as_str(), c)).collect();
    // One blob request for every skill whose files changed (C-1125).
    let mut wanted: Vec<String> = Vec::new();
    for s in &skills {
        let dir = s.library_path.clone().unwrap_or_default();
        if let Some(cand) = by_dir.get(dir.as_str())
            && cand.problems.is_empty()
            && cand.blobs() != blobs_of(&s.library_blobs)
        {
            wanted.extend(cand.blob_ids());
        }
    }
    reader.prefetch(&wanted).await?;
    // (C-1176) What the library offers at this head. A failure keeps the previous counts: it
    // never fails the pass that watches the added skills.
    if let Ok(cat) = catalogue(&reader, &lib.path, head.clone()).await {
        let mut tx = while_current(state, lib).await?;
        Offered::of(&cat.items)
            .store(&mut tx, lib.id)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
    }
    let mut done = Done {
        head: head.clone(),
        proposed: 0,
    };
    // Organizations whose open-proposal budget did not fit a proposal in this pass.
    let mut over: std::collections::BTreeSet<Uuid> = std::collections::BTreeSet::new();
    let mut loaded: BTreeMap<String, Loaded> = BTreeMap::new();
    for s in &skills {
        // (SEC-148) An organization that is over budget gets no more proposals in this pass, so
        // none of its remaining skills is worth loading.
        if over.contains(&s.org_id) {
            continue;
        }
        let dir = s.library_path.clone().unwrap_or_default();
        let pinned = blobs_of(&s.library_blobs);
        let pending = repo::pending_update(&mut c, s.id).await.map_err(db)?;
        // What the repository says now about this skill.
        let (kind, reason, proposed, hash, bytes): Finding = match by_dir.get(dir.as_str()) {
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
                    if pending.is_some() {
                        supersede(state, lib, s.id).await?;
                    }
                    continue;
                }
                // (SEC-148) Each changed directory is loaded once per pass and reused by the
                // skills of every organization that added it.
                if !loaded.contains_key(dir.as_str()) {
                    state
                        .library
                        .loads
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let v = match content::load(&reader, cand).await {
                        Err(LoadError::Read(e)) => return Err(e.into()),
                        Err(LoadError::Invalid(r)) => Loaded::Invalid(r),
                        Ok(l) => Loaded::Ok(l),
                    };
                    loaded.insert(dir.clone(), v);
                }
                match &loaded[dir.as_str()] {
                    Loaded::Invalid(i) => {
                        let r = one_line(&i.reason, 300);
                        let h = content::marker_sha256("invalid", &r, &cand.blobs());
                        ("invalid", Some((i.code, r)), None, h.to_vec(), 0)
                    }
                    Loaded::Ok(l) => {
                        if Some(l.content_sha256.as_slice()) == s.library_content_sha256.as_deref()
                        {
                            if pending.is_some() {
                                supersede(state, lib, s.id).await?;
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
        // (FIX-096, D-449) Sum and insert under the lock of the **skill's** organization (an
        // instance library's skills are in many): two libraries refreshing at once must not both
        // pass one organization's cap, and one organization's budget never stops another's.
        let mut tx = state.pool.begin().await.map_err(db)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("skill_library_pending:{}", s.org_id))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        // (B-1) Lock order: the organization's proposal lock, then the library's row (for
        // share), then proposal rows. A change of source holds the library's row, so either
        // it waits and drops this proposal, or this pass sees it and writes nothing.
        ensure_current(&mut tx, lib).await?;
        let pending_bytes = repo::pending_bytes_org(&mut tx, s.org_id)
            .await
            .map_err(db)?;
        if pending_bytes - replaced + i64::from(bytes)
            > limits::LIBRARY_PENDING_BYTES_PER_ORG as i64
        {
            // No proposal for this organization's skill; its state row says so (below) and the
            // library's other organizations get theirs.
            over.insert(s.org_id);
            continue;
        }
        let u = repo::insert_update(
            &mut tx,
            &NewUpdate {
                skill_id: s.id,
                library_id: lib.id,
                kind,
                commit_sha: &head,
                proposed: proposed.as_ref(),
                content_sha256: &hash,
                reason: reason.as_ref().map(|(_, r)| r.as_str()),
                reason_code: reason.as_ref().map(|(c, _)| c.as_str()),
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
        tx.commit().await.map_err(db)?;
        done.proposed += 1;
        audit::record(
            state,
            AuditEvent::new(
                AuditActor::system("skill-library-refresh"),
                "skill.library.update_proposed",
                Outcome::Success,
            )
            .org(s.org_id)
            .target("skill", s.id)
            .metadata(
                json!({"update_id": u.id, "kind": kind, "library_id": lib.id,
                "commit_sha": head, "name": s.name}),
            ),
        );
    }
    // Exactly the organizations that did not fit are over budget; those that fit again (or no
    // longer use the library) are cleared.
    let over: Vec<Uuid> = over.into_iter().collect();
    let mut tx = while_current(state, lib).await?;
    repo::set_org_states(&mut tx, lib.id, &over)
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(done)
}
