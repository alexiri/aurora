//! Preview shares (phase 4, P4-PRV-1, D-144, D-145, SEC-80; docs/11 §Port previews): rows,
//! limits, wake/start attempt windows and the agent home-sync generation bump.
//!
//! This module only stores and reads; authority (`can_use`), the feature switches and the
//! command/cwd validation live in the API service. The label is minted here: `p` + 24 base32
//! lowercase characters (120 bits of CSPRNG), unique and **never reused** (the unique index plus
//! the ever-growing `preview_labels` tombstones; a deleted share's label is never minted again).

use super::After;
use super::split;
use crate::DbError;
use aurora_core::limits::{
    PREVIEW_CWD_MAX, PREVIEW_LABEL_CHARS, PREVIEW_SHARES_PER_AGENT, PREVIEW_SHARES_PER_ORG,
    PREVIEW_START_COMMAND_MAX, PREVIEW_START_WINDOW, PREVIEW_WAKES_PER_SHARE_HOUR,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Limits (`02 §Limits`); the config may only lower the share counts.
pub const SHARES_PER_AGENT: i64 = PREVIEW_SHARES_PER_AGENT as i64;
pub const SHARES_PER_ORG: i64 = PREVIEW_SHARES_PER_ORG as i64;
pub const START_COMMAND_MAX: usize = PREVIEW_START_COMMAND_MAX;
pub const CWD_MAX: usize = PREVIEW_CWD_MAX;
/// Supervisor loopback listeners a share may never take (docs/12 §Previews).
pub const RESERVED_PORTS: &[i32] = &[3128, 3129, 3130];

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ShareRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub port: i32,
    pub label: String,
    pub visibility: String,
    pub state: String,
    pub start_command: Option<String>,
    pub cwd: Option<String>,
    pub last_start_at: Option<DateTime<Utc>>,
    pub last_start_exit_code: Option<i32>,
    pub last_start_tail: Option<String>,
    pub start_attempts: serde_json::Value,
    pub wake_attempts: serde_json::Value,
    pub created_by_user_id: Option<Uuid>,
    pub created_by_label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_accessed_at: Option<DateTime<Utc>>,
}

const SEL: &str = "SELECT p.id, p.org_id, p.workspace_id, p.agent_id, p.port, p.label::text AS label, \
    p.visibility, p.state, p.start_command, p.cwd, p.last_start_at, p.last_start_exit_code, \
    p.last_start_tail, p.start_attempts, p.wake_attempts, p.created_by_user_id, p.created_by_label, \
    p.created_at, p.updated_at, p.last_accessed_at FROM preview_shares p";

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<ShareRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SEL} WHERE p.id = $1"))
        .bind(id)
        .fetch_optional(exec)
        .await?)
}

pub async fn get_by_label<'e, E: PgExecutor<'e>>(
    exec: E,
    label: &str,
) -> Result<Option<ShareRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SEL} WHERE p.label = $1"))
        .bind(label)
        .fetch_optional(exec)
        .await?)
}

pub async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<Option<ShareRow>, DbError> {
    let locked: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM preview_shares WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    match locked {
        Some(id) => get(&mut *conn, id).await,
        None => Ok(None),
    }
}

/// The agent's shares (management list; all states).
pub async fn list_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    after: After,
    fetch: i64,
) -> Result<Vec<ShareRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "{SEL} WHERE p.agent_id = $1 \
           AND ($2::timestamptz IS NULL OR (p.created_at, p.id) < ($2, $3)) \
         ORDER BY p.created_at DESC, p.id DESC LIMIT $4"
    ))
    .bind(agent)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Serialize share creation per org so `SHARES_PER_ORG` holds under concurrency.
pub async fn lock_org(conn: &mut PgConnection, org: Uuid) -> Result<(), DbError> {
    sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR UPDATE")
        .bind(org)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(())
}

pub async fn count_in_org<'e, E: PgExecutor<'e>>(exec: E, org: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM preview_shares WHERE org_id = $1")
            .bind(org)
            .fetch_one(exec)
            .await?,
    )
}

pub async fn count_for_agent<'e, E: PgExecutor<'e>>(exec: E, agent: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM preview_shares WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(exec)
            .await?,
    )
}

/// Active shares of one agent, newest first (the `previews` home-sync part, docs/16 §W5).
pub async fn active_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<ShareRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "{SEL} WHERE p.agent_id = $1 AND p.state = 'active' ORDER BY p.port, p.id"
    ))
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

/// The base32 (RFC 4648, lowercase, no padding) alphabet of the label.
const B32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// `p` + 24 base32 lowercase chars from the OS CSPRNG, excluding every label that has ever
/// existed (the share rows union the `preview_labels` tombstones: a deleted share's label is
/// never minted again; the checked set stays tiny and is loaded at most a few times ever).
async fn mint_label(conn: &mut PgConnection) -> Result<String, DbError> {
    let used: BTreeSet<String> = sqlx::query_scalar(
        "SELECT label::text FROM preview_shares \
         UNION SELECT label::text FROM preview_labels",
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();
    for _ in 0..8 {
        let mut raw = [0u8; 15]; // 120 bits
        aws_lc_rs::rand::fill(&mut raw)
            .map_err(|_| DbError::Other(sqlx::Error::Protocol("CSPRNG unavailable".into())))?;
        let mut s = String::with_capacity(1 + PREVIEW_LABEL_CHARS);
        s.push('p');
        // 15 bytes = 120 bits = 24 base32 chars, exactly.
        for chunk in raw.as_chunks::<5>().0 {
            let v = u64::from(chunk[0])
                | (u64::from(chunk[1]) << 8)
                | (u64::from(chunk[2]) << 16)
                | (u64::from(chunk[3]) << 24)
                | (u64::from(chunk[4]) << 32);
            for i in 0..8 {
                s.push(B32[((v >> (i * 5)) & 0x1f) as usize] as char);
            }
        }
        debug_assert_eq!(s.len(), 1 + PREVIEW_LABEL_CHARS);
        if !used.contains(&s) {
            return Ok(s);
        }
    }
    Err(DbError::Other(sqlx::Error::Protocol(
        "preview_shares.label could not be minted".into(),
    )))
}

/// Insert a share. Returns the conflict codes the service maps: `preview_exists` (same agent and
/// port) or a colliding label (retried by the caller).
#[allow(clippy::too_many_arguments)]
pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    org: Uuid,
    workspace: Uuid,
    agent: Uuid,
    port: i32,
    start_command: Option<&str>,
    cwd: Option<&str>,
    created_by: (Option<Uuid>, Option<&str>),
) -> Result<(ShareRow, String), DbError> {
    let label = mint_label(conn).await?;
    sqlx::query(
        "INSERT INTO preview_shares (id, org_id, workspace_id, agent_id, port, label, visibility, \
             state, start_command, cwd, created_by_user_id, created_by_label) \
         VALUES ($1, $2, $3, $4, $5, $6, 'private', 'active', $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(org)
    .bind(workspace)
    .bind(agent)
    .bind(port)
    .bind(&label)
    .bind(start_command)
    .bind(cwd)
    .bind(created_by.0)
    .bind(created_by.1)
    .execute(&mut *conn)
    .await?;
    // The label is spent forever (FIX-043): tombstone it in the same transaction as the share
    // row, so a deleted share's label can never be minted again. A conflict is impossible (the
    // mint just excluded this label and tombstones are never deleted); `ON CONFLICT DO NOTHING`
    // keeps the invariant write itself unable to fail the create.
    sqlx::query("INSERT INTO preview_labels (label) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(&label)
        .execute(&mut *conn)
        .await?;
    // Like update/delete: a share change must reach a running sandbox at once (the `previews`
    // home part is re-synced, the runner learns share → port before arm 5 emits
    // `preview.start`); a bump never wakes a parked sandbox (docs/11 §Port previews).
    bump_agent(conn, id).await?;
    let row = get(&mut *conn, id).await?.ok_or(DbError::NotFound)?;
    Ok((row, label))
}

/// Optimistic update; `state` / `start_command` / `cwd` are `Some(None)` to clear.
/// Bumps the agent's home-sync generation (the allow-list reaches the supervisor only this way).
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    state: Option<Option<&str>>,
    start_command: Option<Option<&str>>,
    cwd: Option<Option<&str>>,
) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE preview_shares SET \
           state = CASE WHEN $2 THEN $3 ELSE state END, \
           start_command = CASE WHEN $4 THEN $5 ELSE start_command END, \
           cwd = CASE WHEN $6 THEN $7 ELSE cwd END, updated_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(state.is_some())
    .bind(state.flatten())
    .bind(start_command.is_some())
    .bind(start_command.flatten())
    .bind(cwd.is_some())
    .bind(cwd.flatten())
    .execute(&mut *conn)
    .await?;
    if r.rows_affected() > 0 {
        bump_agent(conn, id).await?;
    }
    Ok(r.rows_affected() > 0)
}

/// Deletes the share (preview sessions go with it, cascade); the `preview_labels` tombstone of
/// its label stays (a deleted share's label is never minted again); bumps the generation.
pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    let row: Option<Uuid> = sqlx::query_scalar("SELECT agent_id FROM preview_shares WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    let n = sqlx::query("DELETE FROM preview_shares WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if n > 0
        && let Some(agent) = row
    {
        bump_agent(conn, agent).await?;
    }
    Ok(n > 0)
}

/// A share change must reach a running sandbox at once: bump `agents.home_sync_generation`
/// (like skills); a bump never wakes a parked sandbox (docs/11 §Port previews).
async fn bump_agent(conn: &mut PgConnection, share: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agents SET home_sync_generation = home_sync_generation + 1 \
         WHERE id = (SELECT agent_id FROM preview_shares WHERE id = $1)",
    )
    .bind(share)
    .execute(conn)
    .await?;
    Ok(())
}

/// Is the (agent, port) pair already taken?
pub async fn port_taken<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    port: i32,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM preview_shares WHERE agent_id = $1 AND port = $2)",
    )
    .bind(agent)
    .bind(port)
    .fetch_one(exec)
    .await?)
}

// ---- attempt windows ----

fn json_times(v: &serde_json::Value) -> Vec<DateTime<Utc>> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    x.as_str()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                })
                .map(|t| t.with_timezone(&Utc))
                .collect()
        })
        .unwrap_or_default()
}

fn push_times(kept: &[DateTime<Utc>], now: DateTime<Utc>) -> serde_json::Value {
    let mut arr: Vec<serde_json::Value> = kept
        .iter()
        .chain(std::iter::once(&now))
        .map(|t| serde_json::Value::from(t.to_rfc3339()))
        .collect();
    arr.truncate(16); // bound the array: the windows are far smaller
    serde_json::Value::from(arr)
}

/// Records a wake inside `wake_attempts`, keeping only the last hour's timestamps.
/// Returns `false` when `PREVIEW_WAKES_PER_SHARE_HOUR` is exhausted (429 `preview_wake_limited`).
pub async fn record_wake(
    conn: &mut PgConnection,
    share: Uuid,
    hour_secs: f64,
) -> Result<bool, DbError> {
    let cur: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT wake_attempts FROM preview_shares WHERE id = $1 FOR UPDATE")
            .bind(share)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(cur) = cur else {
        return Ok(false);
    };
    let now = Utc::now();
    let cutoff = now - chrono::TimeDelta::try_seconds(hour_secs as i64).unwrap_or_default();
    let kept: Vec<DateTime<Utc>> = json_times(&cur)
        .into_iter()
        .filter(|t| *t > cutoff)
        .collect();
    if kept.len() >= PREVIEW_WAKES_PER_SHARE_HOUR as usize {
        return Ok(false);
    }
    sqlx::query("UPDATE preview_shares SET wake_attempts = $2, updated_at = now() WHERE id = $1")
        .bind(share)
        .bind(push_times(&kept, now))
        .execute(conn)
        .await?;
    Ok(true)
}

/// Records a start attempt inside `start_attempts`, keeping only `window_secs` back.
/// Returns `false` when `PREVIEW_START_ATTEMPTS` within the window is exhausted.
pub async fn record_start_attempt(
    conn: &mut PgConnection,
    share: Uuid,
    window_secs: f64,
    max: u32,
) -> Result<bool, DbError> {
    let cur: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT start_attempts FROM preview_shares WHERE id = $1 FOR UPDATE")
            .bind(share)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(cur) = cur else {
        return Ok(false);
    };
    let now = Utc::now();
    let cutoff = now - chrono::TimeDelta::try_seconds(window_secs as i64).unwrap_or_default();
    let kept: Vec<DateTime<Utc>> = json_times(&cur)
        .into_iter()
        .filter(|t| *t > cutoff)
        .collect();
    if kept.len() >= max as usize {
        return Ok(false);
    }
    sqlx::query("UPDATE preview_shares SET start_attempts = $2, updated_at = now() WHERE id = $1")
        .bind(share)
        .bind(push_times(&kept, now))
        .execute(conn)
        .await?;
    Ok(true)
}

/// Result of the runner-reported process outcome (docs/12 §Previews).
pub struct StartOutcome<'a> {
    pub exit_code: Option<i32>,
    pub tail: &'a str,
}

/// Store the last start outcome of a share (dashboard churn columns).
pub async fn record_start_outcome(
    conn: &mut PgConnection,
    share: Uuid,
    exit_code: Option<i32>,
    tail: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE preview_shares SET last_start_at = now(), last_start_exit_code = $2, \
             last_start_tail = $3, updated_at = now() \
         WHERE id = $1",
    )
    .bind(share)
    .bind(exit_code)
    .bind(tail)
    .execute(conn)
    .await?;
    Ok(())
}

/// Relay accounting heartbeat (P4-PRV-2 calls this): at most once a minute.
pub async fn touch_accessed(conn: &mut PgConnection, share: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE preview_shares SET last_accessed_at = now() \
         WHERE id = $1 AND (last_accessed_at IS NULL OR last_accessed_at < now() - interval '60 seconds')",
    )
    .bind(share)
    .execute(conn)
    .await?;
    Ok(())
}

/// Count of the org's active shares (the `aurora_preview_shares` gauge, docs/05 §Background jobs).
pub async fn count_active_in_org<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM preview_shares WHERE org_id = $1 AND state = 'active'",
    )
    .bind(org)
    .fetch_one(exec)
    .await?)
}

/// Stale churn of `sandboxes`: the wake demand and start-request stamps are due for
/// `PREVIEW_WAKE_WINDOW` / `PREVIEW_WAKE_WAIT` and cleared afterwards (60 s housekeeping job).
pub async fn clear_stale_sandbox_stamps(conn: &mut PgConnection) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE sandboxes SET preview_wake_at = NULL, preview_start_requested_at = NULL, updated_at = now() \
         WHERE (preview_wake_at IS NOT NULL AND preview_wake_at < now() - interval '90 seconds') \
            OR (preview_start_requested_at IS NOT NULL \
                AND preview_start_requested_at < now() - interval '90 seconds')",
    )
    .execute(&mut *conn)
    .await?
    .rows_affected())
}

pub const START_WINDOW_SECS: f64 = PREVIEW_START_WINDOW.as_secs_f64();

/// One pending on-demand start: a running sandbox whose channel this replica holds, with a wake
/// or start demand younger than its window and an active share with a start command whose port is
/// not listening (docs/05 arm 5, docs/11 §Port previews).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StartDemand {
    pub share_id: Uuid,
    pub agent_id: Uuid,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
}

/// Shares to start on a running sandbox after its home sync is applied: `home_applied_generation`
/// is at the desired level (the `previews` part has been applied), the share is active with a
/// start command, the port is not in `listening_ports`, and a request is waiting
/// (`preview_start_requested_at` younger than `wake_wait_secs`). The caller records the attempt
/// (`record_start_attempt`, `PREVIEW_START_ATTEMPTS` per window) and stamps
/// `preview_start_requested_at = now()` per emitted item.
pub async fn claim_start_demands(
    conn: &mut PgConnection,
    runner: Uuid,
    replica: Uuid,
    wake_wait_secs: f64,
    limit: i64,
) -> Result<Vec<StartDemand>, DbError> {
    Ok(sqlx::query_as(
        "SELECT ps.id AS share_id, ps.agent_id, s.id AS sandbox_id, s.boot_id \
         FROM preview_shares ps \
           JOIN sandboxes s ON s.agent_id = ps.agent_id \
           JOIN agents a ON a.id = ps.agent_id \
           JOIN workspaces w ON w.id = a.workspace_id \
           JOIN organizations o ON o.id = a.org_id \
         WHERE ps.state = 'active' AND ps.start_command IS NOT NULL AND ps.start_command <> '' \
           AND s.runner_id = $1 AND s.channel_replica_id = $2 AND s.state = 'running' \
           AND s.boot_id IS NOT NULL \
           AND s.home_applied_generation >= (a.home_sync_generation + w.home_sync_generation + 1) \
           AND NOT (s.listening_ports @> ARRAY[ps.port]) \
           AND s.preview_start_requested_at IS NOT NULL \
           AND s.preview_start_requested_at > now() - make_interval(secs => $3) \
           AND NOT (EXISTS (SELECT 1 FROM agents fa \
                 JOIN workspaces fw ON fw.id = fa.workspace_id \
                 JOIN organizations fo ON fo.id = fa.org_id \
                 WHERE fa.id = ps.agent_id \
                   AND (fa.frozen_at IS NOT NULL OR fw.frozen_at IS NOT NULL \
                        OR fo.frozen_at IS NOT NULL))) \
           AND coalesce((o.settings->'previews_enabled')::bool, false) \
         ORDER BY s.preview_start_requested_at LIMIT $4 FOR UPDATE OF ps",
    )
    .bind(runner)
    .bind(replica)
    .bind(wake_wait_secs)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?)
}

/// Stamp the start request of an emitted `preview.start` item (docs/05 arm 5).
pub async fn stamp_start_requested(conn: &mut PgConnection, agent: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE sandboxes SET preview_start_requested_at = now(), updated_at = now() \
         WHERE agent_id = $1",
    )
    .bind(agent)
    .execute(&mut *conn)
    .await?;
    Ok(())
}
