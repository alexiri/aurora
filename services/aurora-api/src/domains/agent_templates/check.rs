//! The check path of C-1108, steps 1–7, shared by the scheduled poll and the manual buttons:
//! read the head, store the templates' head state, record candidates as `awaiting_package`,
//! download the packages and accept one only when its content equals the repository at the
//! recorded commit. Every heavy read goes through the skill library's read slots and its git
//! reader (never a second client); every parse and hash goes through `aurora-template`.

use std::collections::BTreeSet;

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
async fn read_dir_at(
    state: &AppState,
    source: &SourceRow,
    token: Option<&Token>,
    commit_sha: &str,
    dir: &str,
) -> Result<Vec<aurora_template::TreeFile>, String> {
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
    state
        .library
        .reads
        .interactive(run)
        .await
        .map_err(|e| reason_of(&e))
}

// ---- helpers -----------------------------------------------------------------------------------

/// Whether this core can run a version that needs `min_core` (the rule the addon sources
/// apply).
fn core_supports(min_core: Option<&str>) -> bool {
    let Some(mc) = min_core.and_then(parse_semver) else {
        return true;
    };
    let running = parse_semver(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0, true));
    (running.0, running.1, running.2) >= (mc.0, mc.1, mc.2)
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

/// Records a failed check: the source keeps serving its last known state, the reason is
/// stored sanitized (never a token, never credentials) and nothing else changes.
async fn fail(
    state: &AppState,
    source: &SourceRow,
    reason: &str,
) -> Result<CheckOutcome, ApiError> {
    let reason = sanitize::one_line(reason, 200);
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
            next_in_secs: FAILURE_BACKOFF,
        },
    )
    .await?;
    Ok(CheckOutcome::failed(
        reason,
        source.last_head_sha.as_deref().unwrap_or(""),
    ))
}

/// Creates or updates a template row from a healthy head (`ok` or `needs_newer_aurora`):
/// the head's description replaces the stored one, the directory is back. Returns the row id.
async fn store_head(
    state: &AppState,
    source_id: uuid::Uuid,
    m: &aurora_template::TemplateMeta,
    dir: &str,
    head_state: &str,
) -> Result<uuid::Uuid, ApiError> {
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
    Ok(row.id)
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
                let row = store_head(state, source.id, m, &d.dir, head_state).await?;
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
                .await?;
            }
            _ => {}
        }
    }
    // The directory flags: gone → `removed_upstream` (the template stays as it is, the admin
    // decides); back → cleared. The healthy path already cleared it (the upsert does).
    for t in &templates {
        let present = head_dirs.contains(&t.dir);
        if present != !t.removed_upstream {
            let mut c = state
                .pool
                .acquire()
                .await
                .map_err(aurora_db::DbError::from)?;
            repo::set_removed_upstream(&mut c, t.id, !present).await?;
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
            Err(e) => return Err(e.into()),
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
                repo::finish_version(
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
                repo::finish_version(
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

/// What one `awaiting_package` row did in this check (the write lands in the check's final
/// transaction). `Left` carries everything the row needs to become `pending_approval` or
/// `invalid`.
#[allow(clippy::type_complexity)]
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
    // the reason it last failed with. That version is never tried again.
    if v.version.created_at
        + chrono::Duration::from_std(aurora_core::limits::TEMPLATE_PACKAGE_WAIT).unwrap_or_default()
        <= Utc::now()
    {
        return Ok(StepOut::Left(
            "invalid",
            Some(
                v.version
                    .status_reason
                    .clone()
                    .unwrap_or_else(|| "download_failed".to_owned()),
            ),
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
    // The source comparison: the tree at the recorded commit. A tree that cannot be read
    // keeps the row waiting.
    let files = match read_dir_at(state, source, token, &v.version.commit_sha, &v.dir).await {
        Ok(f) => f,
        Err(_) => return Ok(StepOut::Stayed(Some("source_unreadable".into()))),
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
        let png = tokio::task::spawn_blocking(move || avatar::reencode(&raw))
            .await
            .map_err(|e| ApiError::internal(format!("avatar re-encode join failed: {e}")))?;
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
