//! Freeze state of agents, workspaces and organizations (phase 2, RFE-006, D-50; docs/04 §Freeze).
//!
//! An agent is **effectively frozen** when it, its workspace or its organization carries
//! `frozen_at`. The flags are independent. Every function here is plain SQL; the service owns the
//! authority rules, the audit row and the side effects.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use std::collections::HashMap;
use uuid::Uuid;

/// Where a freeze flag lives. Orders broadest first in [`Effective`] results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Agent,
    Workspace,
    Org,
}

impl Scope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Workspace => "workspace",
            Self::Org => "org",
        }
    }

    const fn table(self) -> &'static str {
        match self {
            Self::Agent => "agents",
            Self::Workspace => "workspaces",
            Self::Org => "organizations",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "agent" => Some(Self::Agent),
            "workspace" => Some(Self::Workspace),
            "org" => Some(Self::Org),
            _ => None,
        }
    }
}

/// Rank of a `frozen_authority`: `workspace_owner` < `org_admin` < `instance_admin`.
/// Unknown values rank above everything, so nobody lifts what they cannot read.
pub fn authority_rank(a: &str) -> u8 {
    match a {
        "workspace_owner" => 1,
        "org_admin" => 2,
        "instance_admin" => 3,
        _ => u8::MAX,
    }
}

/// The freeze record of one flag.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Frozen {
    pub at: DateTime<Utc>,
    pub by_user_id: Option<Uuid>,
    pub by_label: String,
    pub reason: String,
    pub authority: String,
}

/// The freeze record that makes an agent (or workspace) effectively frozen, with the level where
/// it is set. When several levels are set the broadest is reported (org, workspace, agent).
#[derive(Debug, Clone)]
pub struct Effective {
    pub level: Scope,
    pub frozen: Frozen,
}

/// The flag of one resource, `None` when it is not frozen (or does not exist). `lock` takes a row
/// lock (`FOR UPDATE`) so a freeze/unfreeze decision is serialized per resource.
pub async fn get(
    conn: &mut PgConnection,
    scope: Scope,
    id: Uuid,
    lock: bool,
) -> Result<Option<Frozen>, DbError> {
    let sql = format!(
        "SELECT t.frozen_at AS at, t.frozen_by_user_id AS by_user_id,
                coalesce(u.email::text, '(deleted user)') AS by_label,
                t.frozen_reason AS reason, t.frozen_authority AS authority
         FROM {} t LEFT JOIN users u ON u.id = t.frozen_by_user_id
         WHERE t.id = $1 AND t.frozen_at IS NOT NULL{}",
        scope.table(),
        if lock { " FOR UPDATE OF t" } else { "" }
    );
    Ok(sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?)
}

/// Lock the resource row (serializes concurrent freeze/unfreeze). `false` when it is gone.
pub async fn lock(conn: &mut PgConnection, scope: Scope, id: Uuid) -> Result<bool, DbError> {
    let sql = format!("SELECT 1 FROM {} WHERE id = $1 FOR UPDATE", scope.table());
    Ok(sqlx::query_scalar::<_, i32>(&sql)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .is_some())
}

/// Does the resource exist (no row lock)?
pub async fn exists(conn: &mut PgConnection, scope: Scope, id: Uuid) -> Result<bool, DbError> {
    let sql = format!("SELECT 1 FROM {} WHERE id = $1", scope.table());
    Ok(sqlx::query_scalar::<_, i32>(&sql)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .is_some())
}

/// Set the flag if it is not set. Returns `true` when this call froze it (`false`: it was already
/// frozen, the original record is kept).
pub async fn freeze(
    conn: &mut PgConnection,
    scope: Scope,
    id: Uuid,
    by: Uuid,
    reason: &str,
    authority: &str,
) -> Result<bool, DbError> {
    let sql = format!(
        "UPDATE {} SET frozen_at = now(), frozen_by_user_id = $2, frozen_reason = $3,
             frozen_authority = $4 WHERE id = $1 AND frozen_at IS NULL",
        scope.table()
    );
    Ok(sqlx::query(&sql)
        .bind(id)
        .bind(by)
        .bind(reason)
        .bind(authority)
        .execute(&mut *conn)
        .await?
        .rows_affected()
        == 1)
}

/// Clear the flag. Returns `true` when it was set.
pub async fn unfreeze(conn: &mut PgConnection, scope: Scope, id: Uuid) -> Result<bool, DbError> {
    let sql = format!(
        "UPDATE {} SET frozen_at = NULL, frozen_by_user_id = NULL, frozen_reason = NULL,
             frozen_authority = NULL WHERE id = $1 AND frozen_at IS NOT NULL",
        scope.table()
    );
    Ok(sqlx::query(&sql)
        .bind(id)
        .execute(&mut *conn)
        .await?
        .rows_affected()
        == 1)
}

/// SQL predicate: the agent named by `agent_col` is effectively frozen (agent, workspace or org
/// flag). Used by the due-work claims, which already alias their tables.
pub fn frozen_agent_sql(agent_col: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM agents fa
                 JOIN workspaces fw ON fw.id = fa.workspace_id
                 JOIN organizations fo ON fo.id = fa.org_id
                 WHERE fa.id = {agent_col}
                   AND (fa.frozen_at IS NOT NULL OR fw.frozen_at IS NOT NULL
                        OR fo.frozen_at IS NOT NULL))"
    )
}

fn level_of(s: &str) -> Scope {
    Scope::parse(s).unwrap_or(Scope::Agent)
}

#[derive(sqlx::FromRow)]
struct EffRow {
    agent_id: Uuid,
    lvl: String,
    at: DateTime<Utc>,
    by_user_id: Option<Uuid>,
    by_label: String,
    reason: String,
    authority: String,
}

impl From<EffRow> for (Uuid, Effective) {
    fn from(r: EffRow) -> Self {
        (
            r.agent_id,
            Effective {
                level: level_of(&r.lvl),
                frozen: Frozen {
                    at: r.at,
                    by_user_id: r.by_user_id,
                    by_label: r.by_label,
                    reason: r.reason,
                    authority: r.authority,
                },
            },
        )
    }
}

/// Effective freeze of each of `agents` (absent key = not frozen).
pub async fn effective_of_agents(
    conn: &mut PgConnection,
    agents: &[Uuid],
) -> Result<HashMap<Uuid, Effective>, DbError> {
    if agents.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<EffRow> = sqlx::query_as(
        "SELECT DISTINCT ON (x.agent_id) x.agent_id, x.lvl, x.at, x.by_user_id, x.by_label,
                x.reason, x.authority
         FROM (
           SELECT a.id AS agent_id, 3 AS ord, 'org' AS lvl, o.frozen_at AS at,
                  o.frozen_by_user_id AS by_user_id,
                  coalesce(u.email::text, '(deleted user)') AS by_label,
                  o.frozen_reason AS reason, o.frozen_authority AS authority
           FROM agents a JOIN organizations o ON o.id = a.org_id
                LEFT JOIN users u ON u.id = o.frozen_by_user_id
           WHERE a.id = ANY($1) AND o.frozen_at IS NOT NULL
           UNION ALL
           SELECT a.id, 2, 'workspace', w.frozen_at, w.frozen_by_user_id,
                  coalesce(u.email::text, '(deleted user)'), w.frozen_reason, w.frozen_authority
           FROM agents a JOIN workspaces w ON w.id = a.workspace_id
                LEFT JOIN users u ON u.id = w.frozen_by_user_id
           WHERE a.id = ANY($1) AND w.frozen_at IS NOT NULL
           UNION ALL
           SELECT a.id, 1, 'agent', a.frozen_at, a.frozen_by_user_id,
                  coalesce(u.email::text, '(deleted user)'), a.frozen_reason, a.frozen_authority
           FROM agents a LEFT JOIN users u ON u.id = a.frozen_by_user_id
           WHERE a.id = ANY($1) AND a.frozen_at IS NOT NULL
         ) x
         ORDER BY x.agent_id, x.ord DESC",
    )
    .bind(agents)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Effective freeze of one agent.
pub async fn effective_of_agent(
    conn: &mut PgConnection,
    agent: Uuid,
) -> Result<Option<Effective>, DbError> {
    Ok(effective_of_agents(conn, &[agent]).await?.remove(&agent))
}

/// Effective freeze of a workspace: its own flag or its organization's (broadest first).
pub async fn effective_of_workspace(
    conn: &mut PgConnection,
    ws: Uuid,
) -> Result<Option<Effective>, DbError> {
    let org: Option<Uuid> = sqlx::query_scalar("SELECT org_id FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(org) = org else { return Ok(None) };
    if let Some(frozen) = get(conn, Scope::Org, org, false).await? {
        return Ok(Some(Effective {
            level: Scope::Org,
            frozen,
        }));
    }
    Ok(get(conn, Scope::Workspace, ws, false)
        .await?
        .map(|frozen| Effective {
            level: Scope::Workspace,
            frozen,
        }))
}

#[derive(sqlx::FromRow)]
struct WsRow {
    id: Uuid,
    lvl: String,
    at: DateTime<Utc>,
    by_user_id: Option<Uuid>,
    by_label: String,
    reason: String,
    authority: String,
}

/// Effective freeze of each of `workspaces` (own flag or its organization's; broadest wins).
pub async fn effective_of_workspaces(
    conn: &mut PgConnection,
    workspaces: &[Uuid],
) -> Result<HashMap<Uuid, Effective>, DbError> {
    if workspaces.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<WsRow> = sqlx::query_as(
        "SELECT DISTINCT ON (x.id) x.id, x.lvl, x.at, x.by_user_id, x.by_label, x.reason,
                x.authority
         FROM (
           SELECT w.id, 2 AS ord, 'org' AS lvl, o.frozen_at AS at, o.frozen_by_user_id AS by_user_id,
                  coalesce(u.email::text, '(deleted user)') AS by_label,
                  o.frozen_reason AS reason, o.frozen_authority AS authority
           FROM workspaces w JOIN organizations o ON o.id = w.org_id
                LEFT JOIN users u ON u.id = o.frozen_by_user_id
           WHERE w.id = ANY($1) AND o.frozen_at IS NOT NULL
           UNION ALL
           SELECT w.id, 1, 'workspace', w.frozen_at, w.frozen_by_user_id,
                  coalesce(u.email::text, '(deleted user)'), w.frozen_reason, w.frozen_authority
           FROM workspaces w LEFT JOIN users u ON u.id = w.frozen_by_user_id
           WHERE w.id = ANY($1) AND w.frozen_at IS NOT NULL
         ) x
         ORDER BY x.id, x.ord DESC",
    )
    .bind(workspaces)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.id,
                Effective {
                    level: level_of(&r.lvl),
                    frozen: Frozen {
                        at: r.at,
                        by_user_id: r.by_user_id,
                        by_label: r.by_label,
                        reason: r.reason,
                        authority: r.authority,
                    },
                },
            )
        })
        .collect())
}

/// Own freeze flag of each of `orgs` (absent key = not frozen).
pub async fn frozen_of_orgs(
    conn: &mut PgConnection,
    orgs: &[Uuid],
) -> Result<HashMap<Uuid, Frozen>, DbError> {
    if orgs.is_empty() {
        return Ok(HashMap::new());
    }
    #[derive(sqlx::FromRow)]
    struct OrgFrozenRow {
        id: Uuid,
        #[sqlx(flatten)]
        f: Frozen,
    }
    let rows: Vec<OrgFrozenRow> = sqlx::query_as(
        "SELECT o.id, o.frozen_at AS at, o.frozen_by_user_id AS by_user_id,
                coalesce(u.email::text, '(deleted user)') AS by_label,
                o.frozen_reason AS reason, o.frozen_authority AS authority
         FROM organizations o LEFT JOIN users u ON u.id = o.frozen_by_user_id
         WHERE o.id = ANY($1) AND o.frozen_at IS NOT NULL",
    )
    .bind(orgs)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.f)).collect())
}

/// Is the agent effectively frozen?
pub async fn is_agent_frozen(conn: &mut PgConnection, agent: Uuid) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar(&format!("SELECT {}", frozen_agent_sql("$1")))
            .bind(agent)
            .fetch_one(&mut *conn)
            .await?,
    )
}

/// Agents covered by a flag of `scope`.
pub async fn agents_in(
    conn: &mut PgConnection,
    scope: Scope,
    id: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    let sql = match scope {
        Scope::Agent => "SELECT id FROM agents WHERE id = $1",
        Scope::Workspace => "SELECT id FROM agents WHERE workspace_id = $1",
        Scope::Org => "SELECT id FROM agents WHERE org_id = $1",
    };
    Ok(sqlx::query_scalar(sql)
        .bind(id)
        .fetch_all(&mut *conn)
        .await?)
}

/// Pending gateway approvals of `agents` -> `abandoned` (they never revive). Returns
/// `(approval, workspace)`; the caller notifies waiters.
pub async fn abandon_pending_approvals(
    conn: &mut PgConnection,
    agents: &[Uuid],
) -> Result<Vec<(Uuid, Uuid)>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE approvals SET status = 'abandoned', decided_at = now(), updated_at = now()
         WHERE status = 'pending' AND agent_id = ANY($1) RETURNING id, workspace_id",
    )
    .bind(agents)
    .fetch_all(&mut *conn)
    .await?)
}

/// Pending action approvals of `agents` -> `rejected` with reason `frozen`.
pub async fn reject_pending_action_approvals(
    conn: &mut PgConnection,
    agents: &[Uuid],
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE action_approvals SET status = 'rejected', error = 'frozen', decided_at = now(),
             updated_at = now()
         WHERE status = 'pending' AND agent_id = ANY($1)",
    )
    .bind(agents)
    .execute(&mut *conn)
    .await?
    .rows_affected())
}

/// Active turns (`queued`, `dispatched`, `running`) of `agents`.
pub async fn active_turns_of(
    conn: &mut PgConnection,
    agents: &[Uuid],
    limit: i64,
) -> Result<Vec<(Uuid, Uuid)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, conversation_id FROM turns
         WHERE role = 'turn' AND status IN ('queued','dispatched','running')
           AND agent_id = ANY($1) ORDER BY created_at LIMIT $2",
    )
    .bind(agents)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?)
}

/// Active turns of effectively frozen agents (the sweep that catches a turn created in a race
/// with the freeze).
pub async fn active_turns_of_frozen(
    conn: &mut PgConnection,
    limit: i64,
) -> Result<Vec<(Uuid, Uuid)>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT t.id, t.conversation_id FROM turns t
         WHERE t.role = 'turn' AND t.status IN ('queued','dispatched','running')
           AND {} ORDER BY t.created_at LIMIT $1",
        frozen_agent_sql("t.agent_id")
    ))
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?)
}

/// One organization with its own freeze flag (the instance-admin list).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AdminOrgRow {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub frozen_at: Option<DateTime<Utc>>,
    pub frozen_by_user_id: Option<Uuid>,
    pub by_label: Option<String>,
    pub frozen_reason: Option<String>,
    pub frozen_authority: Option<String>,
}

impl AdminOrgRow {
    pub fn frozen(&self) -> Option<Frozen> {
        Some(Frozen {
            at: self.frozen_at?,
            by_user_id: self.frozen_by_user_id,
            by_label: self
                .by_label
                .clone()
                .unwrap_or_else(|| "(deleted user)".into()),
            reason: self.frozen_reason.clone().unwrap_or_default(),
            authority: self.frozen_authority.clone().unwrap_or_default(),
        })
    }
}

/// Organizations, newest first, for the operator view; `frozen_only` keeps flagged ones.
pub async fn list_orgs(
    conn: &mut PgConnection,
    frozen_only: bool,
    after: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> Result<Vec<AdminOrgRow>, DbError> {
    let (t, id) = after.map_or((None, None), |(t, i)| (Some(t), Some(i)));
    Ok(sqlx::query_as(
        "SELECT o.id, o.slug, o.name, o.created_at, o.frozen_at, o.frozen_by_user_id,
                u.email::text AS by_label, o.frozen_reason, o.frozen_authority
         FROM organizations o LEFT JOIN users u ON u.id = o.frozen_by_user_id
         WHERE (NOT $1 OR o.frozen_at IS NOT NULL)
           AND ($2::timestamptz IS NULL OR (o.created_at, o.id) < ($2, $3))
         ORDER BY o.created_at DESC, o.id DESC LIMIT $4",
    )
    .bind(frozen_only)
    .bind(t)
    .bind(id)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?)
}
