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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, org_id, workspace_id, handle, name, kind, is_default, harness, model_provider, \
    model_endpoint_id, model_id, effort, proactive_messaging, brief, created_at, updated_at";

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
