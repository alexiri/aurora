//! Default agent of a platform user's DMs with a shared bot (schema `dm_defaults`).

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DmDefaultRow {
    pub integration_id: Uuid,
    pub platform_user_ref: String,
    pub agent_id: Uuid,
}

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
    platform_user_ref: &str,
) -> Result<Option<DmDefaultRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT integration_id, platform_user_ref, agent_id FROM dm_defaults
          WHERE integration_id=$1 AND platform_user_ref=$2",
    )
    .bind(integration)
    .bind(platform_user_ref)
    .fetch_optional(exec)
    .await?)
}

pub async fn upsert<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
    platform_user_ref: &str,
    agent: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO dm_defaults (integration_id, platform_user_ref, agent_id) VALUES ($1,$2,$3)
         ON CONFLICT (integration_id, platform_user_ref)
         DO UPDATE SET agent_id = EXCLUDED.agent_id, updated_at = now()",
    )
    .bind(integration)
    .bind(platform_user_ref)
    .bind(agent)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn delete<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
    platform_user_ref: &str,
) -> Result<bool, DbError> {
    Ok(
        sqlx::query("DELETE FROM dm_defaults WHERE integration_id=$1 AND platform_user_ref=$2")
            .bind(integration)
            .bind(platform_user_ref)
            .execute(exec)
            .await?
            .rows_affected()
            > 0,
    )
}
