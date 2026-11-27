//! Library management, browse and content reads (docs/10 §Git skill library, docs/05 §Skill
//! libraries; phase 4, P4-GSL; phase 5, P5-SKLIB-1, D-448: a library is an https git URL with an
//! optional sealed read-only token, C-1122..C-1127).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
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

use super::content::{
    self, Candidate, LoadError, LoadedSkill, ReadError, Reader, Resolved, TreeEntry,
};
use super::dto::{
    CheckSkillLibraryRequest, CheckSkills, CreateSkillLibraryRequest, LibraryCheck, LibraryFile,
    LibrarySkillContent, PatchSkillLibraryRequest, ResolvedRef, SkillCounts, SkillInvalidReason,
    SkillLibrary,
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
    pub reason_code: Option<SkillInvalidReason>,
    /// What an add counts; `None` only for a skill that could not be judged.
    pub size: Option<u64>,
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

/// The default branch's name a library without a ref stores and shows (C-6).
pub(crate) fn default_branch_of(resolved: &Resolved, reference: Option<&str>) -> Option<String> {
    (reference.is_none() && resolved.default && resolved.name != "HEAD")
        .then(|| one_line(&resolved.name, 200))
}

/// The read before saving (C-1122, C-1125): resolve the ref and read the catalogue at `path`,
/// so the stored library says what it offers (C-1176).
async fn probe(
    state: &AppState,
    src: &SourceAddr,
    token: Option<&Token>,
) -> Result<(Resolved, Offered), ApiError> {
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
            let listing = catalogue(&reader, &src.path, resolved.sha.clone())
                .await
                .map_err(|e| unreachable_on_create(&e))?;
            let offered = Offered::of(&listing.items);
            Ok((resolved, offered))
        })
        .await
}

/// (C-1176) How many skill folders of a listing can be added, and how many cannot.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Offered {
    pub offered: i32,
    pub unavailable: i32,
}

impl Offered {
    pub(crate) fn of(items: &[ListItem]) -> Self {
        let valid = items.iter().filter(|i| i.valid).count();
        let n = |v: usize| i32::try_from(v).unwrap_or(i32::MAX);
        Self {
            offered: n(valid),
            unavailable: n(items.len() - valid),
        }
    }

    pub(crate) async fn store(
        self,
        conn: &mut sqlx::PgConnection,
        library: Uuid,
    ) -> Result<(), DbError> {
        repo::set_offered(conn, library, self.offered, self.unavailable).await
    }
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
    let (resolved, offered) = probe(state, &src, token.as_ref()).await?;
    let default_branch = default_branch_of(&resolved, src.reference.as_deref());
    let head = resolved.sha;
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
                default_branch: default_branch.as_deref(),
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
        offered.store(&mut tx, row.id).await?;
        let mut row = row;
        row.skills_offered = Some(offered.offered);
        row.skills_unavailable = Some(offered.unavailable);
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
    let (resolved, listing, root) = state
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
            // The whole repository's folders, from the tree already read: only the suggested
            // label uses them, so a tree over the caps just leaves the folder out of it.
            let root = if src.path.is_empty() {
                Vec::new()
            } else {
                reader.root_tree(&resolved.sha).await.unwrap_or_default()
            };
            Ok((resolved, listing, root))
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
    let suggested_label = suggested_label(&src.repo, &src.path, &root);
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

/// The label offered for a checked repository (`host/path`): `<owner>/<repo>` (in a nested
/// group, the group that holds the repository), with the folder appended only when another
/// folder of the repository also holds several skills. At most 80 characters.
fn suggested_label(repo: &str, path: &str, root: &[TreeEntry]) -> String {
    let in_host = repo.split_once('/').map_or("", |(_, p)| p);
    let segments: Vec<&str> = in_host.split('/').filter(|s| !s.is_empty()).collect();
    let mut name = segments[segments.len().saturating_sub(2)..].join("/");
    if !path.is_empty() && other_library_folder(path, root) {
        name = format!("{name}/{path}");
    }
    let cleaned: String = clean_label(&name).chars().take(80).collect();
    if cleaned.is_empty() {
        "Skills".to_owned()
    } else {
        cleaned
    }
}

/// Whether a folder other than `path` holds at least two skill folders.
fn other_library_folder(path: &str, root: &[TreeEntry]) -> bool {
    let mut skills: BTreeMap<&str, usize> = BTreeMap::new();
    for e in root.iter().filter(|e| e.kind == "blob") {
        let Some(skill_dir) = e.path.strip_suffix("/SKILL.md") else {
            continue;
        };
        if aurora_core::meta_paths::has_github_segment(skill_dir) {
            continue;
        }
        let folder = skill_dir.rsplit_once('/').map_or("", |(f, _)| f);
        *skills.entry(folder).or_default() += 1;
    }
    skills.iter().any(|(folder, n)| *folder != path && *n >= 2)
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
    list_for_org_on(&mut c, org).await
}

/// [`list_for_org`] on a connection the caller already holds.
pub(crate) async fn list_for_org_on(
    c: &mut sqlx::PgConnection,
    org: Uuid,
) -> Result<Vec<LibraryRow>, ApiError> {
    let mut rows = repo::list_libraries(c, org).await?;
    rows.extend(repo::list_instance_libraries(c).await?);
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
            default_branch: None,
            path: None,
            token_set: None,
            token_updated_at: None,
            last_head_sha: None,
            last_refresh_at: None,
            last_error: None,
            skill_counts: None,
            pending_updates: None,
            orgs_using: None,
            // (C-1176) The picker shows how many skills a library offers.
            skills_offered: row.skills_offered,
            skills_unavailable: row.skills_unavailable,
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
        default_branch: row
            .reference
            .is_none()
            .then(|| row.default_branch.clone())
            .flatten(),
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
        skills_offered: row.skills_offered,
        skills_unavailable: row.skills_unavailable,
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
    let default_branch;
    let mut offered = None;
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
        let (resolved, found) = probe(state, &src, token).await?;
        default_branch = default_branch_of(&resolved, effective_ref.as_deref());
        head = resolved.sha;
        offered = Some(found);
        p.head_sha = Some(&head);
        p.default_branch = default_branch.as_deref();
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
        // (B-1) Another source: what was proposed or accepted from the old one no longer
        // applies. Lock order: the library row (held since `get_library_for_update`), then the
        // proposal rows (an approval locks its skill, then its proposal, and never the
        // library's row).
        let moved =
            row.url != locked.url || row.reference != locked.reference || row.path != locked.path;
        if moved {
            repo::mark_source_changed(&mut tx, id).await?;
        }
        if let Some(h) = p.head_sha {
            repo::record_head(&mut tx, id, h).await?;
        }
        let mut dropped = Vec::new();
        if moved {
            dropped = repo::drop_pending(&mut tx, id).await?;
            // Their bytes no longer count against any organization.
            repo::set_org_states(&mut tx, id, &[]).await?;
        }
        let mut row = row;
        if let Some(found) = offered {
            found.store(&mut tx, id).await?;
            row.skills_offered = Some(found.offered);
            row.skills_unavailable = Some(found.unavailable);
        }
        let mut meta = json!({"change": changed, "label": row.label, "url": row.url,
            "ref": row.reference, "path": row.path});
        if let Some(t) = token_change {
            meta["token"] = json!(t);
        }
        let mut events = Vec::new();
        if !dropped.is_empty() {
            meta["proposals_dropped"] = json!(dropped.len());
            let mut by_org: BTreeMap<Uuid, Vec<&repo::DroppedUpdate>> = BTreeMap::new();
            for d in &dropped {
                by_org.entry(d.org_id).or_default().push(d);
            }
            // An organization's library names what was dropped in its one row. An instance
            // library's row holds the total only; each organization gets its own row, naming
            // its own proposals and no other's.
            for (org, items) in by_org {
                if cur.org_id.is_some() {
                    meta["dropped"] = dropped_json(&items);
                } else {
                    events.push(
                        event(caller, Some(org), "skill_library.update")
                            .target("skill_library", id)
                            .metadata(json!({"change": changed, "label": row.label,
                                "proposals_dropped": items.len(),
                                "dropped": dropped_json(&items)})),
                    );
                }
            }
        }
        events.insert(
            0,
            event(caller, cur.org_id, "skill_library.update")
                .target("skill_library", id)
                .metadata(meta),
        );
        audit::commit_critical_many(tx, &events).await?;
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

/// The audit form of dropped proposals (at most 100 named; the count says how many).
fn dropped_json(items: &[&repo::DroppedUpdate]) -> serde_json::Value {
    items
        .iter()
        .take(100)
        .map(|d| json!({"update_id": d.update_id, "skill_id": d.skill_id}))
        .collect()
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

/// The commit of the library's cached listing, if one is cached for this row as it is now.
/// Reads memory only, never the repository.
pub(crate) fn cached_commit(state: &AppState, row: &LibraryRow) -> Option<String> {
    cached_listing(&state.library, row).map(|l| l.commit_sha.clone())
}

/// What of a library row decides where its content is read from and as whom.
pub(crate) fn fingerprint(row: &LibraryRow) -> String {
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
        // A listing past its five minutes is read again only if the head moved: what was
        // judged for (library, commit) stays true.
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

/// Records `sha` as a head of the library, unless its source changed since `row` was read (a
/// head of the old source is not accepted any more).
async fn record_head(state: &AppState, row: &LibraryRow, sha: &str) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    if repo::lock_source(&mut tx, row.id).await? == Some(row.source_epoch) {
        repo::record_head(&mut tx, row.id, sha).await?;
        tx.commit().await.map_err(DbError::from)?;
    }
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
    record_head(state, row, &head).await?;
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

/// Reads the tree at `sha` and judges the first `LIBRARY_BROWSE_MAX` skill directories by the
/// add's own rules on their real content: one tree read, one blob request for the `SKILL.md`
/// files, then at most `LIBRARY_JUDGE_REQUESTS_MAX` requests for the other files.
pub(crate) async fn catalogue(
    reader: &Reader<'_>,
    path: &str,
    sha: String,
) -> Result<Catalogue, ReadError> {
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
    let unjudged = fetch_for_judgement(reader, &listed).await?;
    let mut items = Vec::with_capacity(listed.len());
    for c in listed {
        items.push(if unjudged.contains(c.dir.as_str()) {
            unjudged_item(c)
        } else {
            judge(reader, c).await?
        });
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
    let resolved = reader
        .resolve_full(row.reference.as_deref())
        .await
        .map_err(|e| read_failure(&e))?;
    let default_branch = default_branch_of(&resolved, row.reference.as_deref());
    let sha = resolved.sha;
    record_head(state, row, &sha).await?;
    let l = match judged_listing(env, row, &sha) {
        Some(l) => l,
        None => {
            let cat = catalogue(&reader, &row.path, sha)
                .await
                .map_err(|e| read_failure(&e))?;
            Arc::new(Listing {
                commit_sha: cat.commit_sha,
                items: cat.items,
                truncated: cat.truncated,
            })
        }
    };
    {
        // Only while the library still reads this source.
        let mut tx = state.pool.begin().await.map_err(DbError::from)?;
        if repo::lock_source(&mut tx, row.id).await? == Some(row.source_epoch) {
            Offered::of(&l.items).store(&mut tx, row.id).await?;
            repo::set_default_branch(&mut tx, row.id, default_branch.as_deref()).await?;
            tx.commit().await.map_err(DbError::from)?;
        }
    }
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

/// The listing already judged for `(library, sha)` as the library is read now, however old.
fn judged_listing(env: &LibraryEnv, row: &LibraryRow, sha: &str) -> Option<Arc<Listing>> {
    let c = env.cache.lock().ok()?;
    let e = c.get(&row.id)?;
    (e.fingerprint == fingerprint(row) && e.listing.commit_sha == sha).then(|| e.listing.clone())
}

/// Fetches the files of the listed skills in as few requests as possible, within the bounds of
/// one judgement, and returns the folders that were left out. A request over a size cap is split
/// in two, so one huge folder does not hide the others.
async fn fetch_for_judgement<'a>(
    reader: &Reader<'_>,
    listed: &[&'a Candidate],
) -> Result<BTreeSet<&'a str>, ReadError> {
    let mut batches: VecDeque<Vec<&'a Candidate>> = VecDeque::new();
    let mut batch: Vec<&'a Candidate> = Vec::new();
    let mut wants = 0;
    for c in listed.iter().copied().filter(|c| c.problems.is_empty()) {
        let n = c.files.len() + 1;
        if wants + n > limits::LIBRARY_WANTS_MAX && !batch.is_empty() {
            batches.push_back(std::mem::take(&mut batch));
            wants = 0;
        }
        wants += n;
        batch.push(c);
    }
    batches.push_back(batch);
    let mut left_out = BTreeSet::new();
    let mut requests = 0;
    while let Some(batch) = batches.pop_front() {
        let ids: Vec<String> = batch
            .iter()
            .flat_map(|c| c.blob_ids())
            .filter(|id| !reader.has(id))
            .collect();
        if ids.is_empty() {
            continue;
        }
        if requests >= limits::LIBRARY_JUDGE_REQUESTS_MAX
            || reader.held_bytes() >= limits::LIBRARY_JUDGE_BYTES_MAX
        {
            left_out.extend(batch.iter().map(|c| c.dir.as_str()));
            continue;
        }
        requests += 1;
        match reader.prefetch(&ids).await {
            Ok(()) => {}
            Err(e) if git::is_size_cap(&e) => {
                if let [one] = batch[..] {
                    left_out.insert(one.dir.as_str());
                } else {
                    let (a, b) = batch.split_at(batch.len() / 2);
                    batches.push_front(b.to_vec());
                    batches.push_front(a.to_vec());
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(left_out)
}

const UNJUDGED: &str = "its size could not be checked: reading the files of this repository's \
                        skills would pass the limits of one read";

fn list_entry(c: &Candidate) -> ListItem {
    ListItem {
        path: c.dir.clone(),
        name: c.dir.clone(),
        description: None,
        valid: false,
        reason: None,
        reason_code: None,
        size: None,
        files: c.files.len() + usize::from(c.skill_md.is_some()),
    }
}

fn unjudged_item(c: &Candidate) -> ListItem {
    ListItem {
        reason: Some(UNJUDGED.to_owned()),
        reason_code: Some(SkillInvalidReason::UnknownSize),
        ..list_entry(c)
    }
}

/// One skill as the add would judge it (its files are already fetched).
async fn judge(reader: &Reader<'_>, c: &Candidate) -> Result<ListItem, ReadError> {
    let refused = |i: content::Invalid, size: u64| ListItem {
        reason: Some(one_line(&i.reason, 200)),
        reason_code: Some(i.code),
        size: Some(size),
        ..list_entry(c)
    };
    // A folder refused for its shape is not read: its size is what the tree says.
    if let Some(p) = c.problem() {
        return Ok(refused(p, c.tree_size()));
    }
    Ok(match content::load(reader, c).await {
        Ok(s) => ListItem {
            description: Some(s.description),
            valid: true,
            size: Some(s.size as u64),
            ..list_entry(c)
        },
        Err(LoadError::Invalid(i)) => refused(i, measured(reader, c)),
        Err(LoadError::Read(e)) => return Err(e),
    })
}

/// The size an add counts (body characters and the files), for a skill it refuses.
fn measured(reader: &Reader<'_>, c: &Candidate) -> u64 {
    let body = c
        .skill_md
        .as_ref()
        .and_then(|s| reader.cached(&s.blob))
        .map_or(0, |raw| match content::parse_skill_md(&raw, &c.dir) {
            Ok((_, body)) => body.chars().count(),
            Err(_) => aurora_registry::skill_format::content_size(&raw),
        });
    let files: usize = c
        .files
        .iter()
        .filter_map(|f| reader.cached(&f.blob))
        .map(|b| aurora_registry::skill_format::content_size(&b))
        .sum();
    (body + files) as u64
}

fn load_failure(e: LoadError) -> ApiError {
    match e {
        LoadError::Invalid(i) => ApiError::unprocessable(
            "library_skill_invalid",
            format!("This skill cannot be added: {}.", one_line(&i.reason, 200)),
        )
        .with_details(json!({"reason": one_line(&i.reason, 200)})),
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
            record_head(state, row, &head).await?;
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
    let empty = |i: content::Invalid| LibrarySkillContent {
        commit_sha: sha.clone(),
        path: c.dir.clone(),
        name: c.dir.clone(),
        valid: false,
        reason: Some(one_line(&i.reason, 200)),
        reason_code: Some(i.code),
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
            reason_code: None,
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
