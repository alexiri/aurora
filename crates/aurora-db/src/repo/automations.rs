//! RFE-021 offboarding view: what removing or suspending a person would disable
//! (docs/04 §Fire-time authority (b)).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// A schedule, webhook or peer task whose authority is the person.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OffboardRow {
    pub id: Uuid,
    pub name: String,
    pub agent_id: Uuid,
    pub agent_name: String,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub enabled: bool,
    pub anchor: String,
    pub next_fire_at: Option<DateTime<Utc>>,
}

/// Enabled, creator-anchored schedules of `user` in the org (or one workspace of it).
pub async fn schedules<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    user: Uuid,
) -> Result<Vec<OffboardRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.id, s.name, a.id AS agent_id, a.name AS agent_name, w.id AS workspace_id,
                w.name AS workspace_name, s.enabled, s.anchor, s.next_fire_at
         FROM schedules s JOIN agents a ON a.id = s.agent_id JOIN workspaces w ON w.id = a.workspace_id
         WHERE w.org_id = $1 AND ($2::uuid IS NULL OR w.id = $2)
           AND s.creator_user_id = $3 AND s.anchor = 'creator' AND s.enabled
         ORDER BY w.name, a.name, s.created_at, s.id",
    )
    .bind(org)
    .bind(workspace)
    .bind(user)
    .fetch_all(exec)
    .await?)
}

/// Enabled, creator-anchored webhooks of `user` in the org (or one workspace of it).
pub async fn webhooks<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    user: Uuid,
) -> Result<Vec<OffboardRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT k.id, k.name, a.id AS agent_id, a.name AS agent_name, w.id AS workspace_id,
                w.name AS workspace_name, k.enabled, k.anchor, NULL::timestamptz AS next_fire_at
         FROM webhooks k JOIN agents a ON a.id = k.agent_id JOIN workspaces w ON w.id = a.workspace_id
         WHERE w.org_id = $1 AND ($2::uuid IS NULL OR w.id = $2)
           AND k.creator_user_id = $3 AND k.anchor = 'creator' AND k.enabled
         ORDER BY w.name, a.name, k.created_at, k.id",
    )
    .bind(org)
    .bind(workspace)
    .bind(user)
    .fetch_all(exec)
    .await?)
}

/// Open or queued peer tasks requested by `user` from agents in scope. The row's `name` is
/// `"<from agent> → <to agent>"`; peer tasks have no anchor and expire within 24 h.
pub async fn peer_tasks<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    user: Uuid,
) -> Result<Vec<OffboardRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT t.id, a.name || ' → ' || b.name AS name, a.id AS agent_id, a.name AS agent_name,
                w.id AS workspace_id, w.name AS workspace_name, true AS enabled,
                'creator'::text AS anchor, NULL::timestamptz AS next_fire_at
         FROM peer_tasks t JOIN agents a ON a.id = t.from_agent_id
         JOIN agents b ON b.id = t.to_agent_id JOIN workspaces w ON w.id = a.workspace_id
         WHERE w.org_id = $1 AND ($2::uuid IS NULL OR w.id = $2)
           AND t.requester_user_id = $3 AND t.status IN ('queued','open')
         ORDER BY w.name, t.expires_at, t.id",
    )
    .bind(org)
    .bind(workspace)
    .bind(user)
    .fetch_all(exec)
    .await?)
}

/// Live personal API keys of `user` that would stop working: every key in the org (org
/// scope), or the keys confined to the workspace (workspace scope).
pub async fn api_keys<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    user: Uuid,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM api_keys
         WHERE org_id = $1 AND user_id = $3 AND kind = 'user' AND revoked_at IS NULL
           AND (expires_at IS NULL OR expires_at > now())
           AND ($2::uuid IS NULL OR workspace_id = $2)",
    )
    .bind(org)
    .bind(workspace)
    .bind(user)
    .fetch_one(exec)
    .await?)
}
