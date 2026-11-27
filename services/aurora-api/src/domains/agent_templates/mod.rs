//! Agent template sources (P6-TPL-2, C-1107): the admin routes for git template sources — the
//! skill library's address rules (C-1122, C-1123) and its reader (C-1125) with the template
//! profile, previews (C-1124's shape) and the sealed token (C-1126). The check/ingest path
//! (C-1108) and the poll job live in `check`/`poll`; the catalogue (C-1109/C-1110) in
//! `catalogue`. Everything here mirrors the addon-source code with the signing parts removed.

pub mod admin;
pub mod check;
pub mod create;
pub mod dto;
pub mod fetch;
pub mod git;
pub(crate) mod merge;
pub mod notify;
pub mod org;
pub mod poll;
pub(crate) mod routes;
pub(crate) mod updates;
pub(crate) mod workspace;
pub use routes::router;

use aurora_core::git_url::{self, UrlError};
use aurora_core::sanitize;
use aurora_core::{ApiError, ids, limits};
use aurora_db::repo::agent_templates::{self as repo, SourceRow};
use aurora_secrets::{Purpose, SealContext, SecretRef, ValueSource};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::skill_libraries::git::Token;
use dto::{
    AddTemplateSourceRequest, AgentTemplateSource, AgentTemplateSourcePreview, ManifestRepoView,
    PreviewTemplateSourceRequest, PreviewTemplateView, ResolvedRefView, SourceTemplateView,
    TemplateProblemView, UpdateTemplateSourceRequest,
};

/// The interval enum, jitter and lease owner are shared with the addon sources (C-1107 is
/// C-598's shape): `crate::domains::polling`.
pub use crate::domains::polling::{
    INTERVALS, interval_secs, jitter, lease_owner, next_in_secs, valid_interval,
};

fn bad_interval() -> ApiError {
    ApiError::invalid_request(
        "Invalid update interval.",
        vec![aurora_core::FieldError::new(
            "/update_interval",
            "must be one of 6h, 12h, 1d, 1w, 2w, 1mo, never",
        )],
    )
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_field("Invalid template source.", path, msg)
}

fn source_exists() -> ApiError {
    ApiError::conflict(
        "template_source_exists",
        "A source with this repository, branch and folder already exists.",
    )
}

fn source_limit() -> ApiError {
    ApiError::unprocessable(
        "template_source_limit",
        format!(
            "This instance can have at most {} template sources.",
            limits::TEMPLATE_SOURCES_MAX
        ),
    )
}

fn url_invalid(e: UrlError) -> ApiError {
    ApiError::unprocessable("template_url_invalid", e.message())
        .with_details(json!({ "reason": e.reason() }))
}

fn check_ref(r: &str) -> Result<(), ApiError> {
    aurora_registry::library::check_ref(r).map_err(|_| {
        bad(
            "/ref",
            "a branch, tag or commit: letters, digits and `._/-`",
        )
    })
}

/// Validated source folder: at most 4 plain segments, no `.`/`..` (C-1103).
fn normalize_path(p: &str) -> Result<String, ApiError> {
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

/// A parsed, normalized source address: what a source stores and reads (C-1123).
pub struct SourceAddr {
    pub url: String,
    pub repo: String,
    pub reference: Option<String>,
    pub path: String,
}

/// Parses `url` (C-1123); `ref`/`path` from a page URL only when the fields are absent.
fn address(
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
                "template_unreachable",
                format!(
                    "The repository could not be read: {}.",
                    crate::domains::skill_libraries::git::reason::ADDRESS_REFUSED
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
    })
}

/// The token of a request (C-1122): 1..1024 printable ASCII without whitespace; the user name
/// 1..100 of `[A-Za-z0-9._@-]`, default `x-access-token`; a user name needs a token.
fn token_input(token: Option<&str>, username: Option<&str>) -> Result<Option<Token>, ApiError> {
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
        token: aurora_core::Secret::new(t.to_owned()),
    }))
}

// ---- the sealed token (C-1126, purpose TemplateSourceToken) ---------------------------------

/// The seal context of a source's token: owner = the source id, org = the nil UUID (C-1106).
fn token_ctx(source: Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::TemplateSourceToken,
        owner_id: source,
        org_id: Uuid::nil(),
        workspace_id: None,
    }
}

async fn seal_token(state: &AppState, source: Uuid, t: &Token) -> Result<String, ApiError> {
    let plain = json!({"username": t.username, "token": t.token.expose()}).to_string();
    let r = state
        .vault
        .seal(
            &token_ctx(source),
            aurora_core::Secret::new(plain.into_bytes()),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "template source token not sealed");
            store_unavailable()
        })?;
    Ok(r.to_column())
}

/// Best effort: a sealed value in an external store is deleted (inline values live in the row).
async fn discard_token(state: &AppState, source: Uuid, token_ref: &str) {
    if let Ok(r) = token_ref.parse::<SecretRef>()
        && let Err(e) = state
            .vault
            .discard(&token_ctx(source), &r, ValueSource::Managed)
            .await
    {
        tracing::warn!(source = %source, error = %e, "a template source token was not discarded");
    }
}

fn store_unavailable() -> ApiError {
    ApiError::unavailable(
        "secret_store_unavailable",
        "The secret store is unavailable; nothing was saved. Try again shortly.",
    )
}

/// Opens a source's token for one read (the repository reader and the package download only).
pub(crate) async fn open_source_token(
    state: &AppState,
    row: &SourceRow,
) -> Result<Option<Token>, ApiError> {
    let Some(tr) = row.token_ref.as_deref() else {
        return Ok(None);
    };
    let r: SecretRef = tr.parse().map_err(|_| store_unavailable())?;
    let plain = state
        .vault
        .open(&token_ctx(row.id), &r)
        .await
        .map_err(|_| store_unavailable())?;
    let v: Value = serde_json::from_slice(plain.expose()).map_err(|_| store_unavailable())?;
    let username = v["username"]
        .as_str()
        .ok_or_else(store_unavailable)?
        .to_owned();
    let token = v["token"]
        .as_str()
        .ok_or_else(store_unavailable)?
        .to_owned();
    Ok(Some(Token {
        username,
        token: aurora_core::Secret::new(token),
    }))
}

// ---- the manifest ---------------------------------------------------------------------------

fn manifest_missing() -> ApiError {
    ApiError::unprocessable(
        "template_manifest_invalid",
        "The folder has no aurora-templates.yaml.",
    )
}

fn manifest_invalid(p: aurora_template::ManifestProblem) -> ApiError {
    ApiError::unprocessable(
        "template_manifest_invalid",
        "The repository manifest is invalid.",
    )
    .with_details(json!({ "reason": sanitize::one_line(&p.to_string(), 200) }))
}

/// Parses the manifest bytes of a read (the shared step of preview, add and patch).
fn parse(mbytes: &[u8]) -> Result<aurora_template::TemplateManifest, ApiError> {
    aurora_template::parse_manifest(mbytes).map_err(manifest_invalid)
}

/// The last accepted manifest as stored on the source row (the dashboard reads this).
fn stored_manifest(m: &aurora_template::TemplateManifest) -> Value {
    json!({
        "api": m.api,
        "repo": {
            "name": m.repo.name,
            "description": m.repo.description,
            "homepage": m.repo.homepage,
            "contact": m.repo.contact,
        },
        "packages": { "url": m.packages.url },
    })
}

/// The source's display name: the manifest's `repo.name`, cleaned (C-1106).
fn source_name(m: &aurora_template::TemplateManifest) -> String {
    let name = sanitize::label(&m.repo.name);
    if name.is_empty() {
        "Agent templates".to_owned()
    } else {
        name
    }
}

// ---- preview, add, patch, remove --------------------------------------------------------------

/// (P6-TPL-2, C-1107) 10 previews per minute per person (as C-1124's checks).
pub(crate) fn check_rate(state: &AppState, user: Uuid) -> Result<(), ApiError> {
    state.rate_limiter.check_quota(
        "agent_template_preview",
        &user.to_string(),
        crate::http::ratelimit::Quota {
            capacity: 10,
            per: std::time::Duration::from_secs(60),
        },
    )
}

/// Read a repository like an add would and say what was found; nothing is stored and no audit
/// row is written (C-1107, as C-1124 does for libraries).
pub async fn preview(
    state: &AppState,
    user: Uuid,
    req: &PreviewTemplateSourceRequest,
) -> Result<AgentTemplateSourcePreview, ApiError> {
    let addr = address(
        state,
        &req.url,
        req.reference.as_deref(),
        req.path.as_deref(),
    )?;
    let token = token_input(req.token.as_deref(), req.token_username.as_deref())?;
    if repo::addr_taken(
        &state.pool,
        &addr.url,
        addr.reference.as_deref(),
        &addr.path,
    )
    .await?
    {
        return Err(source_exists());
    }
    check_rate(state, user)?;
    let read = git::read_source(
        state,
        &addr.url,
        addr.reference.as_deref(),
        &addr.path,
        token.as_ref(),
    )
    .await?;
    let m = parse(read.manifest.as_deref().ok_or_else(manifest_missing)?)?;
    preview_of(state, &addr, &read, &m).await
}

/// The preview answer of an already-read source (shared by preview and the tests).
async fn preview_of(
    state: &AppState,
    addr: &SourceAddr,
    read: &git::SourceRead,
    m: &aurora_template::TemplateManifest,
) -> Result<AgentTemplateSourcePreview, ApiError> {
    let dirs: Vec<String> = read.templates.iter().map(|d| d.dir.clone()).collect();
    let held: std::collections::BTreeSet<String> = repo::template_holders(&state.pool, &dirs)
        .await?
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    let mut templates = Vec::with_capacity(read.templates.len());
    for d in &read.templates {
        let (meta, problems) = match &d.meta {
            Some(bytes) => match aurora_template::parse_meta(bytes) {
                Ok(meta) => (Some(meta), Vec::new()),
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
        let (template_id, name, summary, version) = match &meta {
            Some(meta) => (
                meta.id.clone(),
                sanitize::one_line(&meta.name, 200),
                sanitize::one_line(&meta.summary, 280),
                Some(meta.version.clone()),
            ),
            None => (d.dir.clone(), d.dir.clone(), String::new(), None),
        };
        // A template id another source already holds is listed as `id_taken`; nothing else can
        // be stored-state for a source that is not saved (C-1107).
        let state_ = if held.contains(&template_id) {
            "id_taken"
        } else {
            "new"
        };
        templates.push(PreviewTemplateView {
            template_id,
            dir: d.dir.clone(),
            name,
            summary,
            version,
            state: state_.to_owned(),
            problems: problems.iter().map(TemplateProblemView::from).collect(),
        });
    }
    Ok(AgentTemplateSourcePreview {
        url: addr.url.clone(),
        repo: addr.repo.clone(),
        reference: addr.reference.clone(),
        resolved: ResolvedRefView {
            kind: read.resolved.kind.as_str().to_owned(),
            name: sanitize::one_line(&read.resolved.name, 200),
            default: read.resolved.default,
        },
        commit_sha: read.resolved.sha.clone(),
        path: addr.path.clone(),
        manifest: ManifestRepoView {
            name: sanitize::one_line(&m.repo.name, 80),
            description: sanitize::one_line(&m.repo.description, 280),
            packages_url: m.packages.url.clone(),
        },
        templates,
        truncated: read.truncated,
    })
}

/// Add a source (C-1107): the preview read, then one audited transaction that writes the row
/// and seals the token. The first check (C-1108) runs outside the transaction.
pub async fn add(
    state: &AppState,
    actor: &AuditActor,
    user: Uuid,
    req: &AddTemplateSourceRequest,
) -> Result<SourceRow, ApiError> {
    let addr = address(
        state,
        &req.url,
        req.reference.as_deref(),
        req.path.as_deref(),
    )?;
    let token = token_input(req.token.as_deref(), req.token_username.as_deref())?;
    let interval = req
        .update_interval
        .clone()
        .unwrap_or_else(|| state.ops().agents.templates.update_interval.clone());
    if !valid_interval(&interval) {
        return Err(bad_interval());
    }
    // Early refusals before the slow read; the authoritative checks run again in the
    // transaction (below).
    {
        let mut c = state
            .pool
            .acquire()
            .await
            .map_err(aurora_db::DbError::from)?;
        if repo::count_sources(&mut *c).await? >= limits::TEMPLATE_SOURCES_MAX as i64 {
            return Err(source_limit());
        }
        if repo::addr_taken(&mut *c, &addr.url, addr.reference.as_deref(), &addr.path).await? {
            return Err(source_exists());
        }
    }
    // A create reads the repository like a preview: the same per-person rate.
    check_rate(state, user)?;
    let read = git::read_source(
        state,
        &addr.url,
        addr.reference.as_deref(),
        &addr.path,
        token.as_ref(),
    )
    .await?;
    let m = parse(read.manifest.as_deref().ok_or_else(manifest_missing)?)?;
    let manifest = stored_manifest(&m);
    let id = ids::new_id();
    // Seal first; a failed insert deletes the sealed value again.
    let token_ref = match &token {
        Some(t) => Some(seal_token(state, id, t).await?),
        None => None,
    };
    let result = async {
        let mut tx = audit::begin(state).await?;
        // Serialise creates across the instance: without the lock, concurrent adds at
        // `TEMPLATE_SOURCES_MAX - 1` both pass the count.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind("agent_template_source_count")
            .execute(&mut *tx)
            .await
            .map_err(aurora_db::DbError::from)?;
        if repo::count_sources(&mut *tx).await? >= limits::TEMPLATE_SOURCES_MAX as i64 {
            return Err(source_limit());
        }
        let row = repo::insert_source(
            &mut tx,
            &repo::NewSource {
                id,
                name: &source_name(&m),
                url: &addr.url,
                reference: addr.reference.as_deref(),
                path: &addr.path,
                token_ref: token_ref.as_deref(),
                homepage: m.repo.homepage.as_deref(),
                contact: m.repo.contact.as_deref(),
                packages_url: &m.packages.url,
                manifest: &manifest,
                update_interval: &interval,
                first_check_in_secs: next_in_secs(&interval, id),
                created_by: user,
            },
        )
        .await
        .map_err(|e| match e {
            aurora_db::DbError::UniqueViolation { .. } => source_exists(),
            e => e.into(),
        })?;
        let mut meta = json!({"url": addr.url, "ref": addr.reference, "path": addr.path,
            "templates": read.templates.len()});
        if token.is_some() {
            meta["token"] = json!("set");
        }
        let ev = AuditEvent::new(actor.clone(), "agent_template_source.add", Outcome::Success)
            .target("agent_template_source", row.id)
            .metadata(meta);
        audit::commit_critical(tx, &ev).await?;
        Ok::<_, ApiError>(row)
    }
    .await;
    if result.is_err()
        && let Some(r) = &token_ref
    {
        discard_token(state, id, r).await;
    }
    result
}

/// What the patch changes about the token.
enum TokenChange {
    Keep,
    Remove,
    Set(Token),
}

/// Patch a source (C-1107): the schedule, the address parts and the token. A change of ref,
/// path or token re-reads the repository first (422 keeps the old values); stored versions are
/// never touched.
pub async fn update(
    state: &AppState,
    actor: &AuditActor,
    id: Uuid,
    req: &UpdateTemplateSourceRequest,
) -> Result<SourceRow, ApiError> {
    let row = repo::get_source(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Template source not found."))?;
    if let Some(iv) = &req.update_interval
        && !valid_interval(iv)
    {
        return Err(bad_interval());
    }
    // The new address parts (`ref: ""` clears; absent keeps).
    let reference = match &req.reference {
        Some(r) => {
            let r = r.trim();
            if r.is_empty() {
                None
            } else {
                check_ref(r)?;
                Some(r.to_owned())
            }
        }
        None => row.reference.clone(),
    };
    let path = match &req.path {
        Some(p) => normalize_path(p)?,
        None => row.path.clone(),
    };
    let token_change = match &req.token {
        None => TokenChange::Keep,
        Some(None) => TokenChange::Remove,
        Some(Some(t)) => {
            let token = token_input(Some(t), req.token_username.as_deref())?;
            TokenChange::Set(token.ok_or_else(|| bad("/token", "a token was expected"))?)
        }
    };
    let revalidate = req.reference.is_some() || req.path.is_some();
    let token_touched = !matches!(token_change, TokenChange::Keep);
    // Seal a new token first; a failed write deletes it again and the old token stays.
    let new_token_ref = match &token_change {
        TokenChange::Keep => row.token_ref.clone(),
        TokenChange::Remove => None,
        TokenChange::Set(t) => Some(seal_token(state, id, t).await?),
    };
    // A change of ref, path or token re-reads the repository first (C-1107). The read uses the
    // new values; the old token stays when it fails, because nothing was written.
    let read = if revalidate || token_touched {
        let opened;
        let tok: Option<&Token> = match &token_change {
            TokenChange::Set(t) => Some(t),
            TokenChange::Remove => None,
            TokenChange::Keep => {
                opened = open_source_token(state, &row).await?;
                opened.as_ref()
            }
        };
        let read = git::read_source(state, &row.url, reference.as_deref(), &path, tok).await;
        if read.is_err()
            && matches!(token_change, TokenChange::Set(_))
            && let Some(r) = &new_token_ref
        {
            discard_token(state, id, r).await;
        }
        Some(read?)
    } else {
        None
    };
    let result = async {
        let mut tx = audit::begin(state).await?;
        if let Some(iv) = &req.update_interval {
            repo::update_interval(&mut tx, id, iv, next_in_secs(iv, id)).await?;
        }
        if let Some(read) = &read {
            let mbytes = read.manifest.as_deref().ok_or_else(manifest_missing)?;
            let m = parse(mbytes)?;
            repo::update_address(
                &mut tx,
                id,
                &repo::NewAddress {
                    reference: reference.as_deref(),
                    path: &path,
                    token_ref: new_token_ref.as_deref(),
                    packages_url: &m.packages.url,
                    manifest: &stored_manifest(&m),
                },
            )
            .await?;
        } else if token_touched {
            repo::update_token(&mut tx, id, new_token_ref.as_deref()).await?;
        }
        let mut meta = json!({});
        if let Some(iv) = &req.update_interval {
            meta["interval"] = json!(format!("{} → {}", row.update_interval, iv));
        }
        if req.reference.is_some() {
            meta["ref"] = json!(reference);
        }
        if req.path.is_some() {
            meta["path"] = json!(path);
        }
        if token_touched {
            meta["token"] = json!(match &token_change {
                TokenChange::Remove => "removed",
                TokenChange::Set(_) if row.token_ref.is_some() => "replaced",
                TokenChange::Set(_) => "set",
                TokenChange::Keep => "unchanged",
            });
        }
        let ev = AuditEvent::new(
            actor.clone(),
            "agent_template_source.update",
            Outcome::Success,
        )
        .target("agent_template_source", id)
        .metadata(meta);
        audit::commit_critical(tx, &ev).await?;
        Ok::<_, ApiError>(())
    }
    .await;
    if result.is_err()
        && matches!(token_change, TokenChange::Set(_))
        && let Some(r) = &new_token_ref
    {
        discard_token(state, id, r).await;
    }
    result?;
    // The replaced or removed old token is discarded best effort after the write.
    if token_touched && let Some(old) = &row.token_ref {
        discard_token(state, id, old).await;
    }
    repo::get_source(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Template source not found."))
}

/// Remove a source (C-1107): always allowed; its templates and versions are deleted with it,
/// the organizations' switches stay (they are keyed by the template id). Agents made from the
/// templates do not change: they are copies.
pub async fn remove(state: &AppState, actor: &AuditActor, id: Uuid) -> Result<(), ApiError> {
    let row = repo::get_source(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Template source not found."))?;
    let count = repo::templates_of(&state.pool, id).await?.len();
    let mut tx = audit::begin(state).await?;
    repo::delete_source(&mut tx, id).await?;
    let ev = AuditEvent::new(
        actor.clone(),
        "agent_template_source.remove",
        Outcome::Success,
    )
    .target("agent_template_source", id)
    .metadata(json!({"url": row.url, "templates": count}));
    audit::commit_critical(tx, &ev).await?;
    if let Some(r) = &row.token_ref {
        discard_token(state, id, r).await;
    }
    Ok(())
}

// ---- the check routes' service (C-1108) -------------------------------------------------------

/// The route's view of a check outcome.
fn result_dto(source_id: Uuid, out: check::CheckOutcome) -> dto::TemplateSourceCheckResult {
    dto::TemplateSourceCheckResult {
        source_id,
        status: out.status.to_owned(),
        commit_sha: out.commit_sha,
        ingested: out
            .ingested
            .into_iter()
            .map(|i| dto::CheckIngestedView {
                template_id: i.template_id,
                version: i.version,
                status: i.status,
                status_reason: i.status_reason,
            })
            .collect(),
        waiting: out
            .waiting
            .into_iter()
            .map(|w| dto::CheckWaitingView {
                template_id: w.template_id,
                version: w.version,
                since: w.since,
                last_reason: w.last_reason,
            })
            .collect(),
        problems: out
            .problems
            .into_iter()
            .map(|p| dto::CheckProblemView {
                template_id: p.template_id,
                dir: p.dir,
                code: p.code,
                message: p.message,
            })
            .collect(),
        check_failed_reason: out.check_failed_reason,
    }
}

/// Manual check of one source (C-1107, C-596's lease rules): the same path under the same
/// lease as the poll, synchronous. 409 `template_check_in_progress` when it is busy.
pub async fn check_now(
    state: &AppState,
    actor: &AuditActor,
    id: Uuid,
) -> Result<dto::TemplateSourceCheckResult, ApiError> {
    if repo::get_source(&state.pool, id).await?.is_none() {
        return Err(ApiError::not_found("Template source not found."));
    }
    let owner = lease_owner();
    let mut c = state
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    let source = repo::claim_one(&mut c, id, owner, limits::TEMPLATE_LEASE.as_secs_f64())
        .await?
        .ok_or_else(|| {
            ApiError::conflict(
                "template_check_in_progress",
                "A check of this source is already running.",
            )
        })?;
    drop(c);
    let out = match check::check_source(state, &source, actor).await {
        Ok(out) => out,
        // The check died without writing its outcome: write the failed outcome (which
        // releases the lease and applies the failure backoff) and answer with it.
        Err(e) => check::fail(state, &source, e.message()).await?,
    };
    Ok(result_dto(id, out))
}

/// Manual check of every source (C-1107): sequential; a failing or busy source never blocks
/// the others.
pub async fn check_all(
    state: &AppState,
    actor: &AuditActor,
) -> Result<dto::TemplateSourceCheckAll, ApiError> {
    let mut items = Vec::new();
    for r in repo::list_sources(&state.pool).await? {
        let owner = lease_owner();
        let mut c = state
            .pool
            .acquire()
            .await
            .map_err(aurora_db::DbError::from)?;
        let claimed =
            repo::claim_one(&mut c, r.id, owner, limits::TEMPLATE_LEASE.as_secs_f64()).await?;
        drop(c);
        let Some(source) = claimed else {
            items.push(dto::TemplateSourceCheckResult {
                source_id: r.id,
                status: "failed".to_owned(),
                commit_sha: r.last_head_sha.clone().unwrap_or_default(),
                ingested: Vec::new(),
                waiting: Vec::new(),
                problems: Vec::new(),
                check_failed_reason: Some("busy".to_owned()),
            });
            continue;
        };
        let out = match check::check_source(state, &source, actor).await {
            Ok(out) => out,
            // The check died without writing its outcome: write the failed outcome (which
            // releases the lease and applies the failure backoff) and answer with it.
            Err(e) => check::fail(state, &source, e.message()).await?,
        };
        items.push(result_dto(r.id, out));
    }
    Ok(dto::TemplateSourceCheckAll { items })
}

// ---- the stored views -------------------------------------------------------------------------

/// The `AgentTemplateSource` of a row, from the stored rows (never a live read: the last known
/// state keeps serving when the source is unreachable).
pub async fn source_dto(state: &AppState, r: &SourceRow) -> Result<AgentTemplateSource, ApiError> {
    let templates = repo::templates_of(&state.pool, r.id).await?;
    let counted = repo::version_counts(&state.pool, r.id).await?;
    let has_versions: std::collections::BTreeSet<Uuid> = counted
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(t, _)| t)
        .collect();
    let repo_display = git_url::parse(&r.url)
        .map(|a| a.repo_display)
        .unwrap_or_default();
    let items = templates
        .into_iter()
        .map(|t| {
            let state_ = if t.head_state == "invalid" {
                "invalid"
            } else if has_versions.contains(&t.id) {
                "ingested"
            } else {
                "new"
            };
            SourceTemplateView {
                template_id: t.template_id,
                dir: t.dir,
                name: t.name,
                summary: t.summary,
                head_version: t.head_version,
                head_state: t.head_state,
                state: state_.to_owned(),
            }
        })
        .collect();
    Ok(AgentTemplateSource {
        id: r.id,
        name: r.name.clone(),
        url: r.url.clone(),
        repo: repo_display,
        reference: r.reference.clone(),
        path: r.path.clone(),
        token_set: r.token_ref.is_some(),
        token_updated_at: r.token_updated_at,
        homepage: r.homepage.clone(),
        contact: r.contact.clone(),
        packages_url: r.packages_url.clone(),
        update_interval: r.update_interval.clone(),
        status: r.status.clone(),
        check_failed_at: r.check_failed_at,
        check_failed_reason: r.check_failed_reason.clone(),
        last_checked_at: r.last_checked_at,
        last_head_sha: r.last_head_sha.clone(),
        templates: items,
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}
