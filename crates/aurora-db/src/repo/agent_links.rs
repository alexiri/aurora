//! Agent links (docs/07 §Agent links): one row per unordered pair, ordered `agent_a_id <
//! agent_b_id`, with a policy per side and an optimistic-concurrency `revision`.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AgentLinkRow {
    pub id: Uuid,
    pub agent_a_id: Uuid,
    pub agent_b_id: Uuid,
    pub policy_a: String,
    pub policy_b: String,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentLinkRow {
    /// Policy of `agent`'s side (`None` when the agent is not part of the link).
    pub fn policy_of(&self, agent: Uuid) -> Option<&str> {
        if agent == self.agent_a_id {
            Some(&self.policy_a)
        } else if agent == self.agent_b_id {
            Some(&self.policy_b)
        } else {
            None
        }
    }

    /// The other agent of the pair.
    pub fn other(&self, agent: Uuid) -> Option<Uuid> {
        if agent == self.agent_a_id {
            Some(self.agent_b_id)
        } else if agent == self.agent_b_id {
            Some(self.agent_a_id)
        } else {
            None
        }
    }
}

/// Order a pair as the table requires.
pub fn ordered(x: Uuid, y: Uuid) -> (Uuid, Uuid) {
    if x < y { (x, y) } else { (y, x) }
}

const COLS: &str =
    "id, agent_a_id, agent_b_id, policy_a, policy_b, revision, created_at, updated_at";

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<AgentLinkRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM agent_links WHERE id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// Row-locked read (transaction required): the per-pair serialization point.
pub async fn get_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<AgentLinkRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM agent_links WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Get-or-create the link of a pair; a new link starts with `initial` on both sides. Returns the
/// row and whether it was created.
pub async fn get_or_create(
    conn: &mut PgConnection,
    x: Uuid,
    y: Uuid,
    initial: &str,
) -> Result<(AgentLinkRow, bool), DbError> {
    let (a, b) = ordered(x, y);
    let done = sqlx::query(
        "INSERT INTO agent_links (id, agent_a_id, agent_b_id, policy_a, policy_b)
         VALUES ($1,$2,$3,$4,$4) ON CONFLICT (agent_a_id, agent_b_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(a)
    .bind(b)
    .bind(initial)
    .execute(&mut *conn)
    .await?;
    let row = sqlx::query_as(&format!(
        "SELECT {COLS} FROM agent_links WHERE agent_a_id = $1 AND agent_b_id = $2"
    ))
    .bind(a)
    .bind(b)
    .fetch_one(conn)
    .await?;
    Ok((row, done.rows_affected() == 1))
}

/// Links an agent takes part in, oldest first.
pub async fn list_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<AgentLinkRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM agent_links WHERE agent_a_id = $1 OR agent_b_id = $1
         ORDER BY created_at, id"
    ))
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

/// Set one side's policy, bumping `revision`. With `expected` the update is fenced on the
/// revision; `None` when the fence (or the link) does not hold.
pub async fn set_policy(
    conn: &mut PgConnection,
    id: Uuid,
    agent: Uuid,
    policy: &str,
    expected: Option<i64>,
) -> Result<Option<AgentLinkRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE agent_links SET
            policy_a = CASE WHEN agent_a_id = $2 THEN $3 ELSE policy_a END,
            policy_b = CASE WHEN agent_b_id = $2 THEN $3 ELSE policy_b END,
            revision = revision + 1, updated_at = now()
         WHERE id = $1 AND (agent_a_id = $2 OR agent_b_id = $2)
           AND ($4::bigint IS NULL OR revision = $4)
         RETURNING {COLS}"
    ))
    .bind(id)
    .bind(agent)
    .bind(policy)
    .bind(expected)
    .fetch_optional(conn)
    .await?)
}

/// Agents of `org` that `key` (an id, `handle` or `@handle`) names. A handle matches the
/// caller's own workspace first, then the whole org; the result has more than one entry only
/// when the handle is ambiguous.
pub async fn find_addressable(
    conn: &mut PgConnection,
    org_id: Uuid,
    own_workspace: Uuid,
    key: &str,
) -> Result<Vec<Uuid>, DbError> {
    let key = key.trim();
    if let Ok(id) = Uuid::parse_str(key) {
        return Ok(
            sqlx::query_scalar("SELECT id FROM agents WHERE id = $1 AND org_id = $2")
                .bind(id)
                .bind(org_id)
                .fetch_all(conn)
                .await?,
        );
    }
    let handle = key.strip_prefix('@').unwrap_or(key);
    let own: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM agents WHERE org_id = $1 AND workspace_id = $2 AND handle = $3",
    )
    .bind(org_id)
    .bind(own_workspace)
    .bind(handle)
    .fetch_all(&mut *conn)
    .await?;
    if !own.is_empty() {
        return Ok(own);
    }
    Ok(
        sqlx::query_scalar("SELECT id FROM agents WHERE org_id = $1 AND handle = $2 LIMIT 2")
            .bind(org_id)
            .bind(handle)
            .fetch_all(conn)
            .await?,
    )
}
