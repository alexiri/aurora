//! Library management, browse and content reads (docs/10 §Git skill library, docs/05 §Skill
//! libraries; phase 4, P4-GSL; phase 5, P5-SKLIB-1, D-448: a library is an https git URL with an
//! optional sealed read-only token, C-1122..C-1127).

use std::sync::Arc;
use std::time::Instant;

use aurora_core::git_url::{self, GitAddress, UrlError};
use aurora_core::sanitize::{label as clean_label, one_line};
use aurora_core::{ApiError, Secret, limits};
use aurora_db::DbError;
use aurora_db::repo::skill_libraries::{self as repo, LibraryPatch, LibraryRow, NewLibrary};
use aurora_registry::library;
use aurora_secrets::{Purpose, SealContext, SecretRef, ValueSource};
use axum::http::StatusCode;
use base64::Engine as _;
use serde_json::json;
use uuid::Uuid;

use super::content::{self, Candidate, LoadError, LoadedSkill, ReadError, Reader, Resolved};
use super::dto::{
    CheckSkillLibraryRequest, CheckSkills, CreateSkillLibraryRequest, LibraryCheck, LibraryFile,
    LibrarySkillContent, PatchSkillLibraryRequest, ResolvedRef, SkillCounts, SkillLibrary,
};
use super::git::{self, GitRepo, Token};
use super::{CachedListing, LibraryEnv};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::Caller;

/// The audit event of a library action: an organization library's is an organization row by the
/// caller; an instance library's is an instance row (`org_id` null) by the instance admin (D-449).
fn event(caller: &Caller, org: Option<Uuid>, action: &str) -> AuditEvent {
    match org {
        Some(o) => AuditEvent::by(caller, action).org(o),
        None => AuditEvent::new(
            AuditActor::instance_admin(caller),
            action.to_owned(),
            Outcome::Success,
        ),
    }
}

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
    if state.ops().skills_library.enabled {
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
    ApiError::invalid_field("Invalid skill library.", path, msg)
}

pub fn read_failure(e: &ReadError) -> ApiError {
    match e {
        ReadError::TreeTooLarge => ApiError::unprocessable("library_tree_too_large", e.reason()),
        ReadError::Unreachable(_) | ReadError::Throttled(_) => ApiError::bad_gateway(
            "library_unavailable",
            format!(
                "The repository could not be read: {}.",
                one_line(&e.reason(), 200)
            ),
        ),
    }
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

// ---- the address and the token (C-1122, C-1123) ---------------------------------------------

/// A parsed, normalized source: what a library stores and reads.
#[derive(Debug, Clone)]
pub struct SourceAddr {
    pub url: String,
    pub repo: String,
    pub reference: Option<String>,
    pub path: String,
    pub address: GitAddress,
}

fn url_invalid(e: UrlError) -> ApiError {
    ApiError::unprocessable("library_url_invalid", e.message())
        .with_details(json!({"reason": e.reason()}))
}

/// Parses `url` (C-1123); `ref`/`path` from a page URL only when the fields are absent.
pub fn address(
    state: &AppState,
    url: &str,
    reference: Option<&str>,
    path: Option<&str>,
) -> Result<SourceAddr, ApiError> {
    let a = git_url::parse(url).map_err(url_invalid)?;
    // An IP-literal host is checked against the destination guard now (and again at dial time).
    if let Some(ip) = a.ip_literal() {
        let host = ip.to_string();
        if !state.connect.allow.load().permits(ip, &host) {
            return Err(ApiError::unprocessable(
                "library_unreachable",
                format!(
                    "The repository could not be reached: {}.",
                    git::reason::ADDRESS_REFUSED
                ),
            ));
        }
    }
    let reference = match reference {
        Some(r) => Some(r.to_owned()),
        None => a.ref_hint.clone(),
    }
    .filter(|r| !r.is_empty());
    if let Some(r) = &reference {
        check_ref(r)?;
    }
    let path = normalize_path(path.or(a.path_hint.as_deref()).unwrap_or(""))?;
    Ok(SourceAddr {
        url: a.url.clone(),
        repo: a.repo_display.clone(),
        reference,
        path,
        address: a,
    })
}

/// The token of a request (C-1122): 1..1024 printable ASCII without whitespace; the user name
/// 1..100 of `[A-Za-z0-9._@-]`, default `x-access-token`; a user name needs a token.
pub fn token_input(token: Option<&str>, username: Option<&str>) -> Result<Option<Token>, ApiError> {
    let Some(t) = token else {
        if username.is_some() {
            return Err(bad("/token_username", "a user name needs a token"));
        }
        return Ok(None);
    };
    if t.is_empty() || t.len() > 1024 || !t.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(bad(
            "/token",
            "1 to 1024 printable characters without spaces",
        ));
    }
    let username = username.unwrap_or("x-access-token");
    if username.is_empty()
        || username.len() > 100
        || !username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'@' | b'-'))
    {
        return Err(bad(
            "/token_username",
            "1 to 100 letters, digits, `.`, `_`, `@` or `-`",
        ));
    }
    Ok(Some(Token {
        username: username.to_owned(),
        token: Secret::new(t.to_owned()),
    }))
}

/// The seal context of a library's token (C-1126).
fn token_ctx(library: Uuid, org: Option<Uuid>) -> SealContext {
    SealContext {
        purpose: Purpose::SkillLibraryToken,
        owner_id: library,
        org_id: org.unwrap_or(Uuid::nil()),
        workspace_id: None,
    }
}

fn store_unavailable() -> ApiError {
    ApiError::unavailable(
        "secret_store_unavailable",
        "The secret store is unavailable; nothing was saved. Try again shortly.",
    )
}

async fn seal_token(
    state: &AppState,
    library: Uuid,
    org: Option<Uuid>,
    t: &Token,
) -> Result<String, ApiError> {
    let plain = json!({"username": t.username, "token": t.token.expose()}).to_string();
    let r = state
        .vault
        .seal(&token_ctx(library, org), Secret::new(plain.into_bytes()))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "skill library token not sealed");
            store_unavailable()
        })?;
    Ok(r.to_column())
}

/// Best effort: a sealed value in an external store is deleted (inline values live in the row).
async fn discard_token(state: &AppState, library: Uuid, org: Option<Uuid>, token_ref: &str) {
    if let Ok(r) = token_ref.parse::<SecretRef>()
        && let Err(e) = state
            .vault
            .discard(&token_ctx(library, org), &r, ValueSource::Managed)
            .await
    {
        tracing::warn!(library = %library, error = %e, "a skill library token was not discarded");
    }
}

/// (SEC-148) Opens a library's token for one read. Only the repository reader uses the value.
pub(crate) async fn open_token(
    state: &AppState,
    row: &LibraryRow,
) -> Result<Option<Token>, String> {
    let Some(tr) = row.token_ref.as_deref() else {
        return Ok(None);
    };
    let fail = || "the access token could not be opened; try again later".to_owned();
    let r: SecretRef = tr.parse().map_err(|_| fail())?;
    let plain = state
        .vault
        .open(&token_ctx(row.id, row.org_id), &r)
        .await
        .map_err(|_| fail())?;
    let v: serde_json::Value = serde_json::from_slice(plain.expose()).map_err(|_| fail())?;
    let username = v["username"].as_str().ok_or_else(fail)?.to_owned();
    let token = v["token"].as_str().ok_or_else(fail)?.to_owned();
    Ok(Some(Token {
        username,
        token: Secret::new(token),
    }))
}

/// The stored URL of a row (a migrated row without one cannot be read).
pub(crate) fn row_url(row: &LibraryRow) -> Result<&str, ReadError> {
    row.url
        .as_deref()
        .ok_or_else(|| ReadError::Unreachable(git::reason::ENTER_URL.into()))
}

fn unreachable_on_create(e: &ReadError) -> ApiError {
    match e {
        ReadError::TreeTooLarge => ApiError::unprocessable("library_tree_too_large", e.reason()),
        _ => ApiError::unprocessable(
            "library_unreachable",
            format!(
                "The repository could not be reached: {}.",
                one_line(&e.reason(), 200)
            ),
        )
        .with_details(json!({"reason": one_line(&e.reason(), 200)})),
    }
}

fn refresh_interval(state: &AppState) -> f64 {
    state
        .ops()
        .skills_library
        .refresh_interval_secs
        .max(limits::LIBRARY_REFRESH_INTERVAL_MIN.as_secs()) as f64
}

/// `LIBRARIES_PER_ORG` for an organization, `LIBRARIES_PER_INSTANCE` for the instance's own.
async fn check_library_cap(
    conn: &mut sqlx::PgConnection,
    org: Option<Uuid>,
) -> Result<(), ApiError> {
    let (max, who) = match org {
        Some(_) => (limits::LIBRARIES_PER_ORG, "An organization"),
        None => (limits::LIBRARIES_PER_INSTANCE, "The instance"),
    };
    if repo::count_libraries(conn, org).await? >= max as i64 {
        return Err(ApiError::conflict(
            "library_limit",
            format!("{who} can have at most {max} libraries."),
        ));
    }
    Ok(())
}

fn exists() -> ApiError {
    ApiError::conflict(
        "library_exists",
        "A library with this label, or this repository, branch and folder, already exists.",
    )
}

/// The read before saving (C-1122, C-1125): resolve the ref and list the tree at `path`.
async fn probe(
    state: &AppState,
    src: &SourceAddr,
    token: Option<&Token>,
) -> Result<Resolved, ApiError> {
    // (SEC-148) One of the replica's read permits, under the overall read deadline.
    state
        .library
        .reads
        .interactive(async {
            let git = GitRepo::new(&state.connect, &src.url, token, state.library.fetch_timeout)
                .map_err(|e| unreachable_on_create(&e))?;
            let reader = Reader::new(&git, &src.path);
            let resolved = reader
                .resolve_full(src.reference.as_deref())
                .await
                .map_err(|e| unreachable_on_create(&e))?;
            reader
                .tree(&resolved.sha)
                .await
                .map_err(|e| unreachable_on_create(&e))?;
            Ok(resolved)
        })
        .await
}

/// Creates a library of `org`, or (`None`) an instance library (D-449; the caller has been
/// authorized by the route).
pub async fn create(
    state: &AppState,
    caller: &Caller,
    org: Option<Uuid>,
    req: &CreateSkillLibraryRequest,
) -> Result<LibraryRow, ApiError> {
    let label = clean_label_field(&req.label)?;
    let src = address(
        state,
        &req.url,
        req.reference.as_deref(),
        req.path.as_deref(),
    )?;
    let token = token_input(req.token.as_deref(), req.token_username.as_deref())?;
    // Early refusals before the slow read of the repository; the authoritative checks run
    // under the org's advisory lock right before the insert (below).
    {
        let mut c = state.pool.acquire().await.map_err(DbError::from)?;
        check_library_cap(&mut c, org).await?;
        if repo::url_taken(
            &mut c,
            org,
            &src.url,
            src.reference.as_deref(),
            &src.path,
            None,
        )
        .await?
        {
            return Err(exists());
        }
    }
    // (SEC-148) A create reads the repository like a check: the same per-person rate.
    super::routes::check_rate(caller, state)?;
    let head = probe(state, &src, token.as_ref()).await?.sha;
    let id = Uuid::now_v7();
    // Seal first; a failed insert deletes the sealed value again.
    let token_ref = match &token {
        Some(t) => Some(seal_token(state, id, org, t).await?),
        None => None,
    };
    let result = async {
        let mut tx = audit::begin(state).await?;
        // Serialise creates per organization (or for the instance): without the lock, concurrent
        // creates at 9 libraries all pass the count and exceed `LIBRARIES_PER_ORG`.
        let scope = org.map_or_else(|| "instance".to_owned(), |o| o.to_string());
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("skill_library_count:{scope}"))
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
        check_library_cap(&mut tx, org).await?;
        let row = repo::insert_library(
            &mut tx,
            &NewLibrary {
                id,
                org_id: org,
                label: &label,
                url: &src.url,
                repo: &src.repo,
                reference: src.reference.as_deref(),
                path: &src.path,
                token_ref: token_ref.as_deref(),
                head_sha: &head,
                created_by: caller.user_id,
                next_refresh_in_secs: refresh_interval(state),
            },
        )
        .await
        .map_err(|e| match e {
            DbError::UniqueViolation { .. } => exists(),
            e => e.into(),
        })?;
        repo::record_head(&mut tx, row.id, &head).await?;
        let mut meta = json!({"label": row.label, "url": row.url, "repo": row.repo,
            "ref": row.reference, "path": row.path});
        if token.is_some() {
            meta["token"] = json!("set");
        }
        let ev = event(caller, org, "skill_library.create")
            .target("skill_library", row.id)
            .metadata(meta);
        audit::commit_critical(tx, &ev).await?;
        Ok::<_, ApiError>(row)
    }
    .await;
    if result.is_err()
        && let Some(r) = &token_ref
    {
        discard_token(state, id, org, r).await;
    }
    result
}

/// `POST …/skill-libraries/check` (C-1124): the create read, answered; nothing is stored.
pub async fn check(
    state: &AppState,
    req: &CheckSkillLibraryRequest,
) -> Result<LibraryCheck, ApiError> {
    let src = address(
        state,
        &req.url,
        req.reference.as_deref(),
        req.path.as_deref(),
    )?;
    let token = token_input(req.token.as_deref(), req.token_username.as_deref())?;
    // (SEC-148) One of the replica's read permits, under the overall read deadline.
    let (resolved, listing) = state
        .library
        .reads
        .interactive(async {
            let git = GitRepo::new(
                &state.connect,
                &src.url,
                token.as_ref(),
                state.library.fetch_timeout,
            )
            .map_err(|e| unreachable_on_create(&e))?;
            let reader = Reader::new(&git, &src.path);
            let resolved = reader
                .resolve_full(src.reference.as_deref())
                .await
                .map_err(|e| unreachable_on_create(&e))?;
            let listing = catalogue(&reader, &src.path, resolved.sha.clone())
                .await
                .map_err(|e| unreachable_on_create(&e))?;
            Ok((resolved, listing))
        })
        .await?;
    let mut names: Vec<String> = listing
        .items
        .iter()
        .filter(|i| i.valid)
        .map(|i| i.name.clone())
        .collect();
    names.sort();
    let valid = names.len();
    names.truncate(20);
    let mut suggested = clean_label(src.address.last_segment());
    if suggested.is_empty() {
        suggested = "Skills".to_owned();
    }
    let suggested_label: String = suggested.chars().take(80).collect();
    Ok(LibraryCheck {
        url: src.url,
        repo: src.repo,
        reference: src.reference,
        resolved: ResolvedRef {
            kind: resolved.kind.as_str(),
            name: one_line(&resolved.name, 200),
            default: resolved.default,
        },
        commit_sha: resolved.sha,
        path: src.path,
        skills: CheckSkills {
            found: listing.found,
            valid,
            names,
        },
        truncated: listing.truncated,
        suggested_label,
    })
}

pub async fn get(state: &AppState, id: Uuid) -> Result<LibraryRow, ApiError> {
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    repo::get_library(&mut c, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Skill library not found."))
}

/// The libraries an organization sees: its own by label, then the instance libraries by label
/// (D-449). Every status is returned; the caller hides what its audience may not see.
pub async fn list_for_org(state: &AppState, org: Uuid) -> Result<Vec<LibraryRow>, ApiError> {
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    let mut rows = repo::list_libraries(&mut c, org).await?;
    rows.extend(repo::list_instance_libraries(&mut c).await?);
    Ok(rows)
}

/// The instance's own libraries (the admin list).
pub async fn list_instance(state: &AppState) -> Result<Vec<LibraryRow>, ApiError> {
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    Ok(repo::list_instance_libraries(&mut c).await?)
}

/// What a caller is shown of a library (C-1122, C-1128).
#[derive(Clone, Copy, Debug)]
pub enum View {
    /// A member's picker: `id`, `label`, `status`, `scope`.
    Picker,
    /// An organization owner/admin's view: every field but `token_updated_at` of an instance
    /// library; `skill_counts`, `pending_updates` and `status` (`over_budget` from this
    /// organization's budget state) are **this organization's only**.
    Org(Uuid),
    /// An instance admin's view of an instance library (or an organization library's full
    /// view): totals over every organization, `orgs_using`, the library's own status.
    Manage,
}

fn scope_of(row: &LibraryRow) -> &'static str {
    if row.org_id.is_some() {
        "org"
    } else {
        "instance"
    }
}

/// The dashboard view of one library for one audience.
pub async fn view(state: &AppState, row: &LibraryRow, how: View) -> Result<SkillLibrary, ApiError> {
    if matches!(how, View::Picker) {
        return Ok(SkillLibrary {
            id: row.id,
            scope: scope_of(row),
            org_id: None,
            label: row.label.clone(),
            status: row.status.clone(),
            url: None,
            repo: None,
            reference: None,
            path: None,
            token_set: None,
            token_updated_at: None,
            last_head_sha: None,
            last_refresh_at: None,
            last_error: None,
            skill_counts: None,
            pending_updates: None,
            orgs_using: None,
            created_at: None,
        });
    }
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    let only = match how {
        View::Org(o) => Some(o),
        // An organization library's manager sees its organization's numbers (all of them).
        View::Manage => row.org_id,
        View::Picker => None,
    };
    let (a, w, o) = repo::skill_counts(&mut c, row.id, only).await?;
    let pending = repo::pending_count(&mut c, row.id, only).await?;
    // An organization's view says `over_budget` while its own proposals do not fit (and the
    // library is otherwise fine); the library's own status never becomes `over_budget`.
    let org_scope = match how {
        View::Org(o) => Some(o),
        _ => row.org_id,
    };
    let status = match org_scope {
        Some(o) if row.status == "active" && repo::org_over_budget(&mut c, row.id, o).await? => {
            "over_budget".to_owned()
        }
        _ => row.status.clone(),
    };
    let instance_in_org_view = row.org_id.is_none() && matches!(how, View::Org(_));
    let orgs_using = if row.org_id.is_none() && matches!(how, View::Manage) {
        Some(repo::orgs_using(&mut c, row.id).await?)
    } else {
        None
    };
    Ok(SkillLibrary {
        id: row.id,
        scope: scope_of(row),
        org_id: row.org_id,
        label: row.label.clone(),
        status,
        url: row.url.clone(),
        repo: row.repo.clone(),
        reference: row.reference.clone(),
        path: Some(row.path.clone()),
        token_set: Some(row.token_ref.is_some()),
        token_updated_at: if instance_in_org_view {
            None
        } else {
            row.token_updated_at
        },
        last_head_sha: row.last_head_sha.clone(),
        last_refresh_at: row.last_refresh_at,
        last_error: row.last_error.clone(),
        skill_counts: Some(SkillCounts {
            agent: a,
            workspace: w,
            org: o,
        }),
        pending_updates: Some(pending),
        orgs_using,
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
    // (C-1122) The address is fixed once set: another repository is a new library.
    let new_src = match &req.url {
        Some(_) if cur.url.is_some() => {
            return Err(ApiError::conflict(
                "library_url_fixed",
                "The repository address cannot change; add a new library for another repository.",
            ));
        }
        Some(u) => Some(address(
            state,
            u,
            req.reference.as_deref(),
            req.path.as_deref(),
        )?),
        None => None,
    };
    if let Some(s) = &new_src {
        p.url = Some((&s.url, &s.repo));
        changed.push("url");
    }
    let mut effective_ref = cur.reference.clone();
    let new_ref;
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
    } else if let Some(s) = &new_src
        && s.reference.is_some()
    {
        effective_ref.clone_from(&s.reference);
        p.reference = Some(s.reference.as_deref());
        changed.push("ref");
    }
    let mut effective_path = cur.path.clone();
    let new_path;
    if let Some(path) = &req.path {
        new_path = normalize_path(path)?;
        effective_path.clone_from(&new_path);
        p.path = Some(&new_path);
        changed.push("path");
    } else if let Some(s) = &new_src
        && !s.path.is_empty()
    {
        effective_path.clone_from(&s.path);
        p.path = Some(&s.path);
        changed.push("path");
    }
    // `token`: a value sets or replaces, null removes, absent keeps.
    let new_token = match &req.token {
        Some(Some(t)) => token_input(Some(t), req.token_username.as_deref())?,
        Some(None) | None => {
            if req.token_username.is_some() {
                return Err(bad("/token_username", "a user name needs a token"));
            }
            None
        }
    };
    let token_change = match &req.token {
        Some(Some(_)) if cur.token_ref.is_some() => Some("replaced"),
        Some(Some(_)) => Some("set"),
        Some(None) if cur.token_ref.is_some() => Some("removed"),
        _ => None,
    };
    if token_change.is_some() {
        changed.push("token");
    }
    let effective_url = new_src
        .as_ref()
        .map(|s| s.url.clone())
        .or_else(|| cur.url.clone());
    let source_changed = new_src.is_some() || req.reference.is_some() || req.path.is_some();
    if source_changed && let Some(u) = &effective_url {
        let mut c = state.pool.acquire().await.map_err(DbError::from)?;
        if repo::url_taken(
            &mut c,
            cur.org_id,
            u,
            effective_ref.as_deref(),
            &effective_path,
            Some(id),
        )
        .await?
        {
            return Err(exists());
        }
    }
    // A change of url, ref, path or token re-reads (with the token that will be stored) and
    // never touches stored skills; a failed read keeps everything as it was.
    let head;
    if source_changed || new_token.is_some() || token_change == Some("removed") {
        let Some(u) = &effective_url else {
            return Err(bad("/url", "enter the repository's git address first"));
        };
        let kept;
        let token: Option<&Token> = if new_token.is_some() {
            new_token.as_ref()
        } else if token_change == Some("removed") {
            None
        } else {
            kept = open_token(state, &cur)
                .await
                .map_err(|_| store_unavailable())?;
            kept.as_ref()
        };
        let src = SourceAddr {
            url: u.clone(),
            repo: String::new(),
            reference: effective_ref.clone(),
            path: effective_path.clone(),
            address: git_url::parse(u).map_err(url_invalid)?,
        };
        // (SEC-148) A patch that re-reads is a read like a check: the same per-person rate.
        super::routes::check_rate(caller, state)?;
        head = probe(state, &src, token).await?.sha;
        p.head_sha = Some(&head);
    }
    let sealed = match &new_token {
        Some(t) => Some(seal_token(state, id, cur.org_id, t).await?),
        None => None,
    };
    match (&sealed, token_change) {
        (Some(s), _) => p.token_ref = Some(Some(s)),
        (None, Some("removed")) => p.token_ref = Some(None),
        _ => {}
    }
    let result = async {
        let mut tx = audit::begin(state).await?;
        // (SEC-149) What the row holds *now*, under its lock: a token replaced or removed since
        // the earlier read is the one to discard, never a stale reference.
        let locked = repo::get_library_for_update(&mut tx, id)
            .await?
            .ok_or_else(|| ApiError::not_found("Skill library not found."))?;
        let row = repo::patch_library(&mut tx, id, &p)
            .await
            .map_err(|e| match e {
                DbError::UniqueViolation { .. } => exists(),
                e => e.into(),
            })?;
        if let Some(h) = p.head_sha {
            repo::record_head(&mut tx, id, h).await?;
        }
        let mut meta = json!({"change": changed, "label": row.label, "url": row.url,
            "ref": row.reference, "path": row.path});
        if let Some(t) = token_change {
            meta["token"] = json!(t);
        }
        let ev = event(caller, cur.org_id, "skill_library.update")
            .target("skill_library", id)
            .metadata(meta);
        audit::commit_critical(tx, &ev).await?;
        // The value the row held under the lock and no longer holds.
        let replaced = match p.token_ref {
            Some(new) if locked.token_ref.as_deref() != new => locked.token_ref.clone(),
            _ => None,
        };
        Ok::<_, ApiError>((row, replaced))
    }
    .await;
    let result = match result {
        Ok((row, replaced)) => {
            // The replaced or removed value is gone from the row: discard it from its store.
            if let Some(old) = &replaced {
                discard_token(state, id, cur.org_id, old).await;
            }
            Ok(row)
        }
        Err(e) => {
            if let Some(s) = &sealed {
                discard_token(state, id, cur.org_id, s).await;
            }
            Err(e)
        }
    };
    state.library.forget(id);
    result
}

/// Deletes the library; every skill added from it is detached (keeps its content); the sealed
/// token is deleted.
pub async fn delete(state: &AppState, caller: &Caller, id: Uuid) -> Result<u64, ApiError> {
    let mut tx = audit::begin(state).await?;
    // (SEC-149) Everything below (the token to discard, the label and repository of the audit
    // rows) comes from the row as the lock sees it, not from an earlier read.
    let cur = repo::get_library_for_update(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Skill library not found."))?;
    // (D-449) An instance library's deletion detaches in every organization: one audit row each
    // (the count of that organization's skills), beside the instance row with the total.
    let per_org = if cur.org_id.is_none() {
        repo::affected_orgs(&mut tx, id).await?
    } else {
        Vec::new()
    };
    let detached = repo::detach_all(&mut tx, id).await?;
    repo::delete_library(&mut tx, id).await?;
    let meta = |n: u64| json!({"library_label": cur.label, "label": cur.label, "repo": cur.repo, "skills_detached": n});
    let mut events = vec![
        event(caller, cur.org_id, "skill_library.delete")
            .target("skill_library", id)
            .metadata(meta(detached)),
    ];
    for (org, n) in per_org {
        events.push(
            event(caller, Some(org), "skill_library.delete")
                .target("skill_library", id)
                .metadata(meta(u64::try_from(n).unwrap_or(0))),
        );
    }
    audit::commit_critical_many(tx, &events).await?;
    if let Some(t) = &cur.token_ref {
        discard_token(state, id, cur.org_id, t).await;
    }
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

/// The token of a read (503 when the store cannot open it).
async fn read_token(state: &AppState, row: &LibraryRow) -> Result<Option<Token>, ApiError> {
    open_token(state, row)
        .await
        .map_err(|_| store_unavailable())
}

fn git_for<'a>(
    state: &'a AppState,
    row: &LibraryRow,
    token: Option<&'a Token>,
) -> Result<GitRepo<'a>, ApiError> {
    let url = row_url(row).map_err(|e| read_failure(&e))?;
    GitRepo::new(&state.connect, url, token, state.library.fetch_timeout)
        .map_err(|e| read_failure(&e))
}

fn fingerprint(row: &LibraryRow) -> String {
    format!(
        "{:?}|{:?}|{}|{:?}",
        row.url, row.reference, row.path, row.token_updated_at
    )
}

/// The catalogue of the library's current head (cached 5 minutes in memory; nothing is stored).
pub async fn listing(state: &AppState, row: &LibraryRow) -> Result<Arc<Listing>, ApiError> {
    if let Some(l) = cached_listing(&state.library, row) {
        return Ok(l);
    }
    // (FIX-096) One cold read per library at a time, under a deadline: twenty members opening
    // the picker cost one read, not twenty.
    let flight = state.library.flight(row.id);
    let cold = async {
        let _turn = flight.lock().await;
        if let Some(l) = cached_listing(&state.library, row) {
            return Ok(l);
        }
        // (SEC-148) The replica's shared read permits; the wait counts toward the deadline.
        let _permit = state.library.reads.permit().await?;
        read_listing(state, row).await
    };
    match tokio::time::timeout(BROWSE_DEADLINE, cold).await {
        Ok(r) => r,
        Err(_) => Err(ApiError::bad_gateway(
            "library_unavailable",
            "The repository could not be read: the git host did not answer in time.",
        )),
    }
}

/// How long a browse may take (reading the tree and up to 200 `SKILL.md` files).
const BROWSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(90);

async fn record_head(state: &AppState, library: Uuid, sha: &str) -> Result<(), ApiError> {
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    repo::record_head(&mut c, library, sha).await?;
    Ok(())
}

/// (D-178 Q-R5-3) Browse and add accept only the ref's current head and heads this library has
/// recorded (refresh, browse, create): not a force-pushed or unmerged commit. Unknown ids answer
/// 422 `commit_not_in_library`.
async fn ensure_known_commit(
    state: &AppState,
    row: &LibraryRow,
    reader: &Reader<'_>,
    commit: &str,
) -> Result<(), ApiError> {
    if cached_listing(&state.library, row).is_some_and(|l| l.commit_sha == commit) {
        return Ok(());
    }
    {
        let mut c = state.pool.acquire().await.map_err(DbError::from)?;
        if repo::is_recorded_head(&mut c, row.id, commit).await? {
            return Ok(());
        }
    }
    let head = reader
        .resolve(row.reference.as_deref())
        .await
        .map_err(|e| read_failure(&e))?;
    record_head(state, row.id, &head).await?;
    if head == commit {
        return Ok(());
    }
    Err(ApiError::unprocessable(
        "commit_not_in_library",
        "This commit is neither the library's current head nor one it has recorded; browse the library again and use the commit it shows.",
    ))
}

fn cached_listing(env: &LibraryEnv, row: &LibraryRow) -> Option<Arc<Listing>> {
    let fp = fingerprint(row);
    let c = env.cache.lock().ok()?;
    let e = c.get(&row.id)?;
    (e.fingerprint == fp && e.at.elapsed() < limits::LIBRARY_BROWSE_CACHE)
        .then(|| e.listing.clone())
}

/// A listing plus how many skill directories were found (the check route's `found`).
pub(crate) struct Catalogue {
    pub items: Vec<ListItem>,
    pub found: usize,
    pub truncated: bool,
    pub commit_sha: String,
}

/// Reads the tree at `sha` and the `SKILL.md` of the first `LIBRARY_BROWSE_MAX` skill
/// directories (one blob request).
async fn catalogue(reader: &Reader<'_>, path: &str, sha: String) -> Result<Catalogue, ReadError> {
    let entries = reader.tree(&sha).await?;
    let cands = content::assemble(path, &entries);
    let truncated = cands.len() > limits::LIBRARY_BROWSE_MAX;
    let listed: Vec<&Candidate> = cands.iter().take(limits::LIBRARY_BROWSE_MAX).collect();
    let ids: Vec<String> = listed
        .iter()
        .filter(|c| c.problem().is_none())
        .filter_map(|c| c.skill_md.as_ref().map(|s| s.blob.clone()))
        .collect();
    reader.prefetch(&ids).await?;
    let mut items = Vec::with_capacity(listed.len());
    for c in listed {
        items.push(list_item(reader, c).await?);
    }
    Ok(Catalogue {
        items,
        found: cands.len(),
        truncated,
        commit_sha: sha,
    })
}

async fn read_listing(state: &AppState, row: &LibraryRow) -> Result<Arc<Listing>, ApiError> {
    let env: &LibraryEnv = &state.library;
    let fp = fingerprint(row);
    let token = read_token(state, row).await?;
    let git = git_for(state, row, token.as_ref())?;
    let reader = Reader::new(&git, &row.path);
    let sha = reader
        .resolve(row.reference.as_deref())
        .await
        .map_err(|e| read_failure(&e))?;
    record_head(state, row.id, &sha).await?;
    let cat = catalogue(&reader, &row.path, sha)
        .await
        .map_err(|e| read_failure(&e))?;
    let l = Arc::new(Listing {
        commit_sha: cat.commit_sha,
        items: cat.items,
        truncated: cat.truncated,
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
    let base =
        |valid: bool, description: Option<String>, reason: Option<String>, extra: u64| ListItem {
            path: c.dir.clone(),
            name: c.dir.clone(),
            description,
            valid,
            reason: reason.map(|r| one_line(&r, 200)),
            size: c.tree_size() + extra,
            files: c.files.len() + usize::from(c.skill_md.is_some()),
        };
    if let Some(p) = c.problem() {
        return Ok(base(false, None, Some(p), 0));
    }
    let Some(sm) = &c.skill_md else {
        return Ok(base(false, None, Some("SKILL.md is missing".into()), 0));
    };
    let raw = reader.blob(&sm.blob).await?;
    // The tree of a filtered fetch has no blob sizes: count the SKILL.md just read.
    let extra = if sm.size.is_none() {
        raw.len() as u64
    } else {
        0
    };
    if raw.len() > limits::LIBRARY_FILE_MAX {
        return Ok(base(
            false,
            None,
            Some("SKILL.md is too large".into()),
            extra,
        ));
    }
    Ok(match content::parse_skill_md(&raw, &c.dir) {
        Ok((d, _)) => base(true, Some(d), None, extra),
        Err(r) => base(false, None, Some(r), extra),
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
    state
        .library
        .reads
        .interactive(async {
            let token = read_token(state, row).await?;
            let git = git_for(state, row, token.as_ref())?;
            let reader = Reader::new(&git, &row.path);
            ensure_known_commit(state, row, &reader, commit).await?;
            let entries = reader.tree(commit).await.map_err(|e| read_failure(&e))?;
            let cands = content::assemble(&row.path, &entries);
            let Some(c) = cands.iter().find(|c| c.dir == dir) else {
                return Err(ApiError::unprocessable(
                    "library_skill_invalid",
                    "This commit has no skill directory with that name.",
                ));
            };
            content::load(&reader, c).await.map_err(load_failure)
        })
        .await
}

/// One skill at a commit (default: the current head): what an add would store; an invalid skill
/// is a 200 with `valid = false` and the reason.
pub async fn skill_content(
    state: &AppState,
    row: &LibraryRow,
    dir: &str,
    commit: Option<&str>,
) -> Result<LibrarySkillContent, ApiError> {
    // (SEC-148) One of the replica's read permits, under the overall read deadline.
    state
        .library
        .reads
        .interactive(skill_content_read(state, row, dir, commit))
        .await
}

async fn skill_content_read(
    state: &AppState,
    row: &LibraryRow,
    dir: &str,
    commit: Option<&str>,
) -> Result<LibrarySkillContent, ApiError> {
    let token = read_token(state, row).await?;
    let git = git_for(state, row, token.as_ref())?;
    let reader = Reader::new(&git, &row.path);
    let sha = match commit {
        Some(c) => {
            library::check_hex_id(c, "sha")
                .map_err(|_| bad("/commit_sha", "a full commit id (40 to 64 hex characters)"))?;
            ensure_known_commit(state, row, &reader, c).await?;
            c.to_owned()
        }
        None => {
            let head = reader
                .resolve(row.reference.as_deref())
                .await
                .map_err(|e| read_failure(&e))?;
            record_head(state, row.id, &head).await?;
            head
        }
    };
    let entries = reader.tree(&sha).await.map_err(|e| read_failure(&e))?;
    let cands = content::assemble(&row.path, &entries);
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
            content_sha256: Some(content::hex(s.content_sha256)),
        }),
    }
}
