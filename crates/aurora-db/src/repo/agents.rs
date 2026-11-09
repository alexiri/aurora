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
    model_endpoint_id, model_id, effort, proactive_messaging, brief, avatar_blob_key, avatar_public_key, \
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
    Ok(sqlx::query_as(&format!(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, is_default, harness, effort, brief, created_by)
         VALUES ($1,$2,$3,$4,$5,$6,true,$7,'medium',$8,$9) RETURNING {COLS}"
    ))
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
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM agents WHERE id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

pub async fn list_for_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    after: After,
    fetch: i64,
) -> Result<Vec<AgentRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM agents WHERE workspace_id = $1
         AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3))
         ORDER BY created_at DESC, id DESC LIMIT $4"
    ))
    .bind(ws)
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
    Ok(sqlx::query_as(&format!(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness, model_endpoint_id,
            model_id, effort, proactive_messaging, brief, created_by, model_provider)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) RETURNING {COLS}"
    ))
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
}

pub async fn update<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    p: &AgentPatch<'_>,
) -> Result<AgentRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE agents SET name = COALESCE($2, name), brief = COALESCE($3, brief),
            harness = COALESCE($4, harness), model_endpoint_id = COALESCE($5, model_endpoint_id),
            model_id = COALESCE($6, model_id), effort = COALESCE($7, effort),
            proactive_messaging = COALESCE($8, proactive_messaging),
            resource_overrides = COALESCE($9, resource_overrides),
            model_provider = COALESCE($10, model_provider), updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    ))
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
    Ok(sqlx::query_as(&format!(
        "UPDATE agents SET byo_source_cidrs = $2, byo_require_context = $3, updated_at = now()
         WHERE id = $1 AND kind = 'byo' RETURNING {COLS}"
    ))
    .bind(id)
    .bind(source_cidrs)
    .bind(require_context)
    .fetch_optional(exec)
    .await?)
}
