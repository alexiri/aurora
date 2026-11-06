//! Links between platform users and Aurora users (docs/13 §5.9; schema `user_links`).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserLinkRow {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub platform_user_ref: String,
    pub user_id: Uuid,
    pub method: String,
    pub created_at: DateTime<Utc>,
    pub connector_id: String,
    pub tenant_label: String,
    pub org_id: Uuid,
    pub user_display_name: String,
}

const SELECT: &str =
    "SELECT l.id, l.integration_id, l.platform_user_ref, l.user_id, l.method, l.created_at,
        i.connector_id, i.tenant_label, i.org_id, u.display_name AS user_display_name
    FROM user_links l
    JOIN channel_integrations i ON i.id = l.integration_id
    JOIN users u ON u.id = l.user_id";

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<UserLinkRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SELECT} WHERE l.id=$1"))
        .bind(id)
        .fetch_optional(exec)
        .await?)
}

/// The Aurora user linked to a platform user of an integration (inbox sender resolution).
pub async fn user_link_for<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
    platform_user_ref: &str,
) -> Result<Option<UserLinkRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE l.integration_id=$1 AND l.platform_user_ref=$2"
    ))
    .bind(integration)
    .bind(platform_user_ref)
    .fetch_optional(exec)
    .await?)
}

/// Keyset page of an integration's links, newest first.
pub async fn page_for_integration<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
    after: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> Result<Vec<UserLinkRow>, DbError> {
    let (t, id) = after.unzip();
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE l.integration_id=$1
           AND ($2::timestamptz IS NULL OR (l.created_at, l.id) < ($2, $3))
         ORDER BY l.created_at DESC, l.id DESC LIMIT $4"
    ))
    .bind(integration)
    .bind(t)
    .bind(id)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

pub async fn list_for_user<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
) -> Result<Vec<UserLinkRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE l.user_id=$1 ORDER BY l.created_at, l.id"
    ))
    .bind(user)
    .fetch_all(exec)
    .await?)
}

pub async fn list_for_user_in<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    integration: Uuid,
) -> Result<Vec<UserLinkRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE l.user_id=$1 AND l.integration_id=$2 ORDER BY l.created_at, l.id"
    ))
    .bind(user)
    .bind(integration)
    .fetch_all(exec)
    .await?)
}

/// Insert a link. Unique violation (`user_links_integration_id_platform_user_ref_key`): the
/// platform user is already linked.
pub async fn insert<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
    platform_user_ref: &str,
    user: Uuid,
    method: &str,
) -> Result<Uuid, DbError> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO user_links (id, integration_id, platform_user_ref, user_id, method)
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(id)
    .bind(integration)
    .bind(platform_user_ref)
    .bind(user)
    .bind(method)
    .execute(exec)
    .await?;
    Ok(id)
}

pub async fn delete<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM user_links WHERE id=$1")
        .bind(id)
        .execute(exec)
        .await?
        .rows_affected()
        > 0)
}
