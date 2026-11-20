//! Agents (read side plus default-agent creation).

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AgentRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub handle: String,
    pub name: String,
    pub kind: String,
    pub is_default: bool,
    pub harness: Option<String>,
    pub model_provider: Option<String>,
    pub model_endpoint_id: Option<String>,
    pub model_id: Option<String>,
    pub effort: Option<String>,
    /// (P5-MODELS-1, C-876) `{ endpoint_id, large?, medium?, small? }` or null.
    pub model_roles: Option<serde_json::Value>,
    /// (P5-MODELS-1, C-876) `inherit` | `large` | `medium` | `small`.
    pub subagent_model: String,
    /// (P4-NARR, C-846) Live narration cards on chat platforms that can edit messages.
    pub narration_cards: bool,
    pub proactive_messaging: String,
    pub brief: String,
    pub avatar_blob_key: Option<String>,
    pub avatar_public_key: Option<String>,
    /// (RFE-025) BYO only: own source CIDR list (null = inherit).
    pub byo_source_cidrs: Option<Vec<String>>,
    /// (RFE-025) BYO only: credentialed requests must use a context credential.
    pub byo_require_context: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, org_id, workspace_id, handle, name, kind, is_default, harness, model_provider, \
    model_endpoint_id, model_id, effort, model_roles, subagent_model, narration_cards, proactive_messaging, brief, avatar_blob_key, avatar_public_key, \
    byo_source_cidrs, byo_require_context, created_at, updated_at";

pub struct NewDefaultAgent<'a> {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub handle: &'a str,
    pub name: &'a str,
    pub hosted: bool,
    pub harness: &'a str,
    pub brief: &'a str,
    pub created_by: Uuid,
}

pub async fn insert_default<'e, E: PgExecutor<'e>>(
    exec: E,
    a: &NewDefaultAgent<'_>,
) -> Result<AgentRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, is_default, harness, effort, brief, created_by)
         VALUES ($1,$2,$3,$4,$5,$6,true,$7,'medium',$8,$9) RETURNING {COLS}"
    )))
    .bind(a.id)
    .bind(a.org_id)
    .bind(a.workspace_id)
    .bind(a.handle)
    .bind(a.name)
    .bind(if a.hosted { "hosted" } else { "byo" })
    .bind(a.harness)
    .bind(a.brief)
    .bind(a.created_by)
    .fetch_one(exec)
    .await?)
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<AgentRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM agents WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

pub async fn list_for_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    after: After,
    fetch: i64,
) -> Result<Vec<AgentRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM agents WHERE workspace_id = $1
         AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3))
         ORDER BY created_at DESC, id DESC LIMIT $4"
    )))
    .bind(ws)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// (P5-REFINE-6, C-1090) One agent of an organization, with its workspace.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OrgAgentRow {
    pub id: Uuid,
    pub name: String,
    pub handle: String,
    pub kind: String,
    pub workspace_id: Uuid,
    pub workspace_slug: String,
    pub workspace_name: String,
    /// The person whose personal workspace it is (`None` for a team workspace).
    pub personal_owner_id: Option<Uuid>,
    pub personal_owner_name: Option<String>,
    pub harness: Option<String>,
    /// The agent, its workspace or its organization is paused (frozen).
    pub paused: bool,
    /// When its newest turn was created; `None` when it never ran.
    pub last_active_at: Option<DateTime<Utc>>,
    /// (D-425) A turn is queued, dispatched or running (the keep-awake reason `turn`).
    pub active_turn: bool,
    /// (D-425) The hosted computer's facts (`None` without a sandbox row).
    pub sandbox_state: Option<String>,
    pub start_phase: Option<String>,
    pub failure_code: Option<String>,
    /// A busy process runs on the current boot (the keep-awake reason `process`).
    pub busy_process: bool,
    pub created_at: DateTime<Utc>,
}

/// (C-1090) Every agent of `org`, personal workspaces included, newest first, keyset on
/// `(created_at, id)`.
pub async fn list_for_org<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    after: After,
    fetch: i64,
) -> Result<Vec<OrgAgentRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(
        "SELECT a.id, a.name, a.handle, a.kind, w.id AS workspace_id, w.slug AS workspace_slug,
                w.name AS workspace_name, pu.id AS personal_owner_id,
                pu.display_name AS personal_owner_name, a.harness,
                (a.frozen_at IS NOT NULL OR w.frozen_at IS NOT NULL OR o.frozen_at IS NOT NULL) AS paused,
                (SELECT max(t.created_at) FROM turns t WHERE t.agent_id = a.id) AS last_active_at,
                EXISTS (SELECT 1 FROM turns t WHERE t.agent_id = a.id AND (
                    (t.role = 'turn' AND t.status IN ('queued','dispatched','running'))
                    OR (t.role = 'followup' AND t.status = 'joining'))) AS active_turn,
                s.state AS sandbox_state, s.start_phase, s.failure_code,
                coalesce(EXISTS (SELECT 1 FROM processes p WHERE p.sandbox_id = s.id
                    AND p.boot_id = s.boot_id AND p.status = 'running' AND p.busy), false) AS busy_process,
                a.created_at
           FROM agents a
           JOIN workspaces w ON w.id = a.workspace_id
           JOIN organizations o ON o.id = a.org_id
           LEFT JOIN users pu ON pu.id = w.personal_owner_user_id
           LEFT JOIN sandboxes s ON s.agent_id = a.id
          WHERE a.org_id = $1
            AND ($2::timestamptz IS NULL OR (a.created_at, a.id) < ($2, $3))
          ORDER BY a.created_at DESC, a.id DESC LIMIT $4",
    )
    .bind(org)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

pub struct NewAgent<'a> {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub handle: &'a str,
    pub name: &'a str,
    pub kind: &'a str,
    pub harness: Option<&'a str>,
    /// An explicit model is stamped with its provider and endpoint (08 §Model selection).
    pub model_provider: Option<&'a str>,
    pub model_endpoint_id: Option<&'a str>,
    pub model_id: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub proactive_messaging: &'a str,
    pub brief: &'a str,
    pub created_by: Uuid,
}

/// Unique constraint on `(workspace_id, handle)`.
pub const HANDLE_CONSTRAINT: &str = "agents_workspace_id_handle_key";

pub async fn insert<'e, E: PgExecutor<'e>>(exec: E, a: &NewAgent<'_>) -> Result<AgentRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness, model_endpoint_id,
            model_id, effort, proactive_messaging, brief, created_by, model_provider)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) RETURNING {COLS}"
    )))
    .bind(a.id)
    .bind(a.org_id)
    .bind(a.workspace_id)
    .bind(a.handle)
    .bind(a.name)
    .bind(a.kind)
    .bind(a.harness)
    .bind(a.model_endpoint_id)
    .bind(a.model_id)
    .bind(a.effort)
    .bind(a.proactive_messaging)
    .bind(a.brief)
    .bind(a.created_by)
    .bind(a.model_provider)
    .fetch_one(exec)
    .await?)
}

pub async fn handle_exists<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    handle: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM agents WHERE workspace_id = $1 AND handle = $2)",
    )
    .bind(ws)
    .bind(handle)
    .fetch_one(exec)
    .await?)
}

/// Lock the org row and return `(settings.max_agents, current agent count)`.
pub async fn lock_org_agent_limit(
    tx: &mut sqlx::PgConnection,
    org: Uuid,
) -> Result<(Option<i64>, i64), DbError> {
    let settings: serde_json::Value =
        sqlx::query_scalar("SELECT settings FROM organizations WHERE id = $1 FOR UPDATE")
            .bind(org)
            .fetch_one(&mut *tx)
            .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM agents WHERE org_id = $1")
        .bind(org)
        .fetch_one(&mut *tx)
        .await?;
    Ok((
        settings
            .get("max_agents")
            .and_then(serde_json::Value::as_i64),
        count,
    ))
}

#[derive(Default)]
pub struct AgentPatch<'a> {
    pub name: Option<&'a str>,
    pub brief: Option<&'a str>,
    pub harness: Option<&'a str>,
    /// Set together with `model_endpoint_id` (the endpoint's provider).
    pub model_provider: Option<&'a str>,
    pub model_endpoint_id: Option<&'a str>,
    pub model_id: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub proactive_messaging: Option<&'a str>,
    pub resource_overrides: Option<&'a serde_json::Value>,
    /// (C-876) `None` = unchanged; `Some(None)` = clear; `Some(Some(v))` = replace.
    pub model_roles: Option<Option<&'a serde_json::Value>>,
    pub subagent_model: Option<&'a str>,
    pub narration_cards: Option<bool>,
}

pub async fn update<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    p: &AgentPatch<'_>,
) -> Result<AgentRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE agents SET name = COALESCE($2, name), brief = COALESCE($3, brief),
            harness = COALESCE($4, harness), model_endpoint_id = COALESCE($5, model_endpoint_id),
            model_id = COALESCE($6, model_id), effort = COALESCE($7, effort),
            proactive_messaging = COALESCE($8, proactive_messaging),
            resource_overrides = COALESCE($9, resource_overrides),
            model_provider = COALESCE($10, model_provider),
            model_roles = CASE WHEN $11 THEN $12 ELSE model_roles END,
            subagent_model = COALESCE($13, subagent_model),
            narration_cards = COALESCE($14, narration_cards), updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    )))
    .bind(id)
    .bind(p.name)
    .bind(p.brief)
    .bind(p.harness)
    .bind(p.model_endpoint_id)
    .bind(p.model_id)
    .bind(p.effort)
    .bind(p.proactive_messaging)
    .bind(p.resource_overrides)
    .bind(p.model_provider)
    .bind(p.model_roles.is_some())
    .bind(p.model_roles.flatten())
    .bind(p.subagent_model)
    .bind(p.narration_cards)
    .fetch_one(exec)
    .await?)
}

pub async fn delete<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM agents WHERE id = $1")
        .bind(id)
        .execute(exec)
        .await?
        .rows_affected()
        > 0)
}

/// Replace (or clear, with `None`) the avatar and return the previous blob key. The old row is
/// locked so two concurrent uploads each see (and delete) a distinct previous blob.
pub async fn set_avatar<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    blob_key: Option<&str>,
    public_key: Option<&str>,
) -> Result<Option<Option<String>>, DbError> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "WITH old AS (SELECT id, avatar_blob_key FROM agents WHERE id = $1 FOR UPDATE)
         UPDATE agents a SET avatar_blob_key = $2, avatar_public_key = $3, updated_at = now()
           FROM old WHERE a.id = old.id RETURNING old.avatar_blob_key",
    )
    .bind(id)
    .bind(blob_key)
    .bind(public_key)
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| r.0))
}

/// Blob key of the avatar whose *current* public key is `key`.
pub async fn avatar_blob_by_public_key<'e, E: PgExecutor<'e>>(
    exec: E,
    key: &str,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT avatar_blob_key FROM agents WHERE avatar_public_key = $1 AND avatar_blob_key IS NOT NULL",
    )
    .bind(key)
    .fetch_optional(exec)
    .await?)
}

/// (RFE-025) Replaces a BYO agent's source list and require-context flag; `false` when the agent
/// does not exist or is not BYO.
pub async fn set_byo_settings<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    source_cidrs: Option<&[String]>,
    require_context: bool,
) -> Result<Option<AgentRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE agents SET byo_source_cidrs = $2, byo_require_context = $3, updated_at = now()
         WHERE id = $1 AND kind = 'byo' RETURNING {COLS}"
    )))
    .bind(id)
    .bind(source_cidrs)
    .bind(require_context)
    .fetch_optional(exec)
    .await?)
}
