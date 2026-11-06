//! Space bindings of a shared bot (docs/13 §5.3; schema `space_bindings`).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BindingRow {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub space_ref: String,
    pub space_label: String,
    pub agent_id: Uuid,
    pub listen: String,
    pub followups_need_mention: bool,
    pub active: bool,
    pub bound_by_user_id: Option<Uuid>,
    pub bound_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

const SELECT: &str =
    "SELECT b.id, b.integration_id, b.space_ref, b.space_label, b.agent_id, b.listen,
        b.followups_need_mention, b.active, b.bound_by_user_id, u.display_name AS bound_by_name,
        b.created_at
    FROM space_bindings b LEFT JOIN users u ON u.id = b.bound_by_user_id";

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<BindingRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SELECT} WHERE b.id=$1"))
        .bind(id)
        .fetch_optional(exec)
        .await?)
}

/// Active bindings of an agent (inactive ones are history only).
pub async fn list_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<BindingRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE b.agent_id=$1 AND b.active ORDER BY b.created_at, b.id"
    ))
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

/// The active binding of a space (at most one: `space_bindings_active_space_uq`).
pub async fn active_binding<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
    space_ref: &str,
) -> Result<Option<BindingRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE b.integration_id=$1 AND b.space_ref=$2 AND b.active"
    ))
    .bind(integration)
    .bind(space_ref)
    .fetch_optional(exec)
    .await?)
}

pub struct NewBinding<'a> {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub space_ref: &'a str,
    pub space_label: &'a str,
    pub agent_id: Uuid,
    pub listen: &'a str,
    pub bound_by_user_id: Option<Uuid>,
}

/// Unique violation `space_bindings_active_space_uq`: the space already has an active binding.
pub async fn insert<'e, E: PgExecutor<'e>>(exec: E, n: &NewBinding<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO space_bindings (id, integration_id, space_ref, space_label, agent_id, listen,
             bound_by_user_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(n.id)
    .bind(n.integration_id)
    .bind(n.space_ref)
    .bind(n.space_label)
    .bind(n.agent_id)
    .bind(n.listen)
    .bind(n.bound_by_user_id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Update the editable settings; `None` keeps the current value.
pub async fn update<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    listen: Option<&str>,
    followups_need_mention: Option<bool>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE space_bindings
            SET listen = COALESCE($2, listen),
                followups_need_mention = COALESCE($3, followups_need_mention),
                updated_at = now()
          WHERE id=$1",
    )
    .bind(id)
    .bind(listen)
    .bind(followups_need_mention)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

/// Deactivate (bot removed from the space): re-adding needs an owner again.
pub async fn deactivate<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE space_bindings SET active=false, updated_at=now() WHERE id=$1 AND active",
    )
    .bind(id)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

pub async fn delete<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM space_bindings WHERE id=$1")
        .bind(id)
        .execute(exec)
        .await?
        .rows_affected()
        > 0)
}
