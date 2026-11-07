//! Agent presences on channel integrations (docs/13 §5.7; schema `channel_presences`).
//!
//! `credentials_ref` values are sealed references; nothing here decrypts them.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

/// A presence joined with its integration (connector, tenant label, org).
#[derive(Clone, sqlx::FromRow)]
pub struct PresenceRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub integration_id: Uuid,
    pub mode: String,
    pub platform_user_ref: Option<String>,
    pub handle: Option<String>,
    pub credentials_ref: Option<String>,
    pub credentials_version: i64,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub connector_id: String,
    pub tenant_label: String,
    pub org_id: Uuid,
}

impl std::fmt::Debug for PresenceRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PresenceRow")
            .field("id", &self.id)
            .field("agent_id", &self.agent_id)
            .field("integration_id", &self.integration_id)
            .field("mode", &self.mode)
            .field("status", &self.status)
            .field("credentials_ref", &"[sealed]")
            .finish_non_exhaustive()
    }
}

const SELECT: &str =
    "SELECT p.id, p.agent_id, p.integration_id, p.mode, p.platform_user_ref, p.handle,
        p.credentials_ref, p.credentials_version, p.status, p.status_reason, p.created_at,
        i.connector_id, i.tenant_label, i.org_id
    FROM channel_presences p JOIN channel_integrations i ON i.id = p.integration_id";

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<PresenceRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SELECT} WHERE p.id=$1"))
        .bind(id)
        .fetch_optional(exec)
        .await?)
}

/// `FOR UPDATE OF p` variant for read-modify-write.
pub async fn get_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<PresenceRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("{SELECT} WHERE p.id=$1 FOR UPDATE OF p"))
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

pub async fn list_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<PresenceRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE p.agent_id=$1 ORDER BY p.created_at, p.id"
    ))
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

pub async fn list_for_integration<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
) -> Result<Vec<PresenceRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE p.integration_id=$1 ORDER BY p.created_at, p.id"
    ))
    .bind(integration)
    .fetch_all(exec)
    .await?)
}

pub struct NewPresence<'a> {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub integration_id: Uuid,
    pub mode: &'a str,
    pub status: &'a str,
    pub platform_user_ref: Option<&'a str>,
    pub handle: Option<&'a str>,
}

/// Unique violations: `channel_presences_agent_id_integration_id_key` (already attached),
/// `channel_presences_integration_id_platform_user_ref_key` (bot identity in use).
pub async fn insert<'e, E: PgExecutor<'e>>(exec: E, n: &NewPresence<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO channel_presences (id, agent_id, integration_id, mode, status,
             platform_user_ref, handle)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(n.id)
    .bind(n.agent_id)
    .bind(n.integration_id)
    .bind(n.mode)
    .bind(n.status)
    .bind(n.platform_user_ref)
    .bind(n.handle)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn delete<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM channel_presences WHERE id=$1")
        .bind(id)
        .execute(exec)
        .await?
        .rows_affected()
        > 0)
}

pub async fn set_status<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    status: &str,
    reason: Option<&str>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_presences SET status=$2, status_reason=$3, updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .bind(status)
    .bind(reason)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

/// Store a bot identity and (re)sealed credentials and mark the presence `active`
/// (provision result or manual setup). Bumps the credential version.
pub async fn activate<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    platform_user_ref: Option<&str>,
    handle: Option<&str>,
    credentials_ref: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE channel_presences
            SET platform_user_ref=$2, handle=$3, credentials_ref=$4,
                credentials_version = credentials_version + 1,
                status='active', status_reason=NULL, updated_at=now()
          WHERE id=$1",
    )
    .bind(id)
    .bind(platform_user_ref)
    .bind(handle)
    .bind(credentials_ref)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

/// The agent's `active` shared presence on an integration, if any.
pub async fn active_shared<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    integration: Uuid,
) -> Result<Option<PresenceRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE p.agent_id=$1 AND p.integration_id=$2 AND p.mode='shared' AND p.status='active'"
    ))
    .bind(agent)
    .bind(integration)
    .fetch_optional(exec)
    .await?)
}

/// A dedicated presence that must be deprovisioned before its rows disappear.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeprovisionTarget {
    pub presence_id: Uuid,
    pub integration_id: Uuid,
    pub connector_id: String,
    pub agent_id: Uuid,
    pub handle: Option<String>,
}

const TARGETS: &str =
    "SELECT p.id AS presence_id, p.integration_id, i.connector_id, p.agent_id, p.handle
    FROM channel_presences p
    JOIN channel_integrations i ON i.id = p.integration_id
    JOIN agents a ON a.id = p.agent_id
    WHERE p.mode='dedicated'";

pub async fn dedicated_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<DeprovisionTarget>, DbError> {
    Ok(
        sqlx::query_as(&format!("{TARGETS} AND p.agent_id=$1 ORDER BY p.id"))
            .bind(agent)
            .fetch_all(exec)
            .await?,
    )
}

pub async fn dedicated_for_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace: Uuid,
) -> Result<Vec<DeprovisionTarget>, DbError> {
    Ok(
        sqlx::query_as(&format!("{TARGETS} AND a.workspace_id=$1 ORDER BY p.id"))
            .bind(workspace)
            .fetch_all(exec)
            .await?,
    )
}

/// Every dedicated presence of the integrations of `org`.
pub async fn dedicated_for_org<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<Vec<DeprovisionTarget>, DbError> {
    Ok(
        sqlx::query_as(&format!("{TARGETS} AND i.org_id=$1 ORDER BY p.id"))
            .bind(org)
            .fetch_all(exec)
            .await?,
    )
}

pub async fn dedicated_for_integration<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
) -> Result<Vec<DeprovisionTarget>, DbError> {
    Ok(
        sqlx::query_as(&format!("{TARGETS} AND p.integration_id=$1 ORDER BY p.id"))
            .bind(integration)
            .fetch_all(exec)
            .await?,
    )
}

/// Bump the home sync generation of every agent with a presence on `integration`.
pub async fn bump_agents_of_integration(
    conn: &mut PgConnection,
    integration: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agents SET home_sync_generation = home_sync_generation + 1
          WHERE id IN (SELECT agent_id FROM channel_presences WHERE integration_id=$1)",
    )
    .bind(integration)
    .execute(conn)
    .await?;
    Ok(())
}

/// A shared presence went away: its bindings stop serving and DM defaults no longer point at
/// the agent on that integration.
pub async fn forget_agent_routing(
    conn: &mut PgConnection,
    agent: Uuid,
    integration: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE space_bindings SET active=false, updated_at=now()
          WHERE agent_id=$1 AND integration_id=$2 AND active",
    )
    .bind(agent)
    .bind(integration)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM dm_defaults WHERE agent_id=$1 AND integration_id=$2")
        .bind(agent)
        .bind(integration)
        .execute(conn)
        .await?;
    Ok(())
}

/// An active presence with its agent (inbox routing and commands).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PresenceAgent {
    pub presence_id: Uuid,
    pub mode: String,
    pub platform_user_ref: Option<String>,
    pub agent_id: Uuid,
    pub agent_handle: String,
    pub agent_name: String,
    pub workspace_id: Uuid,
}

pub async fn active_with_agents<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
) -> Result<Vec<PresenceAgent>, DbError> {
    Ok(sqlx::query_as(
        "SELECT p.id AS presence_id, p.mode, p.platform_user_ref, a.id AS agent_id,
                a.handle AS agent_handle, a.name AS agent_name, a.workspace_id
           FROM channel_presences p JOIN agents a ON a.id = p.agent_id
          WHERE p.integration_id=$1 AND p.status='active'
          ORDER BY p.created_at, p.id",
    )
    .bind(integration)
    .fetch_all(exec)
    .await?)
}

/// Users who can own a workspace that has an active shared presence on the integration
/// (org owners/admins and workspace owners), for "who can set this up" replies.
pub async fn setup_owners<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
    limit: i64,
) -> Result<Vec<(Uuid, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT u.id, u.display_name
           FROM channel_presences p
           JOIN agents a ON a.id = p.agent_id
           JOIN workspaces w ON w.id = a.workspace_id
           JOIN org_members m ON m.org_id = w.org_id AND m.status='active'
           LEFT JOIN workspace_access wa ON wa.workspace_id = w.id AND wa.user_id = m.user_id
           JOIN users u ON u.id = m.user_id
          WHERE p.integration_id=$1 AND p.mode='shared' AND p.status='active'
            AND (m.role IN ('owner','admin') OR wa.role='owner')
          ORDER BY u.display_name, u.id LIMIT $2",
    )
    .bind(integration)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}
