//! Preview shares service (phase 4, P4-PRV-1, D-144, D-145, SEC-80; docs/11 §Port previews).
//! Authority is `can_use` of the agent's workspace (session or API key); an agent credential is
//! never a `Caller` here (the extractors refuse `aug_` tokens) and the agent's own tools cannot
//! reach these routes (SEC-80). Every write is one transaction with its security-critical audit
//! row; the command text is never audited, only `start_command_set` and a hash prefix.
use super::dto::*;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::conversations::service::notify_work;
use crate::domains::freeze::service as freeze;
use crate::http::auth::Caller;
use aurora_core::limits::{PREVIEW_WAKE_WAIT, PREVIEW_WAKE_WINDOW};
use aurora_core::{ApiError, FieldError, token};
use aurora_db::DbError;
use aurora_db::repo::agents as agents_repo;
use aurora_db::repo::preview_shares as repo;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid request.", vec![FieldError::new(path, msg)])
}

pub fn previews_disabled() -> ApiError {
    ApiError::not_found("Port previews are not enabled on this instance.")
}

pub fn share_not_found() -> ApiError {
    ApiError::not_found("Preview share not found.")
}

pub fn not_enabled_for_org() -> ApiError {
    ApiError::conflict(
        "previews_not_enabled_for_org",
        "Port previews are switched off for this organization.",
    )
}

fn exists() -> ApiError {
    ApiError::conflict(
        "preview_exists",
        "A preview share for this port already exists.",
    )
}

fn limit_reached() -> ApiError {
    ApiError::conflict("preview_limit", "The preview share limit has been reached.")
}

fn not_supported() -> ApiError {
    ApiError::unprocessable(
        "preview_not_supported",
        "Only hosted agents have a sandbox with preview ports.",
    )
}

fn port_forbidden() -> ApiError {
    ApiError::unprocessable("preview_port_forbidden", "This port cannot be shared.")
}

fn wake_limited() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "preview_wake_limited",
        "This preview has been woken too often; try again later.",
    )
}

fn paused() -> ApiError {
    ApiError::conflict("preview_paused", "This preview share is paused.")
}

fn no_start_command() -> ApiError {
    ApiError::conflict(
        "preview_no_start_command",
        "This share has no start command; start the app yourself and share the port.",
    )
}

/// The instance switch (`previews.enabled`): 404 on every route while off (`GET /v1/instance`
/// reports the same flag so the dashboard hides the tab).
pub fn ensure_enabled(s: &AppState) -> Result<(), ApiError> {
    if s.config.previews.enabled {
        Ok(())
    } else {
        Err(previews_disabled())
    }
}

/// The org switch: 409 `previews_not_enabled_for_org` on create/start while off (existing shares
/// stay but are inert).
async fn ensure_org_enabled(conn: &mut PgConnection, org: Uuid) -> Result<(), ApiError> {
    let on: bool = sqlx::query_scalar(
        "SELECT coalesce((settings->'previews_enabled')::bool, false) FROM organizations WHERE id = $1",
    )
    .bind(org)
    .fetch_one(conn)
    .await
    .map_err(DbError::from)?;
    if on {
        Ok(())
    } else {
        Err(not_enabled_for_org())
    }
}

pub fn event(c: &Caller, action: &str) -> AuditEvent {
    AuditEvent::new(AuditActor::from_caller(c), action, Outcome::Success)
}

/// The command text is never audited (SEC-80): only that it is set and the first 12 hex
/// characters of its SHA-256.
fn hex_prefix(cmd: &str) -> String {
    let h = token::hash(cmd);
    let mut s = String::with_capacity(12);
    for b in h.iter().take(6) {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Trimmed command, one line, at most `PREVIEW_START_COMMAND_MAX` characters.
pub fn validate_command(raw: &str) -> Result<String, ApiError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(bad("/start_command", "must not be empty"));
    }
    if t.len() > repo::START_COMMAND_MAX {
        return Err(bad("/start_command", "must be at most 1024 characters"));
    }
    if t.chars().any(|c| c == '\n' || c == '\r') {
        return Err(bad("/start_command", "must be a single line"));
    }
    if t.chars().any(char::is_control) {
        return Err(bad("/start_command", "must not contain control characters"));
    }
    Ok(t.to_owned())
}

/// Relative to `/workspace`, no `..`, at most `PREVIEW_CWD_MAX` characters.
pub fn validate_cwd(raw: &str) -> Result<String, ApiError> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok("".to_owned());
    }
    if t.len() > repo::CWD_MAX {
        return Err(bad("/cwd", "must be at most 200 characters"));
    }
    if t.starts_with('/') {
        return Err(bad("/cwd", "must be relative to /workspace"));
    }
    if t.split('/').any(|seg| seg == "..") {
        return Err(bad("/cwd", "must not contain `..`"));
    }
    if t.chars().any(char::is_control) {
        return Err(bad("/cwd", "must not contain control characters"));
    }
    Ok(t.to_owned())
}

/// `active` | `paused`.
fn parse_state(s: &str) -> Result<&'static str, ApiError> {
    match s {
        "active" => Ok("active"),
        "paused" => Ok("paused"),
        _ => Err(bad("/state", "must be `active` or `paused`")),
    }
}

/// The agent of an `/agents/{agent}/previews` route, visible (`can_use`) to the caller. Agent
/// credentials never get this far (the `Caller` extractors refuse `aug_` tokens; SEC-80).
async fn load_agent(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
) -> Result<agents_repo::AgentRow, ApiError> {
    let a = agents_repo::get(&s.pool, agent)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    c.workspace(s, a.workspace_id).await?;
    Ok(a)
}

/// The share of a `/v1/previews/{id}` route: 404 unless the caller can use its workspace (key
/// confinement and the access law; no disclosure to outsiders).
pub async fn load_share(
    s: &AppState,
    c: &Caller,
    id: Uuid,
) -> Result<(repo::ShareRow, agents_repo::AgentRow), ApiError> {
    let Some(row) = repo::get(&s.pool, id).await? else {
        return Err(share_not_found());
    };
    let Some(a) = agents_repo::get(&s.pool, row.agent_id).await? else {
        return Err(share_not_found());
    };
    c.workspace(s, a.workspace_id).await.map_err(|e| {
        if e.status() == axum::http::StatusCode::FORBIDDEN {
            e
        } else {
            share_not_found()
        }
    })?;
    Ok((row, a))
}

/// The displayed status (docs/05): derived, never stored. `paused` wins, then `frozen`, then
/// `listening` (`port ∈ listening_ports` of the current boot), then `waking` while the wake
/// demand or the start request is younger than its window, else `not_listening`.
pub async fn status_of(conn: &mut PgConnection, row: &repo::ShareRow) -> Result<String, ApiError> {
    if row.state == "paused" {
        return Ok("paused".to_owned());
    }
    if let Some(e) = aurora_db::repo::freeze::effective_of_agent(conn, row.agent_id).await? {
        let _ = e;
        return Ok("frozen".to_owned());
    }
    /// Wake/start stamps and the current boot's listening ports of the share's sandbox.
    type PreviewStamps = (
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        Option<Vec<i32>>,
    );
    let stamps: Option<PreviewStamps> = sqlx::query_as(
        "SELECT s.preview_wake_at, s.preview_start_requested_at, s.listening_ports \
             FROM sandboxes s WHERE s.agent_id = $1",
    )
    .bind(row.agent_id)
    .fetch_optional(conn)
    .await
    .map_err(DbError::from)?;
    let Some((wake_at, start_requested, ports)) = stamps else {
        return Ok("not_listening".to_owned());
    };
    if ports.is_some_and(|p| p.contains(&row.port)) {
        return Ok("listening".to_owned());
    }
    let now = Utc::now();
    let wake_window =
        chrono::TimeDelta::try_seconds(PREVIEW_WAKE_WINDOW.as_secs() as i64).unwrap_or_default();
    let start_window =
        chrono::TimeDelta::try_seconds(PREVIEW_WAKE_WAIT.as_secs() as i64).unwrap_or_default();
    if wake_at.is_some_and(|t| now - t < wake_window)
        || start_requested.is_some_and(|t| now - t < start_window)
    {
        return Ok("waking".to_owned());
    }
    Ok("not_listening".to_owned())
}

/// Assemble the API shape of one share (`url` is `https://<label>.<previews.domain>/`).
pub async fn to_dto(
    s: &AppState,
    conn: &mut PgConnection,
    row: repo::ShareRow,
) -> Result<PreviewShare, ApiError> {
    let domain = s.config.previews.domain.clone().unwrap_or_default();
    let status = status_of(conn, &row).await?;
    Ok(PreviewShare {
        id: row.id,
        agent_id: row.agent_id,
        port: row.port,
        label: row.label.clone(),
        hostname: format!("{}.{}", row.label, domain),
        url: format!("https://{}.{}/", row.label, domain),
        visibility: PreviewVisibility::Private,
        state: match row.state.as_str() {
            "paused" => PreviewState::Paused,
            _ => PreviewState::Active,
        },
        status,
        start_command: row.start_command.clone(),
        cwd: row.cwd.clone(),
        last_start: row.last_start_at.map(|at| PreviewLastStart {
            at,
            exit_code: row.last_start_exit_code,
            tail: row.last_start_tail.clone(),
        }),
        last_accessed_at: row.last_accessed_at,
        created_by: PreviewCreatedBy {
            id: row.created_by_user_id,
            label: row.created_by_label.clone(),
        },
        created_at: row.created_at,
    })
}

/// List an agent's shares (all states; the dashboard's Previews tab).
pub async fn list(s: &AppState, c: &Caller, agent: Uuid) -> Result<PreviewShareList, ApiError> {
    ensure_enabled(s)?;
    let _a = load_agent(s, c, agent).await?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let rows = repo::list_for_agent(&mut *conn, agent, Default::default(), 500).await?;
    let mut shares = Vec::with_capacity(rows.len());
    for r in rows {
        shares.push(to_dto(s, &mut conn, r).await?);
    }
    Ok(PreviewShareList { shares })
}

/// Create a share (C-541): hosted agents only, allowed ports only, per-agent and per-org limits,
/// one audit row; the agent's home-sync generation bumps inside the transaction (a running
/// sandbox receives the new allow-list at once; a parked one is never woken).
pub async fn create(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    req: CreatePreviewShareRequest,
) -> Result<(axum::http::StatusCode, PreviewShare), ApiError> {
    ensure_enabled(s)?;
    let a = load_agent(s, c, agent).await?;
    if a.kind != "hosted" {
        return Err(not_supported());
    }
    if !(1024..=65535).contains(&req.port) || repo::RESERVED_PORTS.contains(&req.port) {
        return Err(port_forbidden());
    }
    let command = match req.start_command.as_deref() {
        Some(cmd) => Some(validate_command(cmd)?),
        None => None,
    };
    let cwd = match req.cwd.as_deref() {
        Some(cwd) => Some(validate_cwd(cwd)?),
        None => None,
    };
    let id = Uuid::now_v7();
    let mut tx = audit::begin(s).await?;
    ensure_org_enabled(&mut tx, a.org_id).await?;
    // A frozen agent is never given a new share (423 `frozen`, docs/05) — the same law as start.
    freeze::ensure_agent_active(&mut tx, agent).await?;
    repo::lock_org(&mut tx, a.org_id).await?;
    // The config may only lower the built-in limits (validated at startup).
    if repo::count_in_org(&mut *tx, a.org_id).await? >= repo::SHARES_PER_ORG
        || repo::count_in_org(&mut *tx, a.org_id).await?
            >= i64::from(s.config.previews.shares_per_org)
    {
        return Err(limit_reached());
    }
    if repo::count_for_agent(&mut *tx, agent).await? >= repo::SHARES_PER_AGENT
        || repo::count_for_agent(&mut *tx, agent).await?
            >= i64::from(s.config.previews.shares_per_agent)
    {
        return Err(limit_reached());
    }
    if repo::port_taken(&mut *tx, agent, req.port).await? {
        return Err(exists());
    }
    let (row, _label) = repo::insert(
        &mut tx,
        id,
        a.org_id,
        a.workspace_id,
        agent,
        req.port,
        command.as_deref(),
        cwd.as_deref(),
        (Some(c.user_id), Some(c.email.as_str())),
    )
    .await
    .map_err(|e| {
        if matches!(e, DbError::UniqueViolation { .. }) {
            exists()
        } else {
            e.into()
        }
    })?;
    let mut meta = json!({
        "agent_id": agent,
        "port": req.port,
        "start_command_set": false,
    });
    if let Some(cmd) = &command {
        meta["start_command_set"] = json!(true);
        meta["start_command_sha256_12"] = json!(hex_prefix(cmd));
    }
    if let Some(cwd) = &cwd {
        meta["cwd"] = json!(cwd);
    }
    let ev = event(c, "preview.share.create")
        .org(a.org_id)
        .workspace(a.workspace_id)
        .target("preview_share", id)
        .metadata(meta);
    audit::commit_critical(tx, &ev).await?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    let dto = to_dto(s, &mut conn, row).await?;
    Ok((axum::http::StatusCode::CREATED, dto))
}

/// Patch state / start command / cwd (the port and the label are immutable, C-542). Every actual
/// change bumps the agent's home-sync generation inside `repo::update`.
pub async fn patch(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    req: PatchPreviewShareRequest,
) -> Result<PreviewShare, ApiError> {
    ensure_enabled(s)?;
    let (_cur_outer, a) = load_share(s, c, id).await?;
    // `state`: `Some(None)` never occurs (absent vs value); a `Some(Some(state))` patch.
    let state: Option<&str> = match &req.state {
        Some(v) => Some(parse_state(v)?),
        None => None,
    };
    let command = match &req.start_command {
        Some(Some(v)) => Some(Some(validate_command(v)?)),
        Some(None) => Some(None),
        None => None,
    };
    let cwd = match &req.cwd {
        Some(Some(v)) => Some(Some(validate_cwd(v)?)),
        Some(None) => Some(None),
        None => None,
    };
    let mut tx = audit::begin(s).await?;
    let cur = repo::lock(&mut tx, id).await?.ok_or_else(share_not_found)?;
    let changed_state = state.is_some() && state != Some(cur.state.as_str());
    let changed_command = command.is_some()
        && command.as_ref().and_then(|o| o.as_deref()) != cur.start_command.as_deref();
    let changed_cwd =
        cwd.is_some() && cwd.as_ref().and_then(|o| o.as_deref()) != cur.cwd.as_deref();
    if !changed_state && !changed_command && !changed_cwd {
        // Idempotent: nothing to write, nothing to audit.
        tx.rollback().await.map_err(DbError::from)?;
        let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
        return to_dto(s, &mut conn, cur).await;
    }
    repo::update(
        &mut tx,
        id,
        // The PATCH state is a plain `Option<&str>`: present means set (never clear).
        state.map(Some),
        command.as_ref().map(|o| o.as_deref()),
        cwd.as_ref().map(|o| o.as_deref()),
    )
    .await?;
    let mut meta = json!({
        "state": state,
        "previous_state": cur.state,
        "start_command_set": cur.start_command.is_some(),
    });
    if let Some(cmd) = command.as_ref().and_then(|o| o.as_deref()) {
        meta["start_command_set"] = json!(true);
        meta["start_command_sha256_12"] = json!(hex_prefix(cmd));
    } else if changed_command {
        meta["start_command_set"] = json!(false);
        meta["start_command_sha256_12"] = serde_json::Value::Null;
    }
    if let Some(cwd) = cwd.as_ref().and_then(|o| o.as_deref()) {
        meta["cwd"] = json!(cwd);
    }
    let ev = event(c, "preview.share.update")
        .org(a.org_id)
        .workspace(a.workspace_id)
        .target("preview_share", id)
        .metadata(meta);
    audit::commit_critical(tx, &ev).await?;
    let row = repo::get(&s.pool, id).await?.ok_or_else(share_not_found)?;
    let mut conn = s.pool.acquire().await.map_err(DbError::from)?;
    to_dto(s, &mut conn, row).await
}

/// Delete a share (C-542): the hostname label is never reused (the mint excludes every label
/// that has ever existed); sessions of the share go with it (cascade, P4-PRV-2's table). Bumps
/// the agent's home-sync generation.
pub async fn delete(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    ensure_enabled(s)?;
    let (cur, a) = load_share(s, c, id).await?;
    let mut tx = audit::begin(s).await?;
    repo::lock(&mut tx, id).await?.ok_or_else(share_not_found)?;
    repo::delete(&mut tx, id).await?;
    let ev = event(c, "preview.share.delete")
        .org(a.org_id)
        .workspace(a.workspace_id)
        .target("preview_share", id)
        .metadata(json!({
            "agent_id": cur.agent_id,
            "port": cur.port,
            "start_command_set": cur.start_command.is_some(),
            "start_command_sha256_12": cur.start_command.as_deref().map(hex_prefix),
        }));
    audit::commit_critical(tx, &ev).await?;
    Ok(())
}

/// Start the share's app now (C-544): refuse a frozen agent (423 `frozen`), rate-limit wakes per
/// share (429 `preview_wake_limited` beyond `PREVIEW_WAKES_PER_SHARE_HOUR`), record the wake
/// demand (`preview_wake_at`) and notify due work. The `preview.start` work item itself is
/// enqueued by arm 5 when the home part is applied and the port is not listening.
pub async fn start(
    s: &AppState,
    c: &Caller,
    id: Uuid,
) -> Result<(axum::http::StatusCode, PreviewStartAccepted), ApiError> {
    ensure_enabled(s)?;
    let (row, a) = load_share(s, c, id).await?;
    if row.state != "active" {
        return Err(paused());
    }
    if row.start_command.is_none() {
        return Err(no_start_command());
    }
    let mut tx = audit::begin(s).await?;
    let cur = repo::lock(&mut tx, id).await?.ok_or_else(share_not_found)?;
    if cur.state != "active" {
        return Err(paused());
    }
    if cur.start_command.is_none() {
        return Err(no_start_command());
    }
    // A frozen agent (agent, workspace or org) is never woken or started (docs/11 lifecycle).
    freeze::ensure_agent_active(&mut tx, cur.agent_id).await?;
    ensure_org_enabled(&mut tx, a.org_id).await?;
    // Per-share wake rate limit (`PREVIEW_WAKES_PER_SHARE_HOUR` per rolling hour): 429 beyond it.
    if !repo::record_wake(&mut tx, id, 3600.0).await? {
        return Err(wake_limited());
    }
    // Both stamps: `preview_wake_at` makes a stopped sandbox due work (arm 3, the wake);
    // `preview_start_requested_at` says a request is waiting for the port, so arm 5 emits
    // `preview.start` once the `previews` part is applied (`11 §Port previews`).
    sqlx::query(
        "UPDATE sandboxes SET preview_wake_at = now(), preview_start_requested_at = now(), updated_at = now() \
         WHERE agent_id = $1 AND state IS NOT NULL",
    )
    .bind(cur.agent_id)
    .execute(&mut *tx)
    .await
    .map_err(DbError::from)?;
    notify_work(&mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    // Not a security-critical action (not in the `04 §Audit` set): the wake demand itself is the
    // change, and it is visible in `wake_attempts` and the sandbox stamps. Best-effort row.
    audit::record(
        s,
        event(c, "preview.share.start")
            .org(a.org_id)
            .workspace(a.workspace_id)
            .target("preview_share", id)
            .metadata(json!({"agent_id": cur.agent_id, "port": cur.port})),
    );
    Ok((
        axum::http::StatusCode::ACCEPTED,
        PreviewStartAccepted {
            status: "starting".to_owned(),
        },
    ))
}

/// Org `settings.previews_enabled` (C-549): the owner/admin switch, audited
/// `org.previews.update` (security-critical). Switching off keeps the shares but ends access.
pub async fn set_org_enabled(
    s: &AppState,
    c: &Caller,
    org: Uuid,
    enabled: bool,
) -> Result<(), ApiError> {
    let mut tx = audit::begin(s).await?;
    sqlx::query(
        "UPDATE organizations SET settings = coalesce(settings, '{}'::jsonb) \
             || jsonb_build_object('previews_enabled', $2), updated_at = now() \
         WHERE id = $1",
    )
    .bind(org)
    .bind(enabled)
    .execute(&mut *tx)
    .await
    .map_err(DbError::from)?;
    let ev = event(c, "org.previews.update")
        .org(org)
        .target("organization", org)
        .metadata(json!({"previews_enabled": enabled}));
    audit::commit_critical(tx, &ev).await?;
    Ok(())
}
