//! (P5-REFINE-8, D-417, C-1080, C-1082) The workspace feed's account source: the accounts of a
//! workspace's accounts list (its own and the organization's, `GET /v1/workspaces/{ws}/accounts`) whose
//! health may be `failing` or `sign_in_expired`, in one bounded query. The API decides the state with
//! the same function the accounts list uses (`accounts::health::state_of`); this query only narrows the
//! rows to the candidates, so the feed and the counts read exactly the same set.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccountHealthRow {
    /// An app connection (`true`) or a secret (a website token or an AI key).
    pub is_connection: bool,
    pub id: Uuid,
    /// The connection's label or the secret's name.
    pub name: String,
    pub app_id: Option<String>,
    /// `secrets.kind` for a secret.
    pub secret_kind: Option<String>,
    /// The organization's account (`workspace_id IS NULL`).
    pub org_account: bool,
    /// The connection's status; `None` for a secret (its health has no status, C-1033).
    pub status: Option<String>,
    pub last_ok_at: Option<DateTime<Utc>>,
    pub last_error_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// When it started needing attention: the failure, or the status change.
    pub since: DateTime<Utc>,
}

/// An agent's access request with the agent's name and a website's host, for the workspace feed. The
/// agent's own words (`reason`) are not read: the feed never shows them.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccessRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub agent_name: String,
    /// `app` | `website`.
    pub kind: String,
    pub app_id: Option<String>,
    pub host: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// `done = false`: the workspace's open access requests, oldest first. `done = true`: those given or
/// dismissed in the last `days`, newest first.
pub async fn access_requests<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace: Uuid,
    done: bool,
    days: i32,
    limit: i64,
) -> Result<Vec<AccessRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT r.id, r.agent_id, ag.name AS agent_name, r.kind, r.app_id, s.host_pattern AS host, \
                r.status, r.created_at, r.resolved_at \
         FROM agent_access_requests r JOIN agents ag ON ag.id = r.agent_id \
         LEFT JOIN secrets s ON s.id = r.secret_id \
         WHERE r.workspace_id = $1 AND CASE WHEN $2 \
             THEN r.status IN ('given', 'dismissed') AND r.resolved_at >= now() - make_interval(days => $3) \
             ELSE r.status = 'open' END \
         ORDER BY CASE WHEN $2 THEN r.resolved_at END DESC, r.created_at, r.id LIMIT $4",
    )
    .bind(workspace)
    .bind(done)
    .bind(days)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// `(id, name)` of agents.
pub async fn agent_names<'e, E: PgExecutor<'e>>(
    exec: E,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, name FROM agents WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(exec)
            .await?,
    )
}

/// The candidates: a connection that must sign in again, or any account whose newest recorded call
/// failed. Orphaned connections and inactive secrets are not in the accounts list, so not here either.
pub async fn accounts_needing_attention<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Uuid,
    limit: i64,
) -> Result<Vec<AccountHealthRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT * FROM ( \
           SELECT true AS is_connection, c.id, c.label AS name, c.app_id, NULL::text AS secret_kind, \
                  c.workspace_id IS NULL AS org_account, c.status, c.last_ok_at, c.last_error_at, \
                  c.last_error, \
                  CASE WHEN c.status = 'reauth_required' THEN c.updated_at \
                       ELSE COALESCE(c.last_error_at, c.updated_at) END AS since \
           FROM app_connections c \
           WHERE c.org_id = $1 AND (c.workspace_id IS NULL OR c.workspace_id = $2) \
             AND c.status <> 'orphaned' \
             AND (c.status = 'reauth_required' OR (c.status <> 'disabled' AND c.last_error_at IS NOT NULL \
                  AND (c.last_ok_at IS NULL OR c.last_error_at >= c.last_ok_at))) \
           UNION ALL \
           SELECT false, s.id, s.name, NULL, s.kind, s.workspace_id IS NULL, NULL, s.last_ok_at, \
                  s.last_error_at, s.last_error, COALESCE(s.last_error_at, s.updated_at) \
           FROM secrets s \
           WHERE s.org_id = $1 AND (s.workspace_id IS NULL OR s.workspace_id = $2) \
             AND s.status = 'active' AND s.last_error_at IS NOT NULL \
             AND (s.last_ok_at IS NULL OR s.last_error_at >= s.last_ok_at) \
         ) x ORDER BY x.since, x.id LIMIT $3",
    )
    .bind(org)
    .bind(workspace)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}
