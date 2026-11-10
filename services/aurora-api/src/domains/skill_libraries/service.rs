//! Library management, browse and content reads (docs/10 §Git skill library, docs/05 §Skill
//! libraries; phase 4, P4-GSL).

use std::sync::Arc;
use std::time::Instant;

use aurora_core::sanitize::{label as clean_label, one_line};
use aurora_core::{ApiError, FieldError, limits};
use aurora_db::DbError;
use aurora_db::repo::skill_libraries::{self as repo, LibraryPatch, LibraryRow, NewLibrary};
use aurora_registry::library;
use aurora_registry::manifest::Library;
use axum::http::StatusCode;
use base64::Engine as _;
use serde_json::json;
use uuid::Uuid;

use super::content::{self, Candidate, LoadError, LoadedSkill, ReadError, Reader};
use super::dto::{
    ConnectionRef, CreateSkillLibraryRequest, LibraryFile, LibrarySkillContent,
    PatchSkillLibraryRequest, SkillCounts, SkillLibrary,
};
use super::{CachedListing, LibraryEnv};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::plugins::registry;
use crate::http::auth::Caller;

#[derive(Debug)]
pub struct ListItem {
    pub path: String,
    pub name: String,
    pub description: Option<String>,
    pub valid: bool,
    pub reason: Option<String>,
    pub size: u64,
    pub files: usize,
}

#[derive(Debug)]
pub struct Listing {
    pub commit_sha: String,
    pub items: Vec<ListItem>,
    pub truncated: bool,
}

/// 404 `skill_libraries_disabled` while the kill-switch is off.
pub fn ensure_enabled(state: &AppState) -> Result<(), ApiError> {
    if state.config.skills_library.enabled {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "skill_libraries_disabled",
            "Skill libraries are disabled on this instance.",
        ))
    }
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid skill library.", vec![FieldError::new(path, msg)])
}

pub fn read_failure(e: &ReadError) -> ApiError {
    match e {
        ReadError::TreeTooLarge => ApiError::unprocessable("library_tree_too_large", e.reason()),
        ReadError::Unreachable(_) => ApiError::bad_gateway(
            "library_unavailable",
            format!(
                "The repository could not be read: {}.",
                one_line(&e.reason(), 200)
            ),
        ),
    }
}

/// The package's `library` block of `app_id` in the active snapshot.
pub async fn block_for(state: &AppState, app_id: &str) -> Result<Library, String> {
    let snap = registry::active_typed(state)
        .await
        .map_err(|_| "the registry is unavailable".to_owned())?
        .ok_or_else(|| "no plugin registry is active".to_owned())?;
    snap.app(app_id)
        .and_then(|a| a.library.clone())
        .ok_or_else(|| "the package no longer supports skill libraries".to_owned())
}

/// Validated library path: at most 4 plain segments, no `.`/`..`.
pub fn normalize_path(p: &str) -> Result<String, ApiError> {
    let p = p.trim().trim_matches('/');
    if p.is_empty() {
        return Ok(String::new());
    }
    let segs: Vec<&str> = p.split('/').collect();
    let ok = segs.len() <= 4
        && p.len() <= 200
        && segs.iter().all(|s| {
            !s.is_empty()
                && *s != "."
                && *s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        });
    if ok {
        Ok(p.to_owned())
    } else {
        Err(bad(
            "/path",
            "at most 4 folder names of letters, digits, `.`, `_` and `-`",
        ))
    }
}

fn clean_label_field(l: &str) -> Result<String, ApiError> {
    let l = clean_label(l);
    if l.is_empty() || l.chars().count() > 80 {
        return Err(bad("/label", "1 to 80 characters"));
    }
    Ok(l)
}

fn check_ref(r: &str) -> Result<(), ApiError> {
    library::check_ref(r).map_err(|_| {
        bad(
            "/ref",
            "a branch, tag or commit: letters, digits and `._/-`",
        )
    })
}

/// The org-scoped, active connection of an app with a `library` block.
async fn check_connection(
    state: &AppState,
    org: Uuid,
    connection_id: Uuid,
) -> Result<(repo::ConnectionBrief, Library), ApiError> {
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    let brief = repo::connection_brief(&mut c, connection_id)
        .await?
        .filter(|b| b.org_id == org)
        .ok_or_else(|| bad("/connection_id", "no such connection in this organization"))?;
    if brief.workspace_id.is_some() {
        return Err(ApiError::unprocessable(
            "connection_not_org_scoped",
            "A skill library needs an organization-level connection, not a workspace one.",
        ));
    }
    let block = block_for(state, &brief.app_id).await.map_err(|_| {
        ApiError::unprocessable(
            "app_without_library",
            "This connection's app cannot serve a skill library.",
        )
    })?;
    if brief.status != "active" {
        return Err(ApiError::unprocessable(
            "library_unreachable",
            "The connection is not active; reconnect it first.",
        ));
    }
    Ok((brief, block))
}

fn unreachable_on_create(e: &ReadError) -> ApiError {
    ApiError::unprocessable(
        "library_unreachable",
        format!(
            "The repository could not be reached: {}.",
            one_line(&e.reason(), 200)
        ),
    )
}

fn refresh_interval(state: &AppState) -> f64 {
    state
        .config
        .skills_library
        .refresh_interval_secs
        .max(limits::LIBRARY_REFRESH_INTERVAL_MIN.as_secs()) as f64
}

pub async fn create(
    state: &AppState,
    caller: &Caller,
    org: Uuid,
    req: &CreateSkillLibraryRequest,
) -> Result<LibraryRow, ApiError> {
    let label = clean_label_field(&req.label)?;
    let (brief, block) = check_connection(state, org, req.connection_id).await?;
    library::check_repo(&block, &req.repo).map_err(|_| {
        bad(
            "/repo",
            "does not match the repository format of this git host",
        )
    })?;
    let reference = req.reference.as_deref().filter(|r| !r.is_empty());
    if let Some(r) = reference {
        check_ref(r)?;
    }
    let path = normalize_path(req.path.as_deref().unwrap_or(""))?;
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    if repo::count_libraries(&mut c, org).await? >= limits::LIBRARIES_PER_ORG as i64 {
        return Err(ApiError::conflict(
            "library_limit",
            format!(
                "An organization can have at most {} libraries.",
                limits::LIBRARIES_PER_ORG
            ),
        ));
    }
    drop(c);
    let reader = Reader {
        gw: state.library.gateway.as_ref(),
        lib: &block,
        org,
        conn: brief.id,
        repo: &req.repo,
    };
    let head = reader
        .resolve(reference)
        .await
        .map_err(|e| unreachable_on_create(&e))?;
    let mut tx = audit::begin(state).await?;
    let row = repo::insert_library(
        &mut tx,
        &NewLibrary {
            org_id: org,
            label: &label,
            connection_id: brief.id,
            app_id: &brief.app_id,
            repo: &req.repo,
            reference,
            path: &path,
            head_sha: &head,
            created_by: caller.user_id,
            next_refresh_in_secs: refresh_interval(state),
        },
    )
    .await
    .map_err(|e| match e {
        DbError::UniqueViolation { .. } => ApiError::conflict(
            "library_exists",
            "A library with this label already exists in this organization.",
        ),
        e => e.into(),
    })?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(caller),
        "skill_library.create",
        Outcome::Success,
    )
    .org(org)
    .target("skill_library", row.id)
    .metadata(
        json!({"label": row.label, "repo": row.repo, "ref": row.reference,
            "path": row.path, "connection_id": brief.id, "app_id": row.app_id}),
    );
    audit::commit_critical(tx, &ev).await?;
    Ok(row)
}

pub async fn get(state: &AppState, id: Uuid) -> Result<LibraryRow, ApiError> {
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    repo::get_library(&mut c, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Skill library not found."))
}

pub async fn list(state: &AppState, org: Uuid) -> Result<Vec<LibraryRow>, ApiError> {
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    Ok(repo::list_libraries(&mut c, org).await?)
}

/// The dashboard view. `full == false` (a member's picker): `id`, `label`, `status` only.
pub async fn view(
    state: &AppState,
    row: &LibraryRow,
    full: bool,
) -> Result<SkillLibrary, ApiError> {
    if !full {
        return Ok(SkillLibrary {
            id: row.id,
            label: row.label.clone(),
            status: row.status.clone(),
            app_id: None,
            connection: None,
            repo: None,
            reference: None,
            path: None,
            last_head_sha: None,
            last_refresh_at: None,
            last_error: None,
            skill_counts: None,
            pending_updates: None,
            created_at: None,
        });
    }
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    let connection = match row.connection_id {
        Some(id) => repo::connection_brief(&mut c, id)
            .await?
            .map(|b| ConnectionRef {
                id: b.id,
                label: one_line(&b.label, 100),
                status: b.status,
            }),
        None => None,
    };
    let (a, w, o) = repo::skill_counts(&mut c, row.id).await?;
    let pending = repo::pending_count(&mut c, row.id).await?;
    Ok(SkillLibrary {
        id: row.id,
        label: row.label.clone(),
        status: row.status.clone(),
        app_id: Some(row.app_id.clone()),
        connection,
        repo: Some(row.repo.clone()),
        reference: row.reference.clone(),
        path: Some(row.path.clone()),
        last_head_sha: row.last_head_sha.clone(),
        last_refresh_at: row.last_refresh_at,
        last_error: row.last_error.clone(),
        skill_counts: Some(SkillCounts {
            agent: a,
            workspace: w,
            org: o,
        }),
        pending_updates: Some(pending),
        created_at: Some(row.created_at),
    })
}

pub async fn patch(
    state: &AppState,
    caller: &Caller,
    id: Uuid,
    req: &PatchSkillLibraryRequest,
) -> Result<LibraryRow, ApiError> {
    let cur = get(state, id).await?;
    let mut p = LibraryPatch::default();
    let mut changed: Vec<&str> = Vec::new();
    let label;
    if let Some(l) = &req.label {
        label = clean_label_field(l)?;
        p.label = Some(&label);
        changed.push("label");
    }
    if let Some(s) = req.status.as_deref() {
        if !matches!(s, "active" | "paused") {
            return Err(bad("/status", "must be active or paused"));
        }
        p.status = Some(s);
        changed.push("status");
    }
    let new_ref;
    let mut effective_ref = cur.reference.clone();
    if let Some(r) = &req.reference {
        if r.is_empty() {
            p.reference = Some(None);
            effective_ref = None;
        } else {
            check_ref(r)?;
            new_ref = r.clone();
            p.reference = Some(Some(&new_ref));
            effective_ref = Some(new_ref.clone());
        }
        changed.push("ref");
    }
    let new_path;
    if let Some(path) = &req.path {
        new_path = normalize_path(path)?;
        p.path = Some(&new_path);
        changed.push("path");
    }
    let mut connection_id = cur.connection_id;
    if let Some(c) = req.connection_id {
        connection_id = Some(c);
        p.connection_id = Some(c);
        changed.push("connection_id");
    }
    // A change of connection, ref or path re-resolves and never touches stored skills.
    let head;
    if req.connection_id.is_some() || req.reference.is_some() || req.path.is_some() {
        let Some(conn) = connection_id else {
            return Err(bad(
                "/connection_id",
                "the library has no connection; pick one",
            ));
        };
        let (brief, block) = check_connection(state, cur.org_id, conn).await?;
        if brief.app_id != cur.app_id {
            return Err(bad(
                "/connection_id",
                "the connection must be of the same app",
            ));
        }
        let reader = Reader {
            gw: state.library.gateway.as_ref(),
            lib: &block,
            org: cur.org_id,
            conn,
            repo: &cur.repo,
        };
        head = reader
            .resolve(effective_ref.as_deref())
            .await
            .map_err(|e| unreachable_on_create(&e))?;
        p.head_sha = Some(&head);
    }
    let mut tx = audit::begin(state).await?;
    repo::get_library_for_update(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Skill library not found."))?;
    let row = repo::patch_library(&mut tx, id, &p)
        .await
        .map_err(|e| match e {
            DbError::UniqueViolation { .. } => ApiError::conflict(
                "library_exists",
                "A library with this label already exists in this organization.",
            ),
            e => e.into(),
        })?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(caller),
        "skill_library.update",
        Outcome::Success,
    )
    .org(cur.org_id)
    .target("skill_library", id)
    .metadata(json!({"change": changed, "label": row.label}));
    audit::commit_critical(tx, &ev).await?;
    state.library.forget(id);
    Ok(row)
}

/// Deletes the library; every skill added from it is detached (keeps its content).
pub async fn delete(state: &AppState, caller: &Caller, id: Uuid) -> Result<u64, ApiError> {
    let cur = get(state, id).await?;
    let mut tx = audit::begin(state).await?;
    repo::get_library_for_update(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Skill library not found."))?;
    let detached = repo::detach_all(&mut tx, id).await?;
    repo::delete_library(&mut tx, id).await?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(caller),
        "skill_library.delete",
        Outcome::Success,
    )
    .org(cur.org_id)
    .target("skill_library", id)
    .metadata(json!({"label": cur.label, "repo": cur.repo, "skills_detached": detached}));
    audit::commit_critical(tx, &ev).await?;
    state.library.forget(id);
    Ok(detached)
}

pub async fn check_now(state: &AppState, id: Uuid) -> Result<(), ApiError> {
    let cur = get(state, id).await?;
    if cur.status == "paused" {
        return Err(ApiError::conflict(
            "library_paused",
            "Resume the library before checking it.",
        ));
    }
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    let gap = limits::LIBRARY_CHECK_NOW_INTERVAL.as_secs_f64();
    if repo::request_check(&mut c, id, gap).await? {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "library_check_too_soon",
            "This library was checked a moment ago; try again in a minute.",
        ))
    }
}

// ---- reading ------------------------------------------------------------------------------

/// What a read needs: the package's block and the connection id (never a credential).
pub async fn read_ctx(state: &AppState, row: &LibraryRow) -> Result<(Library, Uuid), ApiError> {
    let block = block_for(state, &row.app_id).await.map_err(|r| {
        ApiError::bad_gateway(
            "library_unavailable",
            format!("The repository could not be read: {r}."),
        )
    })?;
    let conn = row.connection_id.ok_or_else(|| {
        ApiError::bad_gateway(
            "library_unavailable",
            "The repository could not be read: the connection was deleted.",
        )
    })?;
    Ok((block, conn))
}

fn fingerprint(row: &LibraryRow) -> String {
    format!(
        "{:?}|{:?}|{}|{}",
        row.connection_id, row.reference, row.path, row.repo
    )
}

/// The catalogue of the library's current head (cached 5 minutes in memory; nothing is stored).
pub async fn listing(state: &AppState, row: &LibraryRow) -> Result<Arc<Listing>, ApiError> {
    let env: &LibraryEnv = &state.library;
    let fp = fingerprint(row);
    if let Ok(c) = env.cache.lock()
        && let Some(e) = c.get(&row.id)
        && e.fingerprint == fp
        && e.at.elapsed() < limits::LIBRARY_BROWSE_CACHE
    {
        return Ok(e.listing.clone());
    }
    let (block, conn) = read_ctx(state, row).await?;
    let reader = Reader {
        gw: env.gateway.as_ref(),
        lib: &block,
        org: row.org_id,
        conn,
        repo: &row.repo,
    };
    let sha = reader
        .resolve(row.reference.as_deref())
        .await
        .map_err(|e| read_failure(&e))?;
    let entries = reader.tree(&sha).await.map_err(|e| read_failure(&e))?;
    let cands = content::assemble(&block, &row.path, &entries);
    let truncated = cands.len() > limits::LIBRARY_BROWSE_MAX;
    let mut items = Vec::new();
    for c in cands.iter().take(limits::LIBRARY_BROWSE_MAX) {
        items.push(list_item(&reader, c).await.map_err(|e| read_failure(&e))?);
    }
    let l = Arc::new(Listing {
        commit_sha: sha,
        items,
        truncated,
    });
    if let Ok(mut c) = env.cache.lock() {
        c.insert(
            row.id,
            CachedListing {
                at: Instant::now(),
                fingerprint: fp,
                listing: l.clone(),
            },
        );
    }
    Ok(l)
}

async fn list_item(reader: &Reader<'_>, c: &Candidate) -> Result<ListItem, ReadError> {
    let base = |valid: bool, description: Option<String>, reason: Option<String>| ListItem {
        path: c.dir.clone(),
        name: c.dir.clone(),
        description,
        valid,
        reason: reason.map(|r| one_line(&r, 200)),
        size: c.tree_size(),
        files: c.files.len() + usize::from(c.skill_md.is_some()),
    };
    if let Some(p) = c.problem() {
        return Ok(base(false, None, Some(p)));
    }
    let Some(sm) = &c.skill_md else {
        return Ok(base(false, None, Some("SKILL.md is missing".into())));
    };
    let raw = reader.blob(&sm.blob).await?;
    if raw.len() > limits::LIBRARY_FILE_MAX {
        return Ok(base(false, None, Some("SKILL.md is too large".into())));
    }
    Ok(match content::parse_skill_md(&raw, &c.dir) {
        Ok((d, _)) => base(true, Some(d), None),
        Err(r) => base(false, None, Some(r)),
    })
}

fn load_failure(e: LoadError) -> ApiError {
    match e {
        LoadError::Invalid(r) => ApiError::unprocessable(
            "library_skill_invalid",
            format!("This skill cannot be added: {}.", one_line(&r, 200)),
        )
        .with_details(json!({"reason": one_line(&r, 200)})),
        LoadError::Read(r) => read_failure(&r),
    }
}

/// Reads and validates `dir` **at exactly `commit`** (a push after the person's browse is never
/// fetched). Errors are API errors for the add route.
pub async fn load_for_add(
    state: &AppState,
    row: &LibraryRow,
    dir: &str,
    commit: &str,
) -> Result<LoadedSkill, ApiError> {
    let (block, conn) = read_ctx(state, row).await?;
    let reader = Reader {
        gw: state.library.gateway.as_ref(),
        lib: &block,
        org: row.org_id,
        conn,
        repo: &row.repo,
    };
    let entries = reader.tree(commit).await.map_err(|e| read_failure(&e))?;
    let cands = content::assemble(&block, &row.path, &entries);
    let Some(c) = cands.iter().find(|c| c.dir == dir) else {
        return Err(ApiError::unprocessable(
            "library_skill_invalid",
            "This commit has no skill directory with that name.",
        ));
    };
    content::load(&reader, c).await.map_err(load_failure)
}

/// One skill at a commit (default: the current head): what an add would store; an invalid skill
/// is a 200 with `valid = false` and the reason.
pub async fn skill_content(
    state: &AppState,
    row: &LibraryRow,
    dir: &str,
    commit: Option<&str>,
) -> Result<LibrarySkillContent, ApiError> {
    let (block, conn) = read_ctx(state, row).await?;
    let reader = Reader {
        gw: state.library.gateway.as_ref(),
        lib: &block,
        org: row.org_id,
        conn,
        repo: &row.repo,
    };
    let sha = match commit {
        Some(c) => {
            library::check_hex_id(c, "sha")
                .map_err(|_| bad("/commit_sha", "a full commit id (40 to 64 hex characters)"))?;
            c.to_owned()
        }
        None => reader
            .resolve(row.reference.as_deref())
            .await
            .map_err(|e| read_failure(&e))?,
    };
    let entries = reader.tree(&sha).await.map_err(|e| read_failure(&e))?;
    let cands = content::assemble(&block, &row.path, &entries);
    let Some(c) = cands.iter().find(|c| c.dir == dir) else {
        return Err(ApiError::not_found(
            "No skill with that name at this commit.",
        ));
    };
    let empty = |reason: String| LibrarySkillContent {
        commit_sha: sha.clone(),
        path: c.dir.clone(),
        name: c.dir.clone(),
        valid: false,
        reason: Some(one_line(&reason, 200)),
        description: None,
        body: None,
        files: Vec::new(),
        size: 0,
        content_sha256: None,
    };
    match content::load(&reader, c).await {
        Err(LoadError::Invalid(r)) => Ok(empty(r)),
        Err(LoadError::Read(r)) => Err(read_failure(&r)),
        Ok(s) => Ok(LibrarySkillContent {
            commit_sha: sha,
            path: c.dir.clone(),
            name: s.name,
            valid: true,
            reason: None,
            description: Some(s.description),
            body: Some(s.body),
            files: s
                .files
                .iter()
                .map(|f| {
                    let text = std::str::from_utf8(&f.content).ok();
                    LibraryFile {
                        path: f.path.clone(),
                        size: crate::domains::skills::service::content_size(&f.content),
                        executable: f.executable,
                        binary: text.is_none(),
                        content: text.map(str::to_owned),
                        content_b64: text
                            .is_none()
                            .then(|| base64::engine::general_purpose::STANDARD.encode(&f.content)),
                    }
                })
                .collect(),
            size: s.size,
            content_sha256: Some(content::hex(&s.content_sha256)),
        }),
    }
}
