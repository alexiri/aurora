//! The check path of C-1108, steps 1–7, shared by the scheduled poll and the manual buttons:
//! read the head, store the templates' head state, record candidates as `awaiting_package`,
//! download the packages and accept one only when its content equals the repository at the
//! recorded commit. Every heavy read goes through the skill library's read slots and its git
//! reader (never a second client); every parse and hash goes through `aurora-template`.

use std::collections::BTreeSet;

use aurora_core::limits;
use aurora_core::{ApiError, ids, sanitize};
use aurora_db::repo::agent_templates::{self as repo, SourceRow};
use aurora_registry::manifest::parse_semver;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent};
use crate::domains::agents::avatar;
use crate::domains::skill_libraries::git::{GitRepo, Token};

use super::dto::content_json;
use super::{fetch, git};

/// The failure backoff of a check that could not run (C-1107, C-598's shape: 1 h).
const FAILURE_BACKOFF: i64 = 3600;
/// At most this many sources per replica per poll tick (C-1107, as the addon sources' 2).
pub(crate) const CHECK_CONCURRENCY: usize = 2;
/// `never` schedules no check; the row still needs a next date (manual only).
const YEAR: i64 = 365 * 24 * 3600;

/// One version that left `awaiting_package` in this check.
#[derive(Debug, Clone)]
pub struct Ingested {
    pub template_id: String,
    pub version: String,
    /// `pending_approval` | `invalid`.
    pub status: String,
    pub status_reason: Option<String>,
}

/// One version still `awaiting_package` after this check.
#[derive(Debug, Clone)]
pub struct Waiting {
    pub template_id: String,
    pub version: String,
    pub since: DateTime<Utc>,
    pub last_reason: Option<String>,
}

/// One problem the head read or the candidate rule found.
#[derive(Debug, Clone)]
pub struct Problem {
    pub template_id: Option<String>,
    pub dir: String,
    pub code: String,
    pub message: String,
}

/// A template's own write failed (a row-level failure, e.g. the database refusing one row):
/// it becomes that template's problem and its row stays as it is (retried next check) — the
/// source's other templates are still ingested. A database that is down for everything
/// still aborts the check at the whole-check writes (the version reads, the final
/// transaction), so this only ever eats one row's failure.
fn row_problem(template_id: &str, dir: &str, message: &str) -> Problem {
    Problem {
        template_id: Some(template_id.to_owned()),
        dir: dir.to_owned(),
        code: "template_not_stored".to_owned(),
        message: sanitize::one_line(message, 200),
    }
}

/// The outcome of one check (C-1108's `CheckResult`, before the route adds `source_id`).
#[derive(Debug, Clone)]
pub struct CheckOutcome {
    /// `updated` | `up_to_date` | `failed`.
    pub status: &'static str,
    pub commit_sha: String,
    pub ingested: Vec<Ingested>,
    pub waiting: Vec<Waiting>,
    pub problems: Vec<Problem>,
    pub check_failed_reason: Option<String>,
}

impl CheckOutcome {
    fn failed(reason: String, commit_sha: &str) -> Self {
        Self {
            status: "failed",
            commit_sha: commit_sha.to_owned(),
            ingested: Vec::new(),
            waiting: Vec::new(),
            problems: Vec::new(),
            check_failed_reason: Some(reason),
        }
    }
}

// ---- the reads ---------------------------------------------------------------------------------

/// The sanitized reason of a read failure (the C-1125 words in `details.reason`; the read
/// slots' own 502 has none, its message is the reason then).
fn reason_of(e: &ApiError) -> String {
    e.details()
        .and_then(|d| d.get("reason"))
        .and_then(Value::as_str)
        .map_or_else(|| sanitize::one_line(e.message(), 200), |r| r.to_owned())
}

/// The head read (steps 1–2): resolve the ref, list the folder, read the manifest and every
/// `template.yaml`. Errors are sanitized reasons, not route errors: a read failure is a check
/// result, never an API error.
async fn read_head(
    state: &AppState,
    source: &SourceRow,
    token: Option<&Token>,
) -> Result<git::SourceRead, String> {
    let run = async {
        let g = GitRepo::with_profile(
            &state.connect,
            &source.url,
            token,
            state.library.fetch_timeout,
            git::profile(),
        )
        .map_err(|e| git::read_failure(&e))?;
        let reader =
            crate::domains::skill_libraries::content::Reader::with_profile(&g, "", git::profile());
        git::read_with(&reader, source.reference.as_deref(), &source.path)
            .await
            .map_err(|e| git::read_failure(&e))
    };
    state
        .library
        .reads
        .interactive(run)
        .await
        .map_err(|e| reason_of(&e))
}

/// Every file under `dir` at `commit_sha` (step 6's source comparison): the reader at that
/// commit, each blob through the template profile. A version's tree is read at the commit the
/// version was recorded with, never at a newer head.
///
/// [`DirFailure::TooLarge`] is a definite outcome: the tree holds more files than a template
/// may carry, answered after at most `TEMPLATE_FILES_MAX + 1` blobs were fetched — the rest
/// are never read (SEC-146; before, every blob was fetched and only the parse then refused).
async fn read_dir_at(
    state: &AppState,
    source: &SourceRow,
    token: Option<&Token>,
    commit_sha: &str,
    dir: &str,
) -> Result<Vec<aurora_template::TreeFile>, DirFailure> {
    let dir = dir.to_owned();
    let run = async {
        let g = GitRepo::with_profile(
            &state.connect,
            &source.url,
            token,
            state.library.fetch_timeout,
            git::profile(),
        )
        .map_err(|e| git::read_failure(&e))?;
        let reader = crate::domains::skill_libraries::content::Reader::with_profile(
            &g,
            &dir,
            git::profile(),
        );
        let entries = reader
            .tree(commit_sha)
            .await
            .map_err(|e| git::read_failure(&e))?;
        let mut files = Vec::with_capacity(entries.len());
        for e in &entries {
            if e.kind != "blob" {
                continue;
            }
            // The file cap is enforced before the next blob is fetched: one past the cap
            // proves the tree too large, and no further blob is read (SEC-146).
            if files.len() > limits::TEMPLATE_FILES_MAX {
                return Err(ApiError::conflict(
                    "template_too_large",
                    "the tree has more files than a template may carry",
                ));
            }
            let mode = u32::from_str_radix(&e.mode, 8).map_err(|_| {
                git::read_failure(
                    &crate::domains::skill_libraries::content::ReadError::Unreachable(format!(
                        "mode {} of {} is not octal",
                        e.mode, e.path
                    )),
                )
            })?;
            let bytes = reader
                .blob(&e.blob)
                .await
                .map_err(|e| git::read_failure(&e))?;
            files.push(aurora_template::TreeFile {
                path: e.path.clone(),
                bytes,
                mode,
            });
        }
        Ok(files)
    };
    match state.library.reads.interactive(run).await {
        Ok(files) => Ok(files),
        Err(e) if e.code() == "template_too_large" => Err(DirFailure::TooLarge),
        Err(_) => Err(DirFailure::Unreadable),
    }
}

/// Why [`read_dir_at`] failed: a too-large tree is the version's outcome (`invalid`), any
/// other read failure keeps it waiting (the sanitized reason is not stored — as before the
/// guard, the stay reason is the fixed `source_unreadable`).
enum DirFailure {
    TooLarge,
    Unreadable,
}

// ---- helpers -----------------------------------------------------------------------------------

/// Whether this core can run a version that needs `min_core` (the rule the addon sources
/// apply).
fn core_supports(min_core: Option<&str>) -> bool {
    let Some(mc) = min_core.and_then(parse_semver) else {
        return true;
    };
    let running = parse_semver(env!("CARGO_PKG_VERSION")).unwrap_or_default();
    running.core >= mc.core
}

/// The origin of the source's stored git address (the token's origin rule). An unparseable
/// stored address (never written) has no origin, so no token is ever attached.
fn repo_origin(url: &str) -> Option<url::Origin> {
    url::Url::parse(url).ok().map(|u| u.origin())
}

fn hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Writes the source's own check outcome (ok) without version work: the `up_to_date`
/// shortcut and the final transaction use the repo call directly; this is the shortcut's.
async fn finish_ok(
    state: &AppState,
    source: &SourceRow,
    next_in_secs: i64,
) -> Result<(), ApiError> {
    let mut c = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    repo::finish_check(
        &mut c,
        &repo::CheckDone {
            id: source.id,
            owner: source.lease_owner.unwrap_or_default(),
            status: "ok",
            reason: None,
            next_in_secs,
        },
    )
    .await?;
    Ok(())
}

/// Records a failed check — including one that died before writing any outcome (the poll
/// and the manual checks route an `Err` from [`check_source`] here): the source keeps
/// serving its last known state, the reason is stored sanitized (never a token, never
/// credentials), the lease is released and the next check backs off [`FAILURE_BACKOFF`].
/// A source with no schedule (`never`) keeps having none — the same one-year stand-in the
/// insert, the interval writes and the success path use — so a failed check never turns a
/// manual-only source into a scheduled one.
pub(crate) async fn fail(
    state: &AppState,
    source: &SourceRow,
    reason: &str,
) -> Result<CheckOutcome, ApiError> {
    let reason = sanitize::one_line(reason, 200);
    let next_in_secs = match super::next_in_secs(&source.update_interval, source.id) {
        Some(_) => FAILURE_BACKOFF,
        None => YEAR,
    };
    let mut c = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    repo::finish_check(
        &mut c,
        &repo::CheckDone {
            id: source.id,
            owner: source.lease_owner.unwrap_or_default(),
            status: "check_failed",
            reason: Some(reason.clone()),
            next_in_secs,
        },
    )
    .await?;
    Ok(CheckOutcome::failed(
        reason,
        source.last_head_sha.as_deref().unwrap_or(""),
    ))
}

/// Creates or updates a template row from a healthy head (`ok` or `needs_newer_aurora`):
/// the head's description replaces the stored one, the directory is back. Returns the row id,
/// or `None` when the upsert refused to take the template id from another source (the
/// ownership race — the caller reports `id_taken`, like the pre-check branch).
async fn store_head(
    state: &AppState,
    source_id: uuid::Uuid,
    m: &aurora_template::TemplateMeta,
    dir: &str,
    head_state: &str,
) -> Result<Option<uuid::Uuid>, ApiError> {
    let mut c = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    let row = repo::upsert_template(
        &mut c,
        &repo::NewTemplate {
            id: ids::new_id(),
            template_id: &m.id,
            source_id,
            dir,
            name: &sanitize::one_line(&m.name, 200),
            summary: &sanitize::one_line(&m.summary, 280),
            head_version: Some(&m.version),
            head_state,
        },
    )
    .await?;
    Ok(row.map(|r| r.id))
}

// ---- the check ---------------------------------------------------------------------------------

/// The full check of one source (C-1108). The caller holds the source's lease; every path
/// here ends in the lease being released with a written outcome.
pub async fn check_source(
    state: &AppState,
    source: &SourceRow,
    actor: &AuditActor,
) -> Result<CheckOutcome, ApiError> {
    let token = match super::open_source_token(state, source).await {
        Ok(t) => t,
        Err(e) => return fail(state, source, e.message()).await,
    };
    let read = match read_head(state, source, token.as_ref()).await {
        Ok(r) => r,
        Err(reason) => return fail(state, source, &reason).await,
    };
    let head = read.resolved.sha.clone();

    // The stored rows this check works from.
    let templates = repo::templates_of(&state.pool, source.id).await?;
    let mut versions = repo::versions_of_source(&state.pool, source.id).await?;
    let has_awaiting = versions
        .iter()
        .any(|v| v.version.status == "awaiting_package");
    let needs_newer = templates
        .iter()
        .any(|t| t.head_state == "needs_newer_aurora");

    // Step 1: the head is the stored one, nothing waits and nothing needs a newer core —
    // the result is `up_to_date` and nothing else is read.
    if source.last_head_sha.as_deref() == Some(head.as_str()) && !has_awaiting && !needs_newer {
        finish_ok(
            state,
            source,
            super::next_in_secs(&source.update_interval, source.id).unwrap_or(YEAR),
        )
        .await?;
        return Ok(CheckOutcome {
            status: "up_to_date",
            commit_sha: head,
            ingested: Vec::new(),
            waiting: Vec::new(),
            problems: Vec::new(),
            check_failed_reason: None,
        });
    }

    // Step 2: the manifest. A failure is a failed check; the stored rows stay as they are.
    let m = match read
        .manifest
        .as_deref()
        .map(aurora_template::parse_manifest)
    {
        Some(Ok(m)) => m,
        _ => return fail(state, source, "template_manifest_invalid").await,
    };

    // Step 3: per template directory — problems, the head state, `id_taken`,
    // `needs_newer_aurora`, `removed_upstream`.
    let mut problems: Vec<Problem> = Vec::new();
    let head_dirs: BTreeSet<String> = read.templates.iter().map(|d| d.dir.clone()).collect();
    // (template row id, template id, dir, meta) of the head's healthy templates, for step 4.
    let mut healthy: Vec<(uuid::Uuid, String, String, aurora_template::TemplateMeta)> = Vec::new();
    for d in &read.templates {
        let (meta, mut ps) = match &d.meta {
            Some(bytes) => match aurora_template::parse_meta(bytes) {
                Ok(m) => (Some(m), Vec::new()),
                Err(ps) => (None, ps),
            },
            None => (
                None,
                vec![aurora_template::TemplateProblem::new(
                    aurora_template::ProblemCode::TemplateYaml,
                    format!("{}/template.yaml", d.dir),
                    d.problem
                        .clone()
                        .unwrap_or_else(|| "template.yaml is missing".into()),
                )],
            ),
        };
        // A directory that is not its id is a problem (C-1108 step 3).
        if let Some(m) = &meta
            && m.id != d.dir
        {
            ps.push(aurora_template::TemplateProblem::new(
                aurora_template::ProblemCode::TemplateDirName,
                format!("{}/template.yaml", d.dir),
                "the folder name is not the template id",
            ));
        }
        for p in &ps {
            problems.push(Problem {
                template_id: meta.as_ref().map(|m| m.id.clone()),
                dir: d.dir.clone(),
                code: p.code.as_str().to_owned(),
                message: sanitize::one_line(&p.message, 200),
            });
        }
        let stored = templates.iter().find(|t| t.template_id == d.dir);
        match (&meta, ps.is_empty(), stored) {
            // Healthy head: create the row or update the stored description.
            (Some(m), true, _) => {
                let taken = repo::id_taken_elsewhere(&state.pool, &m.id, source.id).await?;
                if taken {
                    problems.push(Problem {
                        template_id: Some(m.id.clone()),
                        dir: d.dir.clone(),
                        code: "id_taken".to_owned(),
                        message: "another source already holds this template id".to_owned(),
                    });
                    continue;
                }
                let head_state = if core_supports(m.min_core.as_deref()) {
                    "ok"
                } else {
                    "needs_newer_aurora"
                };
                let row = match store_head(state, source.id, m, &d.dir, head_state).await {
                    Ok(Some(row)) => row,
                    // The upsert landed after another source took the id (the pre-check
                    // above read before that): the id stays where it is.
                    Ok(None) => {
                        problems.push(Problem {
                            template_id: Some(m.id.clone()),
                            dir: d.dir.clone(),
                            code: "id_taken".to_owned(),
                            message: "another source already holds this template id".to_owned(),
                        });
                        continue;
                    }
                    // This template's row could not be written (a row-level failure). That
                    // is one template's problem, not the source's: the others are still
                    // ingested (see [`row_problem`]).
                    Err(e) => {
                        tracing::warn!(template = %m.id, error = %e.message(), "template row not stored");
                        problems.push(row_problem(&m.id, &d.dir, e.message()));
                        continue;
                    }
                };
                if head_state == "ok" {
                    healthy.push((row, m.id.clone(), d.dir.clone(), m.clone()));
                }
            }
            // Problems: a stored template becomes `invalid` and keeps its versions; an
            // unstored one is only listed.
            (_, false, Some(t)) => {
                let listed: Vec<_> = ps
                    .iter()
                    .map(super::dto::TemplateProblemView::from)
                    .collect();
                // A failed state write leaves the row as it is (retried next check) and is
                // one template's problem, not the source's.
                let write = async {
                    let mut c = state
                        .pool
                        .acquire()
                        .await
                        .map_err(aurora_db::DbError::from)?;
                    repo::set_head_state(
                        &mut c,
                        t.id,
                        "invalid",
                        Some(&serde_json::to_value(listed).unwrap_or_default()),
                    )
                    .await
                };
                if let Err(e) = write.await {
                    tracing::warn!(template = %t.template_id, error = %e, "head state not stored");
                    problems.push(row_problem(&t.template_id, &t.dir, &e.to_string()));
                }
            }
            _ => {}
        }
    }
    // The directory flags: gone → `removed_upstream` (the template stays as it is, the admin
    // decides); back → cleared. The healthy path already cleared it (the upsert does).
    for t in &templates {
        let present = head_dirs.contains(&t.dir);
        if present != !t.removed_upstream {
            let write = async {
                let mut c = state
                    .pool
                    .acquire()
                    .await
                    .map_err(aurora_db::DbError::from)?;
                repo::set_removed_upstream(&mut c, t.id, !present).await
            };
            // One template's flag write is one template's problem (retried next check).
            if let Err(e) = write.await {
                tracing::warn!(template = %t.template_id, error = %e, "removed_upstream not stored");
                problems.push(row_problem(&t.template_id, &t.dir, &e.to_string()));
            }
        }
    }

    // Step 4: the candidate — the head's version when it is semver-newer than every stored
    // version of that template (any status); nothing stored yet is taken too. Lower is the
    // `version_not_newer` problem, equal is ignored.
    for (row_id, template_id, dir, meta) in &healthy {
        let newest = versions
            .iter()
            .filter(|v| v.version.agent_template_id == *row_id)
            .filter_map(|v| parse_semver(&v.version.version))
            .max();
        let Some(candidate) = parse_semver(&meta.version) else {
            continue;
        };
        match newest {
            Some(n) if candidate < n => {
                problems.push(Problem {
                    template_id: Some(template_id.clone()),
                    dir: String::new(),
                    code: "version_not_newer".to_owned(),
                    message: format!(
                        "the head version {} is lower than the stored version",
                        meta.version
                    ),
                });
                continue;
            }
            Some(n) if candidate == n => continue, // equal: ignored
            _ => {}                                // newer, or nothing stored yet: take it
        }
        let package_url = match aurora_template::render_package_url(&m, template_id, &meta.version)
        {
            Ok(u) => u,
            Err(p) => {
                problems.push(Problem {
                    template_id: Some(template_id.clone()),
                    dir: String::new(),
                    code: "package_url_invalid".to_owned(),
                    message: sanitize::one_line(&p.to_string(), 200),
                });
                continue;
            }
        };
        let v = repo::NewVersion {
            id: ids::new_id(),
            agent_template_id: *row_id,
            version: &meta.version,
            commit_sha: &head,
            package_url: &package_url,
        };
        match repo::insert_version(&state.pool, &v).await {
            Ok(row) => versions.push(repo::SourceVersionRow {
                version: row,
                template_id: template_id.clone(),
                dir: dir.clone(),
            }),
            Err(aurora_db::DbError::UniqueViolation { .. }) => {}
            // This version's row could not be recorded: one template's problem, not the
            // source's — and since nothing was stored, the next check offers it again.
            Err(e) => {
                tracing::warn!(template = %template_id, version = %meta.version, error = %e, "version not stored");
                problems.push(row_problem(template_id, dir, &e.to_string()));
            }
        }
    }

    // Steps 5–6: every `awaiting_package` row of the source, the new ones included.
    let mut ingested: Vec<Ingested> = Vec::new();
    let mut waiting: Vec<Waiting> = Vec::new();
    let mut done: Vec<(repo::VersionRow, String, StepOut)> = Vec::new();
    for v in versions
        .iter()
        .filter(|v| v.version.status == "awaiting_package")
    {
        let out = process_awaiting(state, source, token.as_ref(), v).await?;
        match &out {
            StepOut::Left(status, reason, _, _, _, _) => {
                ingested.push(Ingested {
                    template_id: v.template_id.clone(),
                    version: v.version.version.clone(),
                    status: (*status).to_owned(),
                    status_reason: reason.clone(),
                });
            }
            StepOut::Stayed(reason) => waiting.push(Waiting {
                template_id: v.template_id.clone(),
                version: v.version.version.clone(),
                since: v.version.created_at,
                last_reason: reason.clone(),
            }),
        }
        done.push((v.version.clone(), v.template_id.clone(), out));
    }

    // Step 7: one transaction — every version outcome, the head, the check's own state and
    // one critical `agent_template.ingest` row per version that left `awaiting_package`.
    let mut events: Vec<AuditEvent> = Vec::new();
    let mut tx = audit::begin(state).await?;
    for (row, template_id, out) in &done {
        match out {
            StepOut::Left(status, reason, content, content_sha, archive_sha, size) => {
                let matched = repo::finish_version(
                    &mut tx,
                    row.id,
                    status,
                    reason.as_deref(),
                    content.as_ref(),
                    content_sha.as_deref(),
                    archive_sha.as_deref(),
                    *size,
                    true,
                )
                .await?;
                if matched == 0 {
                    // The row no longer waits for a package: another check (the lease was
                    // lost) or an admin has moved it since this check read it. Nothing this
                    // transaction writes is true any more — roll it all back (SEC-146).
                    return Err(ApiError::conflict(
                        "template_check_in_progress",
                        "The source changed while this check was running.",
                    ));
                }
                // (C-1022, C-1108) A version that now waits for review is a decision item:
                // one notice per administrator, exactly once (this transaction).
                if *status == "pending_approval" {
                    let name = healthy
                        .iter()
                        .find(|(id, _, _, _)| *id == row.agent_template_id)
                        .map(|(_, _, _, m)| sanitize::one_line(&m.name, 200))
                        .or_else(|| {
                            templates
                                .iter()
                                .find(|t| t.id == row.agent_template_id)
                                .map(|t| t.name.clone())
                        })
                        .unwrap_or_default();
                    crate::domains::admin::attention::notify_template_version(
                        &mut tx,
                        row.id,
                        &name,
                        &row.version,
                    )
                    .await?;
                }
                events.push(
                    AuditEvent::new(
                        actor.clone(),
                        "agent_template.ingest",
                        crate::audit::Outcome::Success,
                    )
                    .target("agent_template", row.agent_template_id)
                    .metadata(json!({
                        "template": template_id, "version": row.version,
                        "status": status, "reason": reason, "commit": row.commit_sha,
                    })),
                );
            }
            StepOut::Stayed(reason) => {
                let matched = repo::finish_version(
                    &mut tx,
                    row.id,
                    "awaiting_package",
                    reason.as_deref(),
                    None,
                    None,
                    None,
                    None,
                    true,
                )
                .await?;
                if matched == 0 {
                    // Lost lease (see the `Left` arm): abort.
                    return Err(ApiError::conflict(
                        "template_check_in_progress",
                        "The source changed while this check was running.",
                    ));
                }
            }
        }
    }
    repo::set_head_sha(&mut tx, source.id, &head).await?;
    repo::finish_check(
        &mut tx,
        &repo::CheckDone {
            id: source.id,
            owner: source.lease_owner.unwrap_or_default(),
            status: "ok",
            reason: None,
            next_in_secs: if !waiting.is_empty() {
                // While a version waits for its package the source is due again within
                // `TEMPLATE_PACKAGE_RETRY` (C-1107), not at the full interval.
                aurora_core::limits::TEMPLATE_PACKAGE_RETRY.as_secs() as i64
            } else {
                super::next_in_secs(&source.update_interval, source.id).unwrap_or(YEAR)
            },
        },
    )
    .await?;
    audit::commit_critical_many(tx, &events).await?;
    let status = if ingested.is_empty() {
        "up_to_date"
    } else {
        "updated"
    };
    Ok(CheckOutcome {
        status,
        commit_sha: head,
        ingested,
        waiting,
        problems,
        check_failed_reason: None,
    })
}

/// Whether a version's last stay reason is transient: the fetching machinery failed, where
/// nothing in the repository or the package needs to change for a retry to succeed — the
/// package download (`download_failed`), the repository read (`source_unreadable`) or this
/// replica's avatar re-encode (`avatar_reencode_failed`). Such a version gets one more
/// retry window before it expires (docs/05 §Agent templates). Not transient:
/// `package_not_found` (the URL answered 404 — the package is not where the manifest
/// points) and every content verdict, which is `invalid` at once anyway.
fn is_transient_stay(reason: &str) -> bool {
    matches!(
        reason,
        "download_failed" | "source_unreadable" | "avatar_reencode_failed"
    )
}

/// What one `awaiting_package` row did in this check (the write lands in the check's final
/// transaction). `Left` carries everything the row needs to become `pending_approval` or
/// `invalid`.#[allow(clippy::type_complexity)]
enum StepOut {
    Left(
        /// `pending_approval` | `invalid`.
        &'static str,
        Option<String>,
        Option<Value>,
        Option<String>,
        Option<String>,
        Option<i32>,
    ),
    /// Stayed `awaiting_package` with its `status_reason`.
    Stayed(Option<String>),
}

/// Processes one `awaiting_package` row (steps 5–6): expire it, download the package and
/// accept it only when its content equals the repository at the recorded commit.
async fn process_awaiting(
    state: &AppState,
    source: &SourceRow,
    token: Option<&Token>,
    v: &repo::SourceVersionRow,
) -> Result<StepOut, ApiError> {
    // Step 5, expiry: waited past `TEMPLATE_PACKAGE_WAIT` from `created_at` — invalid with
    // the reason it last failed with. The comparison runs against the database's `now()`
    // (`repo::package_wait_expired`): the row's `created_at` is the database's clock, and a
    // replica's clock skew must not move the expiry. A version whose last attempt failed
    // transiently gets ONE more retry window before it expires (see
    // [`is_transient_stay`]): a network failure in the very last attempt must not retire a
    // perfectly good version for good, when an equal version is ignored afterwards and
    // only a version bump would recover it.
    let last_reason = v
        .version
        .status_reason
        .clone()
        .unwrap_or_else(|| "download_failed".to_owned());
    let wait_secs = aurora_core::limits::TEMPLATE_PACKAGE_WAIT.as_secs() as i64;
    let windows = if is_transient_stay(&last_reason) {
        2
    } else {
        1
    };
    if repo::package_wait_expired(&state.pool, v.version.id, wait_secs * windows).await? {
        return Ok(StepOut::Left(
            "invalid",
            Some(last_reason),
            None,
            None,
            None,
            None,
        ));
    }
    let invalid = |code: &'static str| {
        StepOut::Left("invalid", Some(code.to_owned()), None, None, None, None)
    };
    // Step 5, the download.
    let repo = repo_origin(&source.url);
    let bytes =
        match fetch::package(&state.connect, repo.as_ref(), &v.version.package_url, token).await {
            Ok(b) => b,
            Err(fetch::DownloadError::NotFound) => {
                return Ok(StepOut::Stayed(Some("package_not_found".into())));
            }
            Err(fetch::DownloadError::TooLarge) => return Ok(invalid("too_large")),
            Err(fetch::DownloadError::Failed(_)) => {
                return Ok(StepOut::Stayed(Some("download_failed".into())));
            }
        };
    // Step 6: parse the archive; the first C-1141 code is the reason.
    let content = match aurora_template::read_archive(&bytes) {
        Ok(c) => c,
        Err(ps) => {
            let code = ps
                .first()
                .map(|p| p.code.as_str())
                .unwrap_or("template_archive");
            return Ok(invalid(code));
        }
    };
    if content.meta.id != v.template_id || content.meta.version != v.version.version {
        return Ok(invalid("identity_mismatch"));
    }
    // The source comparison: the tree at the recorded commit. A too-large tree invalidates
    // the version; a tree that cannot be read keeps the row waiting.
    let files = match read_dir_at(state, source, token, &v.version.commit_sha, &v.dir).await {
        Ok(f) => f,
        Err(DirFailure::TooLarge) => return Ok(invalid("template_too_large")),
        Err(DirFailure::Unreadable) => {
            return Ok(StepOut::Stayed(Some("source_unreadable".into())));
        }
    };
    let tree = match aurora_template::read_tree(&v.dir, &files) {
        Ok(t) => t,
        Err(ps) => {
            let code = ps
                .first()
                .map(|p| p.code.as_str())
                .unwrap_or("template_archive");
            return Ok(invalid(code));
        }
    };
    if aurora_template::content_sha256(&content) != aurora_template::content_sha256(&tree) {
        return Ok(invalid("source_mismatch"));
    }
    // The avatar is re-encoded to a 256×256 PNG before storing; one that does not decode is
    // `invalid` `template_field` (C-1108 step 6).
    let mut content = content;
    if let Some((name, raw)) = content.avatar.take() {
        let png = match tokio::task::spawn_blocking(move || avatar::reencode(&raw)).await {
            Ok(png) => png,
            Err(e) => {
                // The version is fine; this replica could not run the re-encode. Stay and
                // retry (the reason is stored and becomes the expiry reason, see step 5).
                tracing::warn!(template = %v.template_id, version = %v.version.version, error = %e, "avatar re-encode join failed");
                return Ok(StepOut::Stayed(Some("avatar_reencode_failed".into())));
            }
        };
        match png {
            Ok(png) => content.avatar = Some((name, png)),
            Err(_) => return Ok(invalid("template_field")),
        }
    }
    let content_sha = aurora_template::content_sha256(&content);
    let archive_sha = hex(&bytes);
    let size = bytes.len().min(i32::MAX as usize) as i32;
    Ok(StepOut::Left(
        "pending_approval",
        None,
        Some(content_json(&content)),
        Some(content_sha),
        Some(archive_sha),
        Some(size),
    ))
}
