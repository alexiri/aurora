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
    /// (P5-REFINE-10, C-1085) `both` | `a_to_b` | `b_to_a` (a = the lower id).
    pub direction: String,
    /// `person` | `agent`.
    pub created_by_kind: String,
    pub created_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Who may hand tasks to whom, read from one agent's side (C-1085 request form).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideDirection {
    Both,
    /// This agent hands tasks to the peer.
    ToPeer,
    /// The peer hands tasks to this agent.
    FromPeer,
}

/// The stored form of `dir` read from `me`'s side of the pair (`me`, `peer`).
pub fn stored_direction(me: Uuid, peer: Uuid, dir: SideDirection) -> &'static str {
    let (from, to) = match dir {
        SideDirection::Both => return "both",
        SideDirection::ToPeer => (me, peer),
        SideDirection::FromPeer => (peer, me),
    };
    if from < to { "a_to_b" } else { "b_to_a" }
}

/// Does the stored `direction` of the ordered pair (`a`, `b`) let `from` hand `to` a task?
pub fn direction_allows(direction: &str, a: Uuid, from: Uuid) -> bool {
    match direction {
        "a_to_b" => from == a,
        "b_to_a" => from != a,
        _ => true,
    }
}

/// The (a→b, b→a) flows a stored direction allows.
fn flows(direction: &str) -> (bool, bool) {
    match direction {
        "a_to_b" => (true, false),
        "b_to_a" => (false, true),
        _ => (true, true),
    }
}

/// Does moving from `old` to `new` let some agent hand tasks it could not before?
pub fn direction_widens(old: &str, new: &str) -> bool {
    let (oa, ob) = flows(old);
    let (na, nb) = flows(new);
    (na && !oa) || (nb && !ob)
}

impl AgentLinkRow {
    /// The stored direction read from `me`'s side: `both`, `outgoing` (me → peer) or `incoming`.
    pub fn direction_of(&self, me: Uuid) -> &'static str {
        match self.direction.as_str() {
            "a_to_b" if me == self.agent_a_id => "outgoing",
            "a_to_b" => "incoming",
            "b_to_a" if me == self.agent_b_id => "outgoing",
            "b_to_a" => "incoming",
            _ => "both",
        }
    }

    /// May `from` open a task for the other agent of the pair?
    pub fn allows(&self, from: Uuid) -> bool {
        direction_allows(&self.direction, self.agent_a_id, from)
    }

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

const COLS: &str = "id, agent_a_id, agent_b_id, policy_a, policy_b, revision, direction, \
    created_by_kind, created_by_user_id, created_at, updated_at";

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<AgentLinkRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM agent_links WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// Row-locked read (transaction required): the per-pair serialization point.
pub async fn get_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<AgentLinkRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM agent_links WHERE id = $1 FOR UPDATE"
    )))
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
    let row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM agent_links WHERE agent_a_id = $1 AND agent_b_id = $2"
    )))
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
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM agent_links WHERE agent_a_id = $1 OR agent_b_id = $1
         ORDER BY created_at, id"
    )))
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
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE agent_links SET
            policy_a = CASE WHEN agent_a_id = $2 THEN $3 ELSE policy_a END,
            policy_b = CASE WHEN agent_b_id = $2 THEN $3 ELSE policy_b END,
            revision = revision + 1, updated_at = now()
         WHERE id = $1 AND (agent_a_id = $2 OR agent_b_id = $2)
           AND ($4::bigint IS NULL OR revision = $4)
         RETURNING {COLS}"
    )))
    .bind(id)
    .bind(agent)
    .bind(policy)
    .bind(expected)
    .fetch_optional(conn)
    .await?)
}

/// The link of a pair, if any.
pub async fn get_pair<'e, E: PgExecutor<'e>>(
    exec: E,
    x: Uuid,
    y: Uuid,
) -> Result<Option<AgentLinkRow>, DbError> {
    let (a, b) = ordered(x, y);
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM agent_links WHERE agent_a_id = $1 AND agent_b_id = $2"
    )))
    .bind(a)
    .bind(b)
    .fetch_optional(exec)
    .await?)
}

/// A link a person creates (P5-REFINE-10, C-1085): the policy of each side, the stored
/// direction and the creator. `None` when the pair is linked already (nothing written).
pub struct NewPersonLink {
    pub x: Uuid,
    pub policy_x: &'static str,
    pub y: Uuid,
    pub policy_y: &'static str,
    /// Stored form (`both` | `a_to_b` | `b_to_a`).
    pub direction: &'static str,
    pub user_id: Uuid,
}

pub async fn create_by_person(
    conn: &mut PgConnection,
    n: &NewPersonLink,
) -> Result<Option<AgentLinkRow>, DbError> {
    let (a, b) = ordered(n.x, n.y);
    let (pa, pb) = if a == n.x {
        (n.policy_x, n.policy_y)
    } else {
        (n.policy_y, n.policy_x)
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO agent_links (id, agent_a_id, agent_b_id, policy_a, policy_b, direction,
                                  created_by_kind, created_by_user_id)
         VALUES ($1,$2,$3,$4,$5,$6,'person',$7)
         ON CONFLICT (agent_a_id, agent_b_id) DO NOTHING
         RETURNING {COLS}"
    )))
    .bind(Uuid::now_v7())
    .bind(a)
    .bind(b)
    .bind(pa)
    .bind(pb)
    .bind(n.direction)
    .bind(n.user_id)
    .fetch_optional(conn)
    .await?)
}

/// Set the stored direction, bumping `revision` (fenced on `expected`); `None` when the fence or
/// the link does not hold.
pub async fn set_direction(
    conn: &mut PgConnection,
    id: Uuid,
    direction: &str,
    expected: i64,
) -> Result<Option<AgentLinkRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE agent_links SET direction = $2, revision = revision + 1, updated_at = now()
         WHERE id = $1 AND revision = $3
         RETURNING {COLS}"
    )))
    .bind(id)
    .bind(direction)
    .bind(expected)
    .fetch_optional(conn)
    .await?)
}

/// Delete a link (its `peer_tasks` cascade). `true` when a row went.
pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM agent_links WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?
        .rows_affected()
        == 1)
}

/// Queued, then open or waiting, peer tasks of a link (the order a delete closes them in, so
/// that closing the task that holds a slot never promotes a queued one).
pub async fn active_task_ids(conn: &mut PgConnection, link: Uuid) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM peer_tasks WHERE link_id = $1 AND status IN ('queued', 'open', 'waiting')
         ORDER BY (status <> 'queued'), created_at, id",
    )
    .bind(link)
    .fetch_all(conn)
    .await?)
}

/// Pending `message_agent` action approvals of a link, row-locked; rows a concurrent decision
/// holds are skipped (that decision then finds the link gone and fails).
pub async fn lock_pending_approval_ids(
    conn: &mut PgConnection,
    link: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM action_approvals
          WHERE action_key = 'message_agent' AND status = 'pending'
            AND payload->>'link_id' = $1
          ORDER BY created_at, id
          FOR UPDATE SKIP LOCKED",
    )
    .bind(link.to_string())
    .fetch_all(conn)
    .await?)
}

/// What the people's view of a link adds to the row: the peer's workspace name and the
/// creator's display name (null for an agent, or once the user is deleted).
pub async fn view_context<'e, E: PgExecutor<'e>>(
    exec: E,
    peer_workspace: Uuid,
    creator: Option<Uuid>,
) -> Result<(String, Option<String>), DbError> {
    Ok(sqlx::query_as(
        "SELECT (SELECT name FROM workspaces WHERE id = $1),
                (SELECT display_name FROM users WHERE id = $2)",
    )
    .bind(peer_workspace)
    .bind(creator)
    .fetch_one(exec)
    .await
    .map(|(w, u): (Option<String>, Option<String>)| (w.unwrap_or_default(), u))?)
}

/// A candidate of `GET /v1/agents/{agent}/linkable` (C-1085): name, handle and workspace only.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkableRow {
    pub agent_id: Uuid,
    pub name: String,
    pub handle: String,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub linked: bool,
    pub hosted: bool,
    pub frozen: bool,
}

/// Most candidates `linkable` returns.
pub const LINKABLE_MAX: i64 = 200;

/// Agents of `org` other than `agent` in the workspaces `user` can use (the access law's
/// `effective_workspace_access`, so an org owner/admin sees every workspace); `only_workspace`
/// confines the list to one workspace (a workspace API key). Sorted by workspace name, then
/// agent name; at most [`LINKABLE_MAX`].
pub async fn linkable<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    org: Uuid,
    agent: Uuid,
    only_workspace: Option<Uuid>,
) -> Result<Vec<LinkableRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT a.id AS agent_id, a.name, a.handle, a.workspace_id, w.name AS workspace_name,
                EXISTS (SELECT 1 FROM agent_links l
                         WHERE l.agent_a_id = LEAST($3::uuid, a.id)
                           AND l.agent_b_id = GREATEST($3::uuid, a.id)) AS linked,
                a.kind = 'hosted' AS hosted,
                {} AS frozen
           FROM agents a
           JOIN workspaces w ON w.id = a.workspace_id
           JOIN effective_workspace_access ea ON ea.workspace_id = a.workspace_id
                                             AND ea.user_id = $1
          WHERE a.org_id = $2 AND w.org_id = $2 AND a.id <> $3
            AND ($4::uuid IS NULL OR a.workspace_id = $4)
          ORDER BY w.name, a.name, a.id
          LIMIT $5",
        super::freeze::frozen_agent_sql("a.id")
    )))
    .bind(user)
    .bind(org)
    .bind(agent)
    .bind(only_workspace)
    .bind(LINKABLE_MAX)
    .fetch_all(exec)
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod direction_tests {
    use super::*;

    fn row(a: Uuid, b: Uuid, direction: &str) -> AgentLinkRow {
        AgentLinkRow {
            id: Uuid::nil(),
            agent_a_id: a,
            agent_b_id: b,
            policy_a: "allow".into(),
            policy_b: "allow".into(),
            revision: 0,
            direction: direction.into(),
            created_by_kind: "person".into(),
            created_by_user_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// (low, high) ids, so both orders of "me" can be checked.
    fn pair() -> (Uuid, Uuid) {
        let x = Uuid::now_v7();
        let y = Uuid::now_v7();
        ordered(x, y)
    }

    #[test]
    fn side_to_stored_in_both_orders() {
        let (lo, hi) = pair();
        for (me, peer) in [(lo, hi), (hi, lo)] {
            assert_eq!(stored_direction(me, peer, SideDirection::Both), "both");
            let to = stored_direction(me, peer, SideDirection::ToPeer);
            let from = stored_direction(me, peer, SideDirection::FromPeer);
            assert_ne!(to, from);
            let r = row(lo, hi, to);
            assert_eq!(r.direction_of(me), "outgoing");
            assert_eq!(r.direction_of(peer), "incoming");
            assert!(r.allows(me) && !r.allows(peer));
            let r = row(lo, hi, from);
            assert_eq!(r.direction_of(me), "incoming");
            assert_eq!(r.direction_of(peer), "outgoing");
            assert!(!r.allows(me) && r.allows(peer));
        }
        assert_eq!(stored_direction(lo, hi, SideDirection::ToPeer), "a_to_b");
        assert_eq!(stored_direction(hi, lo, SideDirection::ToPeer), "b_to_a");
        assert_eq!(stored_direction(hi, lo, SideDirection::FromPeer), "a_to_b");
        let r = row(lo, hi, "both");
        assert_eq!((r.direction_of(lo), r.direction_of(hi)), ("both", "both"));
        assert!(r.allows(lo) && r.allows(hi));
    }

    #[test]
    fn widening_is_any_new_flow() {
        for (old, new, widens) in [
            ("a_to_b", "both", true),
            ("b_to_a", "both", true),
            ("a_to_b", "b_to_a", true),
            ("b_to_a", "a_to_b", true),
            ("both", "a_to_b", false),
            ("both", "b_to_a", false),
            ("both", "both", false),
            ("a_to_b", "a_to_b", false),
        ] {
            assert_eq!(direction_widens(old, new), widens, "{old} -> {new}");
        }
    }
}
