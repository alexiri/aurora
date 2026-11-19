//! The Files domain's authority, session pool and operations (C-898..C-903, C-906; SEC-125/126;
//! D-316). The API is the SFTP client of the sandbox's `sftp-server` (docs/16 §W21): every
//! request re-checks authority, wakes a parked sandbox on the terminal's path, stamps the
//! `files` keep-awake reason and then runs SFTP requests over the person's pooled session.
//! File contents are never logged or audited; paths and sizes are (C-906).
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use futures::{StreamExt, stream};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::dto::{FileEntry, FileList, FileTarget, FileText};
use super::paths;
use super::sftp::{Attr, BLOCK, Fail, OpenError, PIPELINE, Sftp};
use crate::app::AppState;
use crate::audit::AuditEvent;
use crate::domains::conversations::service::load_agent;
use crate::http::auth::Caller;
use aurora_core::limits::{
    FILE_TRANSFERS_PER_USER, FILES_AWAKE_GRACE, FILES_DELETE_MAX, FILES_EDIT_MAX, FILES_GRANT_TTL,
    FILES_LIST_MAX, FILES_SESSION_IDLE, FILES_SESSION_LIFETIME, FILES_TRANSFER_IDLE,
    FILES_UPLOAD_MAX, TERMINAL_STREAM_WINDOW,
};
use aurora_core::{ApiError, token};
use aurora_db::DbError;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::file_grants as grants_repo;
use aurora_db::repo::runners as runners_repo;
use aurora_protocol::runner::{PreviewStreamOpenRequest, PreviewStreamOpenResponse};

/// How often an in-flight transfer re-checks that its holder still has access (C-901).
pub const ACCESS_RECHECK: Duration = Duration::from_secs(10);
/// At most one keep-awake stamp per sandbox per this long per replica (C-900).
const STAMP_THROTTLE: Duration = Duration::from_secs(30);

// ------------------------------------------------------------------------------- errors

fn err(status: StatusCode, code: &str, msg: &str) -> ApiError {
    ApiError::new(status, code, msg)
}

pub fn not_supported(reason: &str) -> ApiError {
    err(
        StatusCode::UNPROCESSABLE_ENTITY,
        "files_not_supported",
        "Files are not available for this agent.",
    )
    .with_details(json!({ "reason": reason }))
}

fn grant_invalid() -> ApiError {
    err(
        StatusCode::FORBIDDEN,
        "grant_invalid",
        "This stream grant is not valid.",
    )
}

fn not_current() -> ApiError {
    err(
        StatusCode::CONFLICT,
        "not_current",
        "The sandbox's files session is not current.",
    )
}

fn too_many_transfers() -> ApiError {
    err(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_transfers",
        "Too many file transfers are running; wait for one to finish.",
    )
}

pub fn unavailable() -> ApiError {
    err(
        StatusCode::SERVICE_UNAVAILABLE,
        "files_unavailable",
        "The sandbox's file service is unavailable; retry in a moment.",
    )
    .with_retry_after(2)
}

fn waking() -> ApiError {
    err(
        StatusCode::SERVICE_UNAVAILABLE,
        "sandbox_waking",
        "The sandbox is waking; retry in a moment.",
    )
    .with_retry_after(2)
}

pub fn path_not_found() -> ApiError {
    err(
        StatusCode::NOT_FOUND,
        "path_not_found",
        "No such file or directory.",
    )
}

fn permission_denied() -> ApiError {
    err(
        StatusCode::FORBIDDEN,
        "permission_denied",
        "The agent's user may not do that.",
    )
}

fn outside_workspace() -> ApiError {
    err(
        StatusCode::FORBIDDEN,
        "path_outside_workspace",
        "That path resolves outside /workspace.",
    )
}

fn read_only() -> ApiError {
    err(
        StatusCode::FORBIDDEN,
        "path_read_only",
        "That path is managed by the platform and is read-only.",
    )
}

fn op_failed() -> ApiError {
    err(
        StatusCode::UNPROCESSABLE_ENTITY,
        "files_operation_failed",
        "The sandbox refused the operation (for example, the disk is full).",
    )
}

fn exists() -> ApiError {
    err(
        StatusCode::CONFLICT,
        "file_exists",
        "That path already exists.",
    )
}

/// C-899's status mapping of a failed SFTP request.
pub fn fail(f: Fail) -> ApiError {
    match f {
        Fail::NotFound => path_not_found(),
        Fail::Denied => permission_denied(),
        Fail::Down => unavailable(),
        Fail::Failed | Fail::Eof => op_failed(),
    }
}

trait R<T> {
    fn f(self) -> Result<T, ApiError>;
}
impl<T> R<T> for Result<T, Fail> {
    fn f(self) -> Result<T, ApiError> {
        self.map_err(fail)
    }
}

// ------------------------------------------------------------------- authority (C-900)

/// The admission gate (C-900, SEC-125), in order: `terminal.enabled` (404), a dashboard session
/// (401), `can_use` (404), hosted (422 `byo`), not frozen (423). Used per request and by the
/// 10 s re-check of an in-flight transfer.
pub async fn admit(s: &AppState, c: &Caller, agent: Uuid) -> Result<AgentRow, ApiError> {
    // (FIX-168, SEC-125) The setting in force (it defaults from `terminal.available`, SET-5), read
    // live like the terminal does: never the boot snapshot.
    if !s.ops().terminal.enabled {
        return Err(crate::domains::terminal::service::terminal_disabled());
    }
    if !c.is_session() {
        return Err(ApiError::unauthenticated("Files need a dashboard session."));
    }
    let a = load_agent(s, c, agent).await?;
    if a.kind != "hosted" {
        return Err(not_supported("byo"));
    }
    crate::domains::freeze::service::ensure_agent_active_pool(s, a.id).await?;
    Ok(a)
}

/// An admitted request: the agent and its running sandbox's current boot.
pub struct Authed {
    pub agent: AgentRow,
    pub sandbox: Uuid,
    pub boot: Uuid,
    pub user: Uuid,
}

fn stamps() -> &'static Mutex<HashMap<Uuid, Instant>> {
    static M: OnceLock<Mutex<HashMap<Uuid, Instant>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// Keep-awake reason `files` (C-900): throttled to one stamp per sandbox per 30 s per replica.
pub async fn stamp_awake(s: &AppState, sandbox: Uuid) {
    {
        let mut g = stamps().lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if g.len() > 4096 {
            g.retain(|_, t| now.duration_since(*t) < STAMP_THROTTLE);
        }
        match g.get(&sandbox) {
            Some(t) if now.duration_since(*t) < STAMP_THROTTLE => return,
            _ => {
                g.insert(sandbox, now);
            }
        }
    }
    let _ = aurora_db::repo::sandboxes::keep_files_awake(
        &s.pool,
        sandbox,
        FILES_AWAKE_GRACE.as_secs() as i64,
    )
    .await;
}

/// Full per-request authority: [`admit`], then the sandbox (wake demand on a parked one, 503
/// `sandbox_waking` at once; 409 `sandbox_unavailable` when it cannot wake), then the stamp.
pub async fn authorize(s: &AppState, c: &Caller, agent: Uuid) -> Result<Authed, ApiError> {
    let a = admit(s, c, agent).await?;
    let row: Option<(Uuid, String, Option<Uuid>)> =
        sqlx::query_as("SELECT id, state, boot_id FROM sandboxes WHERE agent_id = $1")
            .bind(a.id)
            .fetch_optional(&s.pool)
            .await
            .map_err(DbError::from)?;
    let unavailable_state = |state: &str| {
        err(
            StatusCode::CONFLICT,
            "sandbox_unavailable",
            "The agent's sandbox cannot be started right now.",
        )
        .with_details(json!({ "state": state }))
    };
    let Some((sandbox, state, boot)) = row else {
        return Err(unavailable_state("unprovisioned"));
    };
    match state.as_str() {
        "running" => {}
        // Already coming up: nothing to demand.
        "starting" => return Err(waking()),
        "stopped" | "stopping" => {
            let granted = crate::domains::terminal::service::wake_demand(s, &a, sandbox).await?;
            if !granted {
                return Err(err(
                    StatusCode::TOO_MANY_REQUESTS,
                    "wake_limited",
                    "This sandbox has been woken too often this hour.",
                )
                .with_retry_after(60));
            }
            return Err(waking());
        }
        other => return Err(unavailable_state(other)),
    }
    let boot = boot.ok_or_else(|| unavailable_state("starting"))?;
    stamp_awake(s, sandbox).await;
    Ok(Authed {
        agent: a,
        sandbox,
        boot,
        user: c.user_id,
    })
}

// ---------------------------------------------------------------- transfer slots (C-900)

fn transfers() -> &'static Mutex<HashMap<Uuid, u32>> {
    static M: OnceLock<Mutex<HashMap<Uuid, u32>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// One of a person's `FILE_TRANSFERS_PER_USER` concurrent content transfers on this replica;
/// released by `Drop` on every exit path.
pub struct TransferSlot(Uuid);

impl TransferSlot {
    pub fn acquire(user: Uuid) -> Result<Self, ApiError> {
        let mut g = transfers().lock().unwrap_or_else(|e| e.into_inner());
        let n = g.entry(user).or_insert(0);
        if *n >= FILE_TRANSFERS_PER_USER {
            return Err(too_many_transfers());
        }
        *n += 1;
        Ok(Self(user))
    }
}

impl Drop for TransferSlot {
    fn drop(&mut self) {
        let mut g = transfers().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = g.get_mut(&self.0) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                g.remove(&self.0);
            }
        }
    }
}

/// What an in-flight transfer needs to re-check its holder's access every 10 s.
#[derive(Clone)]
pub struct Recheck {
    s: AppState,
    c: Caller,
    agent: Uuid,
    sandbox: Uuid,
}

impl Recheck {
    pub fn new(s: &AppState, c: &Caller, a: &Authed) -> Self {
        Self {
            s: s.clone(),
            c: c.clone(),
            agent: a.agent.id,
            sandbox: a.sandbox,
        }
    }

    /// `false` only on a definite refusal (a 4xx: lost access, frozen, feature off); an
    /// infrastructure error keeps the transfer. Also keeps the `files` stamp fresh.
    pub async fn still_allowed(&self) -> bool {
        if let crate::http::auth::Principal::Session { id } = self.c.principal {
            let live: Result<Option<bool>, _> = sqlx::query_scalar(
                "SELECT (s.revoked_at IS NULL AND s.expires_at > now() AND u.disabled_at IS NULL) \
                 FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.id = $1",
            )
            .bind(id)
            .fetch_optional(&self.s.pool)
            .await
            .map(Option::flatten);
            if matches!(live, Ok(Some(false) | None)) {
                return false;
            }
        }
        match admit(&self.s, &self.c, self.agent).await {
            Ok(_) => {
                stamp_awake(&self.s, self.sandbox).await;
                true
            }
            Err(e) => !e.status().is_client_error(),
        }
    }
}

// ------------------------------------------------------------------ session pool (C-901)

type Slot = Arc<tokio::sync::Mutex<Option<Arc<Sftp>>>>;

fn pool() -> &'static Mutex<HashMap<(Uuid, Uuid), Slot>> {
    static P: OnceLock<Mutex<HashMap<(Uuid, Uuid), Slot>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

fn open_error(e: OpenError) -> ApiError {
    match e {
        OpenError::RunnerRefused => not_supported("runner"),
        OpenError::ImageRefused => not_supported("image"),
        OpenError::Limit => too_many_transfers(),
        OpenError::Unavailable => unavailable(),
    }
}

/// The person's pooled SFTP session for the agent: reused while its leg is open (the leg task
/// ends it after `FILES_SESSION_IDLE` without a request or at `FILES_SESSION_LIFETIME`), else
/// a new `auf_` grant is minted, redeemed by the runner and the session reopened.
pub async fn session(s: &AppState, a: &Authed) -> Result<Arc<Sftp>, ApiError> {
    let key = (a.user, a.agent.id);
    let slot: Slot = {
        let mut g = pool().lock().unwrap_or_else(|e| e.into_inner());
        if g.len() > 2048 {
            g.retain(|_, v| Arc::strong_count(v) > 1);
        }
        g.entry(key).or_default().clone()
    };
    let mut guard = slot.lock().await;
    if let Some(sftp) = guard.as_ref().filter(|x| !x.is_closed()) {
        return Ok(sftp.clone());
    }
    *guard = None;
    let sftp = open_session(s, a).await?;
    *guard = Some(sftp.clone());
    Ok(sftp)
}

async fn open_session(s: &AppState, a: &Authed) -> Result<Arc<Sftp>, ApiError> {
    // The runner must advertise the `files` capability (an older runner: reason `runner`).
    let runner: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT runner_id FROM sandboxes WHERE id = $1 AND state = 'running'")
            .bind(a.sandbox)
            .fetch_optional(&s.pool)
            .await
            .map_err(DbError::from)?;
    let Some(Some(runner_id)) = runner else {
        return Err(unavailable());
    };
    let r = runners_repo::get(&s.pool, runner_id)
        .await?
        .ok_or_else(unavailable)?;
    let files_cap = crate::domains::runner_plane::work::caps_of(&r)
        .map(|c| c.files)
        .unwrap_or(false);
    if !files_cap {
        return Err(not_supported("runner"));
    }
    let endpoint = crate::domains::terminal::service::dial_endpoint(s, a.sandbox).await?;
    let grant = token::generate(token::TokenPrefix::FilesStream);
    let hash = hex::encode(token::hash(grant.expose()));
    let expires_at = chrono::Utc::now()
        + chrono::Duration::seconds(i64::try_from(FILES_GRANT_TTL.as_secs()).unwrap_or(30));
    grants_repo::insert_file_grant(
        &s.pool,
        Uuid::now_v7(),
        a.agent.id,
        a.sandbox,
        a.boot,
        a.user,
        &hash,
        expires_at,
    )
    .await?;
    Sftp::open(&endpoint.url, grant.expose())
        .await
        .map_err(open_error)
}

// ----------------------------------------------------------- redemption (runner plane)

/// `POST /v1/runner/preview-streams/open` with `kind: "files"` (C-901, `16 §W21`): consume the
/// single-use `auf_` grant (CAS by hash) and re-validate its binding: instance terminal on,
/// boot current, sandbox running, agent hosted and not frozen. The answer carries no port,
/// share, shell or models. A grant of any other prefix is never in this table: 403.
pub(crate) async fn open(
    s: &AppState,
    _runner: &runners_repo::RunnerRow,
    req: PreviewStreamOpenRequest,
) -> Result<PreviewStreamOpenResponse, ApiError> {
    if !req
        .grant
        .expose()
        .starts_with(token::TokenPrefix::FilesStream.as_str())
    {
        return Err(grant_invalid());
    }
    let hash = hex::encode(token::hash(req.grant.expose()));
    let row = grants_repo::consume_file_grant(&s.pool, &hash)
        .await?
        .ok_or_else(grant_invalid)?;
    if !s.ops().terminal.enabled {
        return Err(not_current());
    }
    let cur: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT sb.boot_id, sb.channel_replica_id FROM sandboxes sb \
         WHERE sb.id = $1 AND sb.state = 'running'",
    )
    .bind(row.sandbox_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(DbError::from)?;
    let Some((Some(boot_id), Some(_))) = cur else {
        return Err(not_current());
    };
    if boot_id != row.boot_id {
        return Err(not_current());
    }
    let a = aurora_db::repo::agents::get(&s.pool, row.agent_id)
        .await?
        .ok_or_else(not_current)?;
    if a.kind != "hosted" {
        return Err(not_current());
    }
    // A frozen agent is 423 `frozen` on every route (C-1041), not `not_current`.
    crate::domains::freeze::service::ensure_agent_active_pool(s, a.id).await?;
    Ok(PreviewStreamOpenResponse {
        sandbox_id: row.sandbox_id,
        boot_id: row.boot_id,
        port: None,
        share_id: None,
        window: TERMINAL_STREAM_WINDOW as u32,
        idle_secs: FILES_SESSION_IDLE.as_secs() as u32,
        max_lifetime_secs: FILES_SESSION_LIFETIME.as_secs() as u32,
        shell: None,
        models: None,
    })
}

// ------------------------------------------------------------------------ path resolution

/// A path with U+FFFD cannot be addressed reliably (a lossy listing name): 422 `invalid_path`.
fn reject_lossy(path: &str) -> Result<(), ApiError> {
    if path.contains('\u{fffd}') {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_path",
            "That name is not valid UTF-8 and cannot be addressed.",
        ));
    }
    Ok(())
}

fn scope(resolved: &str) -> Result<(), ApiError> {
    if paths::in_scope(resolved) {
        Ok(())
    } else {
        Err(outside_workspace())
    }
}

/// C-899: the **resolved** path decides. The validated request path goes through SFTP
/// `realpath` (a missing final component resolves to itself), then the scope check.
async fn resolve(sftp: &Sftp, raw: &str) -> Result<String, ApiError> {
    let p = paths::validate(raw)?;
    reject_lossy(&p)?;
    let full = sftp.realpath(&p).await.f()?;
    scope(&full)?;
    Ok(full)
}

/// A target whose parent is missing resolves to 404 (`realpath` of `/a/missing/x` fails in
/// OpenSSH): mark it as the parent so the dashboard can say "the folder does not exist".
fn missing_parent(e: ApiError) -> ApiError {
    if e.code() == "path_not_found" {
        e.with_details(json!({ "missing": "parent" }))
    } else {
        e
    }
}

/// `mkdir -p`: resolve the deepest existing ancestor (scope-checked), then re-append the rest.
async fn resolve_deep(sftp: &Sftp, raw: &str) -> Result<String, ApiError> {
    let p = pre(raw)?;
    let mut rest: Vec<&str> = vec![];
    let mut cur = p.as_str();
    loop {
        match sftp.realpath(cur).await {
            Ok(base) => {
                scope(&base)?;
                let mut out = base;
                for c in rest.iter().rev() {
                    out = paths::join(&out, c);
                }
                return Ok(out);
            }
            Err(Fail::NotFound) => {
                let (parent, name) = paths::split(cur).ok_or_else(path_not_found)?;
                if cur == paths::ROOT {
                    return Err(path_not_found());
                }
                rest.push(name);
                cur = parent;
            }
            Err(f) => return Err(fail(f)),
        }
    }
}

/// For operations on a link itself (rename, delete): resolve the **parent**, keep the leaf.
async fn resolve_leaf(sftp: &Sftp, raw: &str) -> Result<String, ApiError> {
    let p = paths::validate(raw)?;
    reject_lossy(&p)?;
    let Some((parent, name)) = paths::split(&p) else {
        return Err(paths::invalid_path("no parent"));
    };
    if p == paths::ROOT {
        return Ok(p);
    }
    let parent = sftp.realpath(parent).await.f()?;
    scope(&parent)?;
    Ok(paths::join(&parent, name))
}

fn writable_path(resolved: &str) -> Result<(), ApiError> {
    if paths::is_aurora(resolved) || resolved == paths::ROOT {
        return Err(read_only());
    }
    Ok(())
}

// ------------------------------------------------------------------------------ entries

fn mode_string(m: u32) -> String {
    format!("{m:04o}")
}

fn time_of(mtime: Option<i64>) -> Option<chrono::DateTime<chrono::Utc>> {
    mtime.and_then(|t| chrono::DateTime::from_timestamp(t, 0))
}

async fn target_of(sftp: &Sftp, link: &str) -> FileTarget {
    let resolved = match sftp.realpath(link).await {
        Ok(p) => p,
        Err(_) => {
            // The link could not be resolved at all: report it as missing, outside unknown.
            return FileTarget {
                path: link.to_owned(),
                kind: "missing".into(),
                outside_workspace: false,
            };
        }
    };
    let kind = match sftp.stat(&resolved).await {
        Ok(a) => match a.kind {
            "file" => "file",
            "dir" => "dir",
            _ => "other",
        },
        Err(_) => "missing",
    };
    FileTarget {
        outside_workspace: !paths::in_scope(&resolved),
        path: resolved,
        kind: kind.into(),
    }
}

async fn entry_of(sftp: &Sftp, dir: &str, name: &str, a: &Attr) -> FileEntry {
    let path = if name.is_empty() {
        dir.to_owned()
    } else {
        paths::join(dir, name)
    };
    let lossy = name.contains('\u{fffd}');
    let target = if a.kind == "symlink" && !lossy {
        Some(target_of(sftp, &path).await)
    } else {
        None
    };
    FileEntry {
        name: if name.is_empty() {
            path.rsplit('/').next().unwrap_or("").to_owned()
        } else {
            name.to_owned()
        },
        writable: a.owner_write() && !paths::is_aurora(&path) && path != paths::ROOT && !lossy,
        path,
        kind: a.kind.into(),
        size: a.size,
        modified_at: time_of(a.mtime),
        mode: mode_string(a.mode),
        target,
    }
}

fn etag_of(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

// ---------------------------------------------------------------------------------- audit

#[allow(clippy::too_many_arguments)]
fn audit(s: &AppState, c: &Caller, a: &AgentRow, action: &str, meta: serde_json::Value) {
    let mut m = meta;
    if let Some(o) = m.as_object_mut() {
        o.insert("agent".into(), json!(a.id));
        o.insert("workspace".into(), json!(a.workspace_id));
    }
    crate::audit::record(
        s,
        AuditEvent::by(c, action)
            .org(a.org_id)
            .workspace(a.workspace_id)
            .target("agent", a.id)
            .metadata(m),
    );
}

/// Paths in audit metadata are cut at 1,024 bytes on a char boundary (C-906).
fn clip(p: &str) -> String {
    p[..p.floor_char_boundary(1024)].to_owned()
}

// ------------------------------------------------------------------------------ operations

/// Lexical validation (C-899) — always before the first SFTP request, so a 400 never opens a
/// session, let alone sends a request.
fn pre(path: &str) -> Result<String, ApiError> {
    let p = paths::validate(path)?;
    reject_lossy(&p)?;
    Ok(p)
}

pub async fn list(s: &AppState, c: &Caller, agent: Uuid, path: &str) -> Result<FileList, ApiError> {
    let path = pre(path)?;
    let path = path.as_str();
    let a = authorize(s, c, agent).await?;
    let sftp = session(s, &a).await?;
    let full = resolve(&sftp, path).await?;
    let st = sftp.stat(&full).await.f()?;
    if st.kind != "dir" {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "not_a_directory",
            "That path is not a directory.",
        ));
    }
    let (mut raw, truncated) = sftp.list(&full, FILES_LIST_MAX).await.f()?;
    raw.sort_by(|(an, aa), (bn, ba)| {
        let (ad, bd) = (aa.kind == "dir", ba.kind == "dir");
        bd.cmp(&ad).then_with(|| an.as_bytes().cmp(bn.as_bytes()))
    });
    let mut entries = Vec::with_capacity(raw.len());
    for (name, attr) in &raw {
        entries.push(entry_of(&sftp, &full, name, attr).await);
    }
    Ok(FileList {
        path: full,
        entries,
        truncated,
    })
}

/// A readable regular file: resolves, scope-checks and stats it (404 / 422 `is_directory`).
async fn open_regular(sftp: &Sftp, path: &str) -> Result<(String, Attr), ApiError> {
    let full = resolve(sftp, path).await?;
    let st = sftp.stat(&full).await.f()?;
    if st.kind == "dir" {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "is_directory",
            "That path is a directory.",
        ));
    }
    Ok((full, st))
}

/// Read a whole file of at most `max` bytes (the caller checked the size).
async fn read_all(sftp: &Sftp, full: &str, size: u64) -> Result<Vec<u8>, Fail> {
    let h = sftp.open_read(full).await?;
    let mut out = Vec::with_capacity(size as usize);
    let r = async {
        let mut off = 0u64;
        while off < size {
            let want = BLOCK.min((size - off) as usize) as u32;
            let d = sftp.read_at(&h, off, want).await?;
            if d.is_empty() {
                break;
            }
            off += d.len() as u64;
            out.extend_from_slice(&d);
        }
        Ok(())
    }
    .await;
    sftp.close_handle(&h).await;
    r.map(|()| out)
}

pub async fn text(s: &AppState, c: &Caller, agent: Uuid, path: &str) -> Result<FileText, ApiError> {
    let path = pre(path)?;
    let path = path.as_str();
    let a = authorize(s, c, agent).await?;
    let sftp = session(s, &a).await?;
    let (full, st) = open_regular(&sftp, path).await?;
    let not_editable = |reason: &str| {
        err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "file_not_editable",
            "That file cannot be edited as text.",
        )
        .with_details(json!({ "reason": reason }))
    };
    if st.kind != "file" {
        return Err(not_editable("binary"));
    }
    if st.size > FILES_EDIT_MAX {
        return Err(not_editable("too_large"));
    }
    let bytes = read_all(&sftp, &full, st.size).await.f()?;
    if bytes.contains(&0) {
        return Err(not_editable("binary"));
    }
    let content = String::from_utf8(bytes.clone()).map_err(|_| not_editable("not_utf8"))?;
    let line_ending = match content.find('\n') {
        Some(i) if i > 0 && content.as_bytes()[i - 1] == b'\r' => "crlf",
        _ => "lf",
    };
    audit(
        s,
        c,
        &a.agent,
        "agent.files.download",
        json!({ "path": clip(&full), "bytes": bytes.len(), "via": "editor" }),
    );
    Ok(FileText {
        path: full,
        etag: etag_of(&bytes),
        size: bytes.len() as u64,
        modified_at: time_of(st.mtime),
        line_ending: line_ending.into(),
        content,
    })
}

/// What the content GET needs to stream: the headers' inputs and the body.
pub struct Download {
    pub name: String,
    pub size: u64,
    pub body: axum::body::Body,
}

pub async fn download(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    path: &str,
) -> Result<Download, ApiError> {
    let path = pre(path)?;
    let path = path.as_str();
    let a = authorize(s, c, agent).await?;
    let slot = TransferSlot::acquire(a.user)?;
    let sftp = session(s, &a).await?;
    let (full, st) = open_regular(&sftp, path).await?;
    if st.kind != "file" {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "not_a_file",
            "That path is not a regular file.",
        ));
    }
    let h = sftp.open_read(&full).await.f()?;
    // Content-Length is the stat's size; the stream is cut there and ends with an error if the
    // file shrinks meanwhile (never padded).
    let size = sftp.fstat(&h).await.f().map(|a| a.size).unwrap_or(st.size);
    audit(
        s,
        c,
        &a.agent,
        "agent.files.download",
        json!({ "path": clip(&full), "bytes": size, "via": "download" }),
    );
    let recheck = Recheck::new(s, c, &a);
    let offsets: Vec<u64> = (0..size.div_ceil(BLOCK as u64))
        .map(|i| i * BLOCK as u64)
        .collect();
    let reader = sftp.clone();
    let rh = h.clone();
    let chunks = stream::iter(offsets)
        .map(move |off| {
            let reader = reader.clone();
            let h = rh.clone();
            async move {
                let want = BLOCK.min((size - off) as usize);
                let d = reader.read_at(&h, off, want as u32).await;
                match d {
                    Ok(d) if d.len() == want => Ok(axum::body::Bytes::from(d)),
                    Ok(_) => Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "file shrank during download",
                    )),
                    Err(_) => Err(std::io::Error::other("file read failed")),
                }
            }
        })
        .buffered(PIPELINE);
    let last_check = Arc::new(Mutex::new(Instant::now()));
    let checked = chunks.then(move |item| {
        let recheck = recheck.clone();
        let last_check = last_check.clone();
        async move {
            let due = {
                let mut g = last_check.lock().unwrap_or_else(|e| e.into_inner());
                if g.elapsed() >= ACCESS_RECHECK {
                    *g = Instant::now();
                    true
                } else {
                    false
                }
            };
            if due && !recheck.still_allowed().await {
                return Err(std::io::Error::other("access lost"));
            }
            item
        }
    });
    let closer = sftp.clone();
    let closing = stream::once(async move {
        closer.close_handle(&h).await;
        None
    });
    let body_stream = checked
        .map(Some)
        .chain(closing)
        .filter_map(|x| async move { x })
        .map(move |x| {
            let _keep = &slot;
            x
        });
    let name = full.rsplit('/').next().unwrap_or("download").to_owned();
    Ok(Download {
        name,
        size,
        body: axum::body::Body::from_stream(body_stream),
    })
}

/// Preconditions of the content PUT (at most one).
pub enum Precondition {
    None,
    /// `If-None-Match: *`
    Create,
    /// `If-Match: "<etag>"`
    Etag(String),
}

pub struct Written {
    pub entry: FileEntry,
    pub etag: Option<String>,
    pub created: bool,
}

fn temp_name(name: &str) -> Result<String, ApiError> {
    // 16 hex characters: the random tail of a v7 uuid.
    let u = Uuid::now_v7().simple().to_string();
    let t = format!(".{name}.aurora-upload-{}", &u[u.len() - 16..]);
    if t.len() > 255 {
        return Err(paths::invalid_path(
            "the name is too long to upload atomically",
        ));
    }
    Ok(t)
}

fn mismatch(current: Option<String>) -> ApiError {
    let e = err(
        StatusCode::CONFLICT,
        "file_changed",
        "The file changed since you opened it.",
    );
    match current {
        Some(t) => e.with_details(json!({ "current_etag": t })),
        None => e,
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn upload(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    path: &str,
    length: Option<u64>,
    pre: Precondition,
    mut body: axum::body::Body,
) -> Result<Written, ApiError> {
    use http_body_util::BodyExt;
    let path = self::pre(path)?;
    let path = path.as_str();
    let a = authorize(s, c, agent).await?;
    let length = length.ok_or_else(|| {
        err(
            StatusCode::LENGTH_REQUIRED,
            "length_required",
            "A Content-Length header is required.",
        )
    })?;
    // 413 before any byte is written (and before any SFTP call).
    if length > FILES_UPLOAD_MAX {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "file_too_large",
            "That file is larger than the upload limit.",
        ));
    }
    let _slot = TransferSlot::acquire(a.user)?;
    let sftp = session(s, &a).await?;
    let full = resolve(&sftp, path).await.map_err(missing_parent)?;
    writable_path(&full)?;
    let existing = match sftp.stat(&full).await {
        Ok(st) => Some(st),
        Err(Fail::NotFound) => None,
        Err(f) => return Err(fail(f)),
    };
    let (parent, name) = paths::split(&full).ok_or_else(|| paths::invalid_path("no parent"))?;
    let (parent, name) = (parent.to_owned(), name.to_owned());
    let mode = match &existing {
        None => {
            if matches!(pre, Precondition::Etag(_)) {
                return Err(mismatch(None));
            }
            match sftp.stat(&parent).await {
                Ok(p) if p.kind == "dir" => {}
                Ok(_) | Err(Fail::NotFound) => {
                    return Err(path_not_found().with_details(json!({ "missing": "parent" })));
                }
                Err(f) => return Err(fail(f)),
            }
            0o644
        }
        Some(st) => {
            if st.kind == "dir" {
                return Err(err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "is_directory",
                    "That path is a directory.",
                ));
            }
            match &pre {
                Precondition::Create => return Err(exists()),
                Precondition::Etag(want) => {
                    let current = if st.kind == "file" && st.size <= FILES_EDIT_MAX {
                        read_all(&sftp, &full, st.size)
                            .await
                            .ok()
                            .map(|b| etag_of(&b))
                    } else {
                        None
                    };
                    if current.as_deref() != Some(want.as_str()) {
                        return Err(mismatch(current));
                    }
                }
                Precondition::None => {}
            }
            // The temp-file rename would bypass the owner write bit (SEC-125): refuse here.
            if !st.owner_write() {
                return Err(permission_denied());
            }
            st.mode
        }
    };
    let temp = paths::join(&parent, &temp_name(&name)?);
    let h = sftp.create_excl(&temp, 0o600).await.f()?;
    let recheck = Recheck::new(s, c, &a);
    let outcome: Result<(u64, Option<String>), ApiError> = async {
        let mut hasher = Sha256::new();
        let keep_etag = length <= FILES_EDIT_MAX;
        let mut offset = 0u64;
        let mut pending: Vec<u8> = Vec::with_capacity(BLOCK);
        let mut inflight = futures::stream::FuturesOrdered::new();
        let mut last_check = Instant::now();
        let flush = |inflight: &mut futures::stream::FuturesOrdered<_>, block: Vec<u8>, at: u64| {
            let sftp = sftp.clone();
            let h = h.clone();
            inflight.push_back(async move { sftp.write_at(&h, at, block).await });
        };
        loop {
            let frame = match tokio::time::timeout(FILES_TRANSFER_IDLE, body.frame()).await {
                Err(_) => return Err(unavailable()),
                Ok(f) => f,
            };
            let Some(frame) = frame else { break };
            let frame = frame.map_err(|_| {
                err(
                    StatusCode::BAD_REQUEST,
                    "upload_aborted",
                    "The upload body ended early.",
                )
            })?;
            let Ok(data) = frame.into_data() else {
                continue;
            };
            if offset + pending.len() as u64 + data.len() as u64 > length {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "upload_aborted",
                    "The body is longer than its Content-Length.",
                ));
            }
            if keep_etag {
                hasher.update(&data);
            }
            let mut rest: &[u8] = &data;
            while !rest.is_empty() {
                let take = (BLOCK - pending.len()).min(rest.len());
                pending.extend_from_slice(&rest[..take]);
                rest = &rest[take..];
                if pending.len() == BLOCK {
                    let block = std::mem::replace(&mut pending, Vec::with_capacity(BLOCK));
                    flush(&mut inflight, block, offset);
                    offset += BLOCK as u64;
                    if inflight.len() >= PIPELINE
                        && let Some(r) = inflight.next().await
                    {
                        r.f()?;
                    }
                }
            }
            if last_check.elapsed() >= ACCESS_RECHECK {
                last_check = Instant::now();
                if !recheck.still_allowed().await {
                    return Err(err(
                        StatusCode::FORBIDDEN,
                        "forbidden",
                        "Access to this agent was lost.",
                    ));
                }
            }
        }
        if !pending.is_empty() {
            let n = pending.len() as u64;
            flush(&mut inflight, pending, offset);
            offset += n;
        }
        while let Some(r) = inflight.next().await {
            r.f()?;
        }
        if offset != length {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "upload_aborted",
                "The body is shorter than its Content-Length.",
            ));
        }
        sftp.fsetstat_mode(&h, mode & 0o7777).await.f()?;
        Ok((
            offset,
            keep_etag.then(|| format!("sha256:{}", hex::encode(hasher.finalize()))),
        ))
    }
    .await;
    sftp.close_handle(&h).await;
    let (bytes, etag) = match outcome {
        Ok(v) => v,
        Err(e) => {
            let _ = sftp.remove(&temp).await;
            return Err(e);
        }
    };
    if let Err(f) = sftp.posix_rename(&temp, &full).await {
        let _ = sftp.remove(&temp).await;
        return Err(fail(f));
    }
    let created = existing.is_none();
    let st = sftp.lstat(&full).await.f()?;
    audit(
        s,
        c,
        &a.agent,
        "agent.files.write",
        json!({ "path": clip(&full), "bytes": bytes, "created": created }),
    );
    Ok(Written {
        entry: entry_of(&sftp, &parent, &name, &st).await,
        etag,
        created,
    })
}

pub async fn mkdir(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    path: &str,
    parents: bool,
) -> Result<(FileEntry, bool), ApiError> {
    let path = pre(path)?;
    let path = path.as_str();
    let a = authorize(s, c, agent).await?;
    let sftp = session(s, &a).await?;
    let full = if parents {
        resolve_deep(&sftp, path).await?
    } else {
        resolve(&sftp, path).await.map_err(missing_parent)?
    };
    writable_path(&full)?;
    match sftp.stat(&full).await {
        Ok(st) => {
            if parents && st.kind == "dir" {
                let (p, n) = paths::split(&full).ok_or_else(|| paths::invalid_path("no parent"))?;
                return Ok((entry_of(&sftp, p, n, &st).await, false));
            }
            return Err(exists());
        }
        Err(Fail::NotFound) => {}
        Err(f) => return Err(fail(f)),
    }
    let (parent, name) = paths::split(&full).ok_or_else(|| paths::invalid_path("no parent"))?;
    if parents {
        // `mkdir -p`: create every missing ancestor below /workspace, shallowest first.
        let mut missing = vec![];
        let mut cur = parent.to_owned();
        while sftp.stat(&cur).await.is_err() && paths::in_scope(&cur) && cur != paths::ROOT {
            missing.push(cur.clone());
            cur = paths::split(&cur)
                .map(|(p, _)| p.to_owned())
                .unwrap_or_default();
        }
        for d in missing.into_iter().rev() {
            writable_path(&d)?;
            sftp.mkdir(&d, 0o755).await.f()?;
        }
    } else {
        match sftp.stat(parent).await {
            Ok(p) if p.kind == "dir" => {}
            Ok(_) | Err(Fail::NotFound) => {
                return Err(path_not_found().with_details(json!({ "missing": "parent" })));
            }
            Err(f) => return Err(fail(f)),
        }
    }
    sftp.mkdir(&full, 0o755).await.f()?;
    let st = sftp.lstat(&full).await.f()?;
    audit(
        s,
        c,
        &a.agent,
        "agent.files.mkdir",
        json!({ "path": clip(&full) }),
    );
    Ok((entry_of(&sftp, parent, name, &st).await, true))
}

pub async fn rename(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    from: &str,
    to: &str,
) -> Result<FileEntry, ApiError> {
    let (from, to) = (pre(from)?, pre(to)?);
    let (from, to) = (from.as_str(), to.as_str());
    let a = authorize(s, c, agent).await?;
    let sftp = session(s, &a).await?;
    let from_full = resolve_leaf(&sftp, from).await?;
    let to_full = resolve_leaf(&sftp, to).await?;
    writable_path(&from_full)?;
    writable_path(&to_full)?;
    if paths::is_within(&to_full, &from_full) {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_path",
            "A folder cannot be moved into itself.",
        ));
    }
    sftp.lstat(&from_full).await.f()?;
    match sftp.lstat(&to_full).await {
        Ok(_) => return Err(exists()),
        Err(Fail::NotFound) => {}
        Err(f) => return Err(fail(f)),
    }
    match paths::split(&to_full).map(|(p, _)| sftp.stat(p)) {
        Some(fut) => match fut.await {
            Ok(p) if p.kind == "dir" => {}
            Ok(_) | Err(Fail::NotFound) => {
                return Err(path_not_found().with_details(json!({ "missing": "parent" })));
            }
            Err(f) => return Err(fail(f)),
        },
        None => return Err(paths::invalid_path("no parent")),
    }
    sftp.posix_rename(&from_full, &to_full).await.f()?;
    let st = sftp.lstat(&to_full).await.f()?;
    audit(
        s,
        c,
        &a.agent,
        "agent.files.rename",
        json!({ "from": clip(&from_full), "to": clip(&to_full) }),
    );
    let (p, n) = paths::split(&to_full).ok_or_else(|| paths::invalid_path("no parent"))?;
    Ok(entry_of(&sftp, p, n, &st).await)
}

pub async fn delete(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    path: &str,
    recursive: bool,
) -> Result<u64, ApiError> {
    let path = pre(path)?;
    let path = path.as_str();
    let a = authorize(s, c, agent).await?;
    let sftp = session(s, &a).await?;
    let full = resolve_leaf(&sftp, path).await?;
    writable_path(&full)?;
    let st = sftp.lstat(&full).await.f()?;
    let mut deleted = 0u64;
    if st.kind != "dir" {
        // A file or a link (removed as a link, never followed).
        sftp.remove(&full).await.f()?;
        deleted = 1;
    } else if !recursive {
        let (entries, _) = sftp.list(&full, 1).await.f()?;
        if !entries.is_empty() {
            return Err(err(
                StatusCode::CONFLICT,
                "directory_not_empty",
                "That directory is not empty.",
            ));
        }
        sftp.rmdir(&full).await.f()?;
        deleted = 1;
    } else {
        // Walk first with `lstat`-based listings (never following a link); refuse up front
        // beyond FILES_DELETE_MAX with nothing deleted.
        let mut order: Vec<(String, bool)> = vec![(full.clone(), true)];
        let mut stack = vec![full.clone()];
        while let Some(dir) = stack.pop() {
            let (entries, _) = sftp.list(&dir, FILES_DELETE_MAX + 1).await.f()?;
            for (name, at) in entries {
                let p = paths::join(&dir, &name);
                let is_dir = at.kind == "dir";
                order.push((p.clone(), is_dir));
                if order.len() > FILES_DELETE_MAX {
                    return Err(err(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "too_many_entries",
                        "That folder holds too many items to delete at once.",
                    ));
                }
                if is_dir {
                    stack.push(p);
                }
            }
        }
        for (p, is_dir) in order.into_iter().rev() {
            let r = if is_dir {
                sftp.rmdir(&p).await
            } else {
                sftp.remove(&p).await
            };
            if let Err(f) = r {
                return Err(fail(f).with_details(json!({ "deleted": deleted })));
            }
            deleted += 1;
        }
    }
    audit(
        s,
        c,
        &a.agent,
        "agent.files.delete",
        json!({ "path": clip(&full), "deleted": deleted }),
    );
    Ok(deleted)
}
