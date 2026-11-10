//! Skills (docs/10 §Skills, 03 §7): agent- and organization-tier instruction bundles + files.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

pub(crate) const COLS: &str = "id, org_id, agent_id, workspace_id, name, description, body, \
                    enabled, created_by_kind, created_at, updated_at, library_id, library_label, \
                    library_repo, library_path, library_commit_sha, library_blobs, \
                    library_content_sha256, library_pinned_at, library_pinned_by_user_id";

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
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FileRow {
    pub skill_id: Uuid,
    pub path: String,
    pub content: Vec<u8>,
    pub executable: bool,
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> Result<Option<SkillRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM skills WHERE id = $1"))
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Row lock: serializes concurrent edits (and file uploads) of one skill.
pub async fn get_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<SkillRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM skills WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

pub async fn get_agent_named(
    conn: &mut PgConnection,
    agent: Uuid,
    name: &str,
) -> Result<Option<SkillRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM skills WHERE agent_id = $1 AND name = $2"
    ))
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
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM skills WHERE org_id = $1 AND agent_id IS NULL
         AND workspace_id IS NULL AND name = $2"
    ))
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
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM skills WHERE workspace_id = $1 AND name = $2"
    ))
    .bind(workspace)
    .bind(name)
    .fetch_optional(conn)
    .await?)
}

pub async fn list_workspace(
    conn: &mut PgConnection,
    workspace: Uuid,
) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM skills WHERE workspace_id = $1 ORDER BY name"
    ))
    .bind(workspace)
    .fetch_all(conn)
    .await?)
}

/// Every skill of the three tiers that reaches the agent (its own, its workspace's, its
/// organization's), enabled or not.
pub async fn list_reaching(
    conn: &mut PgConnection,
    org: Uuid,
    workspace: Uuid,
    agent: Uuid,
) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM skills WHERE org_id = $1
         AND (agent_id = $3 OR workspace_id = $2 OR (agent_id IS NULL AND workspace_id IS NULL))
         ORDER BY name"
    ))
    .bind(org)
    .bind(workspace)
    .bind(agent)
    .fetch_all(conn)
    .await?)
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
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM skills WHERE agent_id = $1 ORDER BY name"
    ))
    .bind(agent)
    .fetch_all(conn)
    .await?)
}

pub async fn list_org(conn: &mut PgConnection, org: Uuid) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM skills WHERE org_id = $1 AND agent_id IS NULL
         AND workspace_id IS NULL ORDER BY name"
    ))
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
) -> Result<SkillRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO skills (id, org_id, agent_id, workspace_id, name, description, body, enabled,
            created_by_kind)
         VALUES ($1,$2,$3,$9,$4,$5,$6,$7,$8) RETURNING {COLS}"
    ))
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(owner.agent)
    .bind(name)
    .bind(description)
    .bind(body)
    .bind(enabled)
    .bind(created_by_kind)
    .bind(owner.workspace)
    .fetch_one(conn)
    .await?)
}

pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    description: &str,
    body: &str,
    enabled: bool,
) -> Result<SkillRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE skills SET description = $2, body = $3, enabled = $4, updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    ))
    .bind(id)
    .bind(description)
    .bind(body)
    .bind(enabled)
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
