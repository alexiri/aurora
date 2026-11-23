//! Agent helper files (03 §`agent_helper_files`, P6-TPL-6, C-1143): the template helper files
//! stored with an agent and projected read-only at `.aurora/helpers/<path>` by the supervisor.
//! Written only by `agent_helpers::replace_all` / `delete` in the API and by the DELETE route;
//! no agent tool exists.

use crate::DbError;
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HelperRow {
    /// The C-1141 helper path below `helpers/` (the projection is `.aurora/helpers/<path>`).
    pub path: String,
    pub content: Vec<u8>,
    pub executable: bool,
    /// Hex SHA-256 digest of `content`.
    pub sha256: String,
    pub size: i32,
}

/// All helper files of one agent, ordered by path (the list route's order).
pub async fn list<'e, E: PgExecutor<'e>>(exec: E, agent: Uuid) -> Result<Vec<HelperRow>, DbError> {
    Ok(sqlx::query_as::<_, HelperRow>(
        "SELECT path, content, executable, sha256, size FROM agent_helper_files
         WHERE agent_id = $1 ORDER BY path",
    )
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

/// One helper file of an agent by its path below `helpers/`.
pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    path: &str,
) -> Result<Option<HelperRow>, DbError> {
    Ok(sqlx::query_as::<_, HelperRow>(
        "SELECT path, content, executable, sha256, size FROM agent_helper_files
         WHERE agent_id = $1 AND path = $2",
    )
    .bind(agent)
    .bind(path)
    .fetch_optional(exec)
    .await?)
}

/// Insert one helper file (the caller already validated the path and hashed the content; the
/// writer runs this after `delete_all` inside one transaction, so a duplicate path is a bug).
pub async fn insert(
    conn: &mut PgConnection,
    agent: Uuid,
    path: &str,
    content: &[u8],
    executable: bool,
    sha256: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO agent_helper_files (agent_id, path, content, executable, sha256, size)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(agent)
    .bind(path)
    .bind(content)
    .bind(executable)
    .bind(sha256)
    .bind(content.len() as i32)
    .execute(conn)
    .await?;
    Ok(())
}

/// Delete one helper file; true when it existed.
pub async fn delete_one(conn: &mut PgConnection, agent: Uuid, path: &str) -> Result<bool, DbError> {
    Ok(
        sqlx::query("DELETE FROM agent_helper_files WHERE agent_id = $1 AND path = $2")
            .bind(agent)
            .bind(path)
            .execute(conn)
            .await?
            .rows_affected()
            > 0,
    )
}

/// Delete every helper file of an agent; the number removed.
pub async fn delete_all(conn: &mut PgConnection, agent: Uuid) -> Result<u64, DbError> {
    Ok(
        sqlx::query("DELETE FROM agent_helper_files WHERE agent_id = $1")
            .bind(agent)
            .execute(conn)
            .await?
            .rows_affected(),
    )
}
