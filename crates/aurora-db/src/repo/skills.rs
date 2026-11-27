//! Skills (docs/10 §Skills, 03 §7): agent- and organization-tier instruction bundles + files.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

pub(crate) const COLS: &str = "id, org_id, agent_id, workspace_id, name, description, body, \
                    enabled, created_by_kind, created_at, updated_at, library_id, library_label, \
                    library_repo, library_path, library_commit_sha, library_blobs, \
                    library_content_sha256, library_pinned_at, library_pinned_by_user_id, title";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SkillRow {
    pub id: Uuid,
    pub org_id: Uuid,
    /// Set => agent tier.
    pub agent_id: Option<Uuid>,
    /// Set => workspace tier (never together with `agent_id`); both null => organization tier.
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub body: String,
    pub enabled: bool,
    pub created_by_kind: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// (phase 4, P4-GSL) non-null => added from a git skill library: pinned and **read-only**.
    pub library_id: Option<Uuid>,
    /// Provenance snapshot; survives library deletion and detach ("was from <repo>@<sha>").
    pub library_label: Option<String>,
    pub library_repo: Option<String>,
    pub library_path: Option<String>,
    pub library_commit_sha: Option<String>,
    /// `{ "<relative path>": "<blob id>" }` of `SKILL.md` and every file.
    pub library_blobs: Option<serde_json::Value>,
    pub library_content_sha256: Option<Vec<u8>>,
    pub library_pinned_at: Option<DateTime<Utc>>,
    pub library_pinned_by_user_id: Option<Uuid>,
    /// (P5-SKILLS-1, C-1173) The display name; null: the folder name made readable.
    pub title: Option<String>,
}

/// A skill as it reaches one agent: the row and whether it is turned off **for that agent**.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReachingRow {
    #[sqlx(flatten)]
    pub row: SkillRow,
    pub off_for_agent: bool,
}

impl ReachingRow {
    /// Whether the skill can win its name for this agent: switched on by its owners and not
    /// turned off for the agent.
    #[must_use]
    pub fn applies(&self) -> bool {
        self.row.enabled && !self.off_for_agent
    }
}

/// Precedence rank: lower wins (agent > workspace > organization).
#[must_use]
pub fn rank(r: &SkillRow) -> u8 {
    if r.agent_id.is_some() {
        0
    } else if r.workspace_id.is_some() {
        1
    } else {
        2
    }
}

/// The skills of one name.
#[derive(Debug)]
pub struct NameGroup {
    /// The skill the agent receives; none when no skill of the name applies.
    pub winner: Option<ReachingRow>,
    /// Every other skill of the name, highest tier first.
    pub others: Vec<ReachingRow>,
}

/// **The one rule of an agent's effective set** (RFE-020, C-1174): per name the highest tier
/// that applies ([`ReachingRow::applies`]) wins, agent > workspace > organization; a skill that
/// is switched off, or turned off for the agent, never vetoes a lower tier. The API's effective
/// set, the files projected into the agent's computer, the configuration fingerprint and the
/// mapping of a reported skill use all come from here. Groups are ordered by name.
#[must_use]
pub fn resolve(mut rows: Vec<ReachingRow>) -> Vec<NameGroup> {
    rows.sort_by(|a, b| {
        (a.row.name.as_str(), rank(&a.row)).cmp(&(b.row.name.as_str(), rank(&b.row)))
    });
    let mut out = Vec::new();
    let mut it = rows.into_iter().peekable();
    while let Some(first) = it.next() {
        let mut others = vec![first];
        while it.peek().is_some_and(|n| n.row.name == others[0].row.name) {
            others.extend(it.next());
        }
        let winner = others
            .iter()
            .position(ReachingRow::applies)
            .map(|i| others.remove(i));
        out.push(NameGroup { winner, others });
    }
    out
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FileRow {
    pub skill_id: Uuid,
    pub path: String,
    pub content: Vec<u8>,
    pub executable: bool,
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> Result<Option<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM skills WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Row lock: serializes concurrent edits (and file uploads) of one skill.
pub async fn get_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM skills WHERE id = $1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

pub async fn get_agent_named(
    conn: &mut PgConnection,
    agent: Uuid,
    name: &str,
) -> Result<Option<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM skills WHERE agent_id = $1 AND name = $2"
    )))
    .bind(agent)
    .bind(name)
    .fetch_optional(conn)
    .await?)
}

pub async fn get_org_named(
    conn: &mut PgConnection,
    org: Uuid,
    name: &str,
) -> Result<Option<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM skills WHERE org_id = $1 AND agent_id IS NULL
         AND workspace_id IS NULL AND name = $2"
    )))
    .bind(org)
    .bind(name)
    .fetch_optional(conn)
    .await?)
}

pub async fn get_workspace_named(
    conn: &mut PgConnection,
    workspace: Uuid,
    name: &str,
) -> Result<Option<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM skills WHERE workspace_id = $1 AND name = $2"
    )))
    .bind(workspace)
    .bind(name)
    .fetch_optional(conn)
    .await?)
}

pub async fn list_workspace(
    conn: &mut PgConnection,
    workspace: Uuid,
) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM skills WHERE workspace_id = $1 ORDER BY name"
    )))
    .bind(workspace)
    .fetch_all(conn)
    .await?)
}

/// Every skill of the three tiers that reaches the agent (its own, its workspace's, its
/// organization's), enabled or not, each with whether it is turned off for the agent; with
/// `name`, only the skills of that folder name.
pub async fn list_reaching(
    conn: &mut PgConnection,
    org: Uuid,
    workspace: Uuid,
    agent: Uuid,
    name: Option<&str>,
) -> Result<Vec<ReachingRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS}, EXISTS (SELECT 1 FROM agent_skills_off o
                                WHERE o.agent_id = $3 AND o.skill_id = skills.id) AS off_for_agent
         FROM skills WHERE org_id = $1
         AND (agent_id = $3 OR workspace_id = $2 OR (agent_id IS NULL AND workspace_id IS NULL))
         AND ($4::text IS NULL OR name = $4)
         ORDER BY name"
    )))
    .bind(org)
    .bind(workspace)
    .bind(agent)
    .bind(name)
    .fetch_all(conn)
    .await?)
}

/// Turns the skill off for the agent; false when it already was.
pub async fn set_off(
    conn: &mut PgConnection,
    agent: Uuid,
    skill: Uuid,
    by: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO agent_skills_off (agent_id, skill_id, set_by) VALUES ($1, $2, $3)
         ON CONFLICT (agent_id, skill_id) DO NOTHING",
    )
    .bind(agent)
    .bind(skill)
    .bind(by)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Turns the skill back on for the agent; false when it was not off.
pub async fn clear_off(conn: &mut PgConnection, agent: Uuid, skill: Uuid) -> Result<bool, DbError> {
    Ok(
        sqlx::query("DELETE FROM agent_skills_off WHERE agent_id = $1 AND skill_id = $2")
            .bind(agent)
            .bind(skill)
            .execute(conn)
            .await?
            .rows_affected()
            > 0,
    )
}

// ---- usage (C-1175) -------------------------------------------------------------------------

/// Records that the agent loaded the skill; at most one row per turn and skill. The caller has
/// established that the agent, the conversation and the turn belong together.
pub async fn record_use(
    conn: &mut PgConnection,
    skill: Uuid,
    agent: Uuid,
    conversation: Uuid,
    turn: Option<Uuid>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO skill_uses (skill_id, agent_id, conversation_id, turn_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (turn_id, skill_id) WHERE turn_id IS NOT NULL DO NOTHING",
    )
    .bind(skill)
    .bind(agent)
    .bind(conversation)
    .bind(turn)
    .execute(conn)
    .await?;
    Ok(())
}

/// One agent's uses of one skill.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AgentUsageRow {
    pub skill_id: Uuid,
    /// Uses in the last 7 days.
    pub week: i64,
    pub last_at: DateTime<Utc>,
    pub last_conversation_id: Option<Uuid>,
}

/// The agent's usage of the skills among `skills` it has used within `keep_days`.
pub async fn usage_of_agent(
    conn: &mut PgConnection,
    agent: Uuid,
    skills: &[Uuid],
    week_days: i32,
    keep_days: i32,
) -> Result<Vec<AgentUsageRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT skill_id,
                count(*) FILTER (WHERE at > now() - make_interval(days => $3)) AS week,
                max(at) AS last_at,
                (array_agg(conversation_id ORDER BY at DESC))[1] AS last_conversation_id
         FROM skill_uses
         WHERE agent_id = $1 AND skill_id = ANY($2) AND at > now() - make_interval(days => $4)
         GROUP BY skill_id",
    )
    .bind(agent)
    .bind(skills)
    .bind(week_days)
    .bind(keep_days)
    .fetch_all(conn)
    .await?)
}

/// For each of `skills` that some agent used in the last `days`: how many distinct agents.
pub async fn used_by_agents(
    conn: &mut PgConnection,
    skills: &[Uuid],
    days: i32,
) -> Result<Vec<(Uuid, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT skill_id, count(DISTINCT agent_id) FROM skill_uses
         WHERE skill_id = ANY($1) AND at > now() - make_interval(days => $2)
         GROUP BY skill_id",
    )
    .bind(skills)
    .bind(days)
    .fetch_all(conn)
    .await?)
}

/// Deletes up to `batch` uses older than `days`; returns how many.
pub async fn prune_uses(pool: &sqlx::PgPool, days: i32, batch: i64) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM skill_uses WHERE ctid IN (
            SELECT ctid FROM skill_uses WHERE at < now() - make_interval(days => $1) LIMIT $2)",
    )
    .bind(days)
    .bind(batch)
    .execute(pool)
    .await?
    .rows_affected())
}

/// Enabled agent-tier skills of the workspace's agents with the given names, with the agent.
pub async fn agent_overrides(
    conn: &mut PgConnection,
    workspace: Uuid,
    name: &str,
) -> Result<Vec<(Uuid, String, String, Uuid)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.handle, a.name, s.id FROM skills s JOIN agents a ON a.id = s.agent_id
         WHERE a.workspace_id = $1 AND s.name = $2 AND s.enabled ORDER BY a.handle",
    )
    .bind(workspace)
    .bind(name)
    .fetch_all(conn)
    .await?)
}

pub async fn list_agent(conn: &mut PgConnection, agent: Uuid) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM skills WHERE agent_id = $1 ORDER BY name"
    )))
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

pub async fn list_org(conn: &mut PgConnection, org: Uuid) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM skills WHERE org_id = $1 AND agent_id IS NULL
         AND workspace_id IS NULL ORDER BY name"
    )))
    .bind(org)
    .fetch_all(conn)
    .await?)
}

/// Where a skill lives: `(agent, workspace)`; at most one is set, neither => organization tier.
#[derive(Debug, Clone, Copy, Default)]
pub struct Owner {
    pub agent: Option<Uuid>,
    pub workspace: Option<Uuid>,
}

/// Own (non-library) skills of an agent (FIX-110 cap).
pub async fn count_agent_own(conn: &mut PgConnection, agent: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar(
            "SELECT count(*) FROM skills WHERE agent_id = $1 AND library_id IS NULL",
        )
        .bind(agent)
        .fetch_one(conn)
        .await?,
    )
}

/// Files of one skill (FIX-110 cap).
pub async fn count_files(conn: &mut PgConnection, skill: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM skill_files WHERE skill_id = $1")
            .bind(skill)
            .fetch_one(conn)
            .await?,
    )
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    conn: &mut PgConnection,
    org: Uuid,
    owner: Owner,
    name: &str,
    description: &str,
    body: &str,
    enabled: bool,
    created_by_kind: &str,
    title: Option<&str>,
) -> Result<SkillRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO skills (id, org_id, agent_id, workspace_id, name, description, body, enabled,
            created_by_kind, title)
         VALUES ($1,$2,$3,$9,$4,$5,$6,$7,$8,$10) RETURNING {COLS}"
    )))
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(owner.agent)
    .bind(name)
    .bind(description)
    .bind(body)
    .bind(enabled)
    .bind(created_by_kind)
    .bind(owner.workspace)
    .bind(title)
    .fetch_one(conn)
    .await?)
}

pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    description: &str,
    body: &str,
    enabled: bool,
    title: Option<&str>,
) -> Result<SkillRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE skills SET description = $2, body = $3, enabled = $4, title = $5, updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    )))
    .bind(id)
    .bind(description)
    .bind(body)
    .bind(enabled)
    .bind(title)
    .fetch_one(conn)
    .await?)
}

pub async fn touch(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("UPDATE skills SET updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM skills WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Files (with content) of the given skills, ordered by path.
pub async fn files(conn: &mut PgConnection, skills: &[Uuid]) -> Result<Vec<FileRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT skill_id, path, content, executable FROM skill_files
         WHERE skill_id = ANY($1) ORDER BY skill_id, path",
    )
    .bind(skills)
    .fetch_all(conn)
    .await?)
}

pub async fn file(
    conn: &mut PgConnection,
    skill: Uuid,
    path: &str,
) -> Result<Option<FileRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT skill_id, path, content, executable FROM skill_files
         WHERE skill_id = $1 AND path = $2",
    )
    .bind(skill)
    .bind(path)
    .fetch_optional(conn)
    .await?)
}

/// Inserts or replaces a file. `executable = None` keeps the existing bit (false when new).
pub async fn upsert_file(
    conn: &mut PgConnection,
    skill: Uuid,
    path: &str,
    content: &[u8],
    executable: Option<bool>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO skill_files (skill_id, path, content, executable)
         VALUES ($1,$2,$3,coalesce($4,false))
         ON CONFLICT (skill_id, path) DO UPDATE SET content = EXCLUDED.content,
            executable = coalesce($4, skill_files.executable), updated_at = now()",
    )
    .bind(skill)
    .bind(path)
    .bind(content)
    .bind(executable)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn delete_file(
    conn: &mut PgConnection,
    skill: Uuid,
    path: &str,
) -> Result<bool, DbError> {
    Ok(
        sqlx::query("DELETE FROM skill_files WHERE skill_id = $1 AND path = $2")
            .bind(skill)
            .bind(path)
            .execute(conn)
            .await?
            .rows_affected()
            > 0,
    )
}

/// Bumps the home sync generation of one agent (agent-tier skill change).
pub async fn bump_agent(conn: &mut PgConnection, agent: Uuid) -> Result<(), DbError> {
    sqlx::query("UPDATE agents SET home_sync_generation = home_sync_generation + 1 WHERE id = $1")
        .bind(agent)
        .execute(conn)
        .await?;
    Ok(())
}

/// Bumps the workspace generation (workspace-tier skill change; part of every agent's desired
/// home-sync generation of that workspace, and of no other).
pub async fn bump_workspace(conn: &mut PgConnection, workspace: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE workspaces SET home_sync_generation = home_sync_generation + 1 WHERE id = $1",
    )
    .bind(workspace)
    .execute(conn)
    .await?;
    Ok(())
}

/// Bumps every agent of the organization (org-tier skill change).
pub async fn bump_org(conn: &mut PgConnection, org: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE agents SET home_sync_generation = home_sync_generation + 1 WHERE org_id = $1",
    )
    .bind(org)
    .execute(conn)
    .await?;
    Ok(())
}

/// The names among `names` that exist as an enabled organization-tier skill of `org` or an
/// enabled workspace-tier skill of `ws` (C-1142's organization reference resolution).
pub async fn enabled_org_or_workspace_names<'e, E: sqlx::PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    ws: Uuid,
    names: &[String],
) -> Result<Vec<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT name FROM skills
         WHERE name = ANY($1) AND enabled
           AND ((agent_id IS NULL AND workspace_id IS NULL AND org_id = $2)
                OR workspace_id = $3)",
    )
    .bind(names)
    .bind(org)
    .bind(ws)
    .fetch_all(exec)
    .await?)
}
