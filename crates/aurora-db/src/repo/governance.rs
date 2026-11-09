//! Organization governance document, stored-origin re-evaluation inputs and agent model overrides.

use crate::DbError;
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

/// `(document, revision)`; `None` = no such organization.
pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<Option<(serde_json::Value, i64)>, DbError> {
    Ok(
        sqlx::query_as("SELECT governance, governance_revision FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_optional(exec)
            .await?,
    )
}

/// Row-lock the organization and return `(document, revision)`.
pub async fn lock(
    conn: &mut PgConnection,
    org: Uuid,
) -> Result<Option<(serde_json::Value, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT governance, governance_revision FROM organizations WHERE id = $1 FOR UPDATE",
    )
    .bind(org)
    .fetch_optional(conn)
    .await?)
}

/// The document under a share lock: a concurrent `PUT` (which updates the row) waits for this
/// transaction to finish, so a credential minted in it is seen by that `PUT`'s retrofit
/// (RFE-013).
pub async fn read_shared(
    conn: &mut PgConnection,
    org: Uuid,
) -> Result<Option<serde_json::Value>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT governance FROM organizations WHERE id = $1 FOR SHARE")
            .bind(org)
            .fetch_optional(conn)
            .await?,
    )
}

/// Store the document and bump the revision; returns the new revision.
pub async fn store(
    conn: &mut PgConnection,
    org: Uuid,
    doc: &serde_json::Value,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE organizations SET governance = $2, governance_revision = governance_revision + 1, \
         updated_at = now() WHERE id = $1 RETURNING governance_revision",
    )
    .bind(org)
    .bind(doc)
    .fetch_one(conn)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LlmSecretRef {
    pub id: Uuid,
    pub name: String,
    pub workspace_id: Option<Uuid>,
    pub llm_endpoint_id: Option<String>,
    pub status: String,
    pub origin: Option<serde_json::Value>,
    pub value_version: i32,
}

/// Every LLM secret of the org (all scopes) except rejected proposals.
pub async fn llm_secrets<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<Vec<LlmSecretRef>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, name, workspace_id, llm_endpoint_id, status, origin, value_version \
         FROM secrets WHERE org_id = $1 AND kind = 'llm' AND status <> 'rejected' \
         ORDER BY created_at, id",
    )
    .bind(org)
    .fetch_all(exec)
    .await?)
}

/// Set `origin.status = 'mismatch'` on the given secrets.
pub async fn mark_mismatch(conn: &mut PgConnection, ids: &[Uuid]) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE secrets SET origin = jsonb_set(origin, '{status}', '\"mismatch\"'), \
         updated_at = now() WHERE id = ANY($1) AND kind = 'llm' AND origin IS NOT NULL",
    )
    .bind(ids)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AgentModel {
    pub id: Uuid,
    pub model_endpoint_id: String,
    pub model_id: Option<String>,
}

/// Agents of the org with an explicit endpoint override.
pub async fn agent_models(conn: &mut PgConnection, org: Uuid) -> Result<Vec<AgentModel>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, model_endpoint_id, model_id FROM agents \
         WHERE org_id = $1 AND model_endpoint_id IS NOT NULL FOR UPDATE",
    )
    .bind(org)
    .fetch_all(conn)
    .await?)
}

pub async fn clear_agent_models(conn: &mut PgConnection, ids: &[Uuid]) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agents SET model_provider = NULL, model_endpoint_id = NULL, model_id = NULL, \
         updated_at = now() WHERE id = ANY($1)",
    )
    .bind(ids)
    .execute(conn)
    .await?;
    Ok(())
}
